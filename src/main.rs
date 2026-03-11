use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sak::{ImportOptions, import_local_tree};

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
    source: PathBuf,

    #[arg(long)]
    snapshot_path: PathBuf,

    #[arg(long)]
    host: Option<String>,

    #[arg(long)]
    label: Option<String>,

    #[arg(long = "tag")]
    tags: Vec<String>,

    #[arg(long, conflicts_with = "password_file")]
    password: Option<String>,

    #[arg(long, conflicts_with = "password")]
    password_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Import(args) => run_import(args),
    }
}

fn run_import(args: ImportArgs) -> Result<()> {
    let password = read_password(args.password, args.password_file.as_deref())?;
    let snapshot = import_local_tree(&ImportOptions {
        repo: args.repo,
        source: args.source,
        snapshot_path: args.snapshot_path,
        password,
        host: args.host,
        label: args.label,
        tags: args.tags,
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
