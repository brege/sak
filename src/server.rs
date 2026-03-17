use std::{
    collections::HashMap,
    io::{self, BufReader, BufWriter, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt},
    },
    path::Path,
    time::Instant,
};

use anyhow::{Result, bail};
use walkdir::WalkDir;

use crate::proto::{ClientMsg, ServerMsg, WireEntry, WireType, read_frame, write_frame};

pub fn run_server(path: &str) -> Result<()> {
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(512 * 1024, stdout.lock());
    let stdin = io::stdin();
    let mut input = BufReader::with_capacity(64 * 1024, stdin.lock());

    let result = stream_entries(path, &mut out);

    let final_msg = match &result {
        Ok(()) => ServerMsg::Done,
        Err(e) => ServerMsg::Error(e.to_string()),
    };
    write_frame(&mut out, &final_msg)?;
    out.flush()?;

    result?;

    serve_content(&mut input, &mut out)
}

fn stream_entries<W: Write>(path: &str, out: &mut W) -> Result<()> {
    let root = Path::new(path);
    if !root.exists() {
        bail!("path does not exist: {path}");
    }

    let t0 = Instant::now();
    let mut entry_count: u64 = 0;
    let mut file_count: u64 = 0;
    let mut total_size: u64 = 0;
    let mut uid_cache: HashMap<u32, Option<String>> = HashMap::new();
    let mut gid_cache: HashMap<u32, Option<String>> = HashMap::new();

    for dent in WalkDir::new(root).sort_by_file_name().follow_links(false) {
        let dent = match dent {
            Ok(d) => d,
            Err(e) => {
                log::warn!("skipping entry: {e}");
                continue;
            }
        };

        let meta = match dent.metadata() {
            Ok(m) => m,
            Err(e) => {
                log::warn!("skipping {}: {e}", dent.path().display());
                continue;
            }
        };

        let kind = match entry_kind(dent.path(), &meta) {
            Ok(Some(k)) => k,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("skipping {}: {e}", dent.path().display());
                continue;
            }
        };

        let uid = meta.uid();
        let gid = meta.gid();
        let user = uid_cache
            .entry(uid)
            .or_insert_with(|| user_name(uid))
            .clone();
        let group = gid_cache
            .entry(gid)
            .or_insert_with(|| group_name(gid))
            .clone();
        let size = if matches!(kind, WireType::File) {
            meta.len()
        } else {
            0
        };

        let wire = WireEntry {
            path: dent.path().to_path_buf(),
            kind,
            mode: meta.mode(),
            mtime: stamp(meta.mtime(), meta.mtime_nsec()),
            atime: stamp(meta.atime(), meta.atime_nsec()),
            ctime: stamp(meta.ctime(), meta.ctime_nsec()),
            uid,
            gid,
            user,
            group,
            inode: meta.ino(),
            device_id: meta.dev(),
            size,
            links: if meta.is_dir() { 0 } else { meta.nlink() },
            xattrs: Vec::new(),
        };

        entry_count += 1;
        if matches!(wire.kind, WireType::File) {
            file_count += 1;
            total_size += size;
        }
        write_frame(out, &ServerMsg::Entry(Box::new(wire)))?;
    }

    let elapsed = t0.elapsed();
    log::info!(
        "phase 1: scanned {} entries ({} files, {:.2} MiB) in {:.2}s",
        entry_count,
        file_count,
        total_size as f64 / (1024.0 * 1024.0),
        elapsed.as_secs_f64(),
    );
    Ok(())
}

fn serve_content<R: Read, W: Write>(input: &mut R, out: &mut W) -> Result<()> {
    let t0 = Instant::now();
    let mut files_read: u64 = 0;
    let mut bytes_sent: u64 = 0;
    let mut errors: u64 = 0;

    loop {
        let msg: Option<ClientMsg> = read_frame(input)?;
        match msg {
            Some(ClientMsg::ReadFile(path)) => {
                let file_start = Instant::now();
                let mut file_bytes: u64 = 0;
                let mut failed = false;
                match std::fs::File::open(&path) {
                    Ok(mut f) => {
                        let mut buf = vec![0u8; 256 * 1024];
                        loop {
                            match f.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    file_bytes += n as u64;
                                    write_frame(out, &ServerMsg::FileChunk(buf[..n].to_vec()))?;
                                }
                                Err(e) => {
                                    log::warn!("read error {}: {e}", path.display());
                                    errors += 1;
                                    failed = true;
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("cannot open {}: {e}", path.display());
                        errors += 1;
                        failed = true;
                    }
                }
                if failed {
                    let msg = format!("failed to read {}", path.display());
                    write_frame(out, &ServerMsg::Error(msg))?;
                } else {
                    write_frame(out, &ServerMsg::EndFile)?;
                }
                out.flush()?;
                files_read += 1;
                bytes_sent += file_bytes;
                log::debug!(
                    "phase 2: read {} ({:.2} MiB, {:.1}ms)",
                    path.display(),
                    file_bytes as f64 / (1024.0 * 1024.0),
                    file_start.elapsed().as_secs_f64() * 1000.0,
                );
            }
            Some(ClientMsg::Shutdown) | None => break,
        }
    }

    let elapsed = t0.elapsed();
    log::info!(
        "phase 2: served {} files ({:.2} MiB) in {:.2}s ({:.2} MiB/s), {} errors",
        files_read,
        bytes_sent as f64 / (1024.0 * 1024.0),
        elapsed.as_secs_f64(),
        bytes_sent as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64().max(0.001),
        errors,
    );
    Ok(())
}

fn entry_kind(path: &Path, meta: &std::fs::Metadata) -> Result<Option<WireType>> {
    let ft = meta.file_type();
    if ft.is_dir() {
        return Ok(Some(WireType::Dir));
    }
    if ft.is_file() {
        return Ok(Some(WireType::File));
    }
    if ft.is_symlink() {
        let target = std::fs::read_link(path)?;
        return Ok(Some(WireType::Symlink(
            target.as_os_str().as_bytes().to_vec(),
        )));
    }
    if ft.is_block_device() {
        return Ok(Some(WireType::Dev(meta.rdev())));
    }
    if ft.is_char_device() {
        return Ok(Some(WireType::Chardev(meta.rdev())));
    }
    if ft.is_fifo() {
        return Ok(Some(WireType::Fifo));
    }
    if ft.is_socket() {
        return Ok(Some(WireType::Socket));
    }
    log::warn!("skipping unsupported file type: {}", path.display());
    Ok(None)
}

fn stamp(secs: i64, nsecs: i64) -> Option<(i64, i32)> {
    if secs == 0 && nsecs == 0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some((secs, nsecs as i32))
}

fn user_name(uid: u32) -> Option<String> {
    nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()?
        .map(|u| u.name)
}

fn group_name(gid: u32) -> Option<String> {
    nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(gid))
        .ok()?
        .map(|g| g.name)
}
