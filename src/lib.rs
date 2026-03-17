pub mod deploy;
mod progress;
mod proto;
mod server;
mod server_source;

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, bail};
use ignore::{
    Match,
    overrides::{Override, OverrideBuilder},
};
use opendal::{
    EntryMode,
    blocking::{Operator as BlockingOperator, StdReader},
    layers::{ConcurrentLimitLayer, LoggingLayer, RetryLayer, ThrottleLayer},
    options::ListOptions,
};
use rustic_backend::BackendOptions;
use rustic_core::{
    BackupOptions, ConfigOptions, CredentialOptions, ErrorKind, Excludes, KeyOptions,
    LocalSourceFilterOptions, PathList, ReadSource, ReadSourceEntry, ReadSourceOpen, Repository,
    RepositoryOptions, RusticError, RusticResult, SnapshotOptions,
    node::{Metadata, Node, NodeType},
    repofile::SnapshotFile,
};

use crate::progress::UiProgress;

pub use deploy::ServerConfig;
pub use progress::{init_logging, init_server_logging};
pub use server::run_server;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub backend_opts: BackendOptions,
    pub repo_opts: RepositoryOptions,
    pub credential_opts: CredentialOptions,
    pub source: SourceSpec,
    pub backup: BackupOptions,
    pub snapshot: SnapshotOptions,
    pub server: Option<ServerConfig>,
}

pub fn import_local_tree(opts: &ImportOptions) -> Result<SnapshotFile> {
    match &opts.source {
        SourceSpec::Local(path) => import_local_path(opts, path),
        SourceSpec::Remote(remote) => import_remote_path(opts, remote),
    }
}

fn import_local_path(opts: &ImportOptions, source_path: &Path) -> Result<SnapshotFile> {
    let repo = open_or_init_repo(&opts.backend_opts, &opts.repo_opts, &opts.credential_opts)?
        .to_indexed_ids()?;
    let source = path_list(source_path)?;
    let snap = opts.snapshot.to_snapshot()?;
    Ok(repo.backup(&opts.backup, &source, snap)?)
}

fn import_remote_path(opts: &ImportOptions, remote: &RemoteSource) -> Result<SnapshotFile> {
    let repo = open_or_init_repo(&opts.backend_opts, &opts.repo_opts, &opts.credential_opts)?
        .to_indexed_ids()?;

    if let Some(server_cfg) = &opts.server {
        let root = remote.root_path()?;
        let session = deploy::ServerSession::connect(&remote.host, server_cfg)?;
        let channel = session.start_server(&remote.path)?;
        let source = server_source::SakServerSource::new(
            root,
            opts.backup.excludes.clone(),
            opts.backup.ignore_filter_opts.clone(),
            channel,
        )?;
        let snap = opts.snapshot.to_snapshot()?;
        return Ok(repo.backup_source(&opts.backup, source.backup_root(), &source, snap)?);
    }

    let source = RemoteSourceReader::new(
        remote.clone(),
        opts.backup.excludes.clone(),
        opts.backup.ignore_filter_opts.clone(),
    )?;
    let snap = opts.snapshot.to_snapshot()?;
    Ok(repo.backup_source(&opts.backup, source.backup_root(), &source, snap)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    Local(PathBuf),
    Remote(RemoteSource),
}

impl SourceSpec {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }
}

impl FromStr for SourceSpec {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        if let Some((host, path)) = value.split_once(':')
            && is_remote_host(host)
            && !path.is_empty()
        {
            return Ok(Self::Remote(RemoteSource {
                host: host.to_string(),
                path: path.to_string(),
            }));
        }

        Ok(Self::Local(PathBuf::from(value)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSource {
    pub host: String,
    pub path: String,
}

impl RemoteSource {
    fn root_path(&self) -> Result<PathBuf> {
        let trimmed = self.path.trim_end_matches('/');
        if trimmed.is_empty() {
            bail!("remote path must not be empty");
        }
        Ok(PathBuf::from(trimmed))
    }
}

#[derive(Clone)]
pub struct RemoteSourceReader {
    root: PathBuf,
    state: RemoteSourceState,
}

impl RemoteSourceReader {
    pub fn new(
        remote: RemoteSource,
        excludes: Excludes,
        filter_opts: LocalSourceFilterOptions,
    ) -> Result<Self> {
        let op = Arc::new(remote_operator(&remote)?);
        Self::with_operator(remote, op, excludes, filter_opts)
    }

    pub fn with_operator(
        remote: RemoteSource,
        op: Arc<BlockingOperator>,
        excludes: Excludes,
        filter_opts: LocalSourceFilterOptions,
    ) -> Result<Self> {
        let root = remote.root_path()?;
        let stat = op
            .stat(&remote.path)
            .with_context(|| format!("failed to stat remote path {}", remote.path))?;

        if stat.is_file() {
            let entry = remote_source_entry(op, root.clone(), remote.path, &stat)?;
            return Ok(Self {
                root,
                state: RemoteSourceState::File(Box::new(entry)),
            });
        }

        if !stat.is_dir() {
            bail!("unsupported remote path type: {}", remote.path);
        }

        Ok(Self {
            root: root.clone(),
            state: RemoteSourceState::Dir {
                op,
                root,
                root_remote: remote_dir_path(&remote.path),
                filters: Arc::new(build_remote_filters(&excludes, &filter_opts)?),
            },
        })
    }

    pub fn backup_root(&self) -> &Path {
        &self.root
    }
}

impl ReadSource for RemoteSourceReader {
    type Open = RemoteOpen;
    type Iter = RemoteEntries;

    fn size(&self) -> RusticResult<Option<u64>> {
        Ok(match &self.state {
            RemoteSourceState::File(entry) => Some(entry.node.meta.size),
            RemoteSourceState::Dir { .. } => None,
        })
    }

    fn entries(&self) -> Self::Iter {
        RemoteEntries::new(self.state.clone())
    }
}

#[derive(Clone)]
enum RemoteSourceState {
    File(Box<ReadSourceEntry<RemoteOpen>>),
    Dir {
        op: Arc<BlockingOperator>,
        root: PathBuf,
        root_remote: String,
        filters: Arc<RemoteFilters>,
    },
}

pub struct RemoteEntries {
    inner: RemoteEntriesInner,
}

enum RemoteEntriesInner {
    File(Box<Option<RusticResult<ReadSourceEntry<RemoteOpen>>>>),
    Dir(RemoteSourceWalker),
}

impl RemoteEntries {
    fn new(state: RemoteSourceState) -> Self {
        let inner = match state {
            RemoteSourceState::File(entry) => RemoteEntriesInner::File(Box::new(Some(Ok(*entry)))),
            RemoteSourceState::Dir {
                op,
                root,
                root_remote,
                filters,
            } => RemoteEntriesInner::Dir(RemoteSourceWalker::new(op, root, root_remote, filters)),
        };
        Self { inner }
    }
}

impl Iterator for RemoteEntries {
    type Item = RusticResult<ReadSourceEntry<RemoteOpen>>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            RemoteEntriesInner::File(entry) => entry.take(),
            RemoteEntriesInner::Dir(walker) => walker.next(),
        }
    }
}

struct RemoteSourceWalker {
    op: Arc<BlockingOperator>,
    root: PathBuf,
    filters: Arc<RemoteFilters>,
    dirs: VecDeque<String>,
    pending: VecDeque<ReadSourceEntry<RemoteOpen>>,
}

impl RemoteSourceWalker {
    fn new(
        op: Arc<BlockingOperator>,
        root: PathBuf,
        root_remote: String,
        filters: Arc<RemoteFilters>,
    ) -> Self {
        let mut dirs = VecDeque::new();
        dirs.push_back(root_remote.clone());
        Self {
            op,
            root,
            filters,
            dirs,
            pending: VecDeque::new(),
        }
    }

    fn fill_pending(&mut self) -> Result<bool> {
        while self.pending.is_empty() {
            let Some(dir) = self.dirs.pop_front() else {
                return Ok(false);
            };
            if remote_dir_excluded(self.op.as_ref(), &dir, &self.filters.exclude_if_present) {
                continue;
            }
            let mut listed = self
                .op
                .list_options(&dir, ListOptions::default())
                .with_context(|| format!("failed to list remote path {dir}"))?;
            listed.sort_by(|left, right| left.path().cmp(right.path()));
            for entry in listed {
                let trimmed = entry.path().trim_end_matches('/');
                if trimmed == dir.trim_end_matches('/') {
                    continue;
                }
                let Some(meta) =
                    resolve_remote_meta(self.op.as_ref(), entry.path(), entry.metadata())?
                else {
                    continue;
                };
                let path = remote_entry_path(entry.path())?;
                let node = remote_node(&path, &meta)?;
                if !include_remote_entry(&path, &node, &self.root, &self.filters) {
                    continue;
                }
                if node.node_type == NodeType::Dir {
                    self.dirs.push_back(remote_dir_path(trimmed));
                }
                self.pending.push_back(ReadSourceEntry {
                    path,
                    open: node.is_file().then(|| RemoteOpen {
                        op: self.op.clone(),
                        path: entry.path().to_string(),
                    }),
                    node,
                });
            }
        }
        Ok(true)
    }
}

impl Iterator for RemoteSourceWalker {
    type Item = RusticResult<ReadSourceEntry<RemoteOpen>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.fill_pending() {
            Ok(true) => self.pending.pop_front().map(Ok),
            Ok(false) => None,
            Err(err) => Some(Err(RusticError::with_source(
                ErrorKind::InputOutput,
                "Failed to iterate remote source entries.",
                err,
            )
            .ask_report())),
        }
    }
}

#[derive(Clone)]
pub struct RemoteOpen {
    op: Arc<BlockingOperator>,
    path: String,
}

impl ReadSourceOpen for RemoteOpen {
    type Reader = StdReader;

    fn open(self) -> RusticResult<Self::Reader> {
        self.op
            .reader(&self.path)
            .and_then(|reader| reader.into_std_read(..))
            .map_err(|err| {
                RusticError::with_source(
                    ErrorKind::InputOutput,
                    "Failed to open remote file `{path}`.",
                    err,
                )
                .attach_context("path", self.path)
            })
    }
}

fn build_remote_filters(
    excludes: &Excludes,
    filter_opts: &LocalSourceFilterOptions,
) -> Result<RemoteFilters> {
    if filter_opts.git_ignore {
        bail!("remote imports do not support git-ignore yet");
    }
    if filter_opts.no_require_git {
        bail!("remote imports do not support no-require-git yet");
    }
    if !filter_opts.custom_ignorefiles.is_empty() {
        bail!("remote imports do not support custom-ignorefile yet");
    }
    if filter_opts.one_file_system {
        bail!("remote imports do not support one-file-system yet");
    }

    let mut builder = OverrideBuilder::new("");

    for glob in &excludes.globs {
        builder
            .add(glob)
            .with_context(|| format!("failed to add glob pattern {glob}"))?;
    }

    for file in &excludes.glob_files {
        let content =
            fs::read_to_string(file).with_context(|| format!("failed to read glob file {file}"))?;
        for line in content.lines() {
            builder
                .add(line)
                .with_context(|| format!("failed to add glob pattern line {line} from {file}"))?;
        }
    }

    builder
        .case_insensitive(true)
        .context("failed to enable case-insensitive matching for iglob and iglob-file patterns")?;

    for glob in &excludes.iglobs {
        builder
            .add(glob)
            .with_context(|| format!("failed to add iglob pattern {glob}"))?;
    }

    for file in &excludes.iglob_files {
        let content = fs::read_to_string(file)
            .with_context(|| format!("failed to read iglob file {file}"))?;
        for line in content.lines() {
            builder
                .add(line)
                .with_context(|| format!("failed to add iglob pattern line {line} from {file}"))?;
        }
    }

    Ok(RemoteFilters {
        overrides: builder.build().context("failed to build glob overrides")?,
        exclude_if_present: filter_opts.exclude_if_present.clone(),
        exclude_larger_than: filter_opts.exclude_larger_than.map(|size| size.as_u64()),
    })
}

fn include_remote_entry(path: &Path, node: &Node, root: &Path, filters: &RemoteFilters) -> bool {
    if let Some(limit) = filters.exclude_larger_than
        && node.node_type == NodeType::File
        && node.meta.size > limit
    {
        return false;
    }

    match best_override_match(
        path,
        node.node_type == NodeType::Dir,
        root,
        &filters.overrides,
    ) {
        Some(matched) if matched.is_whitelist() => true,
        Some(matched) if matched.is_ignore() => false,
        _ => true,
    }
}

fn best_override_match<'a>(
    path: &'a Path,
    is_dir: bool,
    root: &Path,
    overrides: &'a Override,
) -> Option<Match<ignore::overrides::Glob<'a>>> {
    let full = overrides.matched(path, is_dir);
    let rel = path
        .strip_prefix(root)
        .ok()
        .map(|path| overrides.matched(path, is_dir));

    match (full.is_none(), rel) {
        (false, Some(matched)) if matched.is_whitelist() => Some(matched),
        (false, Some(_)) if full.is_whitelist() => Some(full),
        (false, Some(matched)) if matched.is_ignore() => Some(matched),
        (false, Some(_)) if full.is_ignore() => Some(full),
        (false, _) => Some(full),
        (true, Some(matched)) if !matched.is_none() => Some(matched),
        _ => None,
    }
}

fn remote_source_entry(
    op: Arc<BlockingOperator>,
    path: PathBuf,
    remote_path: String,
    meta: &opendal::Metadata,
) -> Result<ReadSourceEntry<RemoteOpen>> {
    let node = remote_node(&path, meta)?;
    let open = if node.is_file() {
        Some(RemoteOpen {
            op,
            path: remote_path,
        })
    } else {
        None
    };

    Ok(ReadSourceEntry { path, node, open })
}

fn remote_entry_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("remote entry path must not be empty");
    }
    Ok(PathBuf::from(trimmed))
}

fn remote_node(path: &Path, remote_meta: &opendal::Metadata) -> Result<Node> {
    let name = path
        .file_name()
        .with_context(|| format!("remote path has no terminal component: {}", path.display()))?;

    let meta = Metadata {
        mode: None,
        mtime: remote_meta.last_modified().map(|time| time.into_inner()),
        atime: None,
        ctime: None,
        uid: None,
        gid: None,
        user: None,
        group: None,
        inode: 0,
        device_id: 0,
        size: if remote_meta.is_file() {
            remote_meta.content_length()
        } else {
            0
        },
        links: 0,
        extended_attributes: Vec::new(),
    };

    let node_type = match remote_meta.mode() {
        EntryMode::FILE => NodeType::File,
        EntryMode::DIR => NodeType::Dir,
        mode => bail!(
            "unsupported remote entry mode for {}: {mode:?}",
            path.display()
        ),
    };

    Ok(Node::new_node(name, node_type, meta))
}

fn resolve_remote_meta(
    op: &BlockingOperator,
    remote_path: &str,
    meta: &opendal::Metadata,
) -> Result<Option<opendal::Metadata>> {
    if meta.mode() != EntryMode::Unknown {
        return Ok(Some(meta.clone()));
    }

    let resolved = match op.stat(remote_path) {
        Ok(meta) => meta,
        Err(err)
            if matches!(
                err.kind(),
                opendal::ErrorKind::NotFound | opendal::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat remote path {remote_path}"));
        }
    };

    if resolved.mode() == EntryMode::Unknown {
        return Ok(None);
    }

    Ok(Some(resolved))
}

fn remote_dir_excluded(op: &BlockingOperator, dir: &str, markers: &[String]) -> bool {
    let dir = remote_dir_path(dir.trim_end_matches('/'));
    markers.iter().any(|marker| {
        let candidate = format!("{dir}{marker}");
        op.stat(&candidate).is_ok()
    })
}

fn is_remote_host(host: &str) -> bool {
    !host.is_empty() && !host.contains('/') && !host.starts_with('.') && !host.starts_with('~')
}

fn remote_operator(remote: &RemoteSource) -> Result<BlockingOperator> {
    let options = remote_options(remote);
    let retry = remote_retry()?;
    let connections = remote_connections()?;
    let throttle = env::var("SAK_SFTP_THROTTLE").ok();

    let mut operator = opendal::Operator::via_iter("sftp", options)
        .with_context(|| format!("failed to create sftp operator for {}", remote.host))?
        .layer(RetryLayer::new().with_max_times(retry).with_jitter());

    if let Some(throttle) = throttle {
        let (bandwidth, burst) = parse_throttle(&throttle)?;
        operator = operator.layer(ThrottleLayer::new(bandwidth, burst));
    }

    if let Some(connections) = connections {
        operator = operator.layer(ConcurrentLimitLayer::new(connections));
    }

    let _guard = runtime().enter();
    BlockingOperator::new(operator.layer(LoggingLayer::default()))
        .context("failed to create blocking sftp operator")
}

fn remote_options(remote: &RemoteSource) -> BTreeMap<String, String> {
    let mut options = BTreeMap::from([("endpoint".to_string(), remote.host.clone())]);

    for (env_key, option_key) in [
        ("SAK_SFTP_USER", "user"),
        ("SAK_SFTP_KEY", "key"),
        ("SAK_SFTP_ROOT", "root"),
        ("SAK_SFTP_KNOWN_HOSTS_STRATEGY", "known_hosts_strategy"),
    ] {
        if let Ok(value) = env::var(env_key)
            && !value.is_empty()
        {
            options.insert(option_key.to_string(), value);
        }
    }

    options
}

fn remote_retry() -> Result<usize> {
    match env::var("SAK_SFTP_RETRY") {
        Ok(value) => value
            .parse()
            .with_context(|| format!("invalid SAK_SFTP_RETRY value: {value}")),
        Err(env::VarError::NotPresent) => Ok(5),
        Err(err) => Err(err).context("failed to read SAK_SFTP_RETRY"),
    }
}

fn remote_connections() -> Result<Option<usize>> {
    match env::var("SAK_SFTP_CONNECTIONS") {
        Ok(value) => value
            .parse()
            .with_context(|| format!("invalid SAK_SFTP_CONNECTIONS value: {value}"))
            .map(Some),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(err).context("failed to read SAK_SFTP_CONNECTIONS"),
    }
}

fn parse_throttle(value: &str) -> Result<(u32, u32)> {
    let (bandwidth, burst) = value
        .split_once(',')
        .with_context(|| format!("invalid SAK_SFTP_THROTTLE value: {value}"))?;
    Ok((parse_bytesize(bandwidth)?, parse_bytesize(burst)?))
}

fn parse_bytesize(value: &str) -> Result<u32> {
    bytesize::ByteSize::from_str(value.trim())
        .map_err(|err| anyhow::anyhow!("invalid byte size {value}: {err}"))?
        .as_u64()
        .try_into()
        .with_context(|| format!("byte size exceeds u32: {value}"))
}

fn remote_dir_path(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn path_list(path: &Path) -> Result<PathList> {
    let path = path
        .to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))?;
    Ok(PathList::from_string(path)?.sanitize()?)
}

fn open_or_init_repo(
    backend_opts: &BackendOptions,
    repo_opts: &RepositoryOptions,
    credential_opts: &CredentialOptions,
) -> Result<Repository<rustic_core::OpenStatus>> {
    let repo_path = backend_opts
        .repository
        .as_deref()
        .context("repository path required")?;
    fs::create_dir_all(repo_path)
        .with_context(|| format!("failed to create repository dir {repo_path}"))?;

    let credentials = credential_opts
        .credentials()?
        .context("repository credentials required")?;
    let backends = backend_opts.to_backends()?;
    let repo = Repository::new_with_progress(repo_opts, &backends, UiProgress)?;
    if repo.config_id()?.is_none() {
        Ok(repo.init(
            &credentials,
            &KeyOptions::default(),
            &ConfigOptions::default(),
        )?)
    } else {
        Ok(repo.open(&credentials)?)
    }
}

#[derive(Debug, Clone)]
struct RemoteFilters {
    overrides: Override,
    exclude_if_present: Vec<String>,
    exclude_larger_than: Option<u64>,
}
