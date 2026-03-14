use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rustic_core::{BackupOptions, SnapshotOptions};
use sak::{ImportOptions, SourceSpec, import_local_tree, init_logging};

#[derive(Debug, Parser)]
#[command(name = "sak")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Import(ImportArgs),
}

#[derive(Debug, Parser)]
struct ImportArgs {
    #[arg(long)]
    repo: PathBuf,

    #[arg(long)]
    source: String,

    #[command(flatten)]
    backup: BackupOptions,

    #[command(flatten)]
    snapshot: SnapshotOptions,

    #[arg(long, conflicts_with = "password_file")]
    password: Option<String>,

    #[arg(long, conflicts_with = "password")]
    password_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Command::Import(args) => run_import(args),
    }
}

fn run_import(args: ImportArgs) -> Result<()> {
    let password = read_password(args.password, args.password_file.as_deref())?;
    let mut snapshot = args.snapshot;
    if snapshot.command.is_none() {
        snapshot.command = Some("sak import".to_string());
    }

    let snapshot = import_local_tree(&ImportOptions {
        repo: args.repo,
        source: args.source.parse::<SourceSpec>()?,
        password,
        backup: args.backup,
        snapshot,
    })?;

    println!("{}", snapshot.id);
    Ok(())
}

fn read_password(
    password: Option<String>,
    password_file: Option<&std::path::Path>,
) -> Result<String> {
    match (password, password_file) {
        (Some(password), None) => Ok(password),
        (None, Some(path)) => {
            let content = fs::read_to_string(path)
                .with_context(|| format!("failed to read password file {}", path.display()))?;
            let password = content.lines().next().unwrap_or_default().to_string();
            if password.is_empty() {
                bail!("password file is empty: {}", path.display());
            }
            Ok(password)
        }
        (None, None) => bail!("either --password or --password-file is required"),
        (Some(_), Some(_)) => unreachable!(),
    }
}
