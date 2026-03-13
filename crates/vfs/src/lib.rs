#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    Directory,
    File,
    CharDevice,
    Proc,
    Pipe,
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

struct Node {
    _name: String,
    kind: NodeKind,
    data: Mutex<NodeData>,
}

enum NodeData {
    Directory(BTreeMap<String, Arc<Node>>),
    File(Vec<u8>),
    CharDevice,
    ProcFile(Vec<u8>),
    Pipe(Vec<u8>),
}

pub struct KernelVfs {
    root: Arc<Node>,
    mounts: Vec<MountRecord>,
    next_pipe_id: usize,
}

impl KernelVfs {
    pub fn new() -> Self {
        let root = Arc::new(Node::directory("/"));
        let mut vfs = Self {
            root,
            mounts: Vec::new(),
            next_pipe_id: 0,
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

    pub fn mkdir(&mut self, cwd: &str, path: &str, _mode: u32) -> KernelResult<()> {
        let absolute = normalize_path(cwd, path);
        self.create_node(&absolute, NodeKind::Directory, None)
    }

    pub fn open(&mut self, cwd: &str, path: &str, flags: u32, _mode: u32) -> KernelResult<FileHandle> {
        let absolute = normalize_path(cwd, path);
        let node = match self.lookup_abs(&absolute) {
            Ok(node) => node,
            Err(err) if err == ENOENT && (flags & O_CREAT) != 0 => {
                self.create_file("/", &absolute, b"")?;
                self.lookup_abs(&absolute)?
            }
            Err(err) => return Err(err),
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
        match &mut *handle.node.data.lock() {
            NodeData::Directory(_) => Err(EISDIR),
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                let start = handle.offset.min(buf.len());
                let end = (start + len).min(buf.len());
                handle.offset = end;
                Ok(buf[start..end].to_vec())
            }
            NodeData::Pipe(buf) => {
                let end = len.min(buf.len());
                Ok(buf.drain(..end).collect())
            }
            NodeData::CharDevice => Ok(Vec::new()),
        }
    }

    pub fn write(&mut self, handle: &mut FileHandle, data: &[u8]) -> KernelResult<usize> {
        match &mut *handle.node.data.lock() {
            NodeData::Directory(_) => Err(EISDIR),
            NodeData::File(buf) | NodeData::ProcFile(buf) => {
                if handle.offset > buf.len() {
                    buf.resize(handle.offset, 0);
                }
                if handle.offset + data.len() > buf.len() {
                    buf.resize(handle.offset + data.len(), 0);
                }
                buf[handle.offset..handle.offset + data.len()].copy_from_slice(data);
                handle.offset += data.len();
                Ok(data.len())
            }
            NodeData::Pipe(buf) => {
                buf.extend_from_slice(data);
                Ok(data.len())
            }
            NodeData::CharDevice => Ok(data.len()),
        }
    }

    pub fn seek(&self, handle: &mut FileHandle, offset: isize, whence: u32) -> KernelResult<usize> {
        if handle.node.kind == NodeKind::Pipe {
            return Err(EINVAL);
        }
        let size = self.stat(&handle.node)?.size as isize;
        let base = match whence {
            0 => 0,
            1 => handle.offset as isize,
            2 => size,
            _ => return Err(EINVAL),
        };
        let new_offset = base.checked_add(offset).ok_or(EINVAL)?;
        if new_offset < 0 {
            return Err(EINVAL);
        }
        handle.offset = new_offset as usize;
        Ok(handle.offset)
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
            NodeData::Pipe(_) | NodeData::Directory(_) | NodeData::CharDevice => Err(EINVAL),
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
        });
        entries.insert(name.to_string(), node);
        Ok(())
    }

    fn stat(&self, node: &Arc<Node>) -> KernelResult<FileStat> {
        let size = match &*node.data.lock() {
            NodeData::Directory(entries) => entries.len() as u64,
            NodeData::File(buf) | NodeData::ProcFile(buf) => buf.len() as u64,
            NodeData::Pipe(buf) => buf.len() as u64,
            NodeData::CharDevice => 0,
        };
        let mode = match node.kind {
            NodeKind::Directory => S_IFDIR | 0o755,
            NodeKind::File | NodeKind::Proc => S_IFREG | 0o644,
            NodeKind::CharDevice => S_IFCHR | 0o600,
            NodeKind::Pipe => S_IFIFO | 0o644,
        };
        Ok(FileStat {
            mode,
            size,
            nlink: 1,
        })
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
    use super::{KernelVfs, O_CREAT, O_RDWR};

    #[test]
    fn vfs_file_round_trip() {
        let mut vfs = KernelVfs::new();
        let mut file = vfs.open("/", "/tmp/hello.txt", O_CREAT | O_RDWR, 0o644).unwrap();
        vfs.write(&mut file, b"hello").unwrap();
        vfs.seek(&mut file, 0, 0).unwrap();
        assert_eq!(vfs.read(&mut file, 5).unwrap(), b"hello");
        assert_eq!(vfs.chdir("/", "/tmp").unwrap(), "/tmp");
    }
}
