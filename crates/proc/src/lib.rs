#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use mm::{AddressSpace, KernelResult as MmResult};
use vfs::FileHandle;

pub type KernelResult<T> = Result<T, i32>;

const EBADF: i32 = 9;
const ECHILD: i32 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Exited,
}

#[derive(Clone)]
pub struct Process {
    pub pid: usize,
    pub tid: usize,
    pub parent: Option<usize>,
    pub name: String,
    pub cwd: String,
    pub state: ProcessState,
    pub exit_code: Option<i32>,
    pub address_space: AddressSpace,
    pub fds: BTreeMap<i32, FileHandle>,
}

pub struct ProcessTable {
    next_pid: usize,
    current_pid: usize,
    processes: BTreeMap<usize, Process>,
}

impl Process {
    pub fn add_fd(&mut self, handle: FileHandle) -> i32 {
        let mut fd = 3;
        while self.fds.contains_key(&fd) {
            fd += 1;
        }
        self.fds.insert(fd, handle);
        fd
    }

    pub fn close_fd(&mut self, fd: i32) -> KernelResult<()> {
        self.fds.remove(&fd).map(|_| ()).ok_or(EBADF)
    }

    pub fn fd(&self, fd: i32) -> KernelResult<&FileHandle> {
        self.fds.get(&fd).ok_or(EBADF)
    }

    pub fn fd_mut(&mut self, fd: i32) -> KernelResult<&mut FileHandle> {
        self.fds.get_mut(&fd).ok_or(EBADF)
    }

    pub fn read_user_bytes(&self, addr: usize, len: usize) -> MmResult<alloc::vec::Vec<u8>> {
        self.address_space.read_bytes(addr, len)
    }

    pub fn write_user_bytes(&mut self, addr: usize, bytes: &[u8]) -> MmResult<()> {
        self.address_space.write_bytes(addr, bytes)
    }

    pub fn read_user_cstr(&self, addr: usize) -> MmResult<String> {
        self.address_space.read_cstr(addr)
    }
}

impl ProcessTable {
    pub fn new() -> Self {
        Self {
            next_pid: 1,
            current_pid: 0,
            processes: BTreeMap::new(),
        }
    }

    pub fn spawn_init(&mut self, name: &str) -> usize {
        self.spawn(name, None)
    }

    pub fn spawn(&mut self, name: &str, parent: Option<usize>) -> usize {
        let pid = self.next_pid;
        self.next_pid += 1;
        let process = Process {
            pid,
            tid: pid,
            parent,
            name: name.to_string(),
            cwd: "/".to_string(),
            state: ProcessState::Ready,
            exit_code: None,
            address_space: AddressSpace::new_user(),
            fds: BTreeMap::new(),
        };
        self.processes.insert(pid, process);
        if self.current_pid == 0 {
            self.current_pid = pid;
        }
        pid
    }

    pub fn current(&self) -> KernelResult<&Process> {
        self.processes.get(&self.current_pid).ok_or(EBADF)
    }

    pub fn current_mut(&mut self) -> KernelResult<&mut Process> {
        self.processes.get_mut(&self.current_pid).ok_or(EBADF)
    }

    pub fn set_current(&mut self, pid: usize) -> KernelResult<()> {
        if self.processes.contains_key(&pid) {
            self.current_pid = pid;
            Ok(())
        } else {
            Err(EBADF)
        }
    }

    pub fn exit_current(&mut self, code: i32) -> KernelResult<()> {
        let process = self.current_mut()?;
        process.state = ProcessState::Exited;
        process.exit_code = Some(code);
        Ok(())
    }

    pub fn wait(&self, parent_pid: usize) -> KernelResult<(usize, i32)> {
        self.processes
            .values()
            .find(|process| process.parent == Some(parent_pid) && process.state == ProcessState::Exited)
            .map(|process| (process.pid, process.exit_code.unwrap_or_default()))
            .ok_or(ECHILD)
    }
}

