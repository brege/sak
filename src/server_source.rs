use std::{
    ffi::OsStr,
    io::{BufReader, BufWriter, Cursor, Read, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
    time::Instant,
};

use anyhow::Result;
use rustic_core::{
    ErrorKind, Excludes, LocalSourceFilterOptions, ReadSource, ReadSourceEntry, ReadSourceOpen,
    RusticError, RusticResult,
    node::{Metadata, Node, NodeType},
};

use crate::{
    RemoteFilters, build_remote_filters, include_remote_entry,
    proto::{ClientMsg, ServerMsg, WireEntry, WireType, read_frame, write_frame},
};

pub struct ServerChannel {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

type ResponseTx = mpsc::SyncSender<Result<Vec<u8>, String>>;

pub struct SakServerSource {
    root: PathBuf,
    _request_tx: crossbeam_channel::Sender<(PathBuf, ResponseTx)>,
    entries: Mutex<Vec<RusticResult<ReadSourceEntry<DemandOpen>>>>,
    _io_handles: IoHandles,
}

struct IoHandles {
    writer: Option<thread::JoinHandle<()>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl Drop for IoHandles {
    fn drop(&mut self) {
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl SakServerSource {
    pub fn new(
        root: PathBuf,
        excludes: Excludes,
        filter_opts: LocalSourceFilterOptions,
        channel: ServerChannel,
    ) -> Result<Self> {
        if !filter_opts.exclude_if_present.is_empty() {
            log::warn!("exclude-if-present is not supported in server mode; ignoring");
        }
        let filters = build_remote_filters(&excludes, &filter_opts)?;

        // Phase 1: drain metadata entries over the buffered reader.
        // The BufReader may read ahead past Done, so it must be reused for
        // Phase 2 to avoid losing buffered bytes.
        let mut reader = BufReader::with_capacity(256 * 1024, channel.reader);
        let wire_entries = drain_entries(&mut reader)?;

        // Build filtered entry list before spawning I/O threads.
        let (request_tx, request_rx) = crossbeam_channel::bounded::<(PathBuf, ResponseTx)>(64);

        let entries = filter_entries(wire_entries, &request_tx, &root, &filters);

        // FIFO queue: writer thread enqueues response channels, reader thread
        // dequeues them in the same order to match SSH pipe ordering.
        let (pending_tx, pending_rx) = crossbeam_channel::bounded::<ResponseTx>(64);

        let mut writer = BufWriter::with_capacity(64 * 1024, channel.writer);

        let writer_handle = thread::spawn(move || {
            for (path, resp_tx) in request_rx {
                if pending_tx.send(resp_tx).is_err() {
                    break;
                }
                if write_frame(&mut writer, &ClientMsg::ReadFile(path)).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
            let _ = write_frame(&mut writer, &ClientMsg::Shutdown);
            let _ = writer.flush();
        });

        let reader_handle = thread::spawn(move || {
            let t0 = Instant::now();
            let mut files_received: u64 = 0;
            let mut bytes_received: u64 = 0;

            for resp_tx in pending_rx {
                let mut buf = Vec::new();
                let result = loop {
                    match read_frame::<_, ServerMsg>(&mut reader) {
                        Ok(Some(ServerMsg::FileChunk(bytes))) => buf.extend_from_slice(&bytes),
                        Ok(Some(ServerMsg::EndFile)) => break Ok(buf),
                        Ok(Some(ServerMsg::Error(msg))) => break Err(msg),
                        Ok(Some(_)) => break Err("unexpected message during file read".into()),
                        Ok(None) => break Err("unexpected EOF during file read".into()),
                        Err(e) => break Err(e.to_string()),
                    }
                };
                if let Ok(ref data) = result {
                    files_received += 1;
                    bytes_received += data.len() as u64;
                }
                let _ = resp_tx.send(result);
            }

            let elapsed = t0.elapsed();
            log::info!(
                "phase 2: received {} files ({:.2} MiB) in {:.2}s ({:.2} MiB/s)",
                files_received,
                bytes_received as f64 / (1024.0 * 1024.0),
                elapsed.as_secs_f64(),
                bytes_received as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64().max(0.001),
            );
        });

        Ok(Self {
            root,
            _request_tx: request_tx,
            entries: Mutex::new(entries),
            _io_handles: IoHandles {
                writer: Some(writer_handle),
                reader: Some(reader_handle),
            },
        })
    }

    pub fn backup_root(&self) -> &Path {
        &self.root
    }
}

impl ReadSource for SakServerSource {
    type Open = DemandOpen;
    type Iter = ServerEntries;

    fn size(&self) -> RusticResult<Option<u64>> {
        Ok(None)
    }

    fn entries(&self) -> Self::Iter {
        let entries = {
            let mut guard = self.entries.lock().expect("entries mutex poisoned");
            std::mem::take(&mut *guard)
        };
        ServerEntries {
            entries: entries.into_iter(),
        }
    }
}

fn drain_entries(reader: &mut BufReader<Box<dyn Read + Send>>) -> Result<Vec<WireEntry>> {
    let t0 = Instant::now();
    let mut entries = Vec::new();
    loop {
        let msg: Option<ServerMsg> = read_frame(reader)?;
        match msg {
            Some(ServerMsg::Entry(wire)) => entries.push(*wire),
            Some(ServerMsg::Done) => break,
            Some(ServerMsg::Error(msg)) => {
                anyhow::bail!("sak server error: {msg}");
            }
            Some(_) => continue,
            None => anyhow::bail!("server connection lost during metadata scan"),
        }
    }
    let elapsed = t0.elapsed();
    log::info!(
        "phase 1: received {} entries in {:.2}s",
        entries.len(),
        elapsed.as_secs_f64(),
    );
    Ok(entries)
}

fn filter_entries(
    wire_entries: Vec<WireEntry>,
    request_tx: &crossbeam_channel::Sender<(PathBuf, ResponseTx)>,
    root: &Path,
    filters: &RemoteFilters,
) -> Vec<RusticResult<ReadSourceEntry<DemandOpen>>> {
    let mut excluded_dirs: Vec<PathBuf> = Vec::new();
    let mut kept = Vec::new();

    for wire in wire_entries {
        if excluded_dirs.iter().any(|d| wire.path.starts_with(d)) {
            continue;
        }
        match wire_to_entry(wire, request_tx) {
            Ok(entry) => {
                if !include_remote_entry(&entry.path, &entry.node, root, filters) {
                    if entry.node.is_dir() {
                        excluded_dirs.push(entry.path);
                    }
                    continue;
                }
                kept.push(Ok(entry));
            }
            Err(e) => {
                kept.push(Err(RusticError::with_source(
                    ErrorKind::InputOutput,
                    "Failed to convert server entry.",
                    e,
                )
                .ask_report()));
            }
        }
    }

    log::info!("phase 1: {} entries after filtering", kept.len());
    kept
}

pub struct ServerEntries {
    entries: std::vec::IntoIter<RusticResult<ReadSourceEntry<DemandOpen>>>,
}

impl Iterator for ServerEntries {
    type Item = RusticResult<ReadSourceEntry<DemandOpen>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next()
    }
}

pub struct DemandOpen {
    path: PathBuf,
    request_tx: crossbeam_channel::Sender<(PathBuf, ResponseTx)>,
}

impl ReadSourceOpen for DemandOpen {
    type Reader = Cursor<Vec<u8>>;

    fn open(self) -> RusticResult<Self::Reader> {
        let t0 = Instant::now();
        let (resp_tx, resp_rx) = mpsc::sync_channel(1);

        self.request_tx
            .send((self.path.clone(), resp_tx))
            .map_err(|_| {
                RusticError::new(ErrorKind::Internal, "Content pipeline closed.").ask_report()
            })?;

        let buf = resp_rx
            .recv()
            .map_err(|_| {
                RusticError::new(ErrorKind::Internal, "Content pipeline reader dropped.")
                    .ask_report()
            })?
            .map_err(|msg| {
                RusticError::new(
                    ErrorKind::InputOutput,
                    "Server error reading `{path}`: {msg}",
                )
                .attach_context("path", self.path.display().to_string())
                .attach_context("msg", msg)
                .ask_report()
            })?;

        log::debug!(
            "phase 2: open {} ({:.2} MiB, {:.1}ms)",
            self.path.display(),
            buf.len() as f64 / (1024.0 * 1024.0),
            t0.elapsed().as_secs_f64() * 1000.0,
        );

        Ok(Cursor::new(buf))
    }
}

fn wire_to_entry(
    wire: WireEntry,
    request_tx: &crossbeam_channel::Sender<(PathBuf, ResponseTx)>,
) -> Result<ReadSourceEntry<DemandOpen>> {
    let name = wire
        .path
        .file_name()
        .unwrap_or_else(|| wire.path.as_os_str());

    let node_type = wire_node_type(&wire.kind)?;
    let is_file = matches!(node_type, NodeType::File);

    let meta = Metadata {
        mode: Some(map_mode_to_go(wire.mode)),
        mtime: wire
            .mtime
            .and_then(|(s, _)| jiff::Timestamp::new(s, 0).ok()),
        atime: wire
            .atime
            .and_then(|(s, _)| jiff::Timestamp::new(s, 0).ok()),
        ctime: wire
            .ctime
            .and_then(|(s, _)| jiff::Timestamp::new(s, 0).ok()),
        uid: Some(wire.uid),
        gid: Some(wire.gid),
        user: wire.user,
        group: wire.group,
        inode: wire.inode,
        device_id: wire.device_id,
        size: wire.size,
        links: wire.links,
        extended_attributes: Vec::new(),
    };

    let node = Node::new_node(name, node_type, meta);
    let open = is_file.then(|| DemandOpen {
        path: wire.path.clone(),
        request_tx: request_tx.clone(),
    });

    Ok(ReadSourceEntry {
        path: wire.path,
        node,
        open,
    })
}

fn wire_node_type(kind: &WireType) -> Result<NodeType> {
    Ok(match kind {
        WireType::File => NodeType::File,
        WireType::Dir => NodeType::Dir,
        WireType::Symlink(target) => {
            let link_path = Path::new(OsStr::from_bytes(target));
            NodeType::from_link(link_path)
        }
        WireType::Dev(device) => NodeType::Dev { device: *device },
        WireType::Chardev(device) => NodeType::Chardev { device: *device },
        WireType::Fifo => NodeType::Fifo,
        WireType::Socket => NodeType::Socket,
    })
}

const fn map_mode_to_go(mode: u32) -> u32 {
    const MODE_PERM: u32 = 0o777;
    const S_IFFORMAT: u32 = 0o170_000;
    const S_IFSOCK: u32 = 0o140_000;
    const S_IFLNK: u32 = 0o120_000;
    const S_IFBLK: u32 = 0o060_000;
    const S_IFDIR: u32 = 0o040_000;
    const S_IFCHR: u32 = 0o020_000;
    const S_IFIFO: u32 = 0o010_000;
    const S_ISUID: u32 = 0o4000;
    const S_ISGID: u32 = 0o2000;
    const S_ISVTX: u32 = 0o1000;
    const GO_MODE_DIR: u32 = 0b1000_0000_0000_0000_0000_0000_0000_0000;
    const GO_MODE_SYMLINK: u32 = 0b0000_1000_0000_0000_0000_0000_0000_0000;
    const GO_MODE_DEVICE: u32 = 0b0000_0100_0000_0000_0000_0000_0000_0000;
    const GO_MODE_FIFO: u32 = 0b0000_0010_0000_0000_0000_0000_0000_0000;
    const GO_MODE_SOCKET: u32 = 0b0000_0001_0000_0000_0000_0000_0000_0000;
    const GO_MODE_SETUID: u32 = 0b0000_0000_1000_0000_0000_0000_0000_0000;
    const GO_MODE_SETGID: u32 = 0b0000_0000_0100_0000_0000_0000_0000_0000;
    const GO_MODE_CHARDEV: u32 = 0b0000_0000_0010_0000_0000_0000_0000_0000;
    const GO_MODE_STICKY: u32 = 0b0000_0000_0001_0000_0000_0000_0000_0000;
    const GO_MODE_IRREG: u32 = 0b0000_0000_0000_1000_0000_0000_0000_0000;

    let mut go_mode = mode & MODE_PERM;

    go_mode |= match mode & S_IFFORMAT {
        S_IFSOCK => GO_MODE_SOCKET,
        S_IFLNK => GO_MODE_SYMLINK,
        S_IFBLK => GO_MODE_DEVICE,
        S_IFDIR => GO_MODE_DIR,
        S_IFCHR => GO_MODE_CHARDEV & GO_MODE_DEVICE,
        S_IFIFO => GO_MODE_FIFO,
        0o100_000 => 0, // S_IFREG
        _ => GO_MODE_IRREG,
    };

    if mode & S_ISUID > 0 {
        go_mode |= GO_MODE_SETUID;
    }
    if mode & S_ISGID > 0 {
        go_mode |= GO_MODE_SETGID;
    }
    if mode & S_ISVTX > 0 {
        go_mode |= GO_MODE_STICKY;
    }

    go_mode
}
