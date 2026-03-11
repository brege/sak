use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rustic_backend::BackendOptions;
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, PathList, Repository, RepositoryOptions,
    SnapshotOptions, repofile::SnapshotFile,
};

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub repo: PathBuf,
    pub source: PathBuf,
    pub snapshot_path: PathBuf,
    pub password: String,
    pub host: Option<String>,
    pub label: Option<String>,
    pub tags: Vec<String>,
}

pub fn import_local_tree(opts: &ImportOptions) -> Result<SnapshotFile> {
    let credentials = Credentials::password(&opts.password);
    let repo = open_or_init_repo(&opts.repo, &credentials)?.to_indexed_ids()?;
    let source = path_list(&opts.source)?;

    let mut backup_opts = BackupOptions::default();
    backup_opts.as_path = Some(opts.snapshot_path.clone());

    let snap = snapshot_options(opts)?.to_snapshot()?;
    let snapshot = repo.backup(&backup_opts, &source, snap)?;

    Ok(snapshot)
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

    Ok(Repository::new(&RepositoryOptions::default(), &backends)?)
}
