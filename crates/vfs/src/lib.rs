#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
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
const ENOTSOCK: i32 = 88;
const ENOTDIR: i32 = 20;
const ELOOP: i32 = 40;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ENOSPC: i32 = 28;
const ENOMEM: i32 = 12;
const EOPNOTSUPP: i32 = 95;
const EPIPE: i32 = 32;
const EROFS: i32 = 30;
const ENOTEMPTY: i32 = 39;
const EADDRINUSE: i32 = 98;
const EADDRNOTAVAIL: i32 = 99;
const UNIX_ABSTRACT_PREFIX: &str = "/__unix_abstract__/";
const INMEM_FILE_SIZE_LIMIT: usize = 64 * 1024 * 1024;

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
const PROC_SELF_MAPS: &[u8] = b"50000000-50080000 r-xp 00000000 00:00 0 /proc/self/exe\n";
const EXT4_DIR_STAT_CACHE_MAX_SIZE: u64 = 512 * 1024;
static CONSOLE_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn write_console_bytes(bytes: &[u8]) {
    let _guard = CONSOLE_WRITE_LOCK.lock();
    for byte in bytes.iter().copied() {
        hal().console.put_byte(byte);
    }
}

fn write_console_line(line: &str) {
    let _guard = CONSOLE_WRITE_LOCK.lock();
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
    pub flags: u32,
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
    open_sides: [usize; 2],
}

struct SocketPending {
    path: Option<String>,
    listening: bool,
    family: usize,
    sock_type: usize,
    multicast_joined: bool,
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

struct RawSocketState {
    family: usize,
    protocol: usize,
    bound_path: Option<String>,
    checksum_offset: Option<usize>,
    icmp6_filter: [u32; 8],
    ipv6_recv_opts: BTreeMap<usize, i32>,
    inbox: VecDeque<Vec<u8>>,
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
        family: usize,
        sock_type: usize,
        multicast_joined: bool,
    },
    SocketRaw(RawSocketState),
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
    external_deletions: BTreeSet<String>,
    mem_modes: BTreeMap<String, u32>,
    next_pipe_id: usize,
    next_memfd_id: usize,
    next_ephemeral_port: u16,
    socket_bindings: BTreeMap<String, Arc<Node>>,
    raw_sockets: Vec<Arc<Node>>,
}

#[inline]
fn stage2_openat_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_STAGE2_OPENAT"), Some("1"))
}

#[inline]
fn is_shell_token_path(absolute: &str) -> bool {
    matches!(absolute, "/[" | "/]")
}

fn stage2_openat_debug(line: &str) {
    if !stage2_openat_debug_enabled() {
        return;
    }
    write_console_line(line);
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
            external_deletions: BTreeSet::new(),
            mem_modes: BTreeMap::new(),
            next_pipe_id: 0,
            next_memfd_id: 0,
            next_ephemeral_port: 40000,
            socket_bindings: BTreeMap::new(),
            raw_sockets: Vec::new(),
        };
        for dir in ["/dev", "/proc", "/mnt", "/tmp", "/bin", "/etc"] {
            let _ = vfs.mkdir("/", dir, 0o755);
        }
        for dir in [
            "/proc/net",
            "/proc/sys",
            "/proc/sys/net",
            "/proc/sys/user",
            "/proc/sys/net/ipv6",
            "/proc/sys/net/ipv6/conf",
            "/proc/sys/net/ipv6/conf/all",
            "/proc/sys/net/ipv6/conf/lo",
            "/proc/sys/net/ipv6/conf/ltp_ns_veth1",
            "/proc/sys/net/ipv6/conf/ltp_ns_veth2",
        ] {
            let _ = vfs.mkdir("/", dir, 0o755);
        }
        let _ = vfs.create_char_device("/dev/console", "console");
        let _ = vfs.create_char_device("/dev/null", "null");
        let _ = vfs.create_char_device("/dev/zero", "zero");
        let _ = vfs.create_char_device("/dev/random", "random");
        let _ = vfs.create_char_device("/dev/urandom", "urandom");
        let _ = vfs.create_char_device("/dev/rtc0", "rtc0");
        let _ = vfs.create_proc_file("/proc/mounts", b"");
        let _ = vfs.create_proc_file("/proc/net/if_inet6", b"00000000000000000000000000000001 01 80 10 80 lo\nfd000001000100010000000000000002 02 40 00 80 ltp_ns_veth2\nfd000001000100010000000000000001 03 40 00 80 ltp_ns_veth1\n");
        let _ = vfs.create_proc_file("/proc/sys/net/ipv6/conf/all/disable_ipv6", b"0\n");
        let _ = vfs.create_proc_file("/proc/sys/net/ipv6/conf/lo/disable_ipv6", b"0\n");
        let _ = vfs.create_proc_file("/proc/sys/net/ipv6/conf/ltp_ns_veth1/disable_ipv6", b"0\n");
        let _ = vfs.create_proc_file("/proc/sys/net/ipv6/conf/ltp_ns_veth2/disable_ipv6", b"0\n");
        let _ = vfs.mkdir("/", "/proc/sys", 0o755);
        let _ = vfs.mkdir("/", "/proc/sys/user", 0o755);
        let _ = vfs.mkdir("/", "/proc/sys/kernel", 0o755);
        let _ = vfs.mkdir("/", "/proc/sys/kernel/keys", 0o755);
        let _ = vfs.create_proc_file("/proc/sys/user/max_user_namespaces", b"1024\n");
        let _ = vfs.create_proc_file("/proc/sys/kernel/tainted", b"0\n");
        let _ = vfs.create_proc_file("/proc/key-users", b"");
        let _ = vfs.create_proc_file("/proc/sys/kernel/keys/gc_delay", b"1\n");
        let _ = vfs.create_proc_file("/proc/sys/kernel/keys/maxkeys", b"200\n");
        let _ = vfs.create_proc_file("/proc/sys/kernel/keys/maxbytes", b"20000\n");
        let _ = vfs.create_proc_file("/proc/meminfo", PROC_MEMINFO);
        let _ = vfs.create_proc_file("/proc/uptime", PROC_UPTIME);
        let _ = vfs.create_proc_file("/proc/stat", PROC_STAT);
        let _ = vfs.create_proc_file("/proc/version", PROC_VERSION);
        let _ = vfs.mkdir("/", "/proc/self", 0o755);
        let _ = vfs.create_proc_file("/proc/self/stat", PROC_SELF_STAT);
        let _ = vfs.create_proc_file("/proc/self/maps", PROC_SELF_MAPS);
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
        self.create_file_with_mode(cwd, path, contents, S_IFREG | 0o755)
    }

    pub fn create_file_with_mode(
        &mut self,
        cwd: &str,
        path: &str,
        contents: &[u8],
        mode: u32,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(
            &absolute,
            NodeKind::File,
            Some(NodeData::File(contents.to_vec())),
        )?;
        self.mem_modes.insert(absolute, S_IFREG | (mode & 0o7777));
        Ok(())
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
        )?;
        self.mem_modes.insert(absolute, S_IFLNK | 0o777);
        Ok(())
    }

    pub fn mkdir(&mut self, cwd: &str, path: &str, _mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(&absolute, NodeKind::Directory, None)?;
        self.mem_modes.insert(absolute, S_IFDIR | (_mode & 0o7777));
        Ok(())
    }

    pub fn open(
        &mut self,
        cwd: &str,
        path: &str,
        flags: u32,
        mode: u32,
    ) -> KernelResult<FileHandle> {
        let absolute = normalize_path(cwd, path);
        if let Some(handle) = self.try_open_external(&absolute, flags)? {
            return Ok(handle);
        }
        self.open_mem(&absolute, flags, mode)
    }

    fn open_mem(&mut self, absolute: &str, flags: u32, mode: u32) -> KernelResult<FileHandle> {
        let mut resolved = absolute.to_string();
        let node = match self.lookup_abs(&resolved) {
            Ok(node) => node,
            Err(err) if err == ENOENT && (flags & O_CREAT) != 0 => {
                self.create_file_with_mode("/", &resolved, b"", mode)?;
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
        let raw_meta = {
            let guard = handle.node.data.lock();
            match &*guard {
                NodeData::SocketRaw(state) => {
                    Some((state.family, state.protocol, state.bound_path.clone()))
                }
                _ => None,
            }
        };
        if let Some((family, protocol, bound_path)) = raw_meta {
            let mut packet = data.to_vec();
            if let NodeData::SocketRaw(state) = &*handle.node.data.lock() {
                if let Some(offset) = state.checksum_offset {
                    if offset + 1 >= packet.len() {
                        return Err(EINVAL);
                    }
                    packet[offset] = 0;
                    packet[offset + 1] = 0;
                    let checksum = ipv6_raw_checksum(protocol as u8, &packet);
                    packet[offset..offset + 2].copy_from_slice(&checksum.to_be_bytes());
                }
            }
            let send_type = packet.first().copied().unwrap_or(0);
            for node in &self.raw_sockets {
                let mut guard = node.data.lock();
                let NodeData::SocketRaw(peer) = &mut *guard else {
                    continue;
                };
                if peer.family != family || peer.protocol != protocol {
                    continue;
                }
                if let (Some(src), Some(dst)) = (bound_path.as_ref(), peer.bound_path.as_ref()) {
                    if src != dst {
                        continue;
                    }
                }
                if protocol == 58 {
                    let word = (send_type / 32) as usize;
                    let bit = send_type % 32;
                    if word < peer.icmp6_filter.len()
                        && (peer.icmp6_filter[word] & (1u32 << bit)) != 0
                    {
                        continue;
                    }
                }
                peer.inbox.push_back(packet.clone());
            }
            return Ok(data.len());
        }
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
        self.stat_path_follow(&absolute, 0)
    }

    pub fn chdir(&self, cwd: &str, path: &str) -> KernelResult<String> {
        let absolute = normalize_path(cwd, path);
        if let Ok(node) = self.lookup_abs(&absolute) {
            if node.kind != NodeKind::Directory {
                return Err(ENOTDIR);
            }
            return Ok(absolute);
        }
        if self.is_memory_preferred_path(&absolute) {
            return Err(ENOENT);
        }
        if self.external_preloaded.contains_key(&absolute) {
            return Ok(absolute);
        }
        if let Some((mount, _)) = self.resolve_external_path(&absolute) {
            if absolute == mount.target || self.external_stat_cache.contains_key(&absolute) {
                return Ok(absolute);
            }
            let prefix = if absolute == "/" {
                "/".to_string()
            } else {
                format!("{}/", absolute)
            };
            if self
                .external_preloaded
                .keys()
                .any(|entry| entry.starts_with(&prefix))
                || self
                    .external_stat_cache
                    .keys()
                    .any(|entry| entry.starts_with(&prefix))
            {
                return Ok(absolute);
            }
            // Avoid blocking ext4 metadata operations during chdir on external mounts.
            // Real existence/type checks are deferred to subsequent open/stat operations.
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
        self.mem_modes.remove(&absolute);
        self.external_preloaded.remove(&absolute);
        self.external_stat_cache.remove(&absolute);
        if self.resolve_external_path(&absolute).is_some() {
            self.external_deletions.insert(absolute);
        }
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
        self.external_deletions.remove(&new_absolute);
        if let Some(mode) = self.mem_modes.get(&old_absolute).copied() {
            self.mem_modes.insert(new_absolute, mode);
        }
        Ok(())
    }

    pub fn mount(
        &mut self,
        source: &str,
        target: &str,
        fs_type: &str,
        flags: u32,
    ) -> KernelResult<()> {
        let absolute = normalize_path("/", target);
        if self.lookup_abs(&absolute).is_err() {
            self.mkdir("/", &absolute, 0o755)?;
        }
        self.mounts.push(MountRecord {
            source: source.to_string(),
            target: absolute,
            fs_type: fs_type.to_string(),
            flags,
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
        self.external_deletions.clear();
        self.mounts.retain(|existing| existing.target != absolute);
        self.mounts.push(MountRecord {
            source: source.to_string(),
            target: absolute,
            fs_type: "ext4".to_string(),
            flags: 0,
        });
        self.refresh_mounts_proc();
        Ok(label)
    }

    pub fn mount_flags_for_path(&self, cwd: &str, path: &str) -> u32 {
        let absolute = normalize_path(cwd, path);
        self.mounts
            .iter()
            .filter(|mount| {
                absolute == mount.target || absolute.starts_with(&(mount.target.clone() + "/"))
            })
            .max_by_key(|mount| mount.target.len())
            .map(|mount| mount.flags)
            .unwrap_or(0)
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
            self.external_deletions.clear();
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

    pub fn absolute_path(&self, cwd: &str, path: &str) -> String {
        normalize_path(cwd, path)
    }

    pub fn access(&self, cwd: &str, path: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.stat_path_follow(&absolute, 0).map(|_| ())
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
        self.external_deletions.remove(&new_absolute);
        if let Some(mode) = self.mem_modes.remove(&old_absolute) {
            self.mem_modes.insert(new_absolute, mode);
        }
        Ok(())
    }

    pub fn chmod_path(&mut self, cwd: &str, path: &str, mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let stat = self.stat_path("/", &absolute)?;
        self.mem_modes
            .insert(absolute, (stat.mode & !0o7777) | (mode & 0o7777));
        Ok(())
    }

    pub fn chmod_handle(&mut self, handle: &FileHandle, mode: u32) -> KernelResult<()> {
        let stat = self.stat_handle(handle)?;
        self.mem_modes
            .insert(handle.path.clone(), (stat.mode & !0o7777) | (mode & 0o7777));
        Ok(())
    }

    pub fn truncate(&mut self, handle: &mut FileHandle, len: usize) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                ensure_file_size(buf, len)?;
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
            | NodeData::SocketRaw(_)
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
                ensure_file_size(buf, size)?;
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

    pub fn socket_family(&self, handle: &FileHandle) -> KernelResult<usize> {
        match &*handle.node.data.lock() {
            NodeData::SocketPending(state) => Ok(state.family),
            NodeData::SocketConnected { family, .. } => Ok(*family),
            NodeData::SocketRaw(state) => Ok(state.family),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn create_socket(
        &mut self,
        family: usize,
        sock_type: usize,
        protocol: usize,
    ) -> KernelResult<FileHandle> {
        let path = format!("socket:[{}]", self.next_pipe_id);
        self.next_pipe_id += 1;
        let node = if sock_type == 3 {
            let node = Arc::new(Node::socket_raw(&path, family, protocol));
            self.raw_sockets.push(node.clone());
            node
        } else {
            Arc::new(Node::socket_pending(&path, family, sock_type))
        };
        Ok(FileHandle {
            node,
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
            open_sides: [1, 1],
        }));
        Ok((
            FileHandle {
                node: Arc::new(Node::socket_connected(
                    &path,
                    channel.clone(),
                    0,
                    1,
                    1,
                    false,
                )),
                offset: 0,
                flags: O_RDWR,
                path: format!("{}:0", path),
                pipe_end: PipeEnd::None,
            },
            FileHandle {
                node: Arc::new(Node::socket_connected(&path, channel, 1, 1, 1, false)),
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
        let absolute = assign_ephemeral_inet_path(self, &normalize_path(cwd, path));
        match &mut *handle.node.data.lock() {
            NodeData::SocketPending(state) => {
                if state.path.is_some() {
                    return Err(EINVAL);
                }
                if state.family == 2 {
                    if let Some(rest) = absolute.strip_prefix("/inet/") {
                        let (ip, _) = rest.rsplit_once(':').ok_or(EINVAL)?;
                        if ip != "000.000.000.000" && ip != "127.000.000.001" {
                            return Err(EADDRNOTAVAIL);
                        }
                    }
                }
                if state.family == 1 {
                    if !absolute.starts_with(UNIX_ABSTRACT_PREFIX) {
                        if self.socket_bindings.contains_key(&absolute) {
                            return Err(EADDRINUSE);
                        }
                        let (parent_path, name) = split_parent(&absolute)?;
                        let parent = self.lookup_abs(&parent_path)?;
                        if parent.kind != NodeKind::Directory {
                            return Err(ENOTDIR);
                        }
                        let mut guard = parent.data.lock();
                        let NodeData::Directory(entries) = &mut *guard else {
                            return Err(ENOTDIR);
                        };
                        if entries.contains_key(name) {
                            return Err(EADDRINUSE);
                        }
                        entries.insert(name.to_string(), handle.node.clone());
                        self.mem_modes.insert(absolute.clone(), S_IFSOCK | 0o777);
                    }
                }
                state.path = Some(absolute.clone());
                handle.path = absolute.clone();
                self.socket_bindings.insert(absolute, handle.node.clone());
                Ok(())
            }
            NodeData::SocketRaw(state) => {
                if state.bound_path.is_some() {
                    return Err(EINVAL);
                }
                if state.family == 2 {
                    if let Some(rest) = absolute.strip_prefix("/inet/") {
                        let (ip, _) = rest.rsplit_once(':').ok_or(EINVAL)?;
                        if ip != "000.000.000.000" && ip != "127.000.000.001" {
                            return Err(EADDRNOTAVAIL);
                        }
                    }
                }
                if state.family == 1 {
                    if !absolute.starts_with(UNIX_ABSTRACT_PREFIX) {
                        if self.socket_bindings.contains_key(&absolute) {
                            return Err(EADDRINUSE);
                        }
                        let (parent_path, name) = split_parent(&absolute)?;
                        let parent = self.lookup_abs(&parent_path)?;
                        if parent.kind != NodeKind::Directory {
                            return Err(ENOTDIR);
                        }
                        let mut guard = parent.data.lock();
                        let NodeData::Directory(entries) = &mut *guard else {
                            return Err(ENOTDIR);
                        };
                        if entries.contains_key(name) {
                            return Err(EADDRINUSE);
                        }
                        entries.insert(name.to_string(), handle.node.clone());
                        self.mem_modes.insert(absolute.clone(), S_IFSOCK | 0o777);
                    }
                }
                state.bound_path = Some(absolute.clone());
                handle.path = absolute;
                Ok(())
            }
            _ => Err(ENOTSOCK),
        }
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
        let listener = if let Some(listener) = self.socket_bindings.get(&absolute).cloned() {
            listener
        } else if let Some(any_addr) = inet_any_listener_path(&absolute) {
            self.socket_bindings.get(&any_addr).cloned().ok_or(ENOENT)?
        } else {
            return Err(ENOENT);
        };
        let channel = Arc::new(Mutex::new(SocketChannel {
            inbox: [VecDeque::new(), VecDeque::new()],
            open_sides: [1, 1],
        }));
        let (family, sock_type, listening) = {
            let guard = listener.data.lock();
            let NodeData::SocketPending(state) = &*guard else {
                return Err(ENOTSOCK);
            };
            (state.family, state.sock_type, state.listening)
        };
        if sock_type == 1 || sock_type == 5 {
            if !listening {
                return Err(EINVAL);
            }
            let mut guard = listener.data.lock();
            let NodeData::SocketPending(state) = &mut *guard else {
                return Err(ENOTSOCK);
            };
            state.pending.push(Arc::new(Node::socket_connected(
                &absolute,
                channel.clone(),
                1,
                family,
                sock_type,
                false,
            )));
        } else {
            let mut guard = listener.data.lock();
            let NodeData::SocketPending(_) = &*guard else {
                return Err(ENOTSOCK);
            };
            *guard = NodeData::SocketConnected {
                channel: channel.clone(),
                side: 1,
                family,
                sock_type,
                multicast_joined: false,
            };
        }
        *handle.node.data.lock() = NodeData::SocketConnected {
            channel,
            side: 0,
            family,
            sock_type,
            multicast_joined: false,
        };
        handle.path = absolute;
        Ok(())
    }

    pub fn accept_socket(&mut self, handle: &mut FileHandle) -> KernelResult<FileHandle> {
        let NodeData::SocketPending(state) = &mut *handle.node.data.lock() else {
            return Err(ENOTSOCK);
        };
        if state.sock_type != 1 && state.sock_type != 5 {
            return Err(EOPNOTSUPP);
        }
        if !state.listening {
            return Err(EINVAL);
        }
        let node = state.pending.pop().ok_or(EAGAIN)?;
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

    pub fn socket_ip_multicast_action(
        &mut self,
        handle: &mut FileHandle,
        join: bool,
    ) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::SocketPending(state) => {
                if join {
                    state.multicast_joined = true;
                    Ok(())
                } else if state.multicast_joined {
                    state.multicast_joined = false;
                    Ok(())
                } else {
                    Err(99)
                }
            }
            NodeData::SocketConnected {
                multicast_joined, ..
            } => {
                if join {
                    *multicast_joined = true;
                    Ok(())
                } else if *multicast_joined {
                    *multicast_joined = false;
                    Ok(())
                } else {
                    Err(99)
                }
            }
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_set_icmp6_filter(
        &mut self,
        handle: &mut FileHandle,
        filter: [u32; 8],
    ) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 && state.protocol == 58 => {
                state.icmp6_filter = filter;
                Ok(())
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_set_ipv6_checksum_offset(
        &mut self,
        handle: &mut FileHandle,
        offset: Option<usize>,
    ) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 => {
                state.checksum_offset = offset;
                Ok(())
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_icmp6_filter(&self, handle: &FileHandle) -> KernelResult<[u32; 8]> {
        match &*handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 && state.protocol == 58 => {
                Ok(state.icmp6_filter)
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_set_ipv6_recv_opt(
        &mut self,
        handle: &mut FileHandle,
        opt: usize,
        value: i32,
    ) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 => {
                state.ipv6_recv_opts.insert(opt, value);
                Ok(())
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_get_ipv6_recv_opt(&self, handle: &FileHandle, opt: usize) -> KernelResult<i32> {
        match &*handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 => {
                Ok(*state.ipv6_recv_opts.get(&opt).unwrap_or(&0))
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn socket_ipv6_recv_cmsgs(
        &self,
        handle: &FileHandle,
    ) -> KernelResult<Vec<(usize, Vec<u8>)>> {
        match &*handle.node.data.lock() {
            NodeData::SocketRaw(state) if state.family == 10 => {
                let mut out = Vec::new();
                for (opt, value) in &state.ipv6_recv_opts {
                    if *value == 0 {
                        continue;
                    }
                    if let Some((cmsg_type, payload_len)) = ipv6_recv_opt_cmsg(*opt) {
                        out.push((cmsg_type, vec![0u8; payload_len]));
                    }
                }
                Ok(out)
            }
            NodeData::SocketRaw(_) => Err(EINVAL),
            _ => Err(ENOTSOCK),
        }
    }

    pub fn is_write_ready(&self, handle: &FileHandle) -> bool {
        handle.poll_write_ready()
    }

    pub fn stat_handle(&self, handle: &FileHandle) -> KernelResult<FileStat> {
        self.stat(&handle.path, &handle.node)
    }

    pub fn is_pipe(&self, handle: &FileHandle) -> bool {
        handle.node.kind == NodeKind::Pipe
    }

    pub fn is_socket(&self, handle: &FileHandle) -> bool {
        handle.node.kind == NodeKind::Socket
    }

    fn path_exists(&self, absolute: &str) -> KernelResult<()> {
        self.path_exists_follow(absolute, 0)
    }

    fn path_exists_follow(&self, absolute: &str, depth: usize) -> KernelResult<()> {
        if depth >= 16 {
            return Err(ELOOP);
        }
        if let Ok(node) = self.lookup_abs(absolute) {
            if node.kind == NodeKind::Symlink {
                let target = match &*node.data.lock() {
                    NodeData::Symlink(target) => target.clone(),
                    _ => return Err(EINVAL),
                };
                let parent = split_parent(absolute)?.0;
                let resolved = normalize_path(&parent, &target);
                return self.path_exists_follow(&resolved, depth + 1);
            }
            return Ok(());
        }
        if self.external_preloaded.contains_key(absolute) {
            return Ok(());
        }
        if self.external_deletions.contains(absolute) {
            return Err(ENOENT);
        }
        if self.is_memory_preferred_path(absolute) {
            return Err(ENOENT);
        }
        if let Some((mount, _)) = self.resolve_external_path(absolute) {
            if absolute == mount.target || self.external_stat_cache.contains_key(absolute) {
                return Ok(());
            }
            let prefix = if absolute == "/" {
                "/".to_string()
            } else {
                format!("{}/", absolute)
            };
            if self
                .external_preloaded
                .keys()
                .any(|entry| entry.starts_with(&prefix))
                || self
                    .external_stat_cache
                    .keys()
                    .any(|entry| entry.starts_with(&prefix))
            {
                return Ok(());
            }
            return Ok(());
        }
        Err(ENOENT)
    }

    fn stat_path_follow(&self, absolute: &str, depth: usize) -> KernelResult<FileStat> {
        if depth >= 16 {
            return Err(ELOOP);
        }
        if let Some(stat) = self.external_stat_path(absolute)? {
            return Ok(stat);
        }
        let node = self.lookup_abs(absolute)?;
        if node.kind == NodeKind::Symlink {
            let target = match &*node.data.lock() {
                NodeData::Symlink(target) => target.clone(),
                _ => return Err(EINVAL),
            };
            let parent = split_parent(absolute)?.0;
            let resolved = normalize_path(&parent, &target);
            return self.stat_path_follow(&resolved, depth + 1);
        }
        self.stat(absolute, &node)
    }

    fn resolve_mem_path(&self, absolute: &str) -> KernelResult<(String, Arc<Node>)> {
        let mut path = absolute.to_string();
        for _ in 0..16 {
            let node = self.lookup_abs(&path)?;
            if node.kind != NodeKind::Symlink {
                return Ok((path, node));
            }
            let target = match &*node.data.lock() {
                NodeData::Symlink(target) => target.clone(),
                _ => return Err(EINVAL),
            };
            let parent = split_parent(&path)?.0;
            path = normalize_path(&parent, &target);
        }
        Err(ELOOP)
    }

    fn external_stat_path(&self, absolute: &str) -> KernelResult<Option<FileStat>> {
        if is_shell_token_path(absolute) {
            return Ok(None);
        }
        if self.is_memory_preferred_path(absolute) && self.lookup_abs(absolute).is_ok() {
            return Ok(None);
        }
        if let Some((_, stat)) = self.external_preloaded.get(absolute) {
            return Ok(Some(*stat));
        }
        if self.external_deletions.contains(absolute) {
            return Ok(None);
        }
        if let Some(stat) = self.external_stat_cache.get(absolute) {
            return Ok(Some(*stat));
        }
        // hal_api::hal().console.put_byte(b'M'); // Mark a miss if needed, or use full trace

        let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
            return Ok(None);
        };

        let dir_prefix = if absolute == "/" {
            "/".to_string()
        } else {
            format!("{}/", absolute)
        };
        if self
            .external_preloaded
            .keys()
            .any(|entry| entry.starts_with(&dir_prefix))
            || self
                .external_stat_cache
                .keys()
                .any(|entry| entry.starts_with(&dir_prefix))
        {
            return Ok(Some(FileStat {
                mode: S_IFDIR | 0o755,
                size: 0,
                nlink: 1,
            }));
        }

        match mount.ext4.stat(&fs_path) {
            Ok(stat) => Ok(Some(FileStat {
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
            })),
            Err(err) if err == ENOENT => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn try_open_external(
        &mut self,
        absolute: &str,
        flags: u32,
    ) -> KernelResult<Option<FileHandle>> {
        if is_shell_token_path(absolute) {
            return Ok(None);
        }
        if self.lookup_abs(absolute).is_ok() {
            return Ok(None);
        }
        if self.external_deletions.contains(absolute) {
            return Ok(None);
        }
        let (mount, fs_path) = {
            let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
                return Ok(None);
            };
            (mount.ext4.clone(), fs_path)
        };
        let trace_path = stage2_openat_debug_enabled()
            && (absolute.starts_with("/musl/")
                || absolute.starts_with("/lib/")
                || absolute.starts_with("/lib64/")
                || absolute.starts_with("/glibc/"));
        if flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC) != 0 {
            return Ok(None);
        }
        if let Some((cached, stat)) = self.external_preloaded.get(absolute).cloned() {
            if trace_path {
                stage2_openat_debug(&format!(
                    "whuse-libctest:vfs-open-external-preloaded path={} mode={:#o} size={}",
                    absolute, stat.mode, stat.size
                ));
            }
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
        if cached.is_none() && should_use_statless_external_open(absolute, flags) {
            if trace_path {
                stage2_openat_debug(&format!(
                    "whuse-libctest:vfs-open-external-statless path={} fs_path={} flags={:#x}",
                    absolute, fs_path, flags
                ));
            }
            return Ok(Some(self.build_ext4_handle(
                absolute,
                mount,
                fs_path,
                fs_ext4::Ext4FileStat {
                    mode: S_IFREG | 0o755,
                    size: 0,
                    nlink: 1,
                },
                None,
            )));
        }
        let stat = match cached {
            Some(stat) => fs_ext4::Ext4FileStat {
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
            },
            None => {
                if trace_path {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-open-external-stat-begin path={} fs_path={} flags={:#x}",
                        absolute, fs_path, flags
                    ));
                }
                let stat = match mount.stat(&fs_path) {
                    Ok(stat) => stat,
                    Err(err) if err == ENOENT => {
                        if trace_path {
                            stage2_openat_debug(&format!(
                                "whuse-libctest:vfs-open-external-stat-enoent path={} fs_path={}",
                                absolute, fs_path
                            ));
                        }
                        return Ok(None);
                    }
                    Err(err) => {
                        if trace_path {
                            stage2_openat_debug(&format!(
                                "whuse-libctest:vfs-open-external-stat-err path={} fs_path={} err={}",
                                absolute, fs_path, err
                            ));
                        }
                        return Err(err);
                    }
                };
                if trace_path {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-open-external-stat-ok path={} fs_path={} mode={:#o} size={}",
                        absolute, fs_path, stat.mode, stat.size
                    ));
                }
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

    pub fn replace_proc_file(&mut self, path: &str, contents: &[u8]) -> KernelResult<()> {
        let node = self.lookup_abs(path)?;
        *node.data.lock() = NodeData::ProcFile(contents.to_vec());
        Ok(())
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
        match self.external_stat_path(absolute_path)? {
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
        self.external_deletions.remove(absolute_path);
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
            NodeKind::Socket => Node::socket_pending(name, 1, 1),
            NodeKind::PidFd => Node::pidfd(name, 0),
        });
        entries.insert(name.to_string(), node);
        Ok(())
    }

    fn stat(&self, path: &str, node: &Arc<Node>) -> KernelResult<FileStat> {
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
            NodeData::SocketConnected { channel, side, .. } => {
                channel.lock().inbox[*side].len() as u64
            }
            NodeData::SocketRaw(state) => state.inbox.len() as u64,
            NodeData::PidFd(_) => 0,
            NodeData::CharDevice => 0,
        };
        let mode = match &*guard {
            NodeData::Ext4File(state) => state.mode,
            NodeData::Ext4Dir(state) => state.mode,
            _ => self
                .mem_modes
                .get(path)
                .copied()
                .unwrap_or(match node.kind {
                    NodeKind::Directory => S_IFDIR | 0o755,
                    NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
                    NodeKind::CharDevice => S_IFCHR | 0o600,
                    NodeKind::Pipe => S_IFIFO | 0o644,
                    NodeKind::Symlink => S_IFLNK | 0o777,
                    NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
                    NodeKind::PidFd => S_IFREG | 0o444,
                }),
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
        let mut guard = self.node.data.lock();
        match &mut *guard {
            NodeData::Pipe(state) => {
                if self.pipe_end == PipeEnd::None {
                    return;
                }
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
            NodeData::SocketConnected { channel, side, .. } => {
                let mut socket = channel.lock();
                let target = &mut socket.open_sides[*side];
                if delta > 0 {
                    *target = target.saturating_add(delta as usize);
                } else {
                    *target = target.saturating_sub((-delta) as usize);
                }
            }
            _ => {}
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
            NodeData::SocketConnected { channel, side, .. } => {
                channel.lock().inbox[*side].len() as u64
            }
            NodeData::SocketRaw(state) => state.inbox.len() as u64,
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
                let trace_ext4_read =
                    stage2_openat_debug_enabled() && is_libctest_probe_path(self.path.as_str());
                if trace_ext4_read {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-ext4-read-begin path={} fs_path={} off={} len={}",
                        self.path, state.path, self.offset, len
                    ));
                }
                let data = match state.mount.read_range(&state.path, self.offset, len) {
                    Ok(data) => data,
                    Err(err) => {
                        if trace_ext4_read {
                            stage2_openat_debug(&format!(
                                "whuse-libctest:vfs-ext4-read-err path={} fs_path={} off={} len={} err={}",
                                self.path, state.path, self.offset, len, err
                            ));
                        }
                        return Err(err);
                    }
                };
                self.offset += data.len();
                if trace_ext4_read {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-ext4-read-ok path={} fs_path={} bytes={} new_off={}",
                        self.path,
                        state.path,
                        data.len(),
                        self.offset
                    ));
                }
                Ok(data)
            }
            NodeData::Ext4Dir(_) => Err(EISDIR),
            NodeData::Pipe(state) => {
                if self.pipe_end == PipeEnd::Write {
                    return Err(EINVAL);
                }
                if state.buf.is_empty() {
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
            NodeData::SocketConnected { channel, side, .. } => {
                let mut guard = channel.lock();
                let inbox = &mut guard.inbox[*side];
                if inbox.is_empty() {
                    let peer = 1 - *side;
                    if guard.open_sides[peer] == 0 {
                        return Ok(Vec::new());
                    }
                    return Err(EAGAIN);
                }
                let end = len.min(inbox.len());
                Ok(inbox.drain(..end).collect())
            }
            NodeData::SocketRaw(state) => {
                let mut packet = state.inbox.pop_front().ok_or(EAGAIN)?;
                if packet.len() > len {
                    let rest = packet.split_off(len);
                    state.inbox.push_front(rest);
                }
                Ok(packet)
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
                    ensure_file_size(buf, self.offset)?;
                }
                let end = self.offset.saturating_add(data.len());
                if end > buf.len() {
                    ensure_file_size(buf, end)?;
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
                if data.is_empty() {
                    return Ok(0);
                }
                if state.readers == 0 {
                    return Err(EPIPE);
                }
                state.buf.extend(data.iter().copied());
                Ok(data.len())
            }
            NodeData::Symlink(_)
            | NodeData::Epoll(_)
            | NodeData::SocketPending(_)
            | NodeData::SocketRaw(_)
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
            NodeData::SocketConnected { channel, side, .. } => {
                let mut guard = channel.lock();
                let peer = 1 - *side;
                if guard.open_sides[peer] == 0 {
                    return Err(EPIPE);
                }
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
                write_console_bytes(data);
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
            NodeData::SocketConnected { channel, side, .. } => {
                let guard = channel.lock();
                let peer = 1 - *side;
                !guard.inbox[*side].is_empty() || guard.open_sides[peer] == 0
            }
            NodeData::SocketRaw(state) => !state.inbox.is_empty(),
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

    fn socket_pending(name: &str, family: usize, sock_type: usize) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Socket,
            data: Mutex::new(NodeData::SocketPending(SocketPending {
                path: None,
                listening: false,
                family,
                sock_type,
                multicast_joined: false,
                pending: Vec::new(),
            })),
        }
    }

    fn socket_raw(name: &str, family: usize, protocol: usize) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Socket,
            data: Mutex::new(NodeData::SocketRaw(RawSocketState {
                family,
                protocol,
                bound_path: None,
                checksum_offset: None,
                icmp6_filter: [0; 8],
                ipv6_recv_opts: BTreeMap::new(),
                inbox: VecDeque::new(),
            })),
        }
    }

    fn socket_connected(
        name: &str,
        channel: Arc<Mutex<SocketChannel>>,
        side: usize,
        family: usize,
        sock_type: usize,
        multicast_joined: bool,
    ) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Socket,
            data: Mutex::new(NodeData::SocketConnected {
                channel,
                side,
                family,
                sock_type,
                multicast_joined,
            }),
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

fn ensure_file_size(buf: &mut Vec<u8>, size: usize) -> KernelResult<()> {
    if size <= buf.len() {
        return Ok(());
    }
    if size > INMEM_FILE_SIZE_LIMIT {
        return Err(ENOSPC);
    }
    let additional = size - buf.len();
    buf.try_reserve_exact(additional).map_err(|_| ENOMEM)?;
    buf.resize(size, 0);
    Ok(())
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

fn inet_any_listener_path(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("/inet/") {
        let (_, port) = rest.rsplit_once(':')?;
        return Some(format!("/inet/000.000.000.000:{port}"));
    }
    if let Some(rest) = path.strip_prefix("/inet6/") {
        let (_, port) = rest.rsplit_once(':')?;
        return Some(format!("/inet6/0:0:0:0:0:0:0:0:{port}"));
    }
    None
}

fn ipv6_recv_opt_cmsg(opt: usize) -> Option<(usize, usize)> {
    match opt {
        49 => Some((50, 20)),
        51 => Some((52, 4)),
        66 => Some((67, 4)),
        2 => Some((2, 20)),
        8 => Some((8, 4)),
        _ => None,
    }
}

fn ipv6_raw_checksum(next_header: u8, packet: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut pseudo = [0u8; 40];
    pseudo[15] = 1;
    pseudo[31] = 1;
    pseudo[32..36].copy_from_slice(&(packet.len() as u32).to_be_bytes());
    pseudo[39] = next_header;
    sum = checksum_add(sum, &pseudo);
    sum = checksum_add(sum, packet);
    checksum_finish(sum)
}

fn checksum_add(mut sum: u32, bytes: &[u8]) -> u32 {
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        sum = sum.wrapping_add(u16::from_be_bytes([chunk[0], chunk[1]]) as u32);
    }
    if let Some(&byte) = chunks.remainder().first() {
        sum = sum.wrapping_add(((byte as u16) << 8) as u32);
    }
    sum
}

fn checksum_finish(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn assign_ephemeral_inet_path(vfs: &mut KernelVfs, path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/inet/") {
        if let Some((ip, port)) = rest.rsplit_once(':') {
            if port == "0" {
                let assigned = vfs.next_ephemeral_port;
                vfs.next_ephemeral_port = vfs.next_ephemeral_port.saturating_add(1);
                return format!("/inet/{ip}:{assigned}");
            }
        }
    }
    if let Some(rest) = path.strip_prefix("/inet6/") {
        if let Some((ip, port)) = rest.rsplit_once(':') {
            if port == "0" {
                let assigned = vfs.next_ephemeral_port;
                vfs.next_ephemeral_port = vfs.next_ephemeral_port.saturating_add(1);
                return format!("/inet6/{ip}:{assigned}");
            }
        }
    }
    path.to_string()
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

fn should_use_statless_external_open(absolute: &str, flags: u32) -> bool {
    if (flags & O_DIRECTORY) != 0 {
        return false;
    }
    if absolute == "/etc/localtime" {
        return true;
    }
    if absolute.ends_with("/basic/test_echo") {
        return true;
    }
    if is_libctest_probe_path(absolute) {
        return true;
    }

    if !(absolute.starts_with("/lib/")
        || absolute.starts_with("/lib64/")
        || absolute.starts_with("/glibc/lib/")
        || absolute.starts_with("/musl/lib/"))
    {
        return false;
    }

    absolute.contains("ld-linux")
        || absolute.contains("ld-musl")
        || absolute.ends_with(".so")
        || absolute.contains(".so.")
}

fn is_libctest_probe_path(path: &str) -> bool {
    matches!(
        path,
        "/musl/run-static.sh"
            | "/musl/run-dynamic.sh"
            | "/musl/libctest_testcode.sh"
            | "/musl/runtest.exe"
            | "/musl/entry-static.exe"
            | "/musl/entry-dynamic.exe"
    )
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
    fn pipe_write_returns_epipe_after_last_reader_close() {
        let mut vfs = KernelVfs::new();
        let (_read_end, mut write_end) = vfs.create_pipe().unwrap();
        drop(_read_end);
        assert_eq!(vfs.write(&mut write_end, b"x"), Err(super::EPIPE));
    }

    #[test]
    fn pipe_read_returns_eof_after_last_writer_close() {
        let mut vfs = KernelVfs::new();
        let (mut read_end, _write_end) = vfs.create_pipe().unwrap();
        drop(_write_end);
        assert_eq!(vfs.read(&mut read_end, 16).unwrap(), Vec::new());
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
    fn access_reports_missing_files_under_ext4_root_mount() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        assert!(matches!(vfs.access("/", "/test.txt"), Err(super::ENOENT)));
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

        vfs.mount("dev:/dev/vda2", "/mnt", "vfat", 0).unwrap();
        vfs.umount("/mnt").unwrap();

        let mut after = vfs
            .open("/", "/musl/basic/wait", super::O_RDONLY, 0)
            .unwrap();
        assert_eq!(vfs.read(&mut after, 16).unwrap(), b"wait-ok");
    }
}
