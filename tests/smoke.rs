use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

const PASSWORD: &str = "test";

#[test]
#[ignore = "manual smoke benchmark against vendored rustic release binary"]
fn compare_release_sak_to_vendored_rustic() -> Result<()> {
    let workspace = tempdir()?;
    let source = workspace.path().join("source");
    let sak_repo = workspace.path().join("sak-repo");
    let rustic_repo = workspace.path().join("rustic-repo");
    let sak_bin = require_binary("target/release/sak", "cargo build --release")?;
    let rustic_bin = require_binary(
        "refs/rustic/target/release/rustic",
        "cargo build --release --manifest-path refs/rustic/Cargo.toml",
    )?;

    create_fixture(&source, 256 * 1024 * 1024, 64)?;

    let sak_first = time(|| run_sak(&sak_bin, &sak_repo, &source))?;
    let sak_second = time(|| run_sak(&sak_bin, &sak_repo, &source))?;
    let rustic_first = time(|| run_rustic(&rustic_bin, &rustic_repo, &source, true))?;
    let rustic_second = time(|| run_rustic(&rustic_bin, &rustic_repo, &source, false))?;

    eprintln!("source: {}", source.display());
    eprintln!("sak repo: {}", sak_repo.display());
    eprintln!("rustic repo: {}", rustic_repo.display());
    eprintln!("sak first: {:.3}s", sak_first.as_secs_f64());
    eprintln!("sak second: {:.3}s", sak_second.as_secs_f64());
    eprintln!("rustic first: {:.3}s", rustic_first.as_secs_f64());
    eprintln!("rustic second: {:.3}s", rustic_second.as_secs_f64());

    Ok(())
}

fn require_binary(rel: &str, build_hint: &str) -> Result<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    if path.is_file() {
        Ok(path)
    } else {
        bail!("missing {} ; build with {}", path.display(), build_hint)
    }
}

fn run_sak(binary: &Path, repo: &Path, source: &Path) -> Result<()> {
    let output = Command::new(binary)
        .arg("import")
        .arg("--repository")
        .arg(repo)
        .arg("--source")
        .arg(source)
        .arg("--snapshot-path")
        .arg("fixture")
        .arg("--host")
        .arg("bench-host")
        .arg("--label")
        .arg("bench")
        .arg("--tag")
        .arg("bench")
        .arg("--password")
        .arg(PASSWORD)
        .output()
        .context("failed to run sak smoke import")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "sak smoke import failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn run_rustic(binary: &Path, repo: &Path, source: &Path, init: bool) -> Result<()> {
    let mut command = Command::new(binary);
    command
        .arg("--repository")
        .arg(repo)
        .arg("--password")
        .arg(PASSWORD)
        .arg("--no-cache")
        .arg("backup");
    if init {
        command.arg("--init");
    }
    let output = command
        .arg("--as-path")
        .arg("fixture")
        .arg("--host")
        .arg("bench-host")
        .arg("--label")
        .arg("bench")
        .arg("--tag")
        .arg("bench")
        .arg(source)
        .output()
        .context("failed to run rustic smoke backup")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "rustic smoke backup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn time(f: impl FnOnce() -> Result<()>) -> Result<Duration> {
    let start = Instant::now();
    f()?;
    Ok(start.elapsed())
}

fn create_fixture(root: &Path, total_bytes: u64, files: usize) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create fixture root {}", root.display()))?;
    let base = total_bytes / files as u64;
    let remainder = total_bytes % files as u64;

    for index in 0..files {
        let rel = PathBuf::from(format!(
            "section-{section:02}/shelf-{shelf:02}/item-{index:04}.bin",
            section = index % 8,
            shelf = (index / 8) % 8,
        ));
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let size = base + u64::from((index as u64) < remainder);
        write_fixture_file(&path, size, index as u64)?;
    }

    Ok(())
}

fn write_fixture_file(path: &Path, size: u64, seed: u64) -> Result<()> {
    let mut file = File::create(path)
        .with_context(|| format!("failed to create fixture file {}", path.display()))?;
    let mut rng = XorShift64::new(seed + 1);
    let mut remaining = size;
    let mut buf = [0u8; 64 * 1024];

    while remaining > 0 {
        let len = remaining.min(buf.len() as u64) as usize;
        fill_random(&mut buf[..len], &mut rng);
        file.write_all(&buf[..len])?;
        remaining -= len as u64;
    }

    Ok(())
}

fn fill_random(buf: &mut [u8], rng: &mut XorShift64) {
    let mut chunks = buf.chunks_exact_mut(8);
    for chunk in &mut chunks {
        chunk.copy_from_slice(&rng.next().to_le_bytes());
    }
    let rem = chunks.into_remainder();
    if !rem.is_empty() {
        let tail = rng.next().to_le_bytes();
        rem.copy_from_slice(&tail[..rem.len()]);
    }
}

#[derive(Debug, Clone)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
