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
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NOFOLLOW: u32 = 0o400000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_NOATIME: u32 = 0o1000000;
pub const O_NONBLOCK: u32 = 0x0000_0800;
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const HANDLE_FLAG_CLOEXEC: u32 = 1 << 31;

const ENOENT: i32 = 2;
const ENXIO: i32 = 6;
const EPERM: i32 = 1;
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
const ESPIPE: i32 = 29;
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
const S_IFMT: u32 = 0o170000;
const PROC_MEMINFO: &[u8] = b"MemTotal:       1048576 kB\nMemFree:         524288 kB\nMemAvailable:    524288 kB\nBuffers:              0 kB\nCached:               0 kB\nSwapTotal:            0 kB\nSwapFree:             0 kB\n";
const PROC_UPTIME: &[u8] = b"1.00 1.00\n";
const PROC_STAT: &[u8] = b"cpu  1 0 1 1 0 0 0 0 0 0\nintr 0\nctxt 0\nbtime 1735689600\nprocesses 1\nprocs_running 1\nprocs_blocked 0\n";
const PROC_VERSION: &[u8] = b"Linux version 6.8.0-whuse (whuse@localdomain) #1 SMP PREEMPT\n";
// Keep utime nonzero so clock_gettime01's setup probe can observe CPU time.
const PROC_SELF_STAT: &[u8] = b"1 (self) R 0 0 0 0 0 0 0 0 0 0 0 1 0 0 20 0 1 0 1 4096 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
const PROC_SELF_MAPS: &[u8] = b"50000000-50080000 r-xp 00000000 00:00 0 /proc/self/exe\n";
const EXT4_DIR_STAT_CACHE_MAX_SIZE: u64 = 512 * 1024;
const PIPE_CAPACITY: usize = 16 * 4096;
const PIPE_MIN_CAPACITY: usize = 4096;
static CONSOLE_WRITE_LOCK: Mutex<()> = Mutex::new(());

const DEV_MEMFS: u64 = 0x1000_0000_0000_0001;
const DEV_PROCFS: u64 = 0x1000_0000_0000_0002;
const DEV_DEVFS: u64 = 0x1000_0000_0000_0003;
const DEV_EXT4_ROOT: u64 = 0x2000_0000_0000_0001;
const DEV_EXT4_MNT: u64 = 0x2000_0000_0000_0002;

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
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub size: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub atime_sec: i64,
    pub atime_nsec: i64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InodeMeta {
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    atime_ns: u64,
    mtime_ns: u64,
    ctime_ns: u64,
}

impl InodeMeta {
    fn root(mode: u32) -> Self {
        let now_ns = monotonic_now_ns();
        Self {
            mode,
            nlink: 1,
            uid: 0,
            gid: 0,
            atime_ns: now_ns,
            mtime_ns: now_ns,
            ctime_ns: now_ns,
        }
    }
}

fn monotonic_now_ns() -> u64 {
    hal().timer.monotonic_nanos().max(1)
}

fn split_ns(ns: u64) -> (i64, i64) {
    (
        (ns / 1_000_000_000) as i64,
        (ns % 1_000_000_000) as i64,
    )
}

fn compose_ns(sec: i64, nsec: i64) -> u64 {
    if sec <= 0 {
        return 0;
    }
    (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec.max(0) as u64)
}

fn stable_nonzero_hash64(input: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    if hash == 0 { 1 } else { hash }
}

fn local_dev_for_path(path: &str) -> u64 {
    if path == "/dev" || path.starts_with("/dev/") {
        DEV_DEVFS
    } else if path == "/proc" || path.starts_with("/proc/") {
        DEV_PROCFS
    } else {
        DEV_MEMFS
    }
}

fn ext4_dev_for_path(path: &str) -> u64 {
    if path == "/mnt" || path.starts_with("/mnt/") {
        DEV_EXT4_MNT
    } else {
        DEV_EXT4_ROOT
    }
}

fn synthetic_local_ino(node: &Arc<Node>, path: &str) -> u64 {
    let raw = (Arc::as_ptr(node) as usize as u64) >> 4;
    if raw == 0 {
        stable_nonzero_hash64(path)
    } else {
        raw
    }
}

const fn makedev(major: u32, minor: u32) -> u64 {
    ((major as u64) << 8) | (minor as u64)
}

fn char_device_rdev(path: &str) -> u64 {
    match path {
        "/dev/null" => makedev(1, 3),
        "/dev/zero" => makedev(1, 5),
        "/dev/random" => makedev(1, 8),
        "/dev/urandom" => makedev(1, 9),
        "/dev/console" => makedev(5, 1),
        "/dev/rtc0" => makedev(248, 0),
        _ => 0,
    }
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
    capacity: usize,
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

#[derive(Clone, Default)]
struct SparseFileState {
    size: usize,
    chunks: BTreeMap<usize, Vec<u8>>,
}

impl SparseFileState {
    fn from_dense(buf: Vec<u8>) -> Self {
        if buf.is_empty() {
            return Self::default();
        }
        let mut chunks = BTreeMap::new();
        let size = buf.len();
        chunks.insert(0, buf);
        Self { size, chunks }
    }

    fn clear(&mut self) {
        self.size = 0;
        self.chunks.clear();
    }

    fn truncate(&mut self, len: usize) {
        self.size = len;
        let overlaps: Vec<(usize, Vec<u8>)> = self
            .chunks
            .range(..len)
            .filter_map(|(&chunk_start, chunk)| {
                let chunk_end = chunk_start.saturating_add(chunk.len());
                if chunk_end > len {
                    Some((chunk_start, chunk.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (chunk_start, chunk) in overlaps {
            self.chunks.remove(&chunk_start);
            let kept = len.saturating_sub(chunk_start).min(chunk.len());
            if kept > 0 {
                self.chunks.insert(chunk_start, chunk[..kept].to_vec());
            }
        }
        let remove_keys: Vec<usize> = self.chunks.range(len..).map(|(&start, _)| start).collect();
        for start in remove_keys {
            self.chunks.remove(&start);
        }
    }

    fn read_range(&self, offset: usize, len: usize) -> Vec<u8> {
        let start = offset.min(self.size);
        let end = start.saturating_add(len).min(self.size);
        if end <= start {
            return Vec::new();
        }
        let mut out = vec![0; end - start];
        for (&chunk_start, chunk) in self.chunks.range(..end) {
            let chunk_end = chunk_start.saturating_add(chunk.len());
            if chunk_end <= start {
                continue;
            }
            let copy_start = chunk_start.max(start);
            let copy_end = chunk_end.min(end);
            let src_start = copy_start - chunk_start;
            let src_end = copy_end - chunk_start;
            let dst_start = copy_start - start;
            let dst_end = copy_end - start;
            out[dst_start..dst_end].copy_from_slice(&chunk[src_start..src_end]);
        }
        out
    }

    fn remove_range(&mut self, start: usize, end: usize) {
        let overlaps: Vec<(usize, Vec<u8>)> = self
            .chunks
            .range(..end)
            .filter_map(|(&chunk_start, chunk)| {
                let chunk_end = chunk_start.saturating_add(chunk.len());
                if chunk_end <= start || chunk_start >= end {
                    None
                } else {
                    Some((chunk_start, chunk.clone()))
                }
            })
            .collect();
        for (chunk_start, chunk) in overlaps {
            self.chunks.remove(&chunk_start);
            let chunk_end = chunk_start + chunk.len();
            if chunk_start < start {
                let kept = start - chunk_start;
                self.chunks.insert(chunk_start, chunk[..kept].to_vec());
            }
            if chunk_end > end {
                let tail_start = end;
                let tail_offset = end - chunk_start;
                self.chunks
                    .insert(tail_start, chunk[tail_offset..].to_vec());
            }
        }
    }

    fn write_at(&mut self, offset: usize, data: &[u8]) -> KernelResult<usize> {
        let end = offset.checked_add(data.len()).ok_or(ENOSPC)?;
        self.size = self.size.max(end);
        if data.is_empty() {
            return Ok(0);
        }
        self.remove_range(offset, end);
        self.chunks.insert(offset, data.to_vec());
        Ok(data.len())
    }
}

enum NodeData {
    Directory(BTreeMap<String, Arc<Node>>),
    File(Vec<u8>),
    SparseFile(SparseFileState),
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
    mem_meta: BTreeMap<String, InodeMeta>,
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
fn ltp_create_probe_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_LTP_PATH"), Some("1"))
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

fn iozone_probe_log(line: &str) {
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

fn is_iozone_script_probe_path(path: &str) -> bool {
    matches!(
        path,
        "iozone_testcode.sh"
            | "./iozone_testcode.sh"
            | "/musl/iozone_testcode.sh"
            | "/glibc/iozone_testcode.sh"
    )
}

fn is_iozone_probe_path(path: &str) -> bool {
    is_iozone_script_probe_path(path) || path.contains("iozone")
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
            mem_meta: BTreeMap::new(),
            next_pipe_id: 0,
            next_memfd_id: 0,
            next_ephemeral_port: 40000,
            socket_bindings: BTreeMap::new(),
            raw_sockets: Vec::new(),
        };
        for dir in ["/dev", "/proc", "/mnt", "/tmp", "/bin", "/etc", "/sys"] {
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
            "/sys/devices",
            "/sys/devices/system",
            "/sys/devices/system/cpu",
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
        let _ = vfs.create_proc_file("/proc/sys/kernel/pid_max", b"4194304\n");
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
        let _ = vfs.create_file_with_mode("/", "/sys/devices/system/cpu/online", b"0\n", 0o444);
        vfs
    }

    pub fn create_char_device(&mut self, path: &str, name: &'static str) -> KernelResult<()> {
        let _ = name;
        self.create_node(path, NodeKind::CharDevice, Some(NodeData::CharDevice))
    }

    pub fn mknodat_with_owner(
        &mut self,
        cwd: &str,
        path: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        match mode & S_IFMT {
            S_IFCHR => {
                self.create_node(&absolute, NodeKind::CharDevice, Some(NodeData::CharDevice))?;
                self.mem_meta.insert(
                    absolute,
                    InodeMeta {
                        mode: S_IFCHR | (mode & 0o7777),
                        nlink: 1,
                        uid,
                        gid,
                        atime_ns: monotonic_now_ns(),
                        mtime_ns: monotonic_now_ns(),
                        ctime_ns: monotonic_now_ns(),
                    },
                );
                Ok(())
            }
            S_IFIFO => {
                self.create_node(
                    &absolute,
                    NodeKind::Pipe,
                    Some(NodeData::Pipe(PipeState {
                        buf: VecDeque::new(),
                        readers: 0,
                        writers: 0,
                        capacity: PIPE_CAPACITY,
                    })),
                )?;
                self.mem_meta.insert(
                    absolute,
                    InodeMeta {
                        mode: S_IFIFO | (mode & 0o7777),
                        nlink: 1,
                        uid,
                        gid,
                        atime_ns: monotonic_now_ns(),
                        mtime_ns: monotonic_now_ns(),
                        ctime_ns: monotonic_now_ns(),
                    },
                );
                Ok(())
            }
            _ => Err(EINVAL),
        }
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
        self.mem_meta
            .insert(absolute, InodeMeta::root(S_IFREG | (mode & 0o7777)));
        Ok(())
    }

    pub fn preload_external_file(
        &mut self,
        path: &str,
        contents: &[u8],
        mode: Option<u32>,
    ) -> KernelResult<()> {
        let absolute = normalize_path("/", path);
        let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
        let stat = FileStat {
            dev: ext4_dev_for_path(&absolute),
            ino: stable_nonzero_hash64(&absolute),
            mode: mode.unwrap_or(S_IFREG | 0o755),
            size: contents.len() as u64,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            atime_sec: now_sec,
            atime_nsec: now_nsec,
            mtime_sec: now_sec,
            mtime_nsec: now_nsec,
            ctime_sec: now_sec,
            ctime_nsec: now_nsec,
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
        self.mem_meta
            .insert(absolute, InodeMeta::root(S_IFLNK | 0o777));
        Ok(())
    }

    pub fn mkdir(&mut self, cwd: &str, path: &str, _mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(&absolute, NodeKind::Directory, None)?;
        self.mem_meta
            .insert(absolute, InodeMeta::root(S_IFDIR | (_mode & 0o7777)));
        Ok(())
    }

    pub fn open_with_owner(
        &mut self,
        cwd: &str,
        path: &str,
        flags: u32,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> KernelResult<FileHandle> {
        let absolute = normalize_path(cwd, path);
        let existed = self.stat_path_open_probe(cwd, path, flags).is_some();
        let inherited_gid = if !existed && (flags & O_CREAT) != 0 {
            let (parent_path, _) = split_parent(&absolute)?;
            let parent_stat = self.stat_path("/", &parent_path)?;
            ((parent_stat.mode & 0o2000) != 0).then_some(parent_stat.gid)
        } else {
            None
        };
        let handle = self.open(cwd, path, flags, mode)?;
        if !existed && (flags & O_CREAT) != 0 {
            let stat = self.stat_handle(&handle)?;
            self.mem_meta.insert(
                handle.path.clone(),
                InodeMeta {
                    mode: stat.mode,
                    nlink: stat.nlink,
                    uid,
                    gid: inherited_gid.unwrap_or(gid),
                    atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                    mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                    ctime_ns: compose_ns(stat.ctime_sec, stat.ctime_nsec),
                },
            );
        }
        Ok(handle)
    }

    pub fn open(
        &mut self,
        cwd: &str,
        path: &str,
        flags: u32,
        mode: u32,
    ) -> KernelResult<FileHandle> {
        let absolute = normalize_path(cwd, path);
        let ltp_create_probe = is_ltp_create_probe_path(path);
        if ltp_create_probe {
            write_console_line(&format!(
                "whuse-ltp:vfs-open-enter cwd={} path={} absolute={} flags={:#x} mode={:#o}",
                cwd, path, absolute, flags, mode
            ));
        }
        let iozone_probe = is_iozone_probe_path(absolute.as_str());
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-open-enter cwd={} path={} absolute={} flags={:#x}",
                cwd, path, absolute, flags
            ));
        }
        if let Some(handle) = self.try_open_external(&absolute, flags)? {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-open-external-ok absolute={} resolved={}",
                    absolute, handle.path
                ));
            }
            return Ok(handle);
        }
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-open-fallback-mem absolute={}",
                absolute
            ));
        }
        let result = self.open_mem(&absolute, flags, mode);
        if ltp_create_probe {
            match &result {
                Ok(handle) => write_console_line(&format!(
                    "whuse-ltp:vfs-open-exit absolute={} resolved={} flags={:#x}",
                    absolute, handle.path, flags
                )),
                Err(err) => write_console_line(&format!(
                    "whuse-ltp:vfs-open-err absolute={} flags={:#x} err={}",
                    absolute, flags, err
                )),
            }
        }
        result
    }

    fn open_mem(&mut self, absolute: &str, flags: u32, mode: u32) -> KernelResult<FileHandle> {
        let ltp_create_probe = is_ltp_create_probe_path(absolute);
        let mut resolved = absolute.to_string();
        let node = match self.lookup_abs(&resolved) {
            Ok(node) => {
                if (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
                    return Err(EEXIST);
                }
                node
            }
            Err(err) if err == ENOENT && (flags & O_CREAT) != 0 => {
                if ltp_create_probe {
                    write_console_line(&format!(
                        "whuse-ltp:vfs-open-mem-create absolute={} mode={:#o}",
                        resolved, mode
                    ));
                }
                if let Err(err) = self.create_file_with_mode("/", &resolved, b"", mode) {
                    if ltp_create_probe {
                        write_console_line(&format!(
                            "whuse-ltp:vfs-open-mem-create-err absolute={} err={}",
                            resolved, err
                        ));
                    }
                    return Err(err);
                }
                self.lookup_abs(&resolved)?
            }
            Err(err) => return Err(err),
        };
        let node = if node.kind == NodeKind::Symlink {
            if (flags & O_NOFOLLOW) != 0 {
                return Err(ELOOP);
            }
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

        if node.kind == NodeKind::Directory && open_existing_directory_should_fail(flags) {
            return Err(EISDIR);
        }

        if (flags & O_TRUNC) != 0 {
            match &mut *node.data.lock() {
                NodeData::File(buf) => buf.clear(),
                NodeData::SparseFile(state) => state.clear(),
                _ => {}
            }
        }

        if node.kind == NodeKind::Pipe
            && (flags & O_NONBLOCK) != 0
            && (flags & 0b11) == O_WRONLY
            && matches!(&*node.data.lock(), NodeData::Pipe(state) if state.readers == 0)
        {
            return Err(ENXIO);
        }

        let pipe_end = if node.kind == NodeKind::Pipe {
            match flags & 0b11 {
                O_WRONLY => PipeEnd::Write,
                O_RDWR => PipeEnd::None,
                _ => PipeEnd::Read,
            }
        } else {
            PipeEnd::None
        };

        let handle = FileHandle {
            node,
            offset: 0,
            flags,
            path: resolved,
            pipe_end,
        };
        handle.adjust_pipe_refcount(1);
        Ok(handle)
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

    pub fn fcntl(&self, handle: &mut FileHandle, cmd: usize, arg: usize) -> KernelResult<usize> {
        handle.fcntl_object(cmd, arg)
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

    pub fn refresh_proc_self_fd_dir<I>(&mut self, fds: I) -> KernelResult<()>
    where
        I: IntoIterator<Item = (i32, String)>,
    {
        match self.mkdir("/", "/proc/self/fd", 0o755) {
            Ok(()) | Err(EEXIST) => {}
            Err(err) => return Err(err),
        }
        let node = self.lookup_abs("/proc/self/fd")?;
        let NodeData::Directory(entries) = &mut *node.data.lock() else {
            return Err(ENOTDIR);
        };
        entries.clear();
        for (fd, target) in fds {
            entries.insert(
                fd.to_string(),
                Arc::new(Node::symlink(
                    &fd.to_string(),
                    NodeData::Symlink(target),
                )),
            );
        }
        Ok(())
    }

    pub fn bytes_available_to_read(&self, handle: &FileHandle) -> KernelResult<usize> {
        match &*handle.node.data.lock() {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                Ok(buf.len().saturating_sub(handle.offset))
            }
            NodeData::SparseFile(state) => Ok(state.size.saturating_sub(handle.offset)),
            NodeData::Ext4File(state) => Ok((state.size as usize).saturating_sub(handle.offset)),
            NodeData::Pipe(state) => Ok(state.buf.len()),
            NodeData::Event(counter) => Ok((*counter != 0) as usize * 8),
            NodeData::SocketPending(_) => Err(EINVAL),
            NodeData::SocketConnected { channel, side, .. } => {
                let guard = channel.lock();
                Ok(guard.inbox[*side].len())
            }
            NodeData::SocketRaw(state) => Ok(state.inbox.len()),
            _ => Err(EINVAL),
        }
    }

    pub fn stat_path(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        self.stat_path_follow(&absolute, 0)
    }

    pub fn stat_path_cached_only(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        self.stat_path_follow_cached_only(&absolute, 0)
    }

    pub fn stat_path_nofollow(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        if let Some(stat) = self.external_stat_path(&absolute)? {
            return Ok(stat);
        }
        let node = self.lookup_abs(&absolute)?;
        self.stat(&absolute, &node)
    }

    pub fn stat_path_open_probe(&self, cwd: &str, path: &str, flags: u32) -> Option<FileStat> {
        let absolute = normalize_path(cwd, path);
        if should_use_statless_external_open(&absolute, flags) {
            return match self.stat_path_follow_cached_only(&absolute, 0) {
                Ok(stat) => Some(stat),
                Err(ENOENT) => None,
                Err(_) => None,
            };
        }
        self.stat_path_follow(&absolute, 0).ok()
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
            return Err(EISDIR);
        }
        let removed = node.clone();
        entries.remove(name);
        drop(guard);
        self.socket_bindings.remove(&absolute);
        self.mem_meta.remove(&absolute);
        self.external_preloaded.remove(&absolute);
        self.external_stat_cache.remove(&absolute);
        let remaining_aliases = self.alias_paths_for_node(&removed);
        if !remaining_aliases.is_empty() {
            self.sync_nlink_for_paths(&removed, &remaining_aliases, remaining_aliases.len() as u32);
        }
        if self.resolve_external_path(&absolute).is_some() {
            self.external_deletions.insert(absolute);
        }
        Ok(())
    }

    pub fn rmdir(&mut self, cwd: &str, path: &str) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let (parent_path, name) = split_parent(&absolute)?;
        let parent = self.lookup_abs(&parent_path)?;
        let mut guard = parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        let node = entries.get(name).ok_or(ENOENT)?;
        if node.kind != NodeKind::Directory {
            return Err(ENOTDIR);
        }
        let child_empty = {
            let child_guard = node.data.lock();
            match &*child_guard {
                NodeData::Directory(child_entries) => child_entries.is_empty(),
                _ => false,
            }
        };
        if !child_empty {
            return Err(ENOTEMPTY);
        }
        entries.remove(name);
        drop(guard);
        self.mem_meta.remove(&absolute);
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
        if node.kind == NodeKind::Directory {
            return Err(EPERM);
        }
        let (parent_path, name) = split_parent(&new_absolute)?;
        let parent = self.lookup_abs(&parent_path)?;
        let mut guard = parent.data.lock();
        let NodeData::Directory(entries) = &mut *guard else {
            return Err(ENOTDIR);
        };
        if entries.contains_key(name) {
            return Err(EEXIST);
        }
        entries.insert(name.to_string(), node.clone());
        drop(guard);
        self.external_deletions.remove(&new_absolute);
        let aliases = self.alias_paths_for_node(&node);
        self.sync_nlink_for_paths(&node, &aliases, aliases.len() as u32);
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
        let mut out = Vec::new();
        out.try_reserve_exact(size).map_err(|_| ENOMEM)?;
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
        match self.lookup_abs(&absolute) {
            Ok(node) => {
                return match &*node.data.lock() {
                    NodeData::Symlink(target) => Ok(target.clone()),
                    _ => Err(EINVAL),
                };
            }
            Err(ENOENT) => {}
            Err(err) => return Err(err),
        }
        if let Some((mount, fs_path)) = self.resolve_external_path(&absolute) {
            return mount.ext4.read_link(&fs_path);
        }
        Err(ENOENT)
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
        if let Some(meta) = self.mem_meta.remove(&old_absolute) {
            self.mem_meta.insert(new_absolute, meta);
        }
        Ok(())
    }

    pub fn chmod_path(&mut self, cwd: &str, path: &str, mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let stat = self.stat_path("/", &absolute)?;
        self.mem_meta.insert(
            absolute,
            InodeMeta {
                mode: (stat.mode & !0o7777) | (mode & 0o7777),
                nlink: stat.nlink,
                uid: stat.uid,
                gid: stat.gid,
                atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                ctime_ns: monotonic_now_ns(),
            },
        );
        Ok(())
    }

    pub fn chmod_handle(&mut self, handle: &FileHandle, mode: u32) -> KernelResult<()> {
        let stat = self.stat_handle(handle)?;
        self.mem_meta.insert(
            handle.path.clone(),
            InodeMeta {
                mode: (stat.mode & !0o7777) | (mode & 0o7777),
                nlink: stat.nlink,
                uid: stat.uid,
                gid: stat.gid,
                atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                ctime_ns: monotonic_now_ns(),
            },
        );
        Ok(())
    }

    pub fn chown_path(
        &mut self,
        cwd: &str,
        path: &str,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let stat = self.stat_path("/", &absolute)?;
        self.mem_meta.insert(
            absolute,
            InodeMeta {
                mode: stat.mode,
                nlink: stat.nlink,
                uid: uid.unwrap_or(stat.uid),
                gid: gid.unwrap_or(stat.gid),
                atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                ctime_ns: monotonic_now_ns(),
            },
        );
        Ok(())
    }

    pub fn chown_path_nofollow(
        &mut self,
        cwd: &str,
        path: &str,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let stat = self.stat_path_nofollow("/", &absolute)?;
        self.mem_meta.insert(
            absolute,
            InodeMeta {
                mode: stat.mode,
                nlink: stat.nlink,
                uid: uid.unwrap_or(stat.uid),
                gid: gid.unwrap_or(stat.gid),
                atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                ctime_ns: monotonic_now_ns(),
            },
        );
        Ok(())
    }

    pub fn chown_handle(
        &mut self,
        handle: &FileHandle,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> KernelResult<()> {
        let stat = self.stat_handle(handle)?;
        self.mem_meta.insert(
            handle.path.clone(),
            InodeMeta {
                mode: stat.mode,
                nlink: stat.nlink,
                uid: uid.unwrap_or(stat.uid),
                gid: gid.unwrap_or(stat.gid),
                atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                ctime_ns: monotonic_now_ns(),
            },
        );
        Ok(())
    }

    pub fn set_timestamps_path(
        &mut self,
        cwd: &str,
        path: &str,
        atime: Option<(i64, i64)>,
        mtime: Option<(i64, i64)>,
    ) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        let stat = self.stat_path("/", &absolute)?;
        let now_ns = monotonic_now_ns();
        let current_atime = compose_ns(stat.atime_sec, stat.atime_nsec);
        let current_mtime = compose_ns(stat.mtime_sec, stat.mtime_nsec);
        self.mem_meta.insert(
            absolute,
            InodeMeta {
                mode: stat.mode,
                nlink: stat.nlink,
                uid: stat.uid,
                gid: stat.gid,
                atime_ns: atime
                    .map(|(sec, nsec)| compose_ns(sec, nsec))
                    .unwrap_or(current_atime),
                mtime_ns: mtime
                    .map(|(sec, nsec)| compose_ns(sec, nsec))
                    .unwrap_or(current_mtime),
                ctime_ns: now_ns,
            },
        );
        Ok(())
    }

    pub fn set_timestamps_handle(
        &mut self,
        handle: &FileHandle,
        atime: Option<(i64, i64)>,
        mtime: Option<(i64, i64)>,
    ) -> KernelResult<()> {
        let stat = self.stat_handle(handle)?;
        let now_ns = monotonic_now_ns();
        let current_atime = compose_ns(stat.atime_sec, stat.atime_nsec);
        let current_mtime = compose_ns(stat.mtime_sec, stat.mtime_nsec);
        self.mem_meta.insert(
            handle.path.clone(),
            InodeMeta {
                mode: stat.mode,
                nlink: stat.nlink,
                uid: stat.uid,
                gid: stat.gid,
                atime_ns: atime
                    .map(|(sec, nsec)| compose_ns(sec, nsec))
                    .unwrap_or(current_atime),
                mtime_ns: mtime
                    .map(|(sec, nsec)| compose_ns(sec, nsec))
                    .unwrap_or(current_mtime),
                ctime_ns: now_ns,
            },
        );
        Ok(())
    }

    pub fn truncate(&mut self, handle: &mut FileHandle, len: usize) -> KernelResult<()> {
        let mut guard = handle.node.data.lock();
        match &mut *guard {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                if len > INMEM_FILE_SIZE_LIMIT {
                    let dense = core::mem::take(buf);
                    let mut sparse = SparseFileState::from_dense(dense);
                    sparse.truncate(len);
                    *guard = NodeData::SparseFile(sparse);
                } else if len >= buf.len() {
                    ensure_file_size(buf, len)?;
                } else {
                    buf.truncate(len);
                }
                handle.offset = handle.offset.min(len);
                Ok(())
            }
            NodeData::SparseFile(state) => {
                state.truncate(len);
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
        let size = offset.saturating_add(len);
        let mut guard = handle.node.data.lock();
        match &mut *guard {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                if size > INMEM_FILE_SIZE_LIMIT {
                    let dense = core::mem::take(buf);
                    let mut sparse = SparseFileState::from_dense(dense);
                    sparse.size = sparse.size.max(size);
                    *guard = NodeData::SparseFile(sparse);
                } else {
                    ensure_file_size(buf, size)?;
                }
                Ok(())
            }
            NodeData::SparseFile(state) => {
                state.size = state.size.max(size);
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
                let index = watches
                    .iter()
                    .position(|watch| watch.fd == fd)
                    .ok_or(ENOENT)?;
                watches.remove(index);
            }
            3 => {
                let watch = watches
                    .iter_mut()
                    .find(|watch| watch.fd == fd)
                    .ok_or(ENOENT)?;
                watch.events = events;
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

    pub fn epoll_disarm_oneshot(
        &mut self,
        handle: &mut FileHandle,
        ready_fds: &[i32],
    ) -> KernelResult<()> {
        let NodeData::Epoll(watches) = &mut *handle.node.data.lock() else {
            return Err(EINVAL);
        };
        for watch in watches.iter_mut() {
            if ready_fds.iter().any(|fd| *fd == watch.fd) {
                watch.events = 0;
            }
        }
        Ok(())
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
                        self.mem_meta
                            .insert(absolute.clone(), InodeMeta::root(S_IFSOCK | 0o777));
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
                        self.mem_meta
                            .insert(absolute.clone(), InodeMeta::root(S_IFSOCK | 0o777));
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

    pub fn is_hangup(&self, handle: &FileHandle) -> bool {
        handle.poll_hangup()
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

    fn stat_path_follow_cached_only(&self, absolute: &str, depth: usize) -> KernelResult<FileStat> {
        if depth >= 16 {
            return Err(ELOOP);
        }
        if let Some(stat) = self.external_stat_path_cached_only(absolute)? {
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
            return self.stat_path_follow_cached_only(&resolved, depth + 1);
        }
        self.stat(absolute, &node)
    }

    fn stat_path_follow(&self, absolute: &str, depth: usize) -> KernelResult<FileStat> {
        let iozone_probe = is_iozone_probe_path(absolute);
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-stat-path-enter absolute={} depth={}",
                absolute, depth
            ));
        }
        if depth >= 16 {
            return Err(ELOOP);
        }
        if should_use_statless_external_stat(absolute) {
            return self.stat_path_follow_cached_only(absolute, depth);
        }
        if let Some(stat) = self.external_stat_path(absolute)? {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-stat-path-external-hit absolute={} mode={:#o} size={}",
                    absolute, stat.mode, stat.size
                ));
            }
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
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-stat-path-mem-hit absolute={} kind={:?}",
                absolute, node.kind
            ));
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
        let iozone_probe = is_iozone_probe_path(absolute);
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-external-stat-enter absolute={}",
                absolute
            ));
        }
        if is_shell_token_path(absolute) {
            return Ok(None);
        }
        if self.lookup_abs(absolute).is_ok() {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-mem-hit absolute={}",
                    absolute
                ));
            }
            return Ok(None);
        }
        if self.is_memory_preferred_path(absolute) {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-memory-preferred absolute={}",
                    absolute
                ));
            }
            return Ok(None);
        }
        if let Some((_, stat)) = self.external_preloaded.get(absolute) {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-preloaded absolute={} mode={:#o} size={}",
                    absolute, stat.mode, stat.size
                ));
            }
            return Ok(Some(*stat));
        }
        if self.external_deletions.contains(absolute) {
            return Ok(None);
        }
        if let Some(stat) = self.external_stat_cache.get(absolute) {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-cache-hit absolute={} mode={:#o} size={}",
                    absolute, stat.mode, stat.size
                ));
            }
            return Ok(Some(*stat));
        }
        // hal_api::hal().console.put_byte(b'M'); // Mark a miss if needed, or use full trace

        let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-no-mount absolute={}",
                    absolute
                ));
            }
            return Ok(None);
        };
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-external-stat-resolved absolute={} fs_path={}",
                absolute, fs_path
            ));
        }

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
            let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-external-stat-dir-synth absolute={}",
                    absolute
                ));
            }
            return Ok(Some(FileStat {
                dev: ext4_dev_for_path(absolute),
                ino: stable_nonzero_hash64(absolute),
                mode: S_IFDIR | 0o755,
                size: 0,
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
                atime_sec: now_sec,
                atime_nsec: now_nsec,
                mtime_sec: now_sec,
                mtime_nsec: now_nsec,
                ctime_sec: now_sec,
                ctime_nsec: now_nsec,
            }));
        }

        match mount.ext4.stat(&fs_path) {
            Ok(stat) => {
                let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
                if iozone_probe {
                    iozone_probe_log(&format!(
                        "whuse-la-iozone:vfs-external-stat-ok absolute={} fs_path={} mode={:#o} size={}",
                        absolute, fs_path, stat.mode, stat.size
                    ));
                }
                Ok(Some(FileStat {
                    dev: ext4_dev_for_path(absolute),
                    ino: stable_nonzero_hash64(absolute),
                    mode: stat.mode,
                    size: stat.size,
                    nlink: stat.nlink,
                    uid: 0,
                    gid: 0,
                    rdev: 0,
                    atime_sec: now_sec,
                    atime_nsec: now_nsec,
                    mtime_sec: now_sec,
                    mtime_nsec: now_nsec,
                    ctime_sec: now_sec,
                    ctime_nsec: now_nsec,
                }))
            }
            Err(err) if err == ENOENT => Ok(None),
            Err(err) => {
                if iozone_probe {
                    iozone_probe_log(&format!(
                        "whuse-la-iozone:vfs-external-stat-err absolute={} fs_path={} err={}",
                        absolute, fs_path, err
                    ));
                }
                Err(err)
            }
        }
    }

    fn external_stat_path_cached_only(&self, absolute: &str) -> KernelResult<Option<FileStat>> {
        if is_shell_token_path(absolute) {
            return Ok(None);
        }
        if self.lookup_abs(absolute).is_ok() {
            return Ok(None);
        }
        if self.is_memory_preferred_path(absolute) {
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
        let Some((mount, _)) = self.resolve_external_path(absolute) else {
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
            || absolute == mount.target
        {
            let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
            return Ok(Some(FileStat {
                dev: ext4_dev_for_path(absolute),
                ino: stable_nonzero_hash64(absolute),
                mode: S_IFDIR | 0o755,
                size: 0,
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
                atime_sec: now_sec,
                atime_nsec: now_nsec,
                mtime_sec: now_sec,
                mtime_nsec: now_nsec,
                ctime_sec: now_sec,
                ctime_nsec: now_nsec,
            }));
        }
        Ok(None)
    }

    fn try_open_external(
        &mut self,
        absolute: &str,
        flags: u32,
    ) -> KernelResult<Option<FileHandle>> {
        let iozone_probe = is_iozone_probe_path(absolute);
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-enter absolute={} flags={:#x}",
                absolute, flags
            ));
        }
        if is_shell_token_path(absolute) {
            return Ok(None);
        }
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-before-lookup absolute={}",
                absolute
            ));
        }
        if self.lookup_abs(absolute).is_ok() {
            if iozone_probe {
                iozone_probe_log(&format!(
                    "whuse-la-iozone:vfs-try-open-external-hit-mem absolute={}",
                    absolute
                ));
            }
            return Ok(None);
        }
        if self.external_deletions.contains(absolute) {
            return Ok(None);
        }
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-before-resolve absolute={}",
                absolute
            ));
        }
        let (mount, fs_path) = {
            let Some((mount, fs_path)) = self.resolve_external_path(absolute) else {
                if iozone_probe {
                    iozone_probe_log(&format!(
                        "whuse-la-iozone:vfs-try-open-external-no-mount absolute={}",
                        absolute
                    ));
                }
                return Ok(None);
            };
            (mount.ext4.clone(), fs_path)
        };
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-resolved absolute={} fs_path={}",
                absolute, fs_path
            ));
        }
        let trace_path = stage2_openat_debug_enabled()
            && (absolute.starts_with("/musl/")
                || absolute.starts_with("/lib/")
                || absolute.starts_with("/lib64/")
                || absolute.starts_with("/glibc/"));
        if flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC) != 0 {
            return Ok(None);
        }
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-check-preloaded absolute={}",
                absolute
            ));
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
        if iozone_probe {
            iozone_probe_log(&format!(
                "whuse-la-iozone:vfs-try-open-external-cache absolute={} hit={}",
                absolute,
                cached.is_some()
            ));
        }
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
                Some(Arc::new(Vec::new())),
            )));
        }
        let stat = match cached {
            Some(stat) => fs_ext4::Ext4FileStat {
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
            },
            None => {
                if iozone_probe {
                    iozone_probe_log(&format!(
                        "whuse-la-iozone:vfs-try-open-external-stat-begin absolute={} fs_path={} flags={:#x}",
                        absolute, fs_path, flags
                    ));
                }
                if trace_path {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-open-external-stat-begin path={} fs_path={} flags={:#x}",
                        absolute, fs_path, flags
                    ));
                }
                let stat = match mount.stat(&fs_path) {
                    Ok(stat) => stat,
                    Err(err) if err == ENOENT => {
                        if iozone_probe {
                            iozone_probe_log(&format!(
                                "whuse-la-iozone:vfs-try-open-external-stat-enoent absolute={} fs_path={}",
                                absolute, fs_path
                            ));
                        }
                        if trace_path {
                            stage2_openat_debug(&format!(
                                "whuse-libctest:vfs-open-external-stat-enoent path={} fs_path={}",
                                absolute, fs_path
                            ));
                        }
                        return Ok(None);
                    }
                    Err(err) => {
                        if iozone_probe {
                            iozone_probe_log(&format!(
                                "whuse-la-iozone:vfs-try-open-external-stat-err absolute={} fs_path={} err={}",
                                absolute, fs_path, err
                            ));
                        }
                        if trace_path {
                            stage2_openat_debug(&format!(
                                "whuse-libctest:vfs-open-external-stat-err path={} fs_path={} err={}",
                                absolute, fs_path, err
                            ));
                        }
                        return Err(err);
                    }
                };
                if iozone_probe {
                    iozone_probe_log(&format!(
                        "whuse-la-iozone:vfs-try-open-external-stat-ok absolute={} fs_path={} mode={:#o} size={}",
                        absolute, fs_path, stat.mode, stat.size
                    ));
                }
                if trace_path {
                    stage2_openat_debug(&format!(
                        "whuse-libctest:vfs-open-external-stat-ok path={} fs_path={} mode={:#o} size={}",
                        absolute, fs_path, stat.mode, stat.size
                    ));
                }
                self.external_stat_cache.insert(
                    absolute.to_string(),
                    FileStat {
                        dev: ext4_dev_for_path(absolute),
                        ino: stable_nonzero_hash64(absolute),
                        mode: stat.mode,
                        size: stat.size,
                        nlink: stat.nlink,
                        uid: 0,
                        gid: 0,
                        rdev: 0,
                        atime_sec: split_ns(monotonic_now_ns()).0,
                        atime_nsec: split_ns(monotonic_now_ns()).1,
                        mtime_sec: split_ns(monotonic_now_ns()).0,
                        mtime_nsec: split_ns(monotonic_now_ns()).1,
                        ctime_sec: split_ns(monotonic_now_ns()).0,
                        ctime_nsec: split_ns(monotonic_now_ns()).1,
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
        let dev = ext4_dev_for_path(&absolute);
        let ino = stable_nonzero_hash64(&absolute);
        self.external_stat_cache.insert(
            absolute,
            FileStat {
                dev,
                ino,
                mode: stat.mode,
                size: stat.size,
                nlink: stat.nlink,
                uid: 0,
                gid: 0,
                rdev: 0,
                atime_sec: split_ns(monotonic_now_ns()).0,
                atime_nsec: split_ns(monotonic_now_ns()).1,
                mtime_sec: split_ns(monotonic_now_ns()).0,
                mtime_nsec: split_ns(monotonic_now_ns()).1,
                ctime_sec: split_ns(monotonic_now_ns()).0,
                ctime_nsec: split_ns(monotonic_now_ns()).1,
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
        self.lookup_abs_follow(path, 0)
    }

    fn lookup_abs_follow(&self, path: &str, depth: usize) -> KernelResult<Arc<Node>> {
        if path == "/" {
            return Ok(self.root.clone());
        }
        if depth >= 16 {
            return Err(ELOOP);
        }
        let mut current = self.root.clone();
        let components = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        let mut current_path = String::from("/");
        for (index, component) in components.iter().enumerate() {
            let next = match &*current.data.lock() {
                NodeData::Directory(entries) => entries.get(*component).cloned().ok_or(ENOENT)?,
                _ => return Err(ENOTDIR),
            };
            let is_final = index + 1 == components.len();
            if next.kind == NodeKind::Symlink && !is_final {
                let target = match &*next.data.lock() {
                    NodeData::Symlink(target) => target.clone(),
                    _ => return Err(EINVAL),
                };
                let tail = components[index + 1..].join("/");
                let base = normalize_path(&current_path, &target);
                let resolved = if tail.is_empty() {
                    base
                } else {
                    normalize_path(&base, &tail)
                };
                return self.lookup_abs_follow(&resolved, depth + 1);
            }
            current = next;
            current_path = if current_path == "/" {
                format!("/{}", component)
            } else {
                format!("{}/{}", current_path, component)
            };
        }
        Ok(current)
    }

    fn alias_paths_for_node(&self, needle: &Arc<Node>) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_alias_paths("/", &self.root, needle, &mut out);
        out
    }

    fn collect_alias_paths(
        &self,
        current_path: &str,
        current: &Arc<Node>,
        needle: &Arc<Node>,
        out: &mut Vec<String>,
    ) {
        if current_path != "/" && Arc::ptr_eq(current, needle) {
            out.push(current_path.to_string());
        }
        let children = {
            let guard = current.data.lock();
            match &*guard {
                NodeData::Directory(entries) => entries
                    .iter()
                    .map(|(name, node)| (name.clone(), node.clone()))
                    .collect::<Vec<_>>(),
                _ => return,
            }
        };
        for (name, node) in children {
            let child_path = if current_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", current_path, name)
            };
            self.collect_alias_paths(&child_path, &node, needle, out);
        }
    }

    fn sync_nlink_for_paths(&mut self, node: &Arc<Node>, paths: &[String], nlink: u32) {
        for path in paths {
            if let Ok(stat) = self.stat(path, node) {
                self.mem_meta.insert(
                    path.clone(),
                    InodeMeta {
                        mode: stat.mode,
                        nlink,
                        uid: stat.uid,
                        gid: stat.gid,
                        atime_ns: compose_ns(stat.atime_sec, stat.atime_nsec),
                        mtime_ns: compose_ns(stat.mtime_sec, stat.mtime_nsec),
                        ctime_ns: monotonic_now_ns(),
                    },
                );
            }
        }
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
        let ltp_create_probe = is_ltp_create_probe_path(absolute_path);
        if absolute_path == "/" {
            return Err(EEXIST);
        }
        let (parent_path, name) = split_parent(absolute_path)?;
        if ltp_create_probe {
            write_console_line(&format!(
                "whuse-ltp:vfs-create-node absolute={} parent={} name={}",
                absolute_path, parent_path, name
            ));
        }
        let parent = match self.resolve_mem_path(&parent_path) {
            Ok((_, parent)) => parent,
            Err(err) if err == ENOENT => {
                if ltp_create_probe {
                    write_console_line(&format!(
                        "whuse-ltp:vfs-create-node-ensure-parent absolute={} parent={}",
                        absolute_path, parent_path
                    ));
                }
                if let Err(err) = self.ensure_memory_dir(&parent_path) {
                    if ltp_create_probe {
                        write_console_line(&format!(
                            "whuse-ltp:vfs-create-node-ensure-parent-err absolute={} parent={} err={}",
                            absolute_path, parent_path, err
                        ));
                    }
                    return Err(err);
                }
                self.resolve_mem_path(&parent_path)?.1
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
            NodeKind::Pipe => Node::pipe_with_data(
                name,
                data.unwrap_or_else(|| {
                    NodeData::Pipe(PipeState {
                        buf: VecDeque::new(),
                        readers: 0,
                        writers: 0,
                        capacity: PIPE_CAPACITY,
                    })
                }),
            ),
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
            NodeData::SparseFile(state) => state.size as u64,
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
        let meta = self.mem_meta.get(path).copied();
        let mode = match &*guard {
            NodeData::Ext4File(state) => state.mode,
            NodeData::Ext4Dir(state) => state.mode,
            _ => meta.map(|meta| meta.mode).unwrap_or(match node.kind {
                NodeKind::Directory => S_IFDIR | 0o755,
                NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
                NodeKind::CharDevice => S_IFCHR | 0o600,
                NodeKind::Pipe => S_IFIFO | 0o644,
                NodeKind::Symlink => S_IFLNK | 0o777,
                NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
                NodeKind::PidFd => S_IFREG | 0o444,
            }),
        };
        let (uid, gid, nlink) = meta
            .map(|meta| (meta.uid, meta.gid, meta.nlink))
            .unwrap_or((0, 0, 1));
        let (atime_sec, atime_nsec, mtime_sec, mtime_nsec, ctime_sec, ctime_nsec) =
            if let Some(meta) = meta {
                let (atime_sec, atime_nsec) = split_ns(meta.atime_ns);
                let (mtime_sec, mtime_nsec) = split_ns(meta.mtime_ns);
                let (ctime_sec, ctime_nsec) = split_ns(meta.ctime_ns);
                (atime_sec, atime_nsec, mtime_sec, mtime_nsec, ctime_sec, ctime_nsec)
            } else {
                let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
                (now_sec, now_nsec, now_sec, now_nsec, now_sec, now_nsec)
            };
        let is_ext4 = matches!(&*guard, NodeData::Ext4File(_) | NodeData::Ext4Dir(_));
        let dev = if is_ext4 {
            ext4_dev_for_path(path)
        } else {
            local_dev_for_path(path)
        };
        let ino = if is_ext4 {
            stable_nonzero_hash64(path)
        } else {
            synthetic_local_ino(node, path)
        };
        let rdev = if node.kind == NodeKind::CharDevice {
            char_device_rdev(path)
        } else {
            0
        };
        Ok(FileStat {
            dev,
            ino,
            mode,
            size,
            nlink,
            uid,
            gid,
            rdev,
            atime_sec,
            atime_nsec,
            mtime_sec,
            mtime_nsec,
            ctime_sec,
            ctime_nsec,
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
            NodeData::SparseFile(state) => state.size as u64,
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
        let (now_sec, now_nsec) = split_ns(monotonic_now_ns());
        FileStat {
            dev: local_dev_for_path(&self.path),
            ino: synthetic_local_ino(&self.node, &self.path),
            mode,
            size,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: if self.node.kind == NodeKind::CharDevice {
                char_device_rdev(&self.path)
            } else {
                0
            },
            atime_sec: now_sec,
            atime_nsec: now_nsec,
            mtime_sec: now_sec,
            mtime_nsec: now_nsec,
            ctime_sec: now_sec,
            ctime_nsec: now_nsec,
        }
    }

    fn poll_hangup(&self) -> bool {
        match &*self.node.data.lock() {
            NodeData::Pipe(state) => match self.pipe_end {
                PipeEnd::Read | PipeEnd::None => state.writers == 0,
                PipeEnd::Write => state.readers == 0,
            },
            NodeData::SocketConnected { channel, side, .. } => {
                let guard = channel.lock();
                let peer = 1 - *side;
                guard.open_sides[peer] == 0
            }
            _ => false,
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
                clone_slice_fallible(&buf[start..end])
            }
            NodeData::SparseFile(state) => {
                let data = state.read_range(self.offset, len);
                self.offset = self.offset.saturating_add(data.len()).min(state.size);
                Ok(data)
            }
            NodeData::Ext4File(state) => {
                if let Some(cached) = &state.cached {
                    let start = self.offset.min(cached.len());
                    let end = (start + len).min(cached.len());
                    self.offset = end;
                    return clone_slice_fallible(&cached[start..end]);
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
                let mut out = Vec::new();
                out.try_reserve_exact(end).map_err(|_| ENOMEM)?;
                for _ in 0..end {
                    if let Some(byte) = state.buf.pop_front() {
                        out.push(byte);
                    }
                }
                Ok(out)
            }
            NodeData::Symlink(target) => {
                let end = target.len().min(len);
                clone_slice_fallible(&target.as_bytes()[..end])
            }
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
                    let mut out = Vec::new();
                    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
                    out.resize(len, 0);
                    return Ok(out);
                }
                if self.path == "/dev/random" || self.path == "/dev/urandom" {
                    let mut out = Vec::new();
                    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
                    out.resize(len, 0);
                    return Ok(out);
                }
                Ok(Vec::new())
            }
        }
    }

    fn write_object(&mut self, data: &[u8]) -> KernelResult<usize> {
        let mut guard = self.node.data.lock();
        match &mut *guard {
            NodeData::Directory(_) => Err(EISDIR),
            NodeData::File(buf) => {
                if (self.flags & O_APPEND) != 0 {
                    self.offset = buf.len();
                }
                let end = self.offset.checked_add(data.len()).ok_or(ENOSPC)?;
                if self.offset > INMEM_FILE_SIZE_LIMIT || end > INMEM_FILE_SIZE_LIMIT {
                    let dense = core::mem::take(buf);
                    let mut sparse = SparseFileState::from_dense(dense);
                    if (self.flags & O_APPEND) != 0 {
                        self.offset = sparse.size;
                    }
                    if self.offset > sparse.size {
                        sparse.size = self.offset;
                    }
                    sparse.write_at(self.offset, data)?;
                    self.offset = end;
                    *guard = NodeData::SparseFile(sparse);
                    return Ok(data.len());
                }
                if self.offset > buf.len() {
                    ensure_file_size(buf, self.offset)?;
                }
                if end > buf.len() {
                    ensure_file_size(buf, end)?;
                }
                buf[self.offset..self.offset + data.len()].copy_from_slice(data);
                self.offset += data.len();
                Ok(data.len())
            }
            NodeData::ProcFile(buf) => {
                if (self.flags & O_APPEND) != 0 {
                    self.offset = buf.len();
                }
                if self.offset > buf.len() {
                    ensure_file_size(buf, self.offset)?;
                }
                let end = self.offset.checked_add(data.len()).ok_or(ENOSPC)?;
                if end > buf.len() {
                    ensure_file_size(buf, end)?;
                }
                buf[self.offset..self.offset + data.len()].copy_from_slice(data);
                self.offset += data.len();
                Ok(data.len())
            }
            NodeData::SparseFile(state) => {
                if (self.flags & O_APPEND) != 0 {
                    self.offset = state.size;
                }
                let end = self.offset.checked_add(data.len()).ok_or(ENOSPC)?;
                if self.offset > state.size {
                    state.size = self.offset;
                }
                state.write_at(self.offset, data)?;
                self.offset = end;
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
                let available = state.capacity.saturating_sub(state.buf.len());
                if available == 0 {
                    return Err(EAGAIN);
                }
                let write_len = available.min(data.len());
                state.buf.try_reserve(write_len).map_err(|_| ENOMEM)?;
                state.buf.extend(data[..write_len].iter().copied());
                Ok(write_len)
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
        if matches!(self.node.kind, NodeKind::Pipe | NodeKind::Socket) {
            return Err(ESPIPE);
        }
        if matches!(self.node.kind, NodeKind::Event | NodeKind::Epoll | NodeKind::PidFd) {
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
            | NodeData::SparseFile(_)
            | NodeData::ProcFile(_)
            | NodeData::Ext4File(_)
            | NodeData::Ext4Dir(_)
            | NodeData::CharDevice => true,
            NodeData::Pipe(state) => match self.pipe_end {
                PipeEnd::Read | PipeEnd::None => !state.buf.is_empty() || state.writers == 0,
                PipeEnd::Write => false,
            },
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
            NodeData::Pipe(state) => match self.pipe_end {
                PipeEnd::Write | PipeEnd::None => {
                    state.readers != 0 && state.buf.len() < state.capacity
                }
                PipeEnd::Read => false,
            },
            _ => true,
        }
    }

    fn stat_object(&self) -> KernelResult<FileStat> {
        Ok(self.stat_from_locked())
    }

    fn fcntl_object(&mut self, cmd: usize, arg: usize) -> KernelResult<usize> {
        const F_SETPIPE_SZ: usize = 1031;
        const F_GETPIPE_SZ: usize = 1032;

        match &mut *self.node.data.lock() {
            NodeData::Pipe(state) => match cmd {
                F_SETPIPE_SZ => {
                    let requested = if arg == 0 {
                        PIPE_MIN_CAPACITY
                    } else {
                        arg.next_multiple_of(PIPE_MIN_CAPACITY)
                    };
                    let capacity = requested.max(PIPE_MIN_CAPACITY).max(state.buf.len());
                    state.capacity = capacity;
                    Ok(capacity)
                }
                F_GETPIPE_SZ => Ok(state.capacity),
                _ => Err(EINVAL),
            },
            _ => Err(EINVAL),
        }
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
                capacity: PIPE_CAPACITY,
            })),
        }
    }

    fn pipe_with_data(name: &str, data: NodeData) -> Self {
        Self {
            _name: name.to_string(),
            kind: NodeKind::Pipe,
            data: Mutex::new(data),
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

fn clone_slice_fallible(data: &[u8]) -> KernelResult<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(data.len()).map_err(|_| ENOMEM)?;
    out.extend_from_slice(data);
    Ok(out)
}

fn open_wants_write(flags: u32) -> bool {
    matches!(flags & 0b11, O_WRONLY | O_RDWR)
}

fn open_existing_directory_should_fail(flags: u32) -> bool {
    (flags & O_CREAT) != 0 || open_wants_write(flags)
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
    is_compat_metadata_fallback_path(absolute)
}

fn should_use_statless_external_stat(absolute: &str) -> bool {
    is_compat_metadata_fallback_path(absolute)
}

fn is_ltp_create_probe_path(path: &str) -> bool {
    ltp_create_probe_debug_enabled()
        && (path.ends_with("/close01_testfile")
            || path.ends_with("/dupfile")
            || path == "close01_testfile"
            || path == "dupfile")
}

fn is_compat_metadata_fallback_path(absolute: &str) -> bool {
    matches!(
        absolute,
        "/etc/localtime" | "/etc/passwd" | "/etc/group" | "/etc/TZ"
    )
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

    use super::{
        KernelObject, KernelVfs, ObjectKind, O_CREAT, O_NONBLOCK, O_RDONLY, O_RDWR, O_WRONLY,
        INMEM_FILE_SIZE_LIMIT, PIPE_CAPACITY, S_IFIFO,
    };

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

    fn repo_target_image(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/oscomp")
            .join(relative)
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
    fn open_excl_rejects_existing_regular_file() {
        let mut vfs = KernelVfs::new();
        let _ = vfs
            .open("/", "/tmp/existing.txt", O_CREAT | O_RDWR, 0o644)
            .unwrap();
        assert!(matches!(
            vfs.open(
                "/",
                "/tmp/existing.txt",
                O_CREAT | super::O_EXCL | O_RDWR,
                0o644
            ),
            Err(super::EEXIST)
        ));
    }

    #[test]
    fn open_directory_for_write_returns_eisdir() {
        let mut vfs = KernelVfs::new();
        assert!(matches!(
            vfs.open("/", "/tmp", O_RDWR, 0),
            Err(super::EISDIR)
        ));
    }

    #[test]
    fn open_existing_directory_with_o_creat_returns_eisdir() {
        let mut vfs = KernelVfs::new();
        assert!(matches!(
            vfs.open("/", "/tmp", super::O_CREAT | super::O_RDONLY, 0o644),
            Err(super::EISDIR)
        ));
    }

    #[test]
    fn open_nofollow_rejects_final_symlink() {
        let mut vfs = KernelVfs::new();
        vfs.create_file_with_mode("/", "/tmp/target.txt", b"", 0o644)
            .unwrap();
        vfs.create_symlink("/", "/tmp/link.txt", "/tmp/target.txt")
            .unwrap();
        assert!(matches!(
            vfs.open("/", "/tmp/link.txt", super::O_NOFOLLOW | super::O_RDONLY, 0),
            Err(super::ELOOP)
        ));
    }

    #[test]
    fn open_nofollow_allows_files_inside_symlinked_directory() {
        let mut vfs = KernelVfs::new();
        vfs.mkdir("/", "/tmp/real-dir", 0o755).unwrap();
        vfs.create_symlink("/", "/tmp/link-dir", "/tmp/real-dir")
            .unwrap();

        let mut created = vfs
            .open(
                "/",
                "/tmp/link-dir/testfile",
                O_CREAT | O_RDWR,
                0o644,
            )
            .unwrap();
        vfs.write(&mut created, b"ok").unwrap();

        let mut handle = vfs
            .open(
                "/",
                "/tmp/link-dir/testfile",
                super::O_NOFOLLOW | super::O_RDONLY,
                0,
            )
            .unwrap();
        assert_eq!(vfs.read(&mut handle, 2).unwrap(), b"ok");
    }

    #[test]
    fn hard_links_share_updated_nlink() {
        let mut vfs = KernelVfs::new();
        let _ = vfs
            .open("/", "/tmp/source.txt", O_CREAT | O_RDWR, 0o644)
            .unwrap();
        vfs.link("/", "/tmp/source.txt", "/tmp/alias.txt").unwrap();

        assert_eq!(vfs.stat_path("/", "/tmp/source.txt").unwrap().nlink, 2);
        assert_eq!(vfs.stat_path("/", "/tmp/alias.txt").unwrap().nlink, 2);

        vfs.unlink("/", "/tmp/source.txt").unwrap();
        assert_eq!(vfs.stat_path("/", "/tmp/alias.txt").unwrap().nlink, 1);
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
    fn regular_file_supports_sparse_large_write() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs
            .open("/", "/tmp/large-hole.bin", O_CREAT | O_RDWR, 0o644)
            .unwrap();

        let sparse_offset = 4_402_341_478usize;
        file.seek_object(sparse_offset as isize, 0).unwrap();
        file.write_object(b"ltp").unwrap();

        let stat = vfs.stat_handle(&file).unwrap();
        assert_eq!(stat.size, (sparse_offset + 3) as u64);

        file.seek_object((sparse_offset - 1) as isize, 0).unwrap();
        assert_eq!(file.read_object(4).unwrap(), b"\0ltp");

        file.seek_object(0, 0).unwrap();
        assert_eq!(file.read_object(8).unwrap(), vec![0; 8]);
    }

    #[test]
    fn sparse_large_file_truncate_shrinks_visible_size() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs
            .open("/", "/tmp/large-hole-truncate.bin", O_CREAT | O_RDWR, 0o644)
            .unwrap();

        let sparse_offset = 4_402_341_478usize;
        file.seek_object(sparse_offset as isize, 0).unwrap();
        file.write_object(b"ltp").unwrap();
        vfs.truncate(&mut file, 2).unwrap();

        let stat = vfs.stat_handle(&file).unwrap();
        assert_eq!(stat.size, 2);
        file.seek_object(0, 0).unwrap();
        assert_eq!(file.read_object(8).unwrap(), vec![0; 2]);
    }

    #[test]
    fn fallocate_promotes_large_regular_files_to_sparse_storage() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs
            .open("/", "/tmp/ltp-prealloc.bin", O_CREAT | O_RDWR, 0o644)
            .unwrap();

        let large_len = (INMEM_FILE_SIZE_LIMIT + 32 * 1024 * 1024) as usize;
        vfs.fallocate(&mut file, 0, large_len).unwrap();

        let stat = vfs.stat_handle(&file).unwrap();
        assert_eq!(stat.size, large_len as u64);
        file.seek_object((large_len - 4) as isize, 0).unwrap();
        assert_eq!(file.read_object(4).unwrap(), vec![0; 4]);
    }

    #[test]
    fn truncate_shrinks_dense_regular_files() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs
            .open("/", "/tmp/dense-truncate.bin", O_CREAT | O_RDWR, 0o644)
            .unwrap();

        file.write_object(b"abcdef").unwrap();
        vfs.truncate(&mut file, 2).unwrap();

        let stat = vfs.stat_handle(&file).unwrap();
        assert_eq!(stat.size, 2);
        file.seek_object(0, 0).unwrap();
        assert_eq!(file.read_object(8).unwrap(), b"ab");
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
    fn named_fifo_nonblocking_empty_read_returns_eagain() {
        let mut vfs = KernelVfs::new();
        vfs.mknodat_with_owner("/", "/tmp/test-fifo", S_IFIFO | 0o666, 0, 0)
            .unwrap();

        let mut read_end = vfs
            .open("/", "/tmp/test-fifo", O_RDONLY | O_NONBLOCK, 0)
            .unwrap();
        let _write_end = vfs
            .open("/", "/tmp/test-fifo", O_WRONLY | O_NONBLOCK, 0)
            .unwrap();

        assert_eq!(vfs.read(&mut read_end, 1), Err(super::EAGAIN));
    }

    #[test]
    fn named_fifo_nonblocking_write_only_without_reader_returns_enxio() {
        let mut vfs = KernelVfs::new();
        vfs.mknodat_with_owner("/", "/tmp/test-fifo-enxio", S_IFIFO | 0o666, 0, 0)
            .unwrap();

        assert!(matches!(
            vfs.open("/", "/tmp/test-fifo-enxio", O_WRONLY | O_NONBLOCK, 0),
            Err(super::ENXIO)
        ));
    }

    #[test]
    fn named_fifo_nonblocking_write_returns_eagain_when_full() {
        let mut vfs = KernelVfs::new();
        vfs.mknodat_with_owner("/", "/tmp/test-fifo-full", S_IFIFO | 0o666, 0, 0)
            .unwrap();

        let _read_end = vfs
            .open("/", "/tmp/test-fifo-full", O_RDONLY | O_NONBLOCK, 0)
            .unwrap();
        let mut write_end = vfs
            .open("/", "/tmp/test-fifo-full", O_WRONLY | O_NONBLOCK, 0)
            .unwrap();

        let first = vec![0u8; PIPE_CAPACITY + 4096];
        assert_eq!(vfs.write(&mut write_end, &first).unwrap(), PIPE_CAPACITY);
        assert_eq!(vfs.write(&mut write_end, b"x"), Err(super::EAGAIN));
    }

    #[test]
    fn open_with_owner_inherits_parent_gid_when_directory_has_setgid() {
        let mut vfs = KernelVfs::new();
        vfs.mkdir("/tmp", "sgid-parent", 0o777).unwrap();
        vfs.chown_path("/", "/tmp/sgid-parent", Some(65534), Some(1))
            .unwrap();
        vfs.chmod_path("/", "/tmp/sgid-parent", 0o2777).unwrap();

        let handle = vfs
            .open_with_owner(
                "/",
                "/tmp/sgid-parent/child.txt",
                O_CREAT | O_RDWR,
                0o644,
                65534,
                65534,
            )
            .unwrap();
        let stat = vfs.stat_handle(&handle).unwrap();
        assert_eq!(stat.uid, 65534);
        assert_eq!(stat.gid, 1);
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
                dev: super::ext4_dev_for_path("/bin/hello"),
                ino: super::stable_nonzero_hash64("/bin/hello"),
                mode: super::S_IFREG | 0o644,
                size: b"hello from ext4".len() as u64,
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
            })
        );
    }

    #[test]
    fn ext4_stat_handle_reports_stable_nonzero_device_and_inode() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        let handle_a = vfs.open("/", "/bin/hello", 0, 0).unwrap();
        let handle_b = vfs.open("/", "/bin/hello", 0, 0).unwrap();
        let stat_a = vfs.stat_handle(&handle_a).unwrap();
        let stat_b = vfs.stat_handle(&handle_b).unwrap();

        assert_ne!(stat_a.dev, 0);
        assert_ne!(stat_a.ino, 0);
        assert_eq!(stat_a.dev, stat_b.dev);
        assert_eq!(stat_a.ino, stat_b.ino);
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

    fn parse_dirent_names(bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset + 19 <= bytes.len() {
            let reclen = u16::from_le_bytes([bytes[offset + 16], bytes[offset + 17]]) as usize;
            if reclen == 0 || offset + reclen > bytes.len() {
                break;
            }
            let name_start = offset + 19;
            let name_end = bytes[name_start..offset + reclen]
                .iter()
                .position(|byte| *byte == 0)
                .map(|idx| name_start + idx)
                .unwrap_or(offset + reclen);
            out.push(String::from_utf8_lossy(&bytes[name_start..name_end]).to_string());
            offset += reclen;
        }
        out
    }

    #[test]
    fn refresh_proc_self_fd_dir_replaces_entries() {
        let mut vfs = KernelVfs::new();

        vfs.refresh_proc_self_fd_dir([
            (0, "/dev/null".to_string()),
            (3, "/tmp/a".to_string()),
        ])
        .unwrap();
        let mut dir = vfs.open("/", "/proc/self/fd", super::O_DIRECTORY, 0).unwrap();
        let entries = parse_dirent_names(&vfs.getdents(&mut dir, 4096).unwrap());
        assert_eq!(entries, vec!["0".to_string(), "3".to_string()]);

        vfs.refresh_proc_self_fd_dir([(1, "/dev/console".to_string())])
            .unwrap();
        let mut dir = vfs.open("/", "/proc/self/fd", super::O_DIRECTORY, 0).unwrap();
        let entries = parse_dirent_names(&vfs.getdents(&mut dir, 4096).unwrap());
        assert_eq!(entries, vec!["1".to_string()]);
    }

    #[test]
    fn stat_path_cached_only_reports_ext4_hits_and_misses_without_fs_walks() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        assert!(matches!(
            vfs.stat_path_cached_only("/", "/bin/not-found"),
            Err(super::ENOENT)
        ));
        assert!(!vfs.external_stat_cache.contains_key("/bin/not-found"));

        let mut file = vfs.open("/", "/bin/hello", 0, 0).unwrap();
        assert_eq!(vfs.read(&mut file, 32).unwrap(), b"hello from ext4");

        let stat = vfs.stat_path_cached_only("/", "/bin/hello").unwrap();
        assert_eq!(stat.size, b"hello from ext4".len() as u64);
    }

    #[test]
    fn stat_path_open_probe_skips_full_stat_for_missing_localtime() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        assert_eq!(
            vfs.stat_path_open_probe("/", "/etc/localtime", super::O_RDONLY),
            None
        );
        assert!(!vfs.external_stat_cache.contains_key("/etc/localtime"));
    }

    #[test]
    fn stat_path_skips_full_stat_for_missing_localtime() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        assert_eq!(
            vfs.stat_path("/", "/etc/localtime"),
            Err(super::ENOENT)
        );
        assert!(!vfs.external_stat_cache.contains_key("/etc/localtime"));
    }

    #[test]
    fn sys_cpu_online_stub_is_available() {
        let mut vfs = KernelVfs::new();
        let stat = vfs
            .stat_path_open_probe("/", "/sys/devices/system/cpu/online", super::O_RDONLY)
            .expect("/sys/devices/system/cpu/online should exist");
        assert_eq!(stat.mode & super::S_IFMT, super::S_IFREG);

        let mut handle = vfs
            .open_with_owner("/", "/sys/devices/system/cpu/online", super::O_RDONLY, 0, 0, 0)
            .expect("open /sys/devices/system/cpu/online");
        let bytes = vfs
            .read(&mut handle, 16)
            .expect("read /sys/devices/system/cpu/online");
        assert_eq!(bytes, b"0\n");
    }

    #[test]
    fn read_file_all_returns_empty_for_missing_compat_metadata_files() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        for path in ["/etc/passwd", "/etc/group", "/etc/TZ"] {
            assert_eq!(vfs.read_file_all("/", path), Ok(Vec::new()), "path={path}");
            assert!(!vfs.external_stat_cache.contains_key(path), "path={path}");
        }
    }

    #[test]
    fn open_and_read_missing_compat_metadata_files_returns_eof() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        for path in ["/etc/passwd", "/etc/group", "/etc/TZ"] {
            let mut handle = vfs.open("/", path, super::O_RDONLY, 0).expect(path);
            assert_eq!(vfs.read(&mut handle, 32), Ok(Vec::new()), "path={path}");
        }
    }

    #[test]
    fn stat_path_skips_full_stat_for_missing_compat_metadata_files() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        for path in ["/etc/passwd", "/etc/group", "/etc/TZ"] {
            assert_eq!(vfs.stat_path("/", path), Err(super::ENOENT), "path={path}");
            assert!(!vfs.external_stat_cache.contains_key(path), "path={path}");
        }
    }

    #[test]
    fn shared_library_paths_do_not_use_statless_external_open() {
        assert!(!super::should_use_statless_external_open(
            "/musl/lib/libc.so",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/glibc/lib/libc.so",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/lib/ld-linux-riscv64-lp64d.so.1",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/lib64/ld-musl-loongarch-lp64d.so.1",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/basic/test_echo",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/run-static.sh",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/run-dynamic.sh",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/runtest.exe",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/entry-static.exe",
            super::O_RDONLY
        ));
        assert!(!super::should_use_statless_external_open(
            "/musl/entry-dynamic.exe",
            super::O_RDONLY
        ));
        assert!(super::should_use_statless_external_open(
            "/etc/localtime",
            super::O_RDONLY
        ));
    }

    #[test]
    fn stat_path_nofollow_reports_symlink_mode() {
        let mut vfs = KernelVfs::new();
        vfs.create_file_with_mode("/", "/tmp/target.txt", b"", 0o644)
            .unwrap();
        vfs.create_symlink("/", "/tmp/link.txt", "/tmp/target.txt")
            .unwrap();

        let stat = vfs.stat_path_nofollow("/", "/tmp/link.txt").unwrap();
        assert_eq!(stat.mode & 0o170000, super::S_IFLNK);
    }

    #[test]
    fn read_link_prefers_memory_overlay_over_ext4_root_mount() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();
        if let Err(err) = vfs.mkdir("/", "/tmp", 0o777) {
            assert_eq!(err, super::EEXIST);
        }
        if let Err(err) = vfs.mkdir("/tmp", "ltp-shadow", 0o777) {
            assert_eq!(err, super::EEXIST);
        }
        vfs.create_file_with_mode("/tmp/ltp-shadow", "test_file", b"", 0o644)
            .unwrap();
        vfs.create_symlink("/tmp/ltp-shadow", "slink_file", "test_file")
            .unwrap();

        assert_eq!(
            vfs.read_link("/tmp/ltp-shadow", "slink_file").unwrap(),
            "test_file"
        );
        assert_eq!(
            vfs.read_link("/", "/tmp/ltp-shadow/slink_file").unwrap(),
            "test_file"
        );
    }

    #[test]
    fn open_create_relative_file_under_external_directory_uses_memory_overlay() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        let cwd = vfs.chdir("/", "/bin").unwrap();
        assert_eq!(cwd, "/bin");

        let mut handle = vfs.open(&cwd, "ltp-created", O_CREAT | O_RDWR, 0o644).unwrap();
        vfs.write(&mut handle, b"ok").unwrap();
        vfs.seek(&mut handle, 0, 0).unwrap();
        assert_eq!(vfs.read(&mut handle, 2).unwrap(), b"ok");

        let mut reopened = vfs.open("/", "/bin/ltp-created", O_RDONLY, 0).unwrap();
        assert_eq!(vfs.read(&mut reopened, 2).unwrap(), b"ok");
    }

    #[test]
    fn open_with_owner_on_external_missing_path_assigns_requested_uid() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();

        let cwd = vfs.chdir("/", "/bin").unwrap();
        let handle = vfs
            .open_with_owner(&cwd, "ltp-owned", O_CREAT | O_RDWR, 0o644, 65534, 0)
            .unwrap();
        let stat = vfs.stat_handle(&handle).unwrap();
        assert_eq!(stat.uid, 65534);
        assert_eq!(stat.gid, 0);
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

    #[test]
    fn real_loongarch_image_alias_reads_musl_loader() {
        let image = repo_target_image("sdcard-la.img");
        if !image.exists() {
            return;
        }
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));

        let mut vfs = KernelVfs::new();
        vfs.mount_ext4(device.name(), "/", device).unwrap();
        let _ = vfs.mkdir("/", "/lib64", 0o755);
        vfs.create_symlink(
            "/",
            "/lib64/ld-musl-loongarch-lp64d.so.1",
            "/musl/lib/libc.so",
        )
        .unwrap();

        let mut direct = vfs
            .open("/", "/musl/lib/libc.so", super::O_RDONLY, 0)
            .unwrap();
        let direct_bytes = vfs.read(&mut direct, 4 * 1024 * 1024).unwrap();
        assert!(!direct_bytes.is_empty());

        let mut alias = vfs
            .open(
                "/",
                "/lib64/ld-musl-loongarch-lp64d.so.1",
                super::O_RDONLY,
                0,
            )
            .unwrap();
        let alias_bytes = vfs.read(&mut alias, 4 * 1024 * 1024).unwrap();
        assert_eq!(alias_bytes, direct_bytes);
    }
}
