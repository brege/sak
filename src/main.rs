use std::{collections::BTreeSet, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueHint};
use rustic_backend::BackendOptions;
use rustic_core::{BackupOptions, CredentialOptions, RepositoryOptions, SnapshotOptions};
use sak::{ImportOptions, SourceSpec, import_local_tree, init_logging};

#[derive(Debug, Parser)]
#[command(name = "sak")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Import(Box<ImportArgs>),
    Backup(BackupArgs),
}

#[derive(Debug, Parser)]
struct ImportArgs {
    #[arg(long)]
    source: String,

    #[command(flatten)]
    backend: BackendOptions,

    #[command(flatten)]
    repo_opts: RepositoryOptions,

    #[command(flatten)]
    credential_opts: CredentialOptions,

    #[command(flatten)]
    backup: BackupOptions,

    #[command(flatten)]
    snapshot: SnapshotOptions,
}

#[derive(Debug, Parser)]
struct BackupArgs {
    #[arg(long, value_hint = ValueHint::FilePath)]
    config: PathBuf,
}

#[derive(Debug, serde::Deserialize)]
struct SakConfig {
    repository: SakRepositorySection,
    backup: Option<SakBackupSection>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SakRepositorySection {
    #[serde(flatten)]
    backend: BackendOptions,

    #[serde(flatten)]
    repo_opts: RepositoryOptions,

    #[serde(flatten)]
    credential_opts: CredentialOptions,
}

#[derive(Debug, serde::Deserialize)]
struct SakBackupSection {
    snapshots: Vec<SakSnapshotEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SakSnapshotEntry {
    sources: Vec<String>,

    #[serde(flatten)]
    opts: toml::Table,
}

fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Command::Import(args) => run_import(*args),
        Command::Backup(args) => run_backup(args),
    }
}

fn run_import(args: ImportArgs) -> Result<()> {
    let snapshot = import_local_tree(&ImportOptions {
        backend_opts: args.backend,
        repo_opts: args.repo_opts,
        credential_opts: args.credential_opts,
        source: args.source.parse::<SourceSpec>()?,
        backup: args.backup,
        snapshot: default_snapshot_command(args.snapshot, "sak import"),
    })?;

    println!("{}", snapshot.id);
    Ok(())
}

fn run_backup(args: BackupArgs) -> Result<()> {
    let raw = fs::read_to_string(&args.config)
        .with_context(|| format!("failed to read {}", args.config.display()))?;
    let cfg: SakConfig = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", args.config.display()))?;

    let SakConfig { repository, backup } = cfg;
    for snap_cfg in backup.map(|backup| backup.snapshots).unwrap_or_default() {
        let (backup_opts, snapshot_opts) = split_snapshot_options(&snap_cfg.opts)?;
        for source in snap_cfg.sources {
            let snapshot = import_local_tree(&ImportOptions {
                backend_opts: repository.backend.clone(),
                repo_opts: repository.repo_opts.clone(),
                credential_opts: repository.credential_opts.clone(),
                source: source.parse::<SourceSpec>()?,
                backup: backup_opts.clone(),
                snapshot: default_snapshot_command(snapshot_opts.clone(), "sak backup"),
            })?;
            println!("{source} {}", snapshot.id);
        }
    }

    Ok(())
}

fn default_snapshot_command(mut snapshot: SnapshotOptions, command: &str) -> SnapshotOptions {
    if snapshot.command.is_none() {
        snapshot.command = Some(command.to_string());
    }
    snapshot
}

fn split_snapshot_options(table: &toml::Table) -> Result<(BackupOptions, SnapshotOptions)> {
    let backup_keys = backup_option_keys();
    let snapshot_keys = snapshot_option_keys();
    let overlap = backup_keys
        .intersection(&snapshot_keys)
        .cloned()
        .collect::<Vec<_>>();
    if !overlap.is_empty() {
        bail!(
            "backup and snapshot options overlap: {}",
            overlap.join(", ")
        );
    }

    let mut backup = toml::map::Map::new();
    let mut snapshot = toml::map::Map::new();

    for (key, value) in table {
        if backup_keys.contains(key) {
            backup.insert(key.clone(), value.clone());
            continue;
        }
        if snapshot_keys.contains(key) {
            snapshot.insert(key.clone(), value.clone());
            continue;
        }
        bail!("unknown field `{key}` in [[backup.snapshots]]");
    }

    let backup_opts = toml::Value::Table(backup)
        .try_into()
        .context("invalid backup options in [[backup.snapshots]]")?;
    let snapshot_opts = toml::Value::Table(snapshot)
        .try_into()
        .context("invalid snapshot options in [[backup.snapshots]]")?;

    Ok((backup_opts, snapshot_opts))
}

// rustic's config layer documents that nested flattened serde structs do not
// deserialize cleanly in [[backup.snapshots]]. Keep the key split local here
// so sak can still deserialize the upstream option types without mirroring
// their full field logic.
fn backup_option_keys() -> BTreeSet<String> {
    [
        "stdin-filename",
        "stdin-command",
        "as-path",
        "no-scan",
        "dry-run",
        "group-by",
        "parents",
        "skip-if-unchanged",
        "force",
        "ignore-ctime",
        "ignore-inode",
        "set-atime",
        "set-ctime",
        "set-devid",
        "set-blockdev",
        "set-xattrs",
        "globs",
        "iglobs",
        "glob-files",
        "iglob-files",
        "git-ignore",
        "no-require-git",
        "custom-ignorefiles",
        "exclude-if-present",
        "one-file-system",
        "exclude-larger-than",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn snapshot_option_keys() -> BTreeSet<String> {
    [
        "label",
        "tags",
        "description",
        "description-from",
        "time",
        "delete-never",
        "delete-after",
        "host",
        "command",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
