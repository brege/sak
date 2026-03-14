use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use rustic_backend::BackendOptions;
use rustic_core::{Credentials, Repository, RepositoryOptions};
use serde::Deserialize;
use tar::Archive;
use tempfile::{TempDir, tempdir};
use walkdir::WalkDir;

const PASSWORD: &str = "test";

#[test]
fn import_twice_matches_backup_restore_fixture_behavior() -> Result<()> {
    let workspace = tempdir()?;
    let source_root = unpack_src_snapshot(&workspace)?;
    let source = source_root.join("src");
    let repo = workspace.path().join("repo");

    run_sak_import(&repo, &source)?;
    let first = snapshots(&repo)?;
    assert_eq!(first.len(), 1);
    let first_summary = first[0]
        .summary
        .as_ref()
        .context("missing summary on first snapshot")?;
    assert!(first_summary.data_added > 0);
    assert_eq!(first[0].hostname, "fixture-host");
    assert_eq!(first[0].label, "fixture");
    assert_eq!(first[0].paths.to_string(), "src");
    assert_eq!(first[0].tags.to_string(), "fixture");

    run_sak_import(&repo, &source)?;
    let second = snapshots(&repo)?;
    assert_eq!(second.len(), 2);
    let second_summary = second[1]
        .summary
        .as_ref()
        .context("missing summary on second snapshot")?;
    assert_eq!(second_summary.data_added, 0);
    assert_eq!(
        second_summary.files_unmodified,
        first_summary.total_files_processed
    );

    let restic = restic_snapshot_ids(&repo)?;
    assert_eq!(restic.len(), 2);

    Ok(())
}

#[test]
fn imported_fixture_restores_with_restic() -> Result<()> {
    let workspace = tempdir()?;
    let source_root = unpack_src_snapshot(&workspace)?;
    let source = source_root.join("src");
    let repo = workspace.path().join("repo");
    let restore = workspace.path().join("restore");

    run_sak_import(&repo, &source)?;

    let ls = run_restic(&repo)
        .arg("ls")
        .arg("latest")
        .output()
        .context("failed to run restic ls")?;
    if !ls.status.success() {
        bail!("restic ls failed: {}", String::from_utf8_lossy(&ls.stderr));
    }
    let ls = String::from_utf8(ls.stdout).context("restic ls output was not utf-8")?;
    assert!(ls.contains("/src"));
    assert!(ls.contains("/src/bin/rustic.rs"));
    assert!(ls.contains("/src/commands/backup.rs"));

    run_restic(&repo)
        .arg("restore")
        .arg("latest")
        .arg("--target")
        .arg(&restore)
        .status()
        .context("failed to run restic restore")
        .and_then(|status| require_success(status.success(), "restic restore failed"))?;

    compare_trees(&source, &restore.join("src"))?;

    Ok(())
}

fn run_sak_import(repo: &Path, source: &Path) -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_sak"))
        .arg("import")
        .arg("--repo")
        .arg(repo)
        .arg("--source")
        .arg(source)
        .arg("--as-path")
        .arg("src")
        .arg("--host")
        .arg("fixture-host")
        .arg("--label")
        .arg("fixture")
        .arg("--tag")
        .arg("fixture")
        .arg("--password")
        .arg(PASSWORD)
        .output()
        .context("failed to run sak import")?;
    if !output.status.success() {
        bail!(
            "sak import failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn unpack_src_snapshot(workspace: &TempDir) -> Result<PathBuf> {
    let target = workspace.path().join("source");
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

fn snapshots(repo: &Path) -> Result<Vec<rustic_core::repofile::SnapshotFile>> {
    let repo = open_repo(repo)?;
    let snapshots = repo.get_all_snapshots()?;
    let mut snapshots = snapshots;
    snapshots.sort_by_key(|snap| snap.time.clone());
    Ok(snapshots)
}

fn open_repo(repo: &Path) -> Result<Repository<rustic_core::OpenStatus>> {
    let repo = repo
        .to_str()
        .with_context(|| format!("repo path is not valid UTF-8: {}", repo.display()))?;
    let backends = BackendOptions::default().repository(repo).to_backends()?;
    let repo = Repository::new(&RepositoryOptions::default(), &backends)?;
    Ok(repo.open(&Credentials::password(PASSWORD))?)
}

fn run_restic(repo: &Path) -> Command {
    let mut command = Command::new("restic");
    command
        .env("RESTIC_PASSWORD", PASSWORD)
        .arg("--no-cache")
        .arg("--repo")
        .arg(repo);
    command
}

fn restic_snapshot_ids(repo: &Path) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ResticSnapshot {
        id: String,
    }

    let output = run_restic(repo)
        .arg("snapshots")
        .arg("--json")
        .output()
        .context("failed to run restic snapshots")?;
    if !output.status.success() {
        bail!(
            "restic snapshots failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let snapshots: Vec<ResticSnapshot> = serde_json::from_slice(&output.stdout)?;
    Ok(snapshots.into_iter().map(|snap| snap.id).collect())
}

fn compare_trees(left: &Path, right: &Path) -> Result<()> {
    let left = collect_files(left)?;
    let right = collect_files(right)?;
    assert_eq!(left, right);
    Ok(())
}

fn require_success(ok: bool, msg: &str) -> Result<()> {
    if ok { Ok(()) } else { bail!(msg.to_string()) }
}

fn collect_files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();

    for entry in WalkDir::new(root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let rel = entry.path().strip_prefix(root)?.to_path_buf();
        let data = fs::read(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;
        let old = files.insert(rel, data);
        assert!(old.is_none());
    }

    Ok(files)
}
