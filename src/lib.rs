mod progress;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, bail};
use opendal::{
    EntryMode,
    blocking::{Operator as BlockingOperator, StdReader},
    layers::{ConcurrentLimitLayer, LoggingLayer, RetryLayer, ThrottleLayer},
    options::ListOptions,
};
use rustic_backend::BackendOptions;
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, ErrorKind, KeyOptions, PathList, ReadSource,
    ReadSourceEntry, ReadSourceOpen, Repository, RepositoryOptions, RusticError, RusticResult,
    SnapshotOptions,
    node::{Metadata, Node, NodeType},
    repofile::SnapshotFile,
};

use crate::progress::UiProgress;

pub use progress::init_logging;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub repo: PathBuf,
    pub source: SourceSpec,
    pub snapshot_path: PathBuf,
    pub password: String,
    pub host: Option<String>,
    pub label: Option<String>,
    pub tags: Vec<String>,
}

pub fn import_local_tree(opts: &ImportOptions) -> Result<SnapshotFile> {
    match &opts.source {
        SourceSpec::Local(path) => import_local_path(opts, path),
        SourceSpec::Remote(remote) => import_remote_path(opts, remote),
    }
}

fn import_local_path(opts: &ImportOptions, source_path: &Path) -> Result<SnapshotFile> {
    let credentials = Credentials::password(&opts.password);
    let repo = open_or_init_repo(&opts.repo, &credentials)?.to_indexed_ids()?;
    let source = path_list(source_path)?;

    let mut backup_opts = BackupOptions::default();
    backup_opts.as_path = Some(opts.snapshot_path.clone());

    let snap = snapshot_options(opts)?.to_snapshot()?;
    Ok(repo.backup(&backup_opts, &source, snap)?)
}

fn import_remote_path(opts: &ImportOptions, remote: &RemoteSource) -> Result<SnapshotFile> {
    let credentials = Credentials::password(&opts.password);
    let repo = open_or_init_repo(&opts.repo, &credentials)?.to_indexed_ids()?;
    let source = RemoteSourceReader::new(remote.clone())?;

    let mut backup_opts = BackupOptions::default();
    backup_opts.as_path = Some(opts.snapshot_path.clone());

    let snap = snapshot_options(opts)?.to_snapshot()?;
    Ok(repo.backup_source(&backup_opts, source.backup_root(), &source, snap)?)
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
        if let Some((host, path)) = value.split_once(':') {
            if is_remote_host(host) && !path.is_empty() {
                return Ok(Self::Remote(RemoteSource {
                    host: host.to_string(),
                    path: path.to_string(),
                }));
            }
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
    entries: Vec<RemoteEntry>,
    size: u64,
}

impl RemoteSourceReader {
    pub fn new(remote: RemoteSource) -> Result<Self> {
        let op = Arc::new(remote_operator(&remote)?);
        Self::with_operator(remote, op)
    }

    pub fn with_operator(remote: RemoteSource, op: Arc<BlockingOperator>) -> Result<Self> {
        let root = remote.root_path()?;
        let entries = collect_remote_entries(op, &remote, &root)?;
        let size = entries
            .iter()
            .filter(|entry| matches!(entry.node.node_type, NodeType::File))
            .map(|entry| entry.node.meta.size)
            .sum();
        Ok(Self {
            root,
            entries,
            size,
        })
    }

    pub fn backup_root(&self) -> &Path {
        &self.root
    }
}

impl ReadSource for RemoteSourceReader {
    type Open = RemoteOpen;
    type Iter = std::vec::IntoIter<RusticResult<ReadSourceEntry<Self::Open>>>;

    fn size(&self) -> RusticResult<Option<u64>> {
        Ok(Some(self.size))
    }

    fn entries(&self) -> Self::Iter {
        self.entries
            .iter()
            .cloned()
            .map(|entry| {
                Ok(ReadSourceEntry {
                    path: entry.path,
                    node: entry.node,
                    open: entry.open,
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[derive(Clone)]
struct RemoteEntry {
    path: PathBuf,
    node: Node,
    open: Option<RemoteOpen>,
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

fn collect_remote_entries(
    op: Arc<BlockingOperator>,
    remote: &RemoteSource,
    root: &Path,
) -> Result<Vec<RemoteEntry>> {
    let stat = op
        .stat(&remote.path)
        .with_context(|| format!("failed to stat remote path {}", remote.path))?;

    if stat.is_file() {
        return Ok(vec![remote_file_entry(
            op,
            root.to_path_buf(),
            remote.path.clone(),
            &stat,
        )?]);
    }

    if !stat.is_dir() {
        bail!("unsupported remote path type: {}", remote.path);
    }

    let root_path = remote_dir_path(&remote.path);
    let mut entries = op
        .list_options(
            &root_path,
            ListOptions {
                recursive: true,
                ..Default::default()
            },
        )
        .with_context(|| format!("failed to list remote path {root_path}"))?
        .into_iter()
        .filter_map(|entry| {
            let trimmed = entry.path().trim_end_matches('/');
            if trimmed == root_path.trim_end_matches('/') {
                return None;
            }
            Some(remote_list_entry(
                op.clone(),
                entry.path(),
                entry.metadata(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn remote_file_entry(
    op: Arc<BlockingOperator>,
    path: PathBuf,
    remote_path: String,
    meta: &opendal::Metadata,
) -> Result<RemoteEntry> {
    let node = remote_node(&path, meta)?;
    let open = if node.is_file() {
        Some(RemoteOpen {
            op,
            path: remote_path,
        })
    } else {
        None
    };

    Ok(RemoteEntry { path, node, open })
}

fn remote_list_entry(
    op: Arc<BlockingOperator>,
    remote_path: &str,
    meta: &opendal::Metadata,
) -> Result<RemoteEntry> {
    let path = remote_entry_path(remote_path)?;
    remote_file_entry(op, path, remote_path.to_string(), meta)
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

fn snapshot_options(opts: &ImportOptions) -> Result<SnapshotOptions> {
    let mut snap = SnapshotOptions::default();
    snap.host = opts.host.clone();
    snap.label = opts.label.clone();
    snap.command = Some("sak import".to_string());

    for tag in &opts.tags {
        snap = snap.add_tags(tag)?;
    }

    Ok(snap)
}

fn path_list(path: &Path) -> Result<PathList> {
    let path = path
        .to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))?;
    Ok(PathList::from_string(path)?.sanitize()?)
}

fn open_or_init_repo(
    repo: &Path,
    credentials: &Credentials,
) -> Result<Repository<rustic_core::OpenStatus>> {
    fs::create_dir_all(repo)
        .with_context(|| format!("failed to create repository dir {}", repo.display()))?;

    let repo = unopened_repo(repo)?;
    if repo.config_id()?.is_none() {
        Ok(repo.init(
            credentials,
            &KeyOptions::default(),
            &ConfigOptions::default(),
        )?)
    } else {
        Ok(repo.open(credentials)?)
    }
}

fn unopened_repo(repo: &Path) -> Result<Repository<()>> {
    if repo.as_os_str().is_empty() {
        bail!("repository path must not be empty");
    }

    let repo = repo
        .to_str()
        .with_context(|| format!("repository path is not valid UTF-8: {}", repo.display()))?;
    let backends = BackendOptions::default().repository(repo).to_backends()?;

    Ok(Repository::new_with_progress(
        &RepositoryOptions::default(),
        &backends,
        UiProgress,
    )?)
}
