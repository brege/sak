use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Node type variants sent over the wire from sak-server.
/// Symlink carries raw link target bytes to handle non-unicode paths.
#[derive(Serialize, Deserialize)]
pub enum WireType {
    File,
    Dir,
    Symlink(Vec<u8>),
    Dev(u64),
    Chardev(u64),
    Fifo,
    Socket,
}

/// Per-entry metadata streamed from sak-server during directory traversal.
/// Timestamps are (unix_secs, nanos). Mode is raw POSIX st_mode (not Golang-mapped).
#[derive(Serialize, Deserialize)]
pub struct WireEntry {
    pub path: PathBuf,
    pub kind: WireType,
    pub mode: u32,
    pub mtime: Option<(i64, i32)>,
    pub atime: Option<(i64, i32)>,
    pub ctime: Option<(i64, i32)>,
    pub uid: u32,
    pub gid: u32,
    pub user: Option<String>,
    pub group: Option<String>,
    pub inode: u64,
    pub device_id: u64,
    pub size: u64,
    pub links: u64,
    pub xattrs: Vec<(String, Option<Vec<u8>>)>,
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Entry(Box<WireEntry>),
    FileChunk(Vec<u8>),
    EndFile,
    Done,
    Error(String),
}

#[derive(Serialize, Deserialize)]
pub enum ClientMsg {
    ReadFile(PathBuf),
    Shutdown,
}

/// Write a length-prefixed bincode frame to `w`.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let bytes = bincode::serialize(msg)?;
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Read a length-prefixed bincode frame from `r`.
/// Returns `Ok(None)` on clean EOF.
pub fn read_frame<R: Read + ?Sized, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    if let Err(e) = r.read_exact(&mut len_buf) {
        return if e.kind() == io::ErrorKind::UnexpectedEof {
            Ok(None)
        } else {
            Err(e.into())
        };
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(bincode::deserialize(&buf)?))
}
