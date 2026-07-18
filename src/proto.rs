use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const SERVER_PROTOCOL: &str = "sak-server/2";

const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

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

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    ReadFile(PathBuf),
    Shutdown,
}

/// Write a length-prefixed bincode frame to `w`.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    if bytes.len() > MAX_FRAME_SIZE {
        bail!("frame exceeds maximum size of {MAX_FRAME_SIZE} bytes");
    }
    let len = u32::try_from(bytes.len())?;
    w.write_all(&len.to_le_bytes())?;
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
    if len > MAX_FRAME_SIZE {
        bail!("frame exceeds maximum size of {MAX_FRAME_SIZE} bytes");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let (msg, consumed) = bincode::serde::decode_from_slice(&buf, bincode::config::standard())?;
    if consumed != buf.len() {
        bail!("frame contains trailing bytes");
    }
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_frame() -> Result<()> {
        let mut frame = Vec::new();
        write_frame(&mut frame, &ClientMsg::ReadFile(PathBuf::from("/srv/data")))?;

        let decoded = read_frame::<_, ClientMsg>(&mut frame.as_slice())?;
        match decoded {
            Some(ClientMsg::ReadFile(path)) => assert_eq!(path, PathBuf::from("/srv/data")),
            _ => panic!("unexpected decoded message"),
        }
        Ok(())
    }

    #[test]
    fn rejects_oversized_frame() {
        let len = u32::try_from(MAX_FRAME_SIZE + 1).unwrap();
        let err = read_frame::<_, ClientMsg>(&mut len.to_le_bytes().as_slice()).unwrap_err();

        assert!(err.to_string().contains("frame exceeds maximum size"));
    }

    #[test]
    fn rejects_trailing_bytes() -> Result<()> {
        let bytes =
            bincode::serde::encode_to_vec(ClientMsg::Shutdown, bincode::config::standard())?;
        let len = u32::try_from(bytes.len() + 1)?;
        let mut frame = len.to_le_bytes().to_vec();
        frame.extend(bytes);
        frame.push(0);

        let err = read_frame::<_, ClientMsg>(&mut frame.as_slice()).unwrap_err();
        assert!(err.to_string().contains("frame contains trailing bytes"));
        Ok(())
    }
}
