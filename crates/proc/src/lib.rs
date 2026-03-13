#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use mm::{AddressSpace, KernelResult as MmResult};
use hal_api::TrapFrame;
use vfs::FileHandle;

pub type KernelResult<T> = Result<T, i32>;

const EBADF: i32 = 9;
const ECHILD: i32 = 10;
const ENOENT: i32 = 2;
const EINVAL: i32 = 22;

const USER_STACK_SIZE: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Blocked,
    Exited,
}

pub struct Process {
    pub pid: usize,
    pub tid: usize,
    pub tgid: usize,
    pub pgid: usize,
    pub sid: usize,
    pub parent: Option<usize>,
    pub name: String,
    pub cwd: String,
    pub uid: u32,
    pub euid: u32,
    pub gid: u32,
    pub egid: u32,
    pub groups: Vec<u32>,
    pub umask: u32,
    pub state: ProcessState,
    pub exit_code: Option<i32>,
    pub address_space: AddressSpace,
    pub fds: BTreeMap<i32, FileHandle>,
    pub trap_frame: TrapFrame,
    pub user_stack: Box<[u8]>,
    pub tid_address: Option<usize>,
    pub robust_list: Option<(usize, usize)>,
    pub signal_mask: u64,
    pub pending_signals: u64,
    pub sigaltstack: Option<(usize, usize, u32)>,
}

pub struct ProcessTable {
    next_pid: usize,
    current_pid: usize,
    processes: BTreeMap<usize, Process>,
}

impl Process {
    pub fn new(name: &str, pid: usize, parent: Option<usize>, entry: usize) -> Self {
        let user_stack = vec![0u8; USER_STACK_SIZE].into_boxed_slice();
        let sp = user_stack.as_ptr() as usize + user_stack.len() - 16;
        Self {
            pid,
            tid: pid,
            tgid: pid,
            pgid: parent.unwrap_or(pid),
            sid: parent.unwrap_or(pid),
            parent,
            name: name.to_string(),
            cwd: "/".to_string(),
            uid: 0,
            euid: 0,
            gid: 0,
            egid: 0,
            groups: Vec::new(),
            umask: 0o022,
            state: ProcessState::Ready,
            exit_code: None,
            address_space: AddressSpace::new_user(),
            fds: BTreeMap::new(),
            trap_frame: TrapFrame::new_user(entry, sp),
            user_stack,
            tid_address: None,
            robust_list: None,
            signal_mask: 0,
            pending_signals: 0,
            sigaltstack: None,
        }
    }

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

    pub fn reset_image(&mut self, entry: usize) {
        self.address_space.clear();
        self.user_stack = vec![0u8; USER_STACK_SIZE].into_boxed_slice();
        let sp = self.user_stack.as_ptr() as usize + self.user_stack.len() - 16;
        self.trap_frame = TrapFrame::new_user(entry, sp);
        self.state = ProcessState::Ready;
        self.exit_code = None;
    }

    fn fork_from(&self, pid: usize) -> Self {
        let user_stack = self.user_stack.to_vec().into_boxed_slice();
        let old_base = self.user_stack.as_ptr() as usize;
        let new_base = user_stack.as_ptr() as usize;
        let stack_len = self.user_stack.len();
        let old_sp = self.trap_frame.regs[2];
        let new_sp = if (old_base..=old_base + stack_len).contains(&old_sp) {
            new_base + (old_sp - old_base)
        } else {
            old_sp
        };

        let mut trap_frame = self.trap_frame;
        trap_frame.regs[2] = new_sp;
        trap_frame.set_retval(0);
        trap_frame.sepc += 4;

        Self {
            pid,
            tid: pid,
            tgid: pid,
            pgid: self.pgid,
            sid: self.sid,
            parent: Some(self.pid),
            name: self.name.clone(),
            cwd: self.cwd.clone(),
            uid: self.uid,
            euid: self.euid,
            gid: self.gid,
            egid: self.egid,
            groups: self.groups.clone(),
            umask: self.umask,
            state: ProcessState::Ready,
            exit_code: None,
            address_space: self.address_space.clone(),
            fds: self.fds.clone(),
            trap_frame,
            user_stack,
            tid_address: self.tid_address,
            robust_list: self.robust_list,
            signal_mask: self.signal_mask,
            pending_signals: self.pending_signals,
            sigaltstack: self.sigaltstack,
        }
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

    pub fn spawn_init(&mut self, name: &str, entry: usize) -> usize {
        self.spawn(name, None, entry)
    }

    pub fn spawn(&mut self, name: &str, parent: Option<usize>, entry: usize) -> usize {
        let pid = self.next_pid;
        self.next_pid += 1;
        let process = Process::new(name, pid, parent, entry);
        self.processes.insert(pid, process);
        if self.current_pid == 0 {
            self.current_pid = pid;
        }
        pid
    }

    pub fn current(&self) -> KernelResult<&Process> {
        self.processes.get(&self.current_pid).ok_or(EBADF)
    }

    pub fn current_pid(&self) -> KernelResult<usize> {
        Ok(self.current()?.pid)
    }

    pub fn has_pid(&self, pid: usize) -> bool {
        self.processes.contains_key(&pid)
    }

    pub fn current_mut(&mut self) -> KernelResult<&mut Process> {
        self.processes.get_mut(&self.current_pid).ok_or(EBADF)
    }

    pub fn current_frame_mut(&mut self) -> KernelResult<&mut TrapFrame> {
        Ok(&mut self.current_mut()?.trap_frame)
    }

    pub fn duplicate_fd_from(&self, pid: usize, fd: i32) -> KernelResult<FileHandle> {
        let process = self.processes.get(&pid).ok_or(ENOENT)?;
        process.fd(fd).cloned()
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

    pub fn wait(&mut self, parent_pid: usize, pid: i32) -> KernelResult<(usize, i32)> {
        let child_pid = self
            .processes
            .values()
            .find(|process| {
                process.parent == Some(parent_pid)
                    && process.state == ProcessState::Exited
                    && (pid == -1 || process.pid == pid as usize)
            })
            .map(|process| process.pid)
            .ok_or(ECHILD)?;

        let process = self.processes.remove(&child_pid).ok_or(ENOENT)?;
        Ok((child_pid, process.exit_code.unwrap_or_default() << 8))
    }

    pub fn fork_current(&mut self) -> KernelResult<usize> {
        let parent = self.current()?.fork_from(self.next_pid);
        let pid = parent.pid;
        self.next_pid += 1;
        self.processes.insert(pid, parent);
        Ok(pid)
    }

    pub fn execve_current(&mut self, entry: usize) -> KernelResult<()> {
        let process = self.current_mut()?;
        process.reset_image(entry);
        process.pending_signals = 0;
        Ok(())
    }

    pub fn getppid(&self) -> KernelResult<usize> {
        Ok(self.current()?.parent.unwrap_or(0))
    }

    pub fn gettid(&self) -> KernelResult<usize> {
        Ok(self.current()?.tid)
    }

    pub fn set_tid_address(&mut self, addr: usize) -> KernelResult<usize> {
        let tid = self.current()?.tid;
        self.current_mut()?.tid_address = Some(addr);
        Ok(tid)
    }

    pub fn set_robust_list(&mut self, head: usize, len: usize) -> KernelResult<()> {
        if len == 0 {
            return Err(EINVAL);
        }
        self.current_mut()?.robust_list = Some((head, len));
        Ok(())
    }

    pub fn get_robust_list(&self, pid: usize) -> KernelResult<(usize, usize)> {
        let process = if pid == 0 {
            self.current()?
        } else {
            self.processes.get(&pid).ok_or(ENOENT)?
        };
        process.robust_list.ok_or(EINVAL)
    }

    pub fn getpgid(&self, pid: usize) -> KernelResult<usize> {
        let target = if pid == 0 { self.current()? } else { self.processes.get(&pid).ok_or(ENOENT)? };
        Ok(target.pgid)
    }

    pub fn setpgid(&mut self, pid: usize, pgid: usize) -> KernelResult<()> {
        let pid = if pid == 0 { self.current_pid()? } else { pid };
        let process = self.processes.get_mut(&pid).ok_or(ENOENT)?;
        process.pgid = if pgid == 0 { pid } else { pgid };
        Ok(())
    }

    pub fn getsid(&self, pid: usize) -> KernelResult<usize> {
        let target = if pid == 0 { self.current()? } else { self.processes.get(&pid).ok_or(ENOENT)? };
        Ok(target.sid)
    }

    pub fn setsid_current(&mut self) -> KernelResult<usize> {
        let pid = self.current_pid()?;
        let process = self.current_mut()?;
        process.sid = pid;
        process.pgid = pid;
        Ok(pid)
    }

    pub fn setuid_current(&mut self, uid: u32) -> KernelResult<()> {
        let process = self.current_mut()?;
        process.uid = uid;
        process.euid = uid;
        Ok(())
    }

    pub fn setgid_current(&mut self, gid: u32) -> KernelResult<()> {
        let process = self.current_mut()?;
        process.gid = gid;
        process.egid = gid;
        Ok(())
    }

    pub fn getgroups_current(&self) -> KernelResult<Vec<u32>> {
        Ok(self.current()?.groups.clone())
    }

    pub fn setgroups_current(&mut self, groups: &[u32]) -> KernelResult<()> {
        self.current_mut()?.groups = groups.to_vec();
        Ok(())
    }

    pub fn umask_current(&mut self, mask: u32) -> KernelResult<u32> {
        let process = self.current_mut()?;
        let previous = process.umask;
        process.umask = mask & 0o777;
        Ok(previous)
    }

    pub fn signal_mask(&self) -> KernelResult<u64> {
        Ok(self.current()?.signal_mask)
    }

    pub fn set_signal_mask(&mut self, mask: u64) -> KernelResult<()> {
        self.current_mut()?.signal_mask = mask;
        Ok(())
    }

    pub fn pending_signals(&self) -> KernelResult<u64> {
        Ok(self.current()?.pending_signals)
    }

    pub fn set_sigaltstack(
        &mut self,
        stack: Option<(usize, usize, u32)>,
    ) -> KernelResult<Option<(usize, usize, u32)>> {
        let process = self.current_mut()?;
        let previous = process.sigaltstack;
        process.sigaltstack = stack;
        Ok(previous)
    }

    pub fn send_signal(&mut self, pid: usize, signal: usize) -> KernelResult<()> {
        if signal == 0 || signal > 64 {
            return Err(EINVAL);
        }
        let process = self.processes.get_mut(&pid).ok_or(ENOENT)?;
        process.pending_signals |= 1u64 << (signal - 1);
        Ok(())
    }

    pub fn clear_pending_signal(&mut self, signal: usize) -> KernelResult<()> {
        if signal == 0 || signal > 64 {
            return Err(EINVAL);
        }
        self.current_mut()?.pending_signals &= !(1u64 << (signal - 1));
        Ok(())
    }

    pub fn process_count(&self) -> usize {
        self.processes
            .values()
            .filter(|process| process.state != ProcessState::Exited)
            .count()
    }
}
