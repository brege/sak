use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use opendal::blocking::Operator as BlockingOperator;
use rustic_backend::BackendOptions;
use rustic_core::{Credentials, Repository, RepositoryOptions};
use sak::{RemoteSource, RemoteSourceReader, SourceSpec};
use tar::Archive;
use tempfile::{TempDir, tempdir};

const PASSWORD: &str = "test";

#[test]
fn parses_remote_and_local_source_specs() -> Result<()> {
    assert_eq!(
        "beelink:books/".parse::<SourceSpec>()?,
        SourceSpec::Remote(RemoteSource {
            host: "beelink".to_string(),
            path: "books/".to_string(),
        })
    );
    assert_eq!(
        "/home/notroot/books".parse::<SourceSpec>()?,
        SourceSpec::Local(PathBuf::from("/home/notroot/books"))
    );
    Ok(())
}

#[test]
fn remote_source_reader_imports_without_staging() -> Result<()> {
    let workspace = tempdir()?;
    let remote_root = unpack_src_snapshot(&workspace)?;
    let repo_path = workspace.path().join("repo");

    let remote = RemoteSource {
        host: "beelink".to_string(),
        path: "src".to_string(),
    };
    let reader = RemoteSourceReader::with_operator(remote, fs_operator(&remote_root)?)?;

    let credentials = Credentials::password(PASSWORD);
    let repo = open_or_init_repo(&repo_path, &credentials)?.to_indexed_ids()?;
    let mut backup_opts = rustic_core::BackupOptions::default();
    backup_opts.as_path = Some(PathBuf::from("src"));

    let mut snap_opts = rustic_core::SnapshotOptions::default();
    snap_opts.host = Some("beelink".to_string());
    snap_opts.label = Some("fixture".to_string());
    snap_opts = snap_opts.add_tags("fixture")?;

    let snapshot = repo.backup_source(
        &backup_opts,
        reader.backup_root(),
        &reader,
        snap_opts.to_snapshot()?,
    )?;

    assert_eq!(snapshot.paths.to_string(), "src");
    assert_eq!(snapshot.hostname, "beelink");
    assert_eq!(snapshot.label, "fixture");
    assert_eq!(snapshot.tags.to_string(), "fixture");

    let repo = open_repo(&repo_path)?;
    let snapshots = repo.get_all_snapshots()?;
    assert_eq!(snapshots.len(), 1);

    Ok(())
}

fn unpack_src_snapshot(workspace: &TempDir) -> Result<PathBuf> {
    let target = workspace.path().join("remote");
    fs::create_dir_all(&target)?;

    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("refs/rustic/tests/repository-fixtures/src-snapshot.tar.gz");
    let archive =
        File::open(&archive).with_context(|| format!("failed to open {}", archive.display()))?;
    let tar = GzDecoder::new(archive);
    let mut archive = Archive::new(tar);
    archive.set_preserve_permissions(true);
    archive.set_preserve_mtime(true);
    archive.unpack(&target)?;

    Ok(target)
}

fn fs_operator(root: &Path) -> Result<Arc<BlockingOperator>> {
    let options =
        std::collections::BTreeMap::from([("root".to_string(), root.display().to_string())]);
    let operator =
        opendal::Operator::via_iter("fs", options)?.layer(opendal::layers::LoggingLayer::default());
    let _guard = runtime().enter();
    Ok(Arc::new(BlockingOperator::new(operator)?))
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
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
            &rustic_core::KeyOptions::default(),
            &rustic_core::ConfigOptions::default(),
        )?)
    } else {
        Ok(repo.open(credentials)?)
    }
}

fn unopened_repo(repo: &Path) -> Result<Repository<()>> {
    let repo = repo
        .to_str()
        .with_context(|| format!("repo path is not valid UTF-8: {}", repo.display()))?;
    let backends = BackendOptions::default().repository(repo).to_backends()?;
    Ok(Repository::new(&RepositoryOptions::default(), &backends)?)
}

fn open_repo(repo: &Path) -> Result<Repository<rustic_core::OpenStatus>> {
    let repo = repo
        .to_str()
        .with_context(|| format!("repo path is not valid UTF-8: {}", repo.display()))?;
    let backends = BackendOptions::default().repository(repo).to_backends()?;
    let repo = Repository::new(&RepositoryOptions::default(), &backends)?;
    Ok(repo.open(&Credentials::password(PASSWORD))?)
}
