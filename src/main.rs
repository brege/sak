use std::{collections::BTreeSet, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueHint};
use rustic_backend::BackendOptions;
use rustic_core::{BackupOptions, CredentialOptions, RepositoryOptions, SnapshotOptions};
use sak::{
    ImportOptions, ServerConfig, SourceSpec, import_local_tree, init_logging, init_server_logging,
    run_server,
};

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
    Server(ServerArgs),
}

#[derive(Debug, Parser)]
struct ServerArgs {
    #[arg(value_hint = ValueHint::DirPath)]
    path: String,
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

    /// SSH key for connecting to the remote server agent
    #[arg(long, value_hint = ValueHint::FilePath)]
    server_key: Option<PathBuf>,

    /// Path to sak-server binary on the remote (default: .local/bin/sak-server)
    #[arg(long, value_hint = ValueHint::FilePath)]
    server_binary: Option<PathBuf>,
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

#[derive(Debug, Default, serde::Deserialize)]
struct SakBackupSection {
    snapshots: Vec<SakSnapshotEntry>,
    server: Option<ServerConfig>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SakSnapshotEntry {
    sources: Vec<String>,

    #[serde(flatten)]
    opts: toml::Table,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Import(args) => {
            init_logging();
            run_import(*args)
        }
        Command::Backup(args) => {
            init_logging();
            run_backup(args)
        }
        Command::Server(args) => {
            init_server_logging().context("failed to initialize server log file")?;
            run_server(&args.path)
        }
    }
}

fn run_import(args: ImportArgs) -> Result<()> {
    let server = if args.server_key.is_some() || args.server_binary.is_some() {
        Some(ServerConfig {
            key: args.server_key,
            binary: args.server_binary,
        })
    } else {
        None
    };

    let snapshot = import_local_tree(&ImportOptions {
        backend_opts: args.backend,
        repo_opts: args.repo_opts,
        credential_opts: args.credential_opts,
        source: args.source.parse::<SourceSpec>()?,
        backup: args.backup,
        snapshot: default_snapshot_command(args.snapshot, "sak import"),
        server,
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
    let (snapshots, server_cfg) = backup.map(|b| (b.snapshots, b.server)).unwrap_or_default();
    for snap_cfg in snapshots {
        let (backup_opts, snapshot_opts) = split_snapshot_options(&snap_cfg.opts)?;
        for source in snap_cfg.sources {
            let snapshot = import_local_tree(&ImportOptions {
                backend_opts: repository.backend.clone(),
                repo_opts: repository.repo_opts.clone(),
                credential_opts: repository.credential_opts.clone(),
                source: source.parse::<SourceSpec>()?,
                backup: backup_opts.clone(),
                snapshot: default_snapshot_command(snapshot_opts.clone(), "sak backup"),
                server: server_cfg.clone(),
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
