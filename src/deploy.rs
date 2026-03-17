use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::server_source::ServerChannel;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServerConfig {
    #[serde(default)]
    pub key: Option<PathBuf>,
    #[serde(default)]
    pub binary: Option<PathBuf>,
}

impl ServerConfig {
    fn binary_path(&self) -> PathBuf {
        self.binary
            .clone()
            .unwrap_or_else(|| PathBuf::from(".local/bin/sak-server"))
    }
}

pub struct ServerSession {
    host: String,
    config: ServerConfig,
}

impl ServerSession {
    pub fn connect(host: &str, config: &ServerConfig) -> Result<Self> {
        let session = Self {
            host: host.to_string(),
            config: config.clone(),
        };
        session.ensure_binary()?;
        Ok(session)
    }

    pub fn start_server(&self, remote_path: &str) -> Result<ServerChannel> {
        let binary = self.config.binary_path();
        let cmd = format!("{} server {remote_path}", binary.display());

        let mut child = std::process::Command::new("ssh")
            .args(self.ssh_args(&cmd))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("failed to spawn ssh for sak server")?;

        let stdout = child.stdout.take().context("ssh child has no stdout")?;

        let stdin = child.stdin.take().context("ssh child has no stdin")?;

        Ok(ServerChannel {
            reader: Box::new(SshReader { stdout, child }),
            writer: Box::new(stdin),
        })
    }

    fn ensure_binary(&self) -> Result<()> {
        let binary = self.config.binary_path();
        let check_cmd = format!(
            "{} --version 2>/dev/null && echo ok || echo missing",
            binary.display()
        );

        let output = std::process::Command::new("ssh")
            .args(self.ssh_args(&check_cmd))
            .output()
            .context("failed to check remote sak-server version")?;

        let out = String::from_utf8_lossy(&output.stdout);
        let current = env!("CARGO_PKG_VERSION");

        if out.contains(current) {
            return Ok(());
        }

        log::debug!(
            "version check for {}: expected {:?}, got {:?}",
            self.host,
            current,
            out
        );
        log::info!("uploading sak-server to {}:{}", self.host, binary.display());
        self.upload_binary(&binary)
    }

    fn upload_binary(&self, binary: &Path) -> Result<()> {
        let local = std::env::current_exe().context("failed to locate sak binary")?;
        let bytes = std::fs::read(&local).context("failed to read sak binary")?;

        let parent = binary
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string();
        let mkdir_cmd = format!("mkdir -p {parent}");

        let status = std::process::Command::new("ssh")
            .args(self.ssh_args(&mkdir_cmd))
            .status()
            .context("failed to create remote binary directory")?;

        if !status.success() {
            bail!("mkdir on remote failed: {status}");
        }

        let upload_cmd = format!("cat > {0} && chmod +x {0}", binary.display());

        let mut child = std::process::Command::new("ssh")
            .args(self.ssh_args(&upload_cmd))
            .stdin(Stdio::piped())
            .spawn()
            .context("failed to spawn ssh for binary upload")?;

        child
            .stdin
            .take()
            .context("ssh child has no stdin")?
            .write_all(&bytes)
            .context("failed to write binary to remote")?;

        let status = child.wait().context("ssh upload failed")?;
        if !status.success() {
            bail!("binary upload failed: {status}");
        }

        Ok(())
    }

    fn ssh_args(&self, remote_cmd: &str) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(key) = &self.config.key {
            args.extend(["-i".to_string(), key.to_string_lossy().into_owned()]);
        }
        args.extend([
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            self.host.clone(),
            remote_cmd.to_string(),
        ]);
        args
    }
}

struct SshReader {
    stdout: std::process::ChildStdout,
    child: std::process::Child,
}

impl Read for SshReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Drop for SshReader {
    fn drop(&mut self) {
        let _ = self.child.wait();
    }
}
