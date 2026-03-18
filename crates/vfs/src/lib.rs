#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use fs_ext4::{Ext4Mount, Ext4NodeKind};
use hal_api::hal;
use spin::Mutex;

pub type KernelResult<T> = Result<T, i32>;

pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const HANDLE_FLAG_CLOEXEC: u32 = 1 << 31;

const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const EAGAIN: i32 = 11;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const EROFS: i32 = 30;
const ENOTEMPTY: i32 = 39;

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFIFO: u32 = 0o010000;
const S_IFLNK: u32 = 0o120000;
const S_IFSOCK: u32 = 0o140000;
const PROC_MEMINFO: &[u8] = b"MemTotal:       1048576 kB\nMemFree:         524288 kB\nMemAvailable:    524288 kB\nBuffers:              0 kB\nCached:               0 kB\nSwapTotal:            0 kB\nSwapFree:             0 kB\n";
const PROC_UPTIME: &[u8] = b"1.00 1.00\n";
const PROC_STAT: &[u8] = b"cpu  1 0 1 1 0 0 0 0 0 0\nintr 0\nctxt 0\nbtime 1735689600\nprocesses 1\nprocs_running 1\nprocs_blocked 0\n";
const PROC_VERSION: &[u8] = b"Linux version 6.8.0-whuse (whuse@localdomain) #1 SMP PREEMPT\n";
const PROC_SELF_STAT: &[u8] = b"1 (self) R 0 0 0 0 0 0 0 0 0 0 0 0 0 0 20 0 1 0 1 4096 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
const EXT4_DIR_STAT_CACHE_MAX_SIZE: u64 = 512 * 1024;

fn pipe_debug_enabled() -> bool {
    match option_env!("WHUSE_DEBUG_PIPE") {
        Some("1") => true,
        _ => false,
    }
}

fn pipe_debug(line: &str) {
    if !pipe_debug_enabled() {
        return;
    }
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    Directory,
    File,
    CharDevice,
    Proc,
    Pipe,
    Symlink,
    Event,
    Epoll,
    Socket,
    PidFd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileStat {
    pub mode: u32,
    pub size: u64,
    pub nlink: u32,
}

#[derive(Clone, Debug)]
pub struct MountRecord {
    pub source: String,
    pub target: String,
    pub fs_type: String,
}

pub struct FileHandle {
    node: Arc<Node>,
    pub offset: usize,
    pub flags: u32,
    pub path: String,
    pipe_end: PipeEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PipeEnd {
    None,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Regular,
    Directory,
    Pipe,
    EventFd,
    Epoll,
    MemFd,
    PidFd,
    SocketLocal,
    CharDevice,
    Procfs,
    Symlink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpollWatch {
    pub fd: i32,
    pub events: u32,
}

pub trait KernelObject {
    fn object_kind(&self) -> ObjectKind;
    fn read_object(&mut self, len: usize) -> KernelResult<Vec<u8>>;
    fn write_object(&mut self, data: &[u8]) -> KernelResult<usize>;
    fn seek_object(&mut self, offset: isize, whence: u32) -> KernelResult<usize>;
    fn poll_read_ready(&self) -> bool;
    fn poll_write_ready(&self) -> bool;
    fn stat_object(&self) -> KernelResult<FileStat>;
    fn ioctl_object(&mut self, _cmd: usize, _arg: usize) -> KernelResult<usize> {
        Err(EINVAL)
    }
    fn fcntl_object(&mut self, _cmd: usize, _arg: usize) -> KernelResult<usize> {
        Err(EINVAL)
    }
    fn close_object(&mut self) -> KernelResult<()> {
        Ok(())
    }
}

struct Node {
    _name: String,
    kind: NodeKind,
    data: Mutex<NodeData>,
}

struct SocketChannel {
    inbox: [VecDeque<u8>; 2],
}

struct SocketPending {
    path: Option<String>,
    listening: bool,
    pending: Vec<Arc<Node>>,
}

#[derive(Clone)]
struct Ext4FileState {
    mount: Ext4Mount,
    path: String,
    mode: u32,
    size: u64,
    cached: Option<Arc<Vec<u8>>>,
}

#[derive(Clone)]
struct Ext4DirState {
    mount: Ext4Mount,
    path: String,
    mode: u32,
    size: u64,
    entries: Option<Arc<Vec<fs_ext4::Ext4DirEntryLite>>>,
}

struct PipeState {
    buf: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

enum NodeData {
    Directory(BTreeMap<String, Arc<Node>>),
    File(Vec<u8>),
    Ext4File(Ext4FileState),
    Ext4Dir(Ext4DirState),
    CharDevice,
    ProcFile(Vec<u8>),
    Pipe(PipeState),
    Symlink(String),
    Event(u64),
    Epoll(Vec<EpollWatch>),
    SocketPending(SocketPending),
    SocketConnected {
        channel: Arc<Mutex<SocketChannel>>,
        side: usize,
    },
    PidFd(usize),
}

#[derive(Clone)]
struct ExternalMount {
    target: String,
    ext4: Ext4Mount,
}

pub struct KernelVfs {
    root: Arc<Node>,
    mounts: Vec<MountRecord>,
    external_mounts: Vec<ExternalMount>,
    external_stat_cache: BTreeMap<String, FileStat>,
    external_preloaded: BTreeMap<String, (Arc<Vec<u8>>, FileStat)>,
    next_pipe_id: usize,
    next_memfd_id: usize,
    socket_bindings: BTreeMap<String, Arc<Node>>,
}

impl KernelVfs {
    pub fn new() -> Self {
        let root = Arc::new(Node::directory("/"));
        let mut vfs = Self {
            root,
            mounts: Vec::new(),
            external_mounts: Vec::new(),
            external_stat_cache: BTreeMap::new(),
            external_preloaded: BTreeMap::new(),
            next_pipe_id: 0,
            next_memfd_id: 0,
            socket_bindings: BTreeMap::new(),
        };
        for dir in ["/dev", "/proc", "/mnt", "/tmp", "/bin", "/etc"] {
            let _ = vfs.mkdir("/", dir, 0o755);
        }
        let _ = vfs.create_char_device("/dev/console", "console");
        let _ = vfs.create_char_device("/dev/null", "null");
        let _ = vfs.create_char_device("/dev/zero", "zero");
        let _ = vfs.create_char_device("/dev/random", "random");
        let _ = vfs.create_char_device("/dev/urandom", "urandom");
        let _ = vfs.create_char_device("/dev/rtc0", "rtc0");
        let _ = vfs.create_proc_file("/proc/mounts", b"");
        let _ = vfs.create_proc_file("/proc/meminfo", PROC_MEMINFO);
        let _ = vfs.create_proc_file("/proc/uptime", PROC_UPTIME);
        let _ = vfs.create_proc_file("/proc/stat", PROC_STAT);
        let _ = vfs.create_proc_file("/proc/version", PROC_VERSION);
        let _ = vfs.mkdir("/", "/proc/self", 0o755);
        let _ = vfs.create_proc_file("/proc/self/stat", PROC_SELF_STAT);
        vfs
    }

    pub fn create_char_device(&mut self, path: &str, name: &'static str) -> KernelResult<()> {
        let _ = name;
        self.create_node(path, NodeKind::CharDevice, Some(NodeData::CharDevice))
    }

    pub fn create_proc_file(&mut self, path: &str, contents: &[u8]) -> KernelResult<()> {
        self.create_node(
            path,
            NodeKind::Proc,
            Some(NodeData::ProcFile(contents.to_vec())),
        )
    }

    pub fn create_file(&mut self, cwd: &str, path: &str, contents: &[u8]) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(
            &absolute,
            NodeKind::File,
            Some(NodeData::File(contents.to_vec())),
        )
    }

    pub fn preload_external_file(
        &mut self,
        path: &str,
        contents: &[u8],
        mode: Option<u32>,
    ) -> KernelResult<()> {
        let absolute = normalize_path("/", path);
        let stat = FileStat {
            mode: mode.unwrap_or(S_IFREG | 0o755),
            size: contents.len() as u64,
            nlink: 1,
        };
        self.external_preloaded
            .insert(absolute.clone(), (Arc::new(contents.to_vec()), stat));
        self.external_stat_cache.insert(absolute, stat);
        Ok(())
    }

    pub fn create_symlink(&mut self, cwd: &str, path: &str, target: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(
            &absolute,
            NodeKind::Symlink,
            Some(NodeData::Symlink(target.to_string())),
        )
    }

    pub fn mkdir(&mut self, cwd: &str, path: &str, _mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(&absolute, NodeKind::Directory, None)
    }

    pub fn open(
        &mut self,
        cwd: &str,
        path: &str,
        flags: u32,
        _mode: u32,
    ) -> KernelResult<FileHandle> {
        let absolute = normalize_path(cwd, path);
        if let Some(handle) = self.try_open_external(&absolute, flags)? {
            return Ok(handle);
        }
        self.open_mem(&absolute, flags)
    }

    fn open_mem(&mut self, absolute: &str, flags: u32) -> KernelResult<FileHandle> {
        let mut resolved = absolute.to_string();
        let node = match self.lookup_abs(&resolved) {
            Ok(node) => node,
            Err(err) if err == ENOENT && (flags & O_CREAT) != 0 => {
                self.create_file("/", &resolved, b"")?;
                self.lookup_abs(&resolved)?
            }
            Err(err) => return Err(err),
        };
        let node = if node.kind == NodeKind::Symlink {
            let target = match &*node.data.lock() {
                NodeData::Symlink(target) => target.clone(),
                _ => return Err(EINVAL),
            };
            let parent = split_parent(&resolved)?.0;
            resolved = normalize_path(&parent, &target);
            match self.lookup_abs(&resolved) {
                Ok(node) => node,
                Err(err) if err == ENOENT => {
                    if let Some(handle) = self.try_open_external(&resolved, flags)? {
                        return Ok(handle);
                    }
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        } else {
            node
        };

        if (flags & O_DIRECTORY) != 0 && node.kind != NodeKind::Directory {
            return Err(ENOTDIR);
        }

        if (flags & O_TRUNC) != 0 {
            if let NodeData::File(buf) = &mut *node.data.lock() {
                buf.clear();
            }
        }

        Ok(FileHandle {
            node,
            offset: 0,
            flags,
            path: resolved,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn read(&self, handle: &mut FileHandle, len: usize) -> KernelResult<Vec<u8>> {
        handle.read_object(len)
    }

    pub fn write(&mut self, handle: &mut FileHandle, data: &[u8]) -> KernelResult<usize> {
        handle.write_object(data)
    }

    pub fn seek(&self, handle: &mut FileHandle, offset: isize, whence: u32) -> KernelResult<usize> {
        handle.seek_object(offset, whence)
    }

    pub fn getdents(&mut self, handle: &mut FileHandle, max_len: usize) -> KernelResult<Vec<u8>> {
        if max_len == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let start = handle.offset;
        let mut emitted = 0usize;
        match &mut *handle.node.data.lock() {
            NodeData::Directory(entries) => {
                for (index, (name, node)) in entries.iter().enumerate().skip(start) {
                    let reclen = align_up(19 + name.len() + 1, 8);
                    if emitted != 0 && out.len() + reclen > max_len {
                        break;
                    }
                    append_dirent(&mut out, index, name, node_kind_to_dirent_type(node.kind));
                    emitted += 1;
                }
            }
            NodeData::Ext4Dir(state) => {
                let entries = if let Some(entries) = &state.entries {
                    Arc::clone(entries)
                } else {
                    let entries = if state.size <= EXT4_DIR_STAT_CACHE_MAX_SIZE {
                        state
                            .mount
                            .read_dir(&state.path)?
                            .into_iter()
                            .filter(|entry| entry.name != "." && entry.name != "..")
                            .map(|entry| {
                                self.cache_external_dirent_stat(
                                    &handle.path,
                                    &entry.name,
                                    entry.stat,
                                );
                                fs_ext4::Ext4DirEntryLite {
                                    name: entry.name,
                                    kind: entry.kind,
                                }
                            })
                            .collect::<Vec<_>>()
                    } else {
                        state
                            .mount
                            .read_dir_lite(&state.path)?
                            .into_iter()
                            .filter(|entry| entry.name != "." && entry.name != "..")
                            .collect::<Vec<_>>()
                    };
                    let entries = Arc::new(entries);
                    state.entries = Some(Arc::clone(&entries));
                    entries
                };
                let Some(remaining) = entries.get(start..) else {
                    handle.offset = entries.len();
                    return Ok(out);
                };
                for (relative_index, entry) in remaining.iter().enumerate() {
                    let logical_index = start + relative_index;
                    let reclen = align_up(19 + entry.name.len() + 1, 8);
                    if emitted != 0 && out.len() + reclen > max_len {
                        break;
                    }
                    append_dirent(
                        &mut out,
                        logical_index,
                        &entry.name,
                        ext4_kind_to_dirent_type(entry.kind),
                    );
                    emitted += 1;
                }
            }
            _ => return Err(ENOTDIR),
        }
        handle.offset = handle.offset.saturating_add(emitted);
        Ok(out)
    }

    pub fn stat_path(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        self.stat_abs_path(&absolute, true, 0)
    }

    pub fn stat_path_nofollow(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        self.stat_abs_path(&absolute, false, 0)
    }

    pub fn chdir(&self, cwd: &str, path: &str) -> KernelResult<String> {
        let absolute = normalize_path(cwd, path);
        if let Ok(node) = self.lookup_abs(&absolute) {
            if node.kind != NodeKind::Directory {
                return Err(ENOTDIR);
            }
            return Ok(absolute);
        }
        if let Some(stat) = self.external_stat_path(&absolute, true, 0)? {
            if (stat.mode & S_IFDIR) != S_IFDIR {
                return Err(ENOTDIR);
            }
            return Ok(absolute);
        }
        Err(ENOENT)
    }

    pub fn unlink(&mut self, cwd: &str, path: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let (parent_path, name) = split_parent(&absolute)?;
        let parent = self.lookup_abs(&parent_path)?;
        let mut guard = parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        let node = entries.get(name).ok_or(ENOENT)?;
        if node.kind == NodeKind::Directory {
            if let NodeData::Directory(children) = &*node.data.lock() {
                if !children.is_empty() {
                    return Err(ENOTEMPTY);
                }
            }
        }
        entries.remove(name);
        self.socket_bindings.remove(&absolute);
        Ok(())
    }

    pub fn link(&mut self, cwd: &str, old_path: &str, new_path: &str) -> KernelResult<()> {
        let old_absolute = normalize_path(cwd, old_path);
        let new_absolute = normalize_path(cwd, new_path);
        let node = self.lookup_abs(&old_absolute)?;
        let (parent_path, name) = split_parent(&new_absolute)?;
        let parent = self.lookup_abs(&parent_path)?;
        let mut guard = parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        if entries.contains_key(name) {
            return Err(EEXIST);
        }
        entries.insert(name.to_string(), node);
        Ok(())
    }

    pub fn mount(&mut self, source: &str, target: &str, fs_type: &str) -> KernelResult<()> {
        let absolute = normalize_path("/", target);
        let _ = self.lookup_abs(&absolute)?;
        self.mounts.push(MountRecord {
            source: source.to_string(),
            target: absolute,
            fs_type: fs_type.to_string(),
        });
        self.refresh_mounts_proc();
        Ok(())
    }

    pub fn mount_ext4(
        &mut self,
        source: &str,
        target: &str,
        device: &'static dyn hal_api::HalBlockDevice,
    ) -> KernelResult<String> {
        let absolute = normalize_path("/", target);
        let mount = Ext4Mount::probe(device)?;
        let label = mount.label().to_string();
        self.external_mounts
            .retain(|existing| existing.target != absolute);
        self.external_mounts.push(ExternalMount {
            target: absolute.clone(),
            ext4: mount,
        });
        self.external_stat_cache.clear();
        self.external_preloaded.clear();
        self.mounts.retain(|existing| existing.target != absolute);
        self.mounts.push(MountRecord {
            source: source.to_string(),
            target: absolute,
            fs_type: "ext4".to_string(),
        });
        self.refresh_mounts_proc();
        Ok(label)
    }

    pub fn umount(&mut self, target: &str) -> KernelResult<()> {
        let absolute = normalize_path("/", target);
        let before = self.mounts.len();
        let external_before = self.external_mounts.len();
        self.mounts.retain(|mount| mount.target != absolute);
        self.external_mounts
            .retain(|mount| mount.target != absolute);
        if self.external_mounts.len() != external_before {
            self.external_stat_cache.clear();
            self.external_preloaded.clear();
        }
        if before == self.mounts.len() {
            return Err(ENOENT);
        }
        self.refresh_mounts_proc();
        Ok(())
    }

    pub fn cwd_string(&self, cwd: &str) -> String {
        normalize_path("/", cwd)
    }

    pub fn access(&self, cwd: &str, path: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.path_exists(&absolute)
    }

    pub fn access_precise(&self, cwd: &str, path: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.stat_abs_path(&absolute, true, 0).map(|_| ())
    }

    pub fn read_file_all(&mut self, cwd: &str, path: &str) -> KernelResult<Vec<u8>> {
        let mut handle = self.open(cwd, path, O_RDONLY, 0)?;
        let size = self.stat_handle(&handle)?.size as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(size);
        while out.len() < size {
            let chunk_len = size - out.len();
            let chunk = self.read(&mut handle, chunk_len)?;
            if chunk.is_empty() {
                break;
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    pub fn read_link(&self, cwd: &str, path: &str) -> KernelResult<String> {
        let absolute = normalize_path(cwd, path);
        if let Some((mount, fs_path)) = self.resolve_external_path(&absolute) {
            return mount.ext4.read_link(&fs_path);
        }
        let node = self.lookup_abs(&absolute)?;
        let result = match &*node.data.lock() {
            NodeData::Symlink(target) => Ok(target.clone()),
            _ => Err(EINVAL),
        };
        result
    }

    pub fn rename(&mut self, cwd: &str, old_path: &str, new_path: &str) -> KernelResult<()> {
        let old_absolute = normalize_path(cwd, old_path);
        let new_absolute = normalize_path(cwd, new_path);
        let (old_parent_path, old_name) = split_parent(&old_absolute)?;
        let (new_parent_path, new_name) = split_parent(&new_absolute)?;

        let node = {
            let old_parent = self.lookup_abs(&old_parent_path)?;
            let mut guard = old_parent.data.lock();
            let NodeData::Directory(entries) = &mut *guard else {
                return Err(ENOTDIR);
            };
            entries.remove(old_name).ok_or(ENOENT)?
        };

        let new_parent = self.lookup_abs(&new_parent_path)?;
        let mut guard = new_parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        if entries.contains_key(new_name) {
            return Err(EEXIST);
        }
        entries.insert(new_name.to_string(), node);
        Ok(())
    }

    pub fn truncate(&mut self, handle: &mut FileHandle, len: usize) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                buf.resize(len, 0);
                handle.offset = handle.offset.min(len);
                Ok(())
            }
            NodeData::Ext4File(_) | NodeData::Ext4Dir(_) => Err(EROFS),
            NodeData::Pipe(_)
            | NodeData::Directory(_)
            | NodeData::CharDevice
            | NodeData::Symlink(_)
            | NodeData::Event(_)
            | NodeData::Epoll(_)
            | NodeData::SocketPending(_)
            | NodeData::SocketConnected { .. }
            | NodeData::PidFd(_) => Err(EINVAL),
        }
    }

    pub fn truncate_path(&mut self, cwd: &str, path: &str, len: usize) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let node = self.lookup_abs(&absolute)?;
        let mut handle = FileHandle {
            node,
            offset: 0,
            flags: O_RDWR,
            path: absolute,
            pipe_end: PipeEnd::None,
        };
        self.truncate(&mut handle, len)
    }

    pub fn fallocate(
        &mut self,
        handle: &mut FileHandle,
        offset: usize,
        len: usize,
    ) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                let size = offset.saturating_add(len);
                if buf.len() < size {
                    buf.resize(size, 0);
                }
                Ok(())
            }
            NodeData::Ext4File(_) | NodeData::Ext4Dir(_) => Err(EROFS),
            _ => Err(EINVAL),
        }
    }

    pub fn create_pipe(&mut self) -> KernelResult<(FileHandle, FileHandle)> {
        let path = format!("pipe:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        let node = Arc::new(Node::pipe(&path));
        let read_end = FileHandle {
            node: node.clone(),
            offset: 0,
            flags: O_RDONLY,
            path: path.clone(),
            pipe_end: PipeEnd::Read,
        };
        let write_end = FileHandle {
            node,
            offset: 0,
            flags: O_WRONLY,
            path,
            pipe_end: PipeEnd::Write,
        };
        Ok((read_end, write_end))
    }

    pub fn create_eventfd(&mut self, init: u64) -> KernelResult<FileHandle> {
        let path = format!("eventfd:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        Ok(FileHandle {
            node: Arc::new(Node::eventfd(&path, init)),
            offset: 0,
            flags: O_RDWR,
            path,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn create_epoll(&mut self) -> KernelResult<FileHandle> {
        let path = format!("epoll:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        Ok(FileHandle {
            node: Arc::new(Node::epoll(&path)),
            offset: 0,
            flags: O_RDWR,
            path,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn epoll_ctl(
        &mut self,
        handle: &mut FileHandle,
        op: u32,
        fd: i32,
        events: u32,
    ) -> KernelResult<()> {
        let NodeData::Epoll(watches) = &mut *handle.node.data.lock() else {
            return Err(EINVAL);
        };
        match op {
            1 => {
                if watches.iter().any(|watch| watch.fd == fd) {
                    return Err(EEXIST);
                }
                watches.push(EpollWatch { fd, events });
            }
            2 => {
                let watch = watches
                    .iter_mut()
                    .find(|watch| watch.fd == fd)
                    .ok_or(ENOENT)?;
                watch.events = events;
            }
            3 => {
                let index = watches
                    .iter()
                    .position(|watch| watch.fd == fd)
                    .ok_or(ENOENT)?;
                watches.remove(index);
            }
            _ => return Err(EINVAL),
        }
        Ok(())
    }

    pub fn epoll_watches(&self, handle: &FileHandle) -> KernelResult<Vec<EpollWatch>> {
        let NodeData::Epoll(watches) = &*handle.node.data.lock() else {
            return Err(EINVAL);
        };
        Ok(watches.clone())
    }

    pub fn create_memfd(&mut self, name: &str) -> KernelResult<FileHandle> {
        let path = format!("memfd:{}:{}", name, self.next_memfd_id);
        self.next_memfd_id += 1;
        Ok(FileHandle {
            node: Arc::new(Node::file(&path, NodeData::File(Vec::new()))),
            offset: 0,
            flags: O_RDWR,
            path,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn create_pidfd(&mut self, pid: usize) -> KernelResult<FileHandle> {
        let path = format!("pidfd:[{}]", pid);
        Ok(FileHandle {
            node: Arc::new(Node::pidfd(&path, pid)),
            offset: 0,
            flags: O_RDONLY,
            path,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn pidfd_pid(&self, handle: &FileHandle) -> KernelResult<usize> {
        let NodeData::PidFd(pid) = &*handle.node.data.lock() else {
            return Err(EINVAL);
        };
        Ok(*pid)
    }

    pub fn create_socket(&mut self) -> KernelResult<FileHandle> {
        let path = format!("socket:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        Ok(FileHandle {
            node: Arc::new(Node::socket_pending(&path)),
            offset: 0,
            flags: O_RDWR,
            path,
            pipe_end: PipeEnd::None,
        })
    }

    pub fn create_socketpair(&mut self) -> KernelResult<(FileHandle, FileHandle)> {
        let path = format!("socketpair:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        let channel = Arc::new(Mutex::new(SocketChannel {
            inbox: [VecDeque::new(), VecDeque::new()],
        }));
        Ok((
            FileHandle {
                node: Arc::new(Node::socket_connected(&path, channel.clone(), 0)),
                offset: 0,
                flags: O_RDWR,
                path: format!("{}:0", path),
                pipe_end: PipeEnd::None,
            },
            FileHandle {
                node: Arc::new(Node::socket_connected(&path, channel, 1)),
                offset: 0,
                flags: O_RDWR,
                path: format!("{}:1", path),
                pipe_end: PipeEnd::None,
            },
        ))
    }

    pub fn bind_socket(
        &mut self,
        handle: &mut FileHandle,
        cwd: &str,
        path: &str,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let NodeData::SocketPending(state) = &mut *handle.node.data.lock() else {
            return Err(EINVAL);
        };
        state.path = Some(absolute.clone());
        handle.path = absolute.clone();
        self.socket_bindings.insert(absolute, handle.node.clone());
        Ok(())
    }

    pub fn listen_socket(&mut self, handle: &mut FileHandle, _backlog: i32) -> KernelResult<()> {
        let NodeData::SocketPending(state) = &mut *handle.node.data.lock() else {
            return Err(EINVAL);
        };
        state.listening = true;
        Ok(())
    }

    pub fn connect_socket(
        &mut self,
        handle: &mut FileHandle,
        cwd: &str,
        path: &str,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let listener = self.socket_bindings.get(&absolute).cloned().ok_or(ENOENT)?;
        let channel = Arc::new(Mutex::new(SocketChannel {
            inbox: [VecDeque::new(), VecDeque::new()],
        }));
        {
            let mut guard = listener.data.lock();
            let NodeData::SocketPending(state) = &mut *guard else {
                return Err(EINVAL);
            };
            if !state.listening {
                return Err(EINVAL);
            }
            state.pending.push(Arc::new(Node::socket_connected(
                &absolute,
                channel.clone(),
                1,
            )));
        }
        *handle.node.data.lock() = NodeData::SocketConnected { channel, side: 0 };
        handle.path = absolute;
        Ok(())
    }

    pub fn accept_socket(&mut self, handle: &mut FileHandle) -> KernelResult<FileHandle> {
        let NodeData::SocketPending(state) = &mut *handle.node.data.lock() else {
            return Err(EINVAL);
        };
        let node = state.pending.pop().ok_or(EINVAL)?;
        Ok(FileHandle {
            node,
            offset: 0,
            flags: O_RDWR,
            path: handle.path.clone(),
            pipe_end: PipeEnd::None,
        })
    }

    pub fn is_read_ready(&self, handle: &FileHandle) -> bool {
        handle.poll_read_ready()
    }

    pub fn is_write_ready(&self, handle: &FileHandle) -> bool {
        handle.poll_write_ready()
    }

    pub fn stat_handle(&self, handle: &FileHandle) -> KernelResult<FileStat> {
        handle.stat_object()
    }

    pub fn is_pipe(&self, handle: &FileHandle) -> bool {
        handle.node.kind == NodeKind::Pipe
    }

    fn path_exists(&self, absolute: &str) -> KernelResult<()> {
        if self.lookup_abs(absolute).is_ok() {
            return Ok(());
        }
        if self.external_preloaded.contains_key(absolute) {
            return Ok(());
        }
        if let Some((mount, fs_path)) = self.resolve_external_path(absolute) {
            if mount.ext4.exists(&fs_path)? {
                return Ok(());
            }
        }
        Err(ENOENT)
    }

    fn stat_abs_path(
        &self,
        absolute: &str,
        follow_symlink: bool,
        depth: usize,
    ) -> KernelResult<FileStat> {
        const MAX_SYMLINK_DEPTH: usize = 40;
        if depth > MAX_SYMLINK_DEPTH {
            return Err(EINVAL);
        }
        if let Some(stat) = self.external_stat_path(absolute, follow_symlink, depth)? {
            return Ok(stat);
        }
        let node = self.lookup_abs(absolute)?;
        if follow_symlink && node.kind == NodeKind::Symlink {
            let target = match &*node.data.lock() {
                NodeData::Symlink(target) => target.clone(),
                _ => return Err(EINVAL),
            };
            let parent = split_parent(absolute)?.0;
            let resolved = normalize_path(&parent, &target);
            return self.stat_abs_path(&resolved, true, depth + 1);
        }
        self.stat(&node)
    }

    fn external_stat_path(
        &self,
        absolute: &str,
        follow_symlink: bool,
        depth: usize,
    ) -> KernelResult<Option<FileStat>> {
        let mut stat = match self.external_lstat_path(absolute)? {
            Some(stat) => stat,
            None => return Ok(None),
        };
        if follow_symlink && (stat.mode & 0o170000) == S_IFLNK {
            let target = self.external_read_link(absolute)?;
            let parent = split_parent(absolute)?.0;
            let resolved = normalize_path(&parent, &target);
            stat = self.stat_abs_path(&resolved, true, depth + 1)?;
        }
        Ok(Some(stat))
    }

    fn external_lstat_path(&self, absolute: &str) -> KernelResult<Option<FileStat>> {
        if self.is_memory_preferred_path(absolute) && self.lookup_abs(absolute).is_ok() {
            return Ok(None);
        }
        if let Some((_, stat)) = self.external_preloaded.get(absolute) {
            return Ok(Some(*stat));
        }
        let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
            return Ok(None);
        };
        match mount.ext4.read_link(&fs_path) {
            Ok(target) => return Ok(Some(Self::symlink_file_stat(&target))),
            Err(err) if err != EINVAL && err != ENOENT => return Err(err),
            Err(_) => {}
        }
        if let Some(stat) = self.external_stat_cache.get(absolute) {
            return Ok(Some(*stat));
        }
        match mount.ext4.stat(&fs_path) {
            Ok(stat) => {
                let stat = FileStat {
                    mode: stat.mode,
                    size: stat.size,
                    nlink: stat.nlink,
                };
                Ok(Some(stat))
            }
            Err(err) if err == ENOENT => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn external_read_link(&self, absolute: &str) -> KernelResult<String> {
        let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
            return Err(ENOENT);
        };
        mount.ext4.read_link(&fs_path)
    }

    fn symlink_file_stat(target: &str) -> FileStat {
        FileStat {
            mode: S_IFLNK | 0o777,
            size: target.len() as u64,
            nlink: 1,
        }
    }

    fn try_open_external(
        &mut self,
        absolute: &str,
        flags: u32,
    ) -> KernelResult<Option<FileHandle>> {
        if self.is_memory_preferred_path(absolute) && self.lookup_abs(absolute).is_ok() {
            return Ok(None);
        }
        let (mount, fs_path) = {
            let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
                return Ok(None);
            };
            (mount.ext4.clone(), fs_path)
        };
        if flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC) != 0 {
            return Ok(None);
        }
        if let Some((cached, stat)) = self.external_preloaded.get(absolute).cloned() {
            if (flags & O_DIRECTORY) != 0 && (stat.mode & S_IFDIR) != S_IFDIR {
                return Err(ENOTDIR);
            }
            return Ok(Some(self.build_ext4_handle(
                absolute,
                mount,
                fs_path,
                fs_ext4::Ext4FileStat {
                    mode: stat.mode,
                    size: stat.size,
                    nlink: stat.nlink,
                },
                Some(cached),
            )));
        }

        let cached = self.external_stat_cache.get(absolute).copied();
        let stat = match cached {
            Some(stat) => fs_ext4::Ext4FileStat {
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
            },
            None => {
                let stat = match mount.stat(&fs_path) {
                    Ok(stat) => stat,
                    Err(err) if err == ENOENT => return Ok(None),
                    Err(err) => return Err(err),
                };
                self.external_stat_cache.insert(
                    absolute.to_string(),
                    FileStat {
                        mode: stat.mode,
                        size: stat.size,
                        nlink: stat.nlink,
                    },
                );
                stat
            }
        };
        if (flags & O_DIRECTORY) != 0 && (stat.mode & S_IFDIR) != S_IFDIR {
            return Err(ENOTDIR);
        }
        Ok(Some(
            self.build_ext4_handle(absolute, mount, fs_path, stat, None),
        ))
    }

    fn build_ext4_handle(
        &self,
        absolute: &str,
        mount: Ext4Mount,
        fs_path: String,
        stat: fs_ext4::Ext4FileStat,
        cached: Option<Arc<Vec<u8>>>,
    ) -> FileHandle {
        let is_dir = (stat.mode & S_IFDIR) == S_IFDIR;
        let node = Arc::new(if is_dir {
            Node::directory_with_data(
                absolute,
                NodeData::Ext4Dir(Ext4DirState {
                    mount,
                    path: fs_path,
                    mode: stat.mode,
                    size: stat.size,
                    entries: None,
                }),
            )
        } else {
            Node::file(
                absolute,
                NodeData::Ext4File(Ext4FileState {
                    mount,
                    path: fs_path,
                    mode: stat.mode,
                    size: stat.size,
                    cached,
                }),
            )
        });
        FileHandle {
            node,
            offset: 0,
            flags: O_RDONLY,
            path: absolute.to_string(),
            pipe_end: PipeEnd::None,
        }
    }

    fn resolve_external_path(&self, absolute: &str) -> Option<(&ExternalMount, String)> {
        self.external_mounts
            .iter()
            .filter_map(|mount| {
                external_mount_path(&mount.target, absolute).map(|path| (mount, path))
            })
            .max_by_key(|(mount, _)| mount.target.len())
    }

    fn cache_external_dirent_stat(
        &mut self,
        dir_absolute: &str,
        name: &str,
        stat: fs_ext4::Ext4FileStat,
    ) {
        let absolute = normalize_path(dir_absolute, name);
        self.external_stat_cache.insert(
            absolute,
            FileStat {
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
            },
        );
    }

    fn is_memory_preferred_path(&self, absolute: &str) -> bool {
        matches!(absolute, "/dev" | "/proc" | "/tmp" | "/mnt")
            || absolute.starts_with("/dev/")
            || absolute.starts_with("/proc/")
            || absolute.starts_with("/tmp/")
            || absolute.starts_with("/mnt/")
    }

    fn refresh_mounts_proc(&mut self) {
        let mut data = String::new();
        for mount in &self.mounts {
            data.push_str(&format!(
                "{} {} {}\n",
                mount.source, mount.target, mount.fs_type
            ));
        }
        if let Ok(node) = self.lookup_abs("/proc/mounts") {
            *node.data.lock() = NodeData::ProcFile(data.into_bytes());
        }
    }

    fn lookup_abs(&self, path: &str) -> KernelResult<Arc<Node>> {
        if path == "/" {
            return Ok(self.root.clone());
        }
        let mut current = self.root.clone();
        for component in path.split('/').filter(|segment| !segment.is_empty()) {
            let next = match &*current.data.lock() {
                NodeData::Directory(entries) => entries.get(component).cloned().ok_or(ENOENT)?,
                _ => return Err(ENOTDIR),
            };
            current = next;
        }
        Ok(current)
    }

    fn ensure_memory_dir(&mut self, absolute_path: &str) -> KernelResult<()> {
        if absolute_path == "/" || self.lookup_abs(absolute_path).is_ok() {
            return Ok(());
        }
        let (parent_path, _) = split_parent(absolute_path)?;
        self.ensure_memory_dir(&parent_path)?;
        match self.external_stat_path(absolute_path, true, 0)? {
            Some(stat) if (stat.mode & S_IFDIR) == S_IFDIR => {
                self.create_node(absolute_path, NodeKind::Directory, None)
            }
            Some(_) => Err(ENOTDIR),
            None => Err(ENOENT),
        }
    }

    fn create_node(
        &mut self,
        absolute_path: &str,
        kind: NodeKind,
        data: Option<NodeData>,
    ) -> KernelResult<()> {
        if absolute_path == "/" {
            return Err(EEXIST);
        }
        let (parent_path, name) = split_parent(absolute_path)?;
        let parent = match self.lookup_abs(&parent_path) {
            Ok(parent) => parent,
            Err(err) if err == ENOENT => {
                self.ensure_memory_dir(&parent_path)?;
                self.lookup_abs(&parent_path)?
            }
            Err(err) => return Err(err),
        };
        let mut guard = parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        if entries.contains_key(name) {
            return Err(EEXIST);
        }
        let node = Arc::new(match kind {
            NodeKind::Directory => Node::directory(name),
            NodeKind::File => Node::file(name, data.unwrap_or_else(|| NodeData::File(Vec::new()))),
            NodeKind::CharDevice => Node::char_device(name, data.unwrap_or(NodeData::CharDevice)),
            NodeKind::Proc => {
                Node::proc(name, data.unwrap_or_else(|| NodeData::ProcFile(Vec::new())))
            }
            NodeKind::Pipe => Node::pipe(name),
            NodeKind::Symlink => Node::symlink(
                name,
                data.unwrap_or_else(|| NodeData::Symlink(String::new())),
            ),
            NodeKind::Event => Node::eventfd(name, 0),
            NodeKind::Epoll => Node::epoll(name),
            NodeKind::Socket => Node::socket_pending(name),
            NodeKind::PidFd => Node::pidfd(name, 0),
        });
        entries.insert(name.to_string(), node);
        Ok(())
    }

    fn stat(&self, node: &Arc<Node>) -> KernelResult<FileStat> {
        let guard = node.data.lock();
        let size = match &*guard {
            NodeData::Directory(entries) => entries.len() as u64,
            NodeData::File(buf) | NodeData::ProcFile(buf) => buf.len() as u64,
            NodeData::Ext4File(state) => state.size,
            NodeData::Ext4Dir(state) => state.size,
            NodeData::Pipe(state) => state.buf.len() as u64,
            NodeData::Symlink(target) => target.len() as u64,
            NodeData::Event(_) => 8,
            NodeData::Epoll(watches) => watches.len() as u64,
            NodeData::SocketPending(state) => state.pending.len() as u64,
            NodeData::SocketConnected { channel, side } => channel.lock().inbox[*side].len() as u64,
            NodeData::PidFd(_) => 0,
            NodeData::CharDevice => 0,
        };
        let mode = match &*guard {
            NodeData::Ext4File(state) => state.mode,
            NodeData::Ext4Dir(state) => state.mode,
            _ => match node.kind {
                NodeKind::Directory => S_IFDIR | 0o755,
                NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
                NodeKind::CharDevice => S_IFCHR | 0o600,
                NodeKind::Pipe => S_IFIFO | 0o644,
                NodeKind::Symlink => S_IFLNK | 0o777,
                NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
                NodeKind::PidFd => S_IFREG | 0o444,
            },
        };
        Ok(FileStat {
            mode,
            size,
            nlink: 1,
        })
    }
}

impl FileHandle {
    fn adjust_pipe_refcount(&self, delta: isize) {
        if self.node.kind != NodeKind::Pipe || self.pipe_end == PipeEnd::None {
            return;
        }
        let mut guard = self.node.data.lock();
        let NodeData::Pipe(state) = &mut *guard else {
            return;
        };
        let target = match self.pipe_end {
            PipeEnd::Read => &mut state.readers,
            PipeEnd::Write => &mut state.writers,
            PipeEnd::None => return,
        };
        if delta > 0 {
            *target = target.saturating_add(delta as usize);
        } else {
            *target = target.saturating_sub((-delta) as usize);
        }
    }

    fn stat_from_locked(&self) -> FileStat {
        let guard = self.node.data.lock();
        let size = match &*guard {
            NodeData::Directory(entries) => entries.len() as u64,
            NodeData::File(buf) | NodeData::ProcFile(buf) => buf.len() as u64,
            NodeData::Ext4File(state) => state.size,
            NodeData::Ext4Dir(state) => state.size,
            NodeData::Pipe(state) => state.buf.len() as u64,
            NodeData::Symlink(target) => target.len() as u64,
            NodeData::Event(_) => 8,
            NodeData::Epoll(watches) => watches.len() as u64,
            NodeData::SocketPending(state) => state.pending.len() as u64,
            NodeData::SocketConnected { channel, side } => channel.lock().inbox[*side].len() as u64,
            NodeData::PidFd(_) => 0,
            NodeData::CharDevice => 0,
        };
        let mode = match &*guard {
            NodeData::Ext4File(state) => state.mode,
            NodeData::Ext4Dir(state) => state.mode,
            _ => match self.node.kind {
                NodeKind::Directory => S_IFDIR | 0o755,
                NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
                NodeKind::CharDevice => S_IFCHR | 0o600,
                NodeKind::Pipe => S_IFIFO | 0o644,
                NodeKind::Symlink => S_IFLNK | 0o777,
                NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
                NodeKind::PidFd => S_IFREG | 0o444,
            },
        };
        FileStat {
            mode,
            size,
            nlink: 1,
        }
    }
}

impl Clone for FileHandle {
    fn clone(&self) -> Self {
        self.adjust_pipe_refcount(1);
        Self {
            node: self.node.clone(),
            offset: self.offset,
            flags: self.flags,
            path: self.path.clone(),
            pipe_end: self.pipe_end,
        }
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        self.adjust_pipe_refcount(-1);
    }
}

impl KernelObject for FileHandle {
    fn object_kind(&self) -> ObjectKind {
        match self.node.kind {
            NodeKind::Directory => ObjectKind::Directory,
            NodeKind::File => {
                if self.path.starts_with("memfd:") {
                    ObjectKind::MemFd
                } else {
                    ObjectKind::Regular
                }
            }
            NodeKind::CharDevice => ObjectKind::CharDevice,
            NodeKind::Proc => ObjectKind::Procfs,
            NodeKind::Pipe => ObjectKind::Pipe,
            NodeKind::Symlink => ObjectKind::Symlink,
            NodeKind::Event => ObjectKind::EventFd,
            NodeKind::Epoll => ObjectKind::Epoll,
            NodeKind::Socket => ObjectKind::SocketLocal,
            NodeKind::PidFd => ObjectKind::PidFd,
        }
    }

    fn read_object(&mut self, len: usize) -> KernelResult<Vec<u8>> {
        match &mut *self.node.data.lock() {
            NodeData::Directory(_) => Err(EISDIR),
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                let start = self.offset.min(buf.len());
                let end = (start + len).min(buf.len());
                self.offset = end;
                Ok(buf[start..end].to_vec())
            }
            NodeData::Ext4File(state) => {
                if let Some(cached) = &state.cached {
                    let start = self.offset.min(cached.len());
                    let end = (start + len).min(cached.len());
                    self.offset = end;
                    return Ok(cached[start..end].to_vec());
                }
                let data = state.mount.read_range(&state.path, self.offset, len)?;
                self.offset += data.len();
                Ok(data)
            }
            NodeData::Ext4Dir(_) => Err(EISDIR),
            NodeData::Pipe(state) => {
                if self.pipe_end == PipeEnd::Write {
                    return Err(EINVAL);
                }
                if state.buf.is_empty() {
                    pipe_debug(&format!(
                        "whuse-pipe-eof-state: path={} readers={} writers={} empty=1",
                        self.path, state.readers, state.writers
                    ));
                    if state.writers == 0 {
                        return Ok(Vec::new());
                    }
                    return Err(EAGAIN);
                }
                let end = len.min(state.buf.len());
                let mut out = Vec::with_capacity(end);
                for _ in 0..end {
                    if let Some(byte) = state.buf.pop_front() {
                        out.push(byte);
                    }
                }
                Ok(out)
            }
            NodeData::Symlink(target) => Ok(target.as_bytes()[..target.len().min(len)].to_vec()),
            NodeData::Event(counter) => {
                if len < 8 {
                    return Err(EINVAL);
                }
                let value = *counter;
                *counter = 0;
                Ok(value.to_le_bytes().to_vec())
            }
            NodeData::Epoll(_) | NodeData::SocketPending(_) | NodeData::PidFd(_) => Err(EINVAL),
            NodeData::SocketConnected { channel, side } => {
                let mut guard = channel.lock();
                let inbox = &mut guard.inbox[*side];
                let end = len.min(inbox.len());
                Ok(inbox.drain(..end).collect())
            }
            NodeData::CharDevice => {
                if self.path == "/dev/zero" {
                    return Ok(alloc::vec![0; len]);
                }
                if self.path == "/dev/random" || self.path == "/dev/urandom" {
                    return Ok(alloc::vec![0u8; len]);
                }
                Ok(Vec::new())
            }
        }
    }

    fn write_object(&mut self, data: &[u8]) -> KernelResult<usize> {
        match &mut *self.node.data.lock() {
            NodeData::Directory(_) => Err(EISDIR),
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                if self.offset > buf.len() {
                    buf.resize(self.offset, 0);
                }
                if self.offset + data.len() > buf.len() {
                    buf.resize(self.offset + data.len(), 0);
                }
                buf[self.offset..self.offset + data.len()].copy_from_slice(data);
                self.offset += data.len();
                Ok(data.len())
            }
            NodeData::Ext4File(_) | NodeData::Ext4Dir(_) => Err(EROFS),
            NodeData::Pipe(state) => {
                if self.pipe_end == PipeEnd::Read {
                    return Err(EINVAL);
                }
                state.buf.extend(data.iter().copied());
                Ok(data.len())
            }
            NodeData::Symlink(_)
            | NodeData::Epoll(_)
            | NodeData::SocketPending(_)
            | NodeData::PidFd(_) => Err(EINVAL),
            NodeData::Event(counter) => {
                if data.len() < 8 {
                    return Err(EINVAL);
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[..8]);
                *counter = counter.saturating_add(u64::from_le_bytes(bytes));
                Ok(8)
            }
            NodeData::SocketConnected { channel, side } => {
                let mut guard = channel.lock();
                let peer = 1 - *side;
                guard.inbox[peer].extend(data.iter().copied());
                Ok(data.len())
            }
            NodeData::CharDevice => {
                if self.path == "/dev/null"
                    || self.path == "/dev/zero"
                    || self.path == "/dev/random"
                    || self.path == "/dev/urandom"
                {
                    return Ok(data.len());
                }
                for byte in data.iter().copied() {
                    hal().console.put_byte(byte);
                }
                Ok(data.len())
            }
        }
    }

    fn seek_object(&mut self, offset: isize, whence: u32) -> KernelResult<usize> {
        if matches!(
            self.node.kind,
            NodeKind::Pipe | NodeKind::Event | NodeKind::Epoll | NodeKind::Socket | NodeKind::PidFd
        ) {
            return Err(EINVAL);
        }
        let size = self.stat_from_locked().size as isize;
        let base = match whence {
            0 => 0,
            1 => self.offset as isize,
            2 => size,
            _ => return Err(EINVAL),
        };
        let new_offset = base.checked_add(offset).ok_or(EINVAL)?;
        if new_offset < 0 {
            return Err(EINVAL);
        }
        self.offset = new_offset as usize;
        Ok(self.offset)
    }

    fn poll_read_ready(&self) -> bool {
        match &*self.node.data.lock() {
            NodeData::Directory(_) => true,
            NodeData::File(_)
            | NodeData::ProcFile(_)
            | NodeData::Ext4File(_)
            | NodeData::Ext4Dir(_)
            | NodeData::CharDevice => true,
            NodeData::Pipe(state) => !state.buf.is_empty() || state.writers == 0,
            NodeData::Symlink(_) => true,
            NodeData::Event(counter) => *counter != 0,
            NodeData::Epoll(_) => true,
            NodeData::SocketPending(state) => !state.pending.is_empty(),
            NodeData::SocketConnected { channel, side } => !channel.lock().inbox[*side].is_empty(),
            NodeData::PidFd(_) => true,
        }
    }

    fn poll_write_ready(&self) -> bool {
        match &*self.node.data.lock() {
            NodeData::Directory(_)
            | NodeData::Ext4File(_)
            | NodeData::Ext4Dir(_)
            | NodeData::PidFd(_) => false,
            NodeData::Pipe(state) => state.readers != 0,
            _ => true,
        }
    }

    fn stat_object(&self) -> KernelResult<FileStat> {
        Ok(self.stat_from_locked())
    }
}

impl Node {
    fn directory(name: &str) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Directory,
            data: Mutex::new(NodeData::Directory(BTreeMap::new())),
        }
    }

    fn directory_with_data(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Directory,
            data: Mutex::new(data),
        }
    }

    fn file(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::File,
            data: Mutex::new(data),
        }
    }

    fn char_device(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::CharDevice,
            data: Mutex::new(data),
        }
    }

    fn proc(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Proc,
            data: Mutex::new(data),
        }
    }

    fn pipe(name: &str) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Pipe,
            data: Mutex::new(NodeData::Pipe(PipeState {
                buf: VecDeque::new(),
                readers: 1,
                writers: 1,
            })),
        }
    }

    fn symlink(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Symlink,
            data: Mutex::new(data),
        }
    }

    fn eventfd(name: &str, init: u64) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Event,
            data: Mutex::new(NodeData::Event(init)),
        }
    }

    fn epoll(name: &str) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Epoll,
            data: Mutex::new(NodeData::Epoll(Vec::new())),
        }
    }

    fn socket_pending(name: &str) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Socket,
            data: Mutex::new(NodeData::SocketPending(SocketPending {
                path: None,
                listening: false,
                pending: Vec::new(),
            })),
        }
    }

    fn socket_connected(name: &str, channel: Arc<Mutex<SocketChannel>>, side: usize) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Socket,
            data: Mutex::new(NodeData::SocketConnected { channel, side }),
        }
    }

    fn pidfd(name: &str, pid: usize) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::PidFd,
            data: Mutex::new(NodeData::PidFd(pid)),
        }
    }
}

fn normalize_path(cwd: &str, path: &str) -> String {
    let mut components = Vec::new();
    let source = if path.starts_with('/') {
        path.to_string()
    } else if cwd == "/" {
        format!("/{}", path)
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    };
    for component in source.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other.to_string()),
        }
    }
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

fn split_parent(path: &str) -> KernelResult<(String, &str)> {
    let trimmed = path.trim_end_matches('/');
    let index = trimmed.rfind('/').ok_or(EINVAL)?;
    let parent = if index == 0 {
        "/".to_string()
    } else {
        trimmed[..index].to_string()
    };
    let name = &trimmed[index + 1..];
    if name.is_empty() {
        return Err(EINVAL);
    }
    Ok((parent, name))
}

fn external_mount_path(target: &str, absolute: &str) -> Option<String> {
    if target == "/" {
        return Some(absolute.to_string());
    }
    if absolute == target {
        return Some("/".to_string());
    }
    absolute
        .strip_prefix(target)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .map(|suffix| format!("/{}", suffix))
}

fn append_dirent(out: &mut Vec<u8>, index: usize, name: &str, file_type: u8) {
    let reclen = align_up(19 + name.len() + 1, 8) as u16;
    out.extend_from_slice(&(index as u64 + 1).to_le_bytes());
    out.extend_from_slice(&((index + 1) as u64).to_le_bytes());
    out.extend_from_slice(&reclen.to_le_bytes());
    out.push(file_type);
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    while out.len() % 8 != 0 {
        out.push(0);
    }
}

fn node_kind_to_dirent_type(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::Directory => 4,
        NodeKind::Symlink => 10,
        _ => 8,
    }
}

fn ext4_kind_to_dirent_type(kind: Ext4NodeKind) -> u8 {
    match kind {
        Ext4NodeKind::Directory => 4,
        Ext4NodeKind::Symlink => 10,
        _ => 8,
    }
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicBool, Ordering};
    use hal_api::HalBlockDevice;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{KernelObject, KernelVfs, ObjectKind, O_CREAT, O_RDWR};

    struct VecBlockDevice {
        ready: AtomicBool,
        data: Vec<u8>,
        sector_size: usize,
    }

    impl VecBlockDevice {
        fn from_image(path: &Path) -> Self {
            Self {
                ready: AtomicBool::new(false),
                data: fs::read(path).unwrap(),
                sector_size: 512,
            }
        }
    }

    impl HalBlockDevice for VecBlockDevice {
        fn name(&self) -> &'static str {
            "vecblk0"
        }

        fn init(&self) -> Result<(), i32> {
            self.ready.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }

        fn sector_size(&self) -> usize {
            self.sector_size
        }

        fn sector_count(&self) -> usize {
            self.data.len() / self.sector_size
        }

        fn read_sector(&self, sector: usize, buf: &mut [u8]) -> Result<(), i32> {
            let start = sector * self.sector_size;
            let end = start + buf.len();
            buf.copy_from_slice(self.data.get(start..end).ok_or(5)?);
            Ok(())
        }

        fn write_sector(&self, _sector: usize, _buf: &[u8]) -> Result<(), i32> {
            Err(95)
        }
    }

    fn fresh_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "whuse-vfs-{}-{}-{}",
            name,
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_test_image() -> PathBuf {
        let base = fresh_dir("ext4");
        let stage = base.join("stage");
        fs::create_dir_all(stage.join("bin")).unwrap();
        fs::write(stage.join("bin/hello"), b"hello from ext4").unwrap();

        let image = base.join("rootfs.ext4");
        let status = Command::new("truncate")
            .args(["-s", "8M", image.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("mke2fs")
            .args([
                "-t",
                "ext4",
                "-d",
                stage.to_str().unwrap(),
                "-F",
                image.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        image
    }

    #[test]
    fn vfs_file_round_trip() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs
            .open("/", "/tmp/hello.txt", O_CREAT | O_RDWR, 0o644)
            .unwrap();
        vfs.write(&mut file, b"hello").unwrap();
        vfs.seek(&mut file, 0, 0).unwrap();
        assert_eq!(vfs.read(&mut file, 5).unwrap(), b"hello");
        assert_eq!(vfs.chdir("/", "/tmp").unwrap(), "/tmp");
    }

    #[test]
    fn stat_path_follow_and_nofollow_for_symlink() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", "/tmp/real.txt", b"target").unwrap();
        vfs.create_symlink("/", "/tmp/link.txt", "/tmp/real.txt")
            .unwrap();

        let followed = vfs.stat_path("/", "/tmp/link.txt").unwrap();
        assert_eq!(followed.mode & 0o170000, super::S_IFREG);
        assert_eq!(followed.size, 6);

        let nofollow = vfs.stat_path_nofollow("/", "/tmp/link.txt").unwrap();
        assert_eq!(nofollow.mode & 0o170000, super::S_IFLNK);
        assert_eq!(nofollow.size, "/tmp/real.txt".len() as u64);
    }

    #[test]
    fn access_precise_preserves_enotdir() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", "/tmp/file", b"x").unwrap();
        assert_eq!(
            vfs.access_precise("/", "/tmp/file/child").unwrap_err(),
            super::ENOTDIR
        );
    }

    #[test]
    fn object_layer_reports_backend_kind() {
        let mut vfs = KernelVfs::new();

        let event = vfs.create_eventfd(1).unwrap();
        assert_eq!(event.object_kind(), ObjectKind::EventFd);
        assert!(event.poll_read_ready());

        let mut memfd = vfs.create_memfd("demo").unwrap();
        assert_eq!(memfd.object_kind(), ObjectKind::MemFd);
        memfd.write_object(b"abc").unwrap();
        memfd.seek_object(0, 0).unwrap();
        assert_eq!(memfd.read_object(3).unwrap(), b"abc");

        let regular = vfs
            .open("/", "/tmp/object.txt", O_CREAT | O_RDWR, 0o644)
            .unwrap();
        assert_eq!(regular.object_kind(), ObjectKind::Regular);
    }

    #[test]
    fn ext4_root_mount_supports_open_and_getdents() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        let mut file = vfs.open("/", "/bin/hello", 0, 0).unwrap();
        assert_eq!(file.object_kind(), ObjectKind::Regular);
        assert_eq!(vfs.read(&mut file, 32).unwrap(), b"hello from ext4");
        assert!(matches!(
            vfs.open("/", "/bin/not-found", 0, 0),
            Err(super::ENOENT)
        ));

        let mut dir = vfs.open("/", "/bin", super::O_DIRECTORY, 0).unwrap();
        let entries = vfs.getdents(&mut dir, 4096).unwrap();
        assert!(!entries.is_empty());
        assert_eq!(
            vfs.external_stat_cache.get("/bin/hello"),
            Some(&super::FileStat {
                mode: super::S_IFREG | 0o644,
                size: b"hello from ext4".len() as u64,
                nlink: 1,
            })
        );
    }

    #[test]
    fn umount_non_external_mount_keeps_external_preload_cache() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();
        vfs.preload_external_file("/musl/basic/wait", b"wait-ok", Some(super::S_IFREG | 0o755))
            .unwrap();

        let mut before = vfs
            .open("/", "/musl/basic/wait", super::O_RDONLY, 0)
            .unwrap();
        assert_eq!(vfs.read(&mut before, 16).unwrap(), b"wait-ok");

        vfs.mount("dev:/dev/vda2", "/mnt", "vfat").unwrap();
        vfs.umount("/mnt").unwrap();

        let mut after = vfs
            .open("/", "/musl/basic/wait", super::O_RDONLY, 0)
            .unwrap();
        assert_eq!(vfs.read(&mut after, 16).unwrap(), b"wait-ok");
    }
}
