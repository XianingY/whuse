#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub type KernelResult<T> = Result<T, i32>;

pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;

const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ENOTEMPTY: i32 = 39;

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFIFO: u32 = 0o010000;
const S_IFLNK: u32 = 0o120000;
const S_IFSOCK: u32 = 0o140000;

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

#[derive(Clone)]
pub struct FileHandle {
    node: Arc<Node>,
    pub offset: usize,
    pub flags: u32,
    pub path: String,
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

enum NodeData {
    Directory(BTreeMap<String, Arc<Node>>),
    File(Vec<u8>),
    CharDevice,
    ProcFile(Vec<u8>),
    Pipe(Vec<u8>),
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

pub struct KernelVfs {
    root: Arc<Node>,
    mounts: Vec<MountRecord>,
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
            next_pipe_id: 0,
            next_memfd_id: 0,
            socket_bindings: BTreeMap::new(),
        };
        for dir in ["/dev", "/proc", "/mnt", "/tmp", "/bin", "/etc"] {
            let _ = vfs.mkdir("/", dir, 0o755);
        }
        let _ = vfs.create_char_device("/dev/console", "console");
        let _ = vfs.create_proc_file("/proc/mounts", b"");
        vfs
    }

    pub fn create_char_device(&mut self, path: &str, name: &'static str) -> KernelResult<()> {
        let _ = name;
        self.create_node(path, NodeKind::CharDevice, Some(NodeData::CharDevice))
    }

    pub fn create_proc_file(&mut self, path: &str, contents: &[u8]) -> KernelResult<()> {
        self.create_node(path, NodeKind::Proc, Some(NodeData::ProcFile(contents.to_vec())))
    }

    pub fn create_file(&mut self, cwd: &str, path: &str, contents: &[u8]) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(&absolute, NodeKind::File, Some(NodeData::File(contents.to_vec())))
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

    pub fn open(&mut self, cwd: &str, path: &str, flags: u32, _mode: u32) -> KernelResult<FileHandle> {
        let mut absolute = normalize_path(cwd, path);
        let node = match self.lookup_abs(&absolute) {
            Ok(node) => node,
            Err(err) if err == ENOENT && (flags & O_CREAT) != 0 => {
                self.create_file("/", &absolute, b"")?;
                self.lookup_abs(&absolute)?
            }
            Err(err) => return Err(err),
        };
        let node = if node.kind == NodeKind::Symlink {
            let target = match &*node.data.lock() {
                NodeData::Symlink(target) => target.clone(),
                _ => return Err(EINVAL),
            };
            let parent = split_parent(&absolute)?.0;
            absolute = normalize_path(&parent, &target);
            self.lookup_abs(&absolute)?
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
            path: absolute,
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

    pub fn getdents(&self, handle: &mut FileHandle) -> KernelResult<Vec<u8>> {
        let guard = handle.node.data.lock();
        let entries = match &*guard {
            NodeData::Directory(entries) => entries,
            _ => return Err(ENOTDIR),
        };

        let mut out = Vec::new();
        for (index, (name, node)) in entries.iter().enumerate() {
            let file_type = match node.kind {
                NodeKind::Directory => 4u8,
                _ => 8u8,
            };
            let reclen = align_up(19 + name.len() + 1, 8) as u16;
            out.extend_from_slice(&(index as u64 + 1).to_le_bytes());
            out.extend_from_slice(&((index + 1) as u64).to_le_bytes());
            out.extend_from_slice(&reclen.to_le_bytes());
            out.push(file_type);
            out.push(0);
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            while out.len() % 8 != 0 {
                out.push(0);
            }
        }
        handle.offset = out.len();
        Ok(out)
    }

    pub fn stat_path(&self, cwd: &str, path: &str) -> KernelResult<FileStat> {
        let absolute = normalize_path(cwd, path);
        let node = self.lookup_abs(&absolute)?;
        self.stat(&node)
    }

    pub fn chdir(&self, cwd: &str, path: &str) -> KernelResult<String> {
        let absolute = normalize_path(cwd, path);
        let node = self.lookup_abs(&absolute)?;
        if node.kind != NodeKind::Directory {
            return Err(ENOTDIR);
        }
        Ok(absolute)
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

    pub fn umount(&mut self, target: &str) -> KernelResult<()> {
        let absolute = normalize_path("/", target);
        let before = self.mounts.len();
        self.mounts.retain(|mount| mount.target != absolute);
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
        let _ = self.lookup_abs(&absolute)?;
        Ok(())
    }

    pub fn read_file_all(&mut self, cwd: &str, path: &str) -> KernelResult<Vec<u8>> {
        let mut handle = self.open(cwd, path, O_RDONLY, 0)?;
        let size = self.stat_handle(&handle)?.size as usize;
        self.read(&mut handle, size)
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
        };
        self.truncate(&mut handle, len)
    }

    pub fn fallocate(&mut self, handle: &mut FileHandle, offset: usize, len: usize) -> KernelResult<()> {
        match &mut *handle.node.data.lock() {
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                let size = offset.saturating_add(len);
                if buf.len() < size {
                    buf.resize(size, 0);
                }
                Ok(())
            }
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
        };
        let write_end = FileHandle {
            node,
            offset: 0,
            flags: O_WRONLY,
            path,
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
                let watch = watches.iter_mut().find(|watch| watch.fd == fd).ok_or(ENOENT)?;
                watch.events = events;
            }
            3 => {
                let index = watches.iter().position(|watch| watch.fd == fd).ok_or(ENOENT)?;
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
        })
    }

    pub fn create_pidfd(&mut self, pid: usize) -> KernelResult<FileHandle> {
        let path = format!("pidfd:[{}]", pid);
        Ok(FileHandle {
            node: Arc::new(Node::pidfd(&path, pid)),
            offset: 0,
            flags: O_RDONLY,
            path,
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
            },
            FileHandle {
                node: Arc::new(Node::socket_connected(&path, channel, 1)),
                offset: 0,
                flags: O_RDWR,
                path: format!("{}:1", path),
            },
        ))
    }

    pub fn bind_socket(&mut self, handle: &mut FileHandle, cwd: &str, path: &str) -> KernelResult<()> {
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

    pub fn connect_socket(&mut self, handle: &mut FileHandle, cwd: &str, path: &str) -> KernelResult<()> {
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
            state.pending.push(Arc::new(Node::socket_connected(&absolute, channel.clone(), 1)));
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

    fn refresh_mounts_proc(&mut self) {
        let mut data = String::new();
        for mount in &self.mounts {
            data.push_str(&format!("{} {} {}\n", mount.source, mount.target, mount.fs_type));
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

    fn create_node(&mut self, absolute_path: &str, kind: NodeKind, data: Option<NodeData>) -> KernelResult<()> {
        if absolute_path == "/" {
            return Err(EEXIST);
        }
        let (parent_path, name) = split_parent(absolute_path)?;
        let parent = self.lookup_abs(&parent_path)?;
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
            NodeKind::CharDevice => {
                Node::char_device(name, data.unwrap_or(NodeData::CharDevice))
            }
            NodeKind::Proc => Node::proc(name, data.unwrap_or_else(|| NodeData::ProcFile(Vec::new()))),
            NodeKind::Pipe => Node::pipe(name),
            NodeKind::Symlink => Node::symlink(name, data.unwrap_or_else(|| NodeData::Symlink(String::new()))),
            NodeKind::Event => Node::eventfd(name, 0),
            NodeKind::Epoll => Node::epoll(name),
            NodeKind::Socket => Node::socket_pending(name),
            NodeKind::PidFd => Node::pidfd(name, 0),
        });
        entries.insert(name.to_string(), node);
        Ok(())
    }

    fn stat(&self, node: &Arc<Node>) -> KernelResult<FileStat> {
        let size = match &*node.data.lock() {
            NodeData::Directory(entries) => entries.len() as u64,
            NodeData::File(buf) | NodeData::ProcFile(buf) => buf.len() as u64,
            NodeData::Pipe(buf) => buf.len() as u64,
            NodeData::Symlink(target) => target.len() as u64,
            NodeData::Event(_) => 8,
            NodeData::Epoll(watches) => watches.len() as u64,
            NodeData::SocketPending(state) => state.pending.len() as u64,
            NodeData::SocketConnected { channel, side } => channel.lock().inbox[*side].len() as u64,
            NodeData::PidFd(_) => 0,
            NodeData::CharDevice => 0,
        };
        let mode = match node.kind {
            NodeKind::Directory => S_IFDIR | 0o755,
            NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
            NodeKind::CharDevice => S_IFCHR | 0o600,
            NodeKind::Pipe => S_IFIFO | 0o644,
            NodeKind::Symlink => S_IFLNK | 0o777,
            NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
            NodeKind::PidFd => S_IFREG | 0o444,
        };
        Ok(FileStat {
            mode,
            size,
            nlink: 1,
        })
    }
}

impl FileHandle {
    fn stat_from_locked(&self) -> FileStat {
        let size = match &*self.node.data.lock() {
            NodeData::Directory(entries) => entries.len() as u64,
            NodeData::File(buf) | NodeData::ProcFile(buf) => buf.len() as u64,
            NodeData::Pipe(buf) => buf.len() as u64,
            NodeData::Symlink(target) => target.len() as u64,
            NodeData::Event(_) => 8,
            NodeData::Epoll(watches) => watches.len() as u64,
            NodeData::SocketPending(state) => state.pending.len() as u64,
            NodeData::SocketConnected { channel, side } => channel.lock().inbox[*side].len() as u64,
            NodeData::PidFd(_) => 0,
            NodeData::CharDevice => 0,
        };
        let mode = match self.node.kind {
            NodeKind::Directory => S_IFDIR | 0o755,
            NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
            NodeKind::CharDevice => S_IFCHR | 0o600,
            NodeKind::Pipe => S_IFIFO | 0o644,
            NodeKind::Symlink => S_IFLNK | 0o777,
            NodeKind::Event | NodeKind::Epoll | NodeKind::Socket => S_IFSOCK | 0o644,
            NodeKind::PidFd => S_IFREG | 0o444,
        };
        FileStat {
            mode,
            size,
            nlink: 1,
        }
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
            NodeData::Pipe(buf) => {
                let end = len.min(buf.len());
                Ok(buf.drain(..end).collect())
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
            NodeData::CharDevice => Ok(Vec::new()),
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
            NodeData::Pipe(buf) => {
                buf.extend_from_slice(data);
                Ok(data.len())
            }
            NodeData::Symlink(_) | NodeData::Epoll(_) | NodeData::SocketPending(_) | NodeData::PidFd(_) => {
                Err(EINVAL)
            }
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
            NodeData::CharDevice => Ok(data.len()),
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
            NodeData::File(_) | NodeData::ProcFile(_) | NodeData::CharDevice => true,
            NodeData::Pipe(buf) => !buf.is_empty(),
            NodeData::Symlink(_) => true,
            NodeData::Event(counter) => *counter != 0,
            NodeData::Epoll(_) => true,
            NodeData::SocketPending(state) => !state.pending.is_empty(),
            NodeData::SocketConnected { channel, side } => !channel.lock().inbox[*side].is_empty(),
            NodeData::PidFd(_) => true,
        }
    }

    fn poll_write_ready(&self) -> bool {
        !matches!(&*self.node.data.lock(), NodeData::Directory(_) | NodeData::PidFd(_))
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
            data: Mutex::new(NodeData::Pipe(Vec::new())),
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

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::{KernelObject, KernelVfs, ObjectKind, O_CREAT, O_RDWR};

    #[test]
    fn vfs_file_round_trip() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs.open("/", "/tmp/hello.txt", O_CREAT | O_RDWR, 0o644).unwrap();
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

        let regular = vfs.open("/", "/tmp/object.txt", O_CREAT | O_RDWR, 0o644).unwrap();
        assert_eq!(regular.object_kind(), ObjectKind::Regular);
    }
}
