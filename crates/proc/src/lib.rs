#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use hal_api::TrapFrame;
use mm::{AddressSpace, KernelResult as MmResult};
use vfs::{FileHandle, HANDLE_FLAG_CLOEXEC};

pub type KernelResult<T> = Result<T, i32>;

const EBADF: i32 = 9;
const ECHILD: i32 = 10;
const ENOENT: i32 = 2;
const EINVAL: i32 = 22;
const ESRCH: i32 = 3;

const USER_STACK_SIZE: usize = 8192;
const USER_STACK_TOP: usize = 0x7fff_f000;
const FUTEX_WAITERS: u32 = 0x8000_0000;
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
const ROBUST_LIST_MAX_SCAN: usize = 2048;
const ROBUST_HEAD_WORDS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Blocked,
    Exited,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Credentials {
    pub uid: u32,
    pub euid: u32,
    pub gid: u32,
    pub egid: u32,
    pub groups: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessGroupState {
    pub pid: usize,
    pub parent: Option<usize>,
    pub pgid: usize,
    pub sid: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalState {
    pub blocked_mask: u64,
    pub pending_mask: u64,
    pub altstack: Option<(usize, usize, u32)>,
    pub robust_list: Option<(usize, usize)>,
    pub clear_child_tid: Option<usize>,
    pub tid_address: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SigAction {
    pub handler: usize,
    pub flags: usize,
    pub restorer: usize,
    pub mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitSelector {
    Any,
    Pid(usize),
    Pgid(usize),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadExit {
    pub tid: usize,
    pub tgid: usize,
    pub clear_child_tid: Option<usize>,
    pub robust_futex_addrs: Vec<usize>,
    pub group_exited: bool,
    pub parent_tgid: Option<usize>,
    pub vfork_parent_tid: Option<usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GroupExit {
    pub tgid: usize,
    pub tids: Vec<usize>,
    pub clear_child_tids: Vec<usize>,
    pub robust_futex_addrs: Vec<usize>,
    pub parent_tgid: Option<usize>,
    pub vfork_parent_tid: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    pub tid: usize,
    pub tgid: usize,
    pub name: String,
    pub state: ProcessState,
    pub is_thread: bool,
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
    fd_alias: BTreeMap<i32, i32>,
    pub trap_frame: TrapFrame,
    pub user_stack: Box<[u8]>,
    pub tid_address: Option<usize>,
    pub clear_child_tid: Option<usize>,
    pub robust_list: Option<(usize, usize)>,
    pub signal_mask: u64,
    pub pending_signals: u64,
    pub sigaltstack: Option<(usize, usize, u32)>,
    pub signal_actions: BTreeMap<usize, SigAction>,
    pub sleep_deadline_ns: Option<u64>,
    pub sleep_requested_ns: u64,
    pub sleep_remain_ptr: Option<usize>,
    pub sleep_absolute: bool,
    pub futex_wait_addr: Option<usize>,
    pub futex_wait_deadline_ns: Option<u64>,
    pub epoll_wait_deadline_ns: Option<u64>,
    pub sigsuspend_saved_mask: Option<u64>,
    pub is_thread: bool,
    /// Set to true when a signal frame has been dispatched into the thread's
    /// trap frame (sepc redirected to signal handler) but the handler has not
    /// yet returned via rt_sigreturn.  Any FUTEX_WAIT entered while this flag
    /// is true must return -EINTR so the pending handler can actually run.
    pub signal_frame_pending: bool,
    /// Set to true when SIGCANCEL (sig 33) is dispatched and not yet consumed
    /// by rt_sigreturn. This tracks cancellation arrival without forcing all
    /// subsequent blocking syscalls to return EINTR forever.
    pub cancel_signal_seen: bool,
    /// One-shot futex interrupt token armed by rt_sigreturn after SIGCANCEL.
    /// FUTEX_WAIT consumes this token and returns -EINTR once.
    pub cancel_interrupt_once: bool,
    /// Parent task id blocked by CLONE_VFORK. Set on the child process until
    /// the child reaches execve/exit and releases the parent.
    pub vfork_parent_tid: Option<usize>,
}

pub struct ProcessTable {
    next_pid: usize,
    current_tid: usize,
    processes: BTreeMap<usize, Process>,
    futex_waiters: BTreeMap<usize, VecDeque<usize>>,
}

impl Process {
    pub fn new(name: &str, pid: usize, parent: Option<usize>, entry: usize) -> Self {
        let user_stack = vec![0u8; USER_STACK_SIZE].into_boxed_slice();
        let address_space = AddressSpace::new_user();
        let stack_base = USER_STACK_TOP - USER_STACK_SIZE;
        let _ = address_space.map_fixed_bytes(stack_base, &[], USER_STACK_SIZE, 0b11);
        let sp = USER_STACK_TOP - 16;
        Self {
            pid,
            tid: pid,
            tgid: pid,
            pgid: parent.unwrap_or(pid),
            sid: parent.unwrap_or(pid),
            parent,
            name: name.to_string(),
            cwd: String::from("/"),
            uid: 0,
            euid: 0,
            gid: 0,
            egid: 0,
            groups: Vec::new(),
            umask: 0o022,
            state: ProcessState::Ready,
            exit_code: None,
            address_space,
            fds: BTreeMap::new(),
            fd_alias: BTreeMap::new(),
            trap_frame: TrapFrame::new_user(entry, sp),
            user_stack,
            tid_address: None,
            clear_child_tid: None,
            robust_list: None,
            signal_mask: 0,
            pending_signals: 0,
            sigaltstack: None,
            signal_actions: BTreeMap::new(),
            sleep_deadline_ns: None,
            sleep_requested_ns: 0,
            sleep_remain_ptr: None,
            sleep_absolute: false,
            futex_wait_addr: None,
            futex_wait_deadline_ns: None,
            epoll_wait_deadline_ns: None,
            sigsuspend_saved_mask: None,
            is_thread: false,
            signal_frame_pending: false,
            cancel_signal_seen: false,
            cancel_interrupt_once: false,
            vfork_parent_tid: None,
        }
    }

    pub fn add_fd(&mut self, handle: FileHandle) -> i32 {
        let mut fd = 3;
        while self.fds.contains_key(&fd) {
            fd += 1;
        }
        self.fds.insert(fd, handle);
        self.fd_alias.insert(fd, fd);
        fd
    }

    pub fn add_fd_from(&mut self, source_fd: i32, handle: FileHandle) -> KernelResult<i32> {
        let new_fd = self.add_fd(handle);
        let leader = self.fd_alias_leader(source_fd)?;
        self.fd_alias.insert(new_fd, leader);
        self.sync_fd_offset_from_alias(new_fd)?;
        Ok(new_fd)
    }

    pub fn close_fd(&mut self, fd: i32) -> KernelResult<()> {
        self.fds.remove(&fd).ok_or(EBADF)?;
        self.fd_alias.remove(&fd);
        if self.fd_alias.values().any(|leader| *leader == fd) {
            let replacements = self
                .fd_alias
                .keys()
                .copied()
                .filter(|candidate| self.fd_alias.get(candidate).copied() == Some(fd))
                .collect::<Vec<_>>();
            if let Some(new_leader) = replacements.first().copied() {
                self.fd_alias.insert(new_leader, new_leader);
                for candidate in replacements.into_iter().skip(1) {
                    self.fd_alias.insert(candidate, new_leader);
                }
            }
        }
        Ok(())
    }

    pub fn fd(&self, fd: i32) -> KernelResult<&FileHandle> {
        self.fds.get(&fd).ok_or(EBADF)
    }

    pub fn fd_mut(&mut self, fd: i32) -> KernelResult<&mut FileHandle> {
        self.fds.get_mut(&fd).ok_or(EBADF)
    }

    pub fn fd_alias_leader(&self, fd: i32) -> KernelResult<i32> {
        let _ = self.fd(fd)?;
        let mut leader = self.fd_alias.get(&fd).copied().unwrap_or(fd);
        for _ in 0..16 {
            let Some(next) = self.fd_alias.get(&leader).copied() else {
                break;
            };
            if next == leader {
                break;
            }
            leader = next;
        }
        Ok(leader)
    }

    pub fn sync_fd_offset_from_alias(&mut self, fd: i32) -> KernelResult<()> {
        let leader = self.fd_alias_leader(fd)?;
        if leader == fd {
            return Ok(());
        }
        let leader_offset = self.fd(leader)?.offset;
        self.fd_mut(fd)?.offset = leader_offset;
        Ok(())
    }

    pub fn sync_fd_offset_to_aliases(&mut self, fd: i32) -> KernelResult<()> {
        let leader = self.fd_alias_leader(fd)?;
        let offset = self.fd(fd)?.offset;
        let peers = self
            .fd_alias
            .iter()
            .filter_map(|(candidate, mapped_leader)| {
                (*mapped_leader == leader).then_some(*candidate)
            })
            .collect::<Vec<_>>();
        for peer in peers {
            if let Some(handle) = self.fds.get_mut(&peer) {
                handle.offset = offset;
            }
        }
        if let Some(handle) = self.fds.get_mut(&leader) {
            handle.offset = offset;
        }
        Ok(())
    }

    pub fn set_fd_alias(&mut self, fd: i32, leader: i32) {
        self.fd_alias.insert(fd, leader);
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

    pub fn mark_cancel_signal_dispatched(&mut self) {
        self.cancel_signal_seen = true;
        self.cancel_interrupt_once = false;
    }

    pub fn arm_cancel_interrupt_once(&mut self) {
        if self.cancel_signal_seen {
            self.cancel_signal_seen = false;
            self.cancel_interrupt_once = true;
        }
    }

    pub fn consume_cancel_interrupt_once(&mut self) -> bool {
        if self.cancel_interrupt_once {
            self.cancel_interrupt_once = false;
            return true;
        }
        false
    }

    fn collect_robust_futex_addrs_for_exit(&mut self) -> Vec<usize> {
        let Some((head, len)) = self.robust_list else {
            return Vec::new();
        };
        if len < ROBUST_HEAD_WORDS * size_of::<usize>() {
            return Vec::new();
        }

        let list_next = match read_user_usize(self, head) {
            Ok(next) => next,
            Err(_) => return Vec::new(),
        };
        let futex_offset = match read_user_isize(self, head + size_of::<usize>()) {
            Ok(offset) => offset,
            Err(_) => return Vec::new(),
        };
        let list_op_pending = match read_user_usize(self, head + size_of::<usize>() * 2) {
            Ok(ptr) => ptr,
            Err(_) => 0,
        };

        let mut addrs = Vec::new();
        let mut seen = Vec::new();
        let mut node = list_next;
        for _ in 0..ROBUST_LIST_MAX_SCAN {
            if node == 0 || node == head {
                break;
            }
            if seen.iter().any(|seen_node| *seen_node == node) {
                break;
            }
            seen.push(node);
            if let Some(addr) = robust_futex_addr(node, futex_offset) {
                maybe_mark_owner_died(self, addr, self.tid);
                push_unique_addr(&mut addrs, addr);
            }
            node = match read_user_usize(self, node) {
                Ok(next) => next,
                Err(_) => break,
            };
        }

        if list_op_pending != 0 && list_op_pending != head {
            if let Some(addr) = robust_futex_addr(list_op_pending, futex_offset) {
                maybe_mark_owner_died(self, addr, self.tid);
                push_unique_addr(&mut addrs, addr);
            }
        }
        addrs
    }

    pub fn reset_image(&mut self, entry: usize, stack_pointer: Option<usize>) {
        self.address_space.clear();
        self.user_stack = vec![0u8; USER_STACK_SIZE].into_boxed_slice();
        let stack_base = USER_STACK_TOP - USER_STACK_SIZE;
        let _ = self
            .address_space
            .map_fixed_bytes(stack_base, &[], USER_STACK_SIZE, 0b11);
        let sp = stack_pointer.unwrap_or(USER_STACK_TOP - 16);
        self.trap_frame = TrapFrame::new_user(entry, sp);
        self.state = ProcessState::Ready;
        self.exit_code = None;
        self.pending_signals = 0;
        self.signal_mask = 0;
        self.clear_child_tid = None;
        self.tid_address = None;
        self.robust_list = None;
        self.sleep_deadline_ns = None;
        self.sleep_requested_ns = 0;
        self.sleep_remain_ptr = None;
        self.sleep_absolute = false;
        self.futex_wait_addr = None;
        self.futex_wait_deadline_ns = None;
        self.epoll_wait_deadline_ns = None;
        self.sigsuspend_saved_mask = None;
        self.signal_frame_pending = false;
        self.cancel_signal_seen = false;
        self.cancel_interrupt_once = false;
        self.vfork_parent_tid = None;
    }

    fn fork_from(&self, pid: usize) -> Self {
        let user_stack = self.user_stack.to_vec().into_boxed_slice();
        let new_sp = self.trap_frame.regs[2];

        let mut trap_frame = self.trap_frame;
        trap_frame.regs[2] = new_sp;
        trap_frame.set_retval(0);
        trap_frame.sepc += 4;

        let address_space = self.address_space.clone_private();

        Self {
            pid,
            tid: pid,
            tgid: pid,
            pgid: self.pgid,
            sid: self.sid,
            parent: Some(self.tgid),
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
            address_space,
            fds: self.fds.clone(),
            fd_alias: self.fd_alias.clone(),
            trap_frame,
            user_stack,
            tid_address: self.tid_address,
            clear_child_tid: self.clear_child_tid,
            robust_list: self.robust_list,
            signal_mask: self.signal_mask,
            pending_signals: 0,
            sigaltstack: self.sigaltstack,
            signal_actions: self.signal_actions.clone(),
            sleep_deadline_ns: None,
            sleep_requested_ns: 0,
            sleep_remain_ptr: None,
            sleep_absolute: false,
            futex_wait_addr: None,
            futex_wait_deadline_ns: None,
            epoll_wait_deadline_ns: None,
            sigsuspend_saved_mask: None,
            is_thread: false,
            signal_frame_pending: false,
            cancel_signal_seen: false,
            cancel_interrupt_once: false,
            vfork_parent_tid: None,
        }
    }

    fn fork_from_shared(&self, pid: usize) -> Self {
        let user_stack = self.user_stack.to_vec().into_boxed_slice();
        let new_sp = self.trap_frame.regs[2];

        let mut trap_frame = self.trap_frame;
        trap_frame.regs[2] = new_sp;
        trap_frame.set_retval(0);
        trap_frame.sepc += 4;

        let address_space = self.address_space.clone();

        Self {
            pid,
            tid: pid,
            tgid: pid,
            pgid: self.pgid,
            sid: self.sid,
            parent: Some(self.tgid),
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
            address_space,
            fds: self.fds.clone(),
            fd_alias: self.fd_alias.clone(),
            trap_frame,
            user_stack,
            tid_address: self.tid_address,
            clear_child_tid: self.clear_child_tid,
            robust_list: self.robust_list,
            signal_mask: self.signal_mask,
            pending_signals: 0,
            sigaltstack: self.sigaltstack,
            signal_actions: self.signal_actions.clone(),
            sleep_deadline_ns: None,
            sleep_requested_ns: 0,
            sleep_remain_ptr: None,
            sleep_absolute: false,
            futex_wait_addr: None,
            futex_wait_deadline_ns: None,
            epoll_wait_deadline_ns: None,
            sigsuspend_saved_mask: None,
            is_thread: false,
            signal_frame_pending: false,
            cancel_signal_seen: false,
            cancel_interrupt_once: false,
            vfork_parent_tid: None,
        }
    }

    fn clone_thread_from(&self, tid: usize, stack: usize, tls: Option<usize>) -> Self {
        let user_stack = vec![0u8; USER_STACK_SIZE].into_boxed_slice();
        let address_space = self.address_space.clone();
        let fallback_sp = if stack == 0 {
            address_space
                .map_anonymous(USER_STACK_SIZE, 0b11)
                .map(|base| base + USER_STACK_SIZE - 16)
                .unwrap_or(USER_STACK_TOP - 16)
        } else {
            USER_STACK_TOP - 16
        };
        let mut trap_frame = self.trap_frame;
        trap_frame.set_retval(0);
        trap_frame.sepc += 4;
        if stack != 0 {
            trap_frame.regs[2] = stack;
        } else {
            trap_frame.regs[2] = fallback_sp;
        }
        if let Some(tls) = tls {
            trap_frame.regs[4] = tls;
        }
        Self {
            pid: tid,
            tid,
            tgid: self.tgid,
            pgid: self.pgid,
            sid: self.sid,
            parent: self.parent,
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
            address_space,
            fds: self.fds.clone(),
            fd_alias: self.fd_alias.clone(),
            trap_frame,
            user_stack,
            tid_address: None,
            clear_child_tid: None,
            robust_list: None,
            signal_mask: self.signal_mask,
            pending_signals: 0,
            sigaltstack: self.sigaltstack,
            signal_actions: self.signal_actions.clone(),
            sleep_deadline_ns: None,
            sleep_requested_ns: 0,
            sleep_remain_ptr: None,
            sleep_absolute: false,
            futex_wait_addr: None,
            futex_wait_deadline_ns: None,
            epoll_wait_deadline_ns: None,
            sigsuspend_saved_mask: None,
            is_thread: true,
            signal_frame_pending: false,
            cancel_signal_seen: false,
            cancel_interrupt_once: false,
            vfork_parent_tid: None,
        }
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            uid: self.uid,
            euid: self.euid,
            gid: self.gid,
            egid: self.egid,
            groups: self.groups.clone(),
        }
    }

    pub fn process_group(&self) -> ProcessGroupState {
        ProcessGroupState {
            pid: self.tgid,
            parent: self.parent,
            pgid: self.pgid,
            sid: self.sid,
        }
    }

    pub fn session(&self) -> usize {
        self.sid
    }

    pub fn signal_state(&self) -> SignalState {
        SignalState {
            blocked_mask: self.signal_mask,
            pending_mask: self.pending_signals,
            altstack: self.sigaltstack,
            robust_list: self.robust_list,
            clear_child_tid: self.clear_child_tid,
            tid_address: self.tid_address,
        }
    }

    pub fn fd_table(&self) -> &BTreeMap<i32, FileHandle> {
        &self.fds
    }

    pub fn clear_child_tid(&self) -> Option<usize> {
        self.clear_child_tid
    }
}

impl ProcessTable {
    pub fn new() -> Self {
        Self {
            next_pid: 1,
            current_tid: 0,
            processes: BTreeMap::new(),
            futex_waiters: BTreeMap::new(),
        }
    }

    pub fn spawn_init(&mut self, name: &str, entry: usize) -> usize {
        self.spawn(name, None, entry)
    }

    pub fn spawn(&mut self, name: &str, parent: Option<usize>, entry: usize) -> usize {
        let pid = self.next_id();
        let process = Process::new(name, pid, parent, entry);
        self.processes.insert(pid, process);
        if self.current_tid == 0 {
            self.current_tid = pid;
        }
        pid
    }

    pub fn current(&self) -> KernelResult<&Process> {
        self.processes.get(&self.current_tid).ok_or(EBADF)
    }

    pub fn current_tid(&self) -> KernelResult<usize> {
        Ok(self.current()?.tid)
    }

    pub fn current_tgid(&self) -> KernelResult<usize> {
        Ok(self.current()?.tgid)
    }

    pub fn current_pid(&self) -> KernelResult<usize> {
        self.current_tgid()
    }

    pub fn current_pgid(&self) -> KernelResult<usize> {
        Ok(self.current()?.pgid)
    }

    pub fn has_pid(&self, pid: usize) -> bool {
        self.processes.contains_key(&pid)
            || self.processes.values().any(|process| process.tgid == pid)
    }

    pub fn current_mut(&mut self) -> KernelResult<&mut Process> {
        self.processes.get_mut(&self.current_tid).ok_or(EBADF)
    }

    pub fn find_by_tid_mut(&mut self, tid: usize) -> KernelResult<&mut Process> {
        self.processes.get_mut(&tid).ok_or(ENOENT)
    }

    pub fn current_frame_mut(&mut self) -> KernelResult<&mut TrapFrame> {
        Ok(&mut self.current_mut()?.trap_frame)
    }

    pub fn duplicate_fd_from(&self, pid: usize, fd: i32) -> KernelResult<FileHandle> {
        let process = self.find_process_by_pid(pid)?;
        process.fd(fd).cloned()
    }

    pub fn set_current(&mut self, tid: usize) -> KernelResult<()> {
        if self.processes.contains_key(&tid) {
            self.current_tid = tid;
            Ok(())
        } else {
            Err(EBADF)
        }
    }

    pub fn set_vfork_parent_tid(
        &mut self,
        child_tid: usize,
        parent_tid: usize,
    ) -> KernelResult<()> {
        let child = self.find_by_tid_mut(child_tid)?;
        child.vfork_parent_tid = Some(parent_tid);
        Ok(())
    }

    pub fn release_vfork_parent_for_tgid(&mut self, tgid: usize) -> Option<usize> {
        let mut parent_tid = None;
        let tids = self
            .processes
            .iter()
            .filter_map(|(tid, process)| (process.tgid == tgid).then_some(*tid))
            .collect::<Vec<_>>();
        for tid in tids {
            if let Some(process) = self.processes.get_mut(&tid) {
                if parent_tid.is_none() {
                    parent_tid = process.vfork_parent_tid;
                }
                process.vfork_parent_tid = None;
            }
        }
        parent_tid
    }

    pub fn release_current_vfork_parent(&mut self) -> KernelResult<Option<usize>> {
        let tgid = self.current_tgid()?;
        Ok(self.release_vfork_parent_for_tgid(tgid))
    }

    pub fn wait(&mut self, parent_pid: usize, pid: i32) -> KernelResult<(usize, i32)> {
        self.wait_child(
            parent_pid,
            selector_from_wait_pid(pid, self.find_process_by_pid(parent_pid)?.pgid),
            0,
        )
    }

    pub fn exit_current(&mut self, code: i32) -> KernelResult<()> {
        let _ = self.exit_current_thread(code)?;
        Ok(())
    }

    pub fn wait_child(
        &mut self,
        parent_tgid: usize,
        selector: WaitSelector,
        _options: u32,
    ) -> KernelResult<(usize, i32)> {
        let child_tid = self
            .processes
            .values()
            .filter(|process| !process.is_thread)
            .filter(|process| process.parent == Some(parent_tgid))
            .find(|process| {
                process.state == ProcessState::Exited && selector_matches(selector, process)
            })
            .map(|process| process.tgid);

        let Some(child_tid) = child_tid else {
            let has_any_child = self.processes.values().any(|process| {
                !process.is_thread
                    && process.parent == Some(parent_tgid)
                    && selector_matches(selector, process)
            });
            if has_any_child {
                return Ok((0, 0));
            }
            return Err(ECHILD);
        };

        let status = self
            .processes
            .get(&child_tid)
            .and_then(|process| process.exit_code)
            .unwrap_or_default()
            << 8;
        self.reap_thread_group(child_tid);
        Ok((child_tid, status))
    }

    pub fn fork_current(&mut self) -> KernelResult<usize> {
        self.fork_process_from_current()
    }

    pub fn fork_process_from_current(&mut self) -> KernelResult<usize> {
        let pid = self.next_id();
        let parent = self.current()?.fork_from(pid);
        self.processes.insert(pid, parent);
        Ok(pid)
    }

    pub fn fork_process_from_current_shared(&mut self) -> KernelResult<usize> {
        let pid = self.next_id();
        let parent = self.current()?.fork_from_shared(pid);
        self.processes.insert(pid, parent);
        Ok(pid)
    }

    pub fn clone_thread_from_current(
        &mut self,
        stack: usize,
        tls: Option<usize>,
    ) -> KernelResult<usize> {
        let tid = self.next_id();
        let thread = self.current()?.clone_thread_from(tid, stack, tls);
        self.processes.insert(tid, thread);
        Ok(tid)
    }

    pub fn set_thread_stack_pointer(
        &mut self,
        tid: usize,
        stack_pointer: usize,
    ) -> KernelResult<()> {
        let process = self.processes.get_mut(&tid).ok_or(EBADF)?;
        process.trap_frame.regs[2] = stack_pointer;
        Ok(())
    }

    pub fn execve_current(&mut self, entry: usize) -> KernelResult<()> {
        self.execve_current_image(entry, None)
    }

    pub fn execve_current_image(
        &mut self,
        entry: usize,
        stack_pointer: Option<usize>,
    ) -> KernelResult<()> {
        let current_tgid = self.current_tgid()?;
        let current_tid = self.current_tid()?;
        let other_threads = self
            .processes
            .iter()
            .filter_map(|(tid, process)| {
                (process.tgid == current_tgid && *tid != current_tid).then_some(*tid)
            })
            .collect::<Vec<_>>();
        for tid in other_threads {
            self.processes.remove(&tid);
            self.remove_futex_waiter(tid);
        }
        let process = self.current_mut()?;
        if process.address_space.is_shared() {
            process.address_space = process.address_space.clone_private();
        }
        process
            .fds
            .retain(|_, handle| (handle.flags & HANDLE_FLAG_CLOEXEC) == 0);
        process.reset_image(entry, stack_pointer);
        process.is_thread = false;
        process.pid = process.tgid;
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
        let process = self.current_mut()?;
        process.tid_address = Some(addr);
        process.clear_child_tid = Some(addr);
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
            self.find_process_by_pid(pid)?
        };
        process.robust_list.ok_or(EINVAL)
    }

    pub fn getpgid(&self, pid: usize) -> KernelResult<usize> {
        let target = if pid == 0 {
            self.current()?
        } else {
            self.find_process_by_pid(pid)?
        };
        Ok(target.pgid)
    }

    pub fn setpgid(&mut self, pid: usize, pgid: usize) -> KernelResult<()> {
        let pid = if pid == 0 { self.current_tgid()? } else { pid };
        let process = self.find_process_by_pid_mut(pid)?;
        process.pgid = if pgid == 0 { pid } else { pgid };
        Ok(())
    }

    pub fn getsid(&self, pid: usize) -> KernelResult<usize> {
        let target = if pid == 0 {
            self.current()?
        } else {
            self.find_process_by_pid(pid)?
        };
        Ok(target.sid)
    }

    pub fn setsid_current(&mut self) -> KernelResult<usize> {
        let pid = self.current_tgid()?;
        for process in self
            .processes
            .values_mut()
            .filter(|process| process.tgid == pid)
        {
            process.sid = pid;
            process.pgid = pid;
        }
        Ok(pid)
    }

    pub fn setuid_current(&mut self, uid: u32) -> KernelResult<()> {
        let tgid = self.current_tgid()?;
        for process in self
            .processes
            .values_mut()
            .filter(|process| process.tgid == tgid)
        {
            process.uid = uid;
            process.euid = uid;
        }
        Ok(())
    }

    pub fn setgid_current(&mut self, gid: u32) -> KernelResult<()> {
        let tgid = self.current_tgid()?;
        for process in self
            .processes
            .values_mut()
            .filter(|process| process.tgid == tgid)
        {
            process.gid = gid;
            process.egid = gid;
        }
        Ok(())
    }

    pub fn getgroups_current(&self) -> KernelResult<Vec<u32>> {
        Ok(self.current()?.groups.clone())
    }

    pub fn setgroups_current(&mut self, groups: &[u32]) -> KernelResult<()> {
        let tgid = self.current_tgid()?;
        for process in self
            .processes
            .values_mut()
            .filter(|process| process.tgid == tgid)
        {
            process.groups = groups.to_vec();
        }
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

    pub fn set_clear_child_tid(&mut self, addr: Option<usize>) -> KernelResult<()> {
        self.current_mut()?.clear_child_tid = addr;
        Ok(())
    }

    pub fn pending_signals(&self) -> KernelResult<u64> {
        Ok(self.current()?.pending_signals & !self.current()?.signal_mask)
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

    pub fn deliver_signal(&mut self, pid: usize, signal: usize) -> KernelResult<()> {
        self.send_signal(pid, signal)
    }

    pub fn send_signal_tid(&mut self, pid: usize, signal: usize) -> KernelResult<usize> {
        if signal > 64 {
            return Err(EINVAL);
        }
        if signal == 0 {
            return self.resolve_target_tid(pid);
        }
        let target_tid = self.resolve_target_tid(pid)?;
        let process = self.processes.get_mut(&target_tid).ok_or(ENOENT)?;
        if process.state == ProcessState::Exited {
            return Err(ESRCH);
        }
        process.pending_signals |= 1u64 << (signal - 1);
        Ok(target_tid)
    }

    pub fn send_signal_exact_tid(&mut self, tid: usize, signal: usize) -> KernelResult<usize> {
        if signal > 64 {
            return Err(EINVAL);
        }
        let process = self.processes.get_mut(&tid).ok_or(ESRCH)?;
        if process.state == ProcessState::Exited {
            return Err(ESRCH);
        }
        if signal == 0 {
            return Ok(tid);
        }
        process.pending_signals |= 1u64 << (signal - 1);
        Ok(tid)
    }

    pub fn send_signal(&mut self, pid: usize, signal: usize) -> KernelResult<()> {
        let _ = self.send_signal_tid(pid, signal)?;
        Ok(())
    }

    pub fn send_signal_pgid(
        &mut self,
        pgid: usize,
        signal: usize,
        exclude_tgid: Option<usize>,
    ) -> KernelResult<Vec<usize>> {
        if signal > 64 {
            return Err(EINVAL);
        }
        let targets = self
            .processes
            .values()
            .filter(|process| {
                !process.is_thread
                    && process.state != ProcessState::Exited
                    && process.pgid == pgid
                    && exclude_tgid.map_or(true, |tgid| process.tgid != tgid)
            })
            .map(|process| process.tid)
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Err(ESRCH);
        }
        if signal == 0 {
            return Ok(targets);
        }
        for tid in &targets {
            if let Some(process) = self.processes.get_mut(tid) {
                process.pending_signals |= 1u64 << (signal - 1);
            }
        }
        Ok(targets)
    }

    pub fn send_signal_all(
        &mut self,
        signal: usize,
        exclude_tgid: Option<usize>,
        include_init: bool,
    ) -> KernelResult<Vec<usize>> {
        if signal > 64 {
            return Err(EINVAL);
        }
        let targets = self
            .processes
            .values()
            .filter(|process| {
                !process.is_thread
                    && process.state != ProcessState::Exited
                    && (include_init || process.tgid > 1)
                    && exclude_tgid.map_or(true, |tgid| process.tgid != tgid)
            })
            .map(|process| process.tid)
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Err(ESRCH);
        }
        if signal == 0 {
            return Ok(targets);
        }
        for tid in &targets {
            if let Some(process) = self.processes.get_mut(tid) {
                process.pending_signals |= 1u64 << (signal - 1);
            }
        }
        Ok(targets)
    }

    pub fn clear_pending_signal(&mut self, signal: usize) -> KernelResult<()> {
        if signal == 0 || signal > 64 {
            return Err(EINVAL);
        }
        self.current_mut()?.pending_signals &= !(1u64 << (signal - 1));
        Ok(())
    }

    pub fn set_sigaction(&mut self, signal: usize, action: SigAction) -> KernelResult<()> {
        if signal == 0 || signal > 64 {
            return Err(EINVAL);
        }
        let tgid = self.current_tgid()?;
        for process in self
            .processes
            .values_mut()
            .filter(|process| process.tgid == tgid)
        {
            process.signal_actions.insert(signal, action);
        }
        Ok(())
    }

    pub fn sigaction(&self, signal: usize) -> KernelResult<SigAction> {
        if signal == 0 || signal > 64 {
            return Err(EINVAL);
        }
        Ok(self
            .current()?
            .signal_actions
            .get(&signal)
            .copied()
            .unwrap_or_default())
    }

    pub fn dequeue_unmasked_signal(&mut self) -> KernelResult<Option<usize>> {
        let pending = self.pending_signals()?;
        if pending == 0 {
            return Ok(None);
        }
        let signal = pending.trailing_zeros() as usize + 1;
        self.clear_pending_signal(signal)?;
        Ok(Some(signal))
    }

    pub fn enqueue_futex_waiter(&mut self, addr: usize, tid: usize) {
        let waiters = self.futex_waiters.entry(addr).or_default();
        if !waiters.iter().any(|waiter| *waiter == tid) {
            waiters.push_back(tid);
        }
    }

    pub fn wake_futex(&mut self, addr: usize, count: usize) -> Vec<usize> {
        let mut woke = Vec::new();
        if let Some(waiters) = self.futex_waiters.get_mut(&addr) {
            for _ in 0..count {
                let Some(tid) = waiters.pop_front() else {
                    break;
                };
                woke.push(tid);
            }
            if waiters.is_empty() {
                self.futex_waiters.remove(&addr);
            }
        }
        woke
    }

    pub fn is_futex_waiting(&self, addr: usize, tid: usize) -> bool {
        self.futex_waiters
            .get(&addr)
            .is_some_and(|waiters| waiters.iter().any(|waiter| *waiter == tid))
    }

    pub fn remove_futex_waiter_at(&mut self, addr: usize, tid: usize) {
        if let Some(waiters) = self.futex_waiters.get_mut(&addr) {
            waiters.retain(|waiter| *waiter != tid);
            if waiters.is_empty() {
                self.futex_waiters.remove(&addr);
            }
        }
    }

    pub fn requeue_futex(
        &mut self,
        from: usize,
        to: usize,
        wake_count: usize,
        requeue_count: usize,
    ) -> Vec<usize> {
        let woke = self.wake_futex(from, wake_count);
        let mut moved = VecDeque::new();
        if let Some(waiters) = self.futex_waiters.get_mut(&from) {
            for _ in 0..requeue_count {
                let Some(tid) = waiters.pop_front() else {
                    break;
                };
                moved.push_back(tid);
            }
            if waiters.is_empty() {
                self.futex_waiters.remove(&from);
            }
        }
        if !moved.is_empty() {
            self.futex_waiters.entry(to).or_default().extend(moved);
        }
        woke
    }

    pub fn exit_current_thread(&mut self, code: i32) -> KernelResult<ThreadExit> {
        let current_tid = self.current_tid()?;
        let current_tgid = self.current_tgid()?;
        let parent_tgid = self.current()?.parent;
        let vfork_parent_tid = self.current()?.vfork_parent_tid;
        let robust_futex_addrs = self.current_mut()?.collect_robust_futex_addrs_for_exit();
        let clear_child_tid = self.current()?.clear_child_tid;
        if let Some(addr) = clear_child_tid {
            let _ = self
                .current_mut()?
                .write_user_bytes(addr, &0u32.to_le_bytes());
        }
        self.remove_futex_waiter(current_tid);
        {
            let process = self.current_mut()?;
            process.state = ProcessState::Exited;
            process.exit_code = Some(code);
            process.fds.clear();
        }

        let group_alive = self.processes.values().any(|process| {
            process.tgid == current_tgid
                && process.tid != current_tid
                && process.state != ProcessState::Exited
        });
        if !group_alive {
            for process in self
                .processes
                .values_mut()
                .filter(|process| process.tgid == current_tgid)
            {
                process.state = ProcessState::Exited;
                process.exit_code = Some(code);
                process.address_space.clear();
            }
            if let Some(parent_tgid) = parent_tgid {
                let _ = self.send_signal(parent_tgid, 17);
            }
        }

        Ok(ThreadExit {
            tid: current_tid,
            tgid: current_tgid,
            clear_child_tid,
            robust_futex_addrs,
            group_exited: !group_alive,
            parent_tgid,
            vfork_parent_tid,
        })
    }

    pub fn exit_current_process_group(&mut self, code: i32) -> KernelResult<ThreadExit> {
        let current_tgid = self.current_tgid()?;
        let current_tid = self.current_tid()?;
        let parent_tgid = self.current()?.parent;
        let vfork_parent_tid = self.current()?.vfork_parent_tid;
        let mut robust_futex_addrs = Vec::new();
        let clear_child_tid = self.current()?.clear_child_tid;
        let tids = self
            .processes
            .iter()
            .filter_map(|(tid, process)| (process.tgid == current_tgid).then_some(*tid))
            .collect::<Vec<_>>();
        for tid in tids {
            if let Some(process) = self.processes.get_mut(&tid) {
                for addr in process.collect_robust_futex_addrs_for_exit() {
                    push_unique_addr(&mut robust_futex_addrs, addr);
                }
                if let Some(addr) = process.clear_child_tid {
                    let _ = process.write_user_bytes(addr, &0u32.to_le_bytes());
                }
                process.state = ProcessState::Exited;
                process.exit_code = Some(code);
                process.fds.clear();
                process.address_space.clear();
            }
            self.remove_futex_waiter(tid);
        }
        if let Some(parent_tgid) = parent_tgid {
            let _ = self.send_signal(parent_tgid, 17);
        }
        Ok(ThreadExit {
            tid: current_tid,
            tgid: current_tgid,
            clear_child_tid,
            robust_futex_addrs,
            group_exited: true,
            parent_tgid,
            vfork_parent_tid,
        })
    }

    pub fn process_count(&self) -> usize {
        self.processes
            .values()
            .filter(|process| !process.is_thread && process.state != ProcessState::Exited)
            .count()
    }

    pub fn process_snapshots(&self) -> Vec<ProcessSnapshot> {
        self.processes
            .values()
            .filter(|process| process.state != ProcessState::Exited)
            .map(|process| ProcessSnapshot {
                tid: process.tid,
                tgid: process.tgid,
                name: process.name.clone(),
                state: process.state,
                is_thread: process.is_thread,
            })
            .collect()
    }

    pub fn has_child_process_group(&self, tgid: usize) -> bool {
        self.processes.values().any(|process| {
            !process.is_thread
                && process.state != ProcessState::Exited
                && process.parent == Some(tgid)
                && process.tgid != tgid
        })
    }

    pub fn descendant_process_groups(&self, root_tgid: usize) -> Vec<usize> {
        let mut children = BTreeMap::<usize, Vec<usize>>::new();
        let mut has_root = false;
        for process in self.processes.values() {
            if process.is_thread || process.state == ProcessState::Exited {
                continue;
            }
            if process.tgid == root_tgid {
                has_root = true;
            }
            if let Some(parent) = process.parent {
                if process.tgid != parent {
                    children.entry(parent).or_default().push(process.tgid);
                }
            }
        }
        if !has_root {
            return Vec::new();
        }
        let mut queue = VecDeque::new();
        let mut out = Vec::new();
        queue.push_back(root_tgid);
        while let Some(current) = queue.pop_front() {
            if out.iter().any(|tgid| *tgid == current) {
                continue;
            }
            out.push(current);
            if let Some(kids) = children.get(&current) {
                for kid in kids {
                    queue.push_back(*kid);
                }
            }
        }
        out
    }

    pub fn active_process_group_count_in(&self, groups: &[usize]) -> usize {
        self.processes
            .values()
            .filter(|process| {
                !process.is_thread
                    && process.state != ProcessState::Exited
                    && groups.iter().any(|tgid| *tgid == process.tgid)
            })
            .count()
    }

    pub fn force_exit_subtree(
        &mut self,
        root_tgid: usize,
        code: i32,
    ) -> KernelResult<Vec<GroupExit>> {
        let groups = self.descendant_process_groups(root_tgid);
        if groups.is_empty() {
            return Ok(Vec::new());
        }
        let mut exits = Vec::new();
        for tgid in groups.into_iter().rev() {
            if let Some(exit) = self.force_exit_group(tgid, code)? {
                exits.push(exit);
            }
        }
        Ok(exits)
    }

    pub fn all_blocked_are_futex_waiters(&self, blocked_tids: &[usize]) -> bool {
        if blocked_tids.is_empty() {
            return false;
        }
        blocked_tids.iter().all(|tid| {
            self.processes
                .get(tid)
                .is_some_and(|p| p.futex_wait_addr.is_some())
        })
    }

    pub fn futex_blocked_with_pending_signal_tids(&self) -> Vec<usize> {
        self.processes
            .values()
            .filter(|p| {
                p.futex_wait_addr.is_some()
                    && ((p.pending_signals & !p.signal_mask) != 0
                        || p.signal_frame_pending
                        || p.cancel_signal_seen
                        || p.cancel_interrupt_once)
                    && p.state != ProcessState::Exited
            })
            .map(|p| p.tid)
            .collect()
    }

    pub fn tgids_with_all_threads_futex_blocked(&self) -> Vec<usize> {
        static mut CHECK_COUNT: usize = 0;
        unsafe {
            CHECK_COUNT += 1;
            if CHECK_COUNT % 50 == 0 {
                for b in b"[CHK50]" {
                    hal_api::hal().console.put_byte(*b);
                }
                for p in self.processes.values() {
                    if p.tgid == 124 {
                        for b in alloc::format!(
                            " T{}F{}",
                            p.tid,
                            if p.futex_wait_addr.is_some() { 1 } else { 0 }
                        )
                        .bytes()
                        {
                            hal_api::hal().console.put_byte(b);
                        }
                    }
                }
                for b in b"\n" {
                    hal_api::hal().console.put_byte(*b);
                }
            }
        }

        let mut tgid_thread_counts: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for p in self.processes.values() {
            if p.state == ProcessState::Exited {
                continue;
            }
            let entry = tgid_thread_counts.entry(p.tgid).or_insert((0, 0));
            entry.0 += 1;
            if p.futex_wait_addr.is_some() {
                entry.1 += 1;
            }
        }

        tgid_thread_counts
            .into_iter()
            .filter_map(|(tgid, (total, futex))| {
                if total > 1 && futex == total {
                    if tgid == 124 {
                        for b in b"DEADLOCK-124!\n" {
                            hal_api::hal().console.put_byte(*b);
                        }
                    }
                    Some(tgid)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn tids_in_tgid_with_futex_wait(&self, tgid: usize) -> Vec<usize> {
        self.processes
            .values()
            .filter(|p| p.tgid == tgid && p.futex_wait_addr.is_some())
            .map(|p| p.tid)
            .collect()
    }

    pub fn clear_futex_wait_state(&mut self, tid: usize) {
        let addr = self.processes.get_mut(&tid).and_then(|p| {
            let a = p.futex_wait_addr.take();
            p.futex_wait_deadline_ns = None;
            a
        });
        if let Some(addr) = addr {
            self.remove_futex_waiter_at(addr, tid);
        }
    }

    pub fn wake_all_futex_waiters_in_tgid(&mut self, tgid: usize) -> Vec<usize> {
        let tids = self
            .processes
            .values()
            .filter(|p| {
                p.tgid == tgid && p.futex_wait_addr.is_some() && p.state != ProcessState::Exited
            })
            .map(|p| p.tid)
            .collect::<Vec<_>>();
        for tid in &tids {
            self.clear_futex_wait_state(*tid);
        }
        tids
    }

    pub fn timed_wait_expired_tids(&self, now_ns: u64) -> Vec<usize> {
        self.processes
            .values()
            .filter_map(|process| {
                let sleep_expired = process
                    .sleep_deadline_ns
                    .is_some_and(|deadline| deadline != u64::MAX && now_ns >= deadline);
                let futex_expired = process
                    .futex_wait_deadline_ns
                    .is_some_and(|deadline| deadline != u64::MAX && now_ns >= deadline);
                let epoll_expired = process
                    .epoll_wait_deadline_ns
                    .is_some_and(|deadline| deadline != u64::MAX && now_ns >= deadline);
                (sleep_expired || futex_expired || epoll_expired).then_some(process.tid)
            })
            .collect()
    }

    pub fn force_exit_group(&mut self, tgid: usize, code: i32) -> KernelResult<Option<GroupExit>> {
        let tids = self
            .processes
            .iter()
            .filter_map(|(tid, process)| (process.tgid == tgid).then_some(*tid))
            .collect::<Vec<_>>();
        if tids.is_empty() {
            return Ok(None);
        }
        let parent_tgid = self
            .processes
            .get(&tids[0])
            .and_then(|process| process.parent);
        let vfork_parent_tid = self
            .processes
            .get(&tids[0])
            .and_then(|process| process.vfork_parent_tid);
        let mut clear_child_tids = Vec::new();
        let mut robust_futex_addrs = Vec::new();
        for tid in &tids {
            if let Some(process) = self.processes.get_mut(tid) {
                for addr in process.collect_robust_futex_addrs_for_exit() {
                    push_unique_addr(&mut robust_futex_addrs, addr);
                }
                if let Some(addr) = process.clear_child_tid {
                    let _ = process.write_user_bytes(addr, &0u32.to_le_bytes());
                    clear_child_tids.push(addr);
                }
                process.state = ProcessState::Exited;
                process.exit_code = Some(code);
                process.fds.clear();
                process.address_space.clear();
            }
            self.remove_futex_waiter(*tid);
        }
        if let Some(parent_tgid) = parent_tgid {
            let _ = self.send_signal(parent_tgid, 17);
        }
        Ok(Some(GroupExit {
            tgid,
            tids,
            clear_child_tids,
            robust_futex_addrs,
            parent_tgid,
            vfork_parent_tid,
        }))
    }

    fn next_id(&mut self) -> usize {
        let id = self.next_pid;
        self.next_pid += 1;
        id
    }

    fn reap_thread_group(&mut self, tgid: usize) {
        let tids = self
            .processes
            .iter()
            .filter_map(|(tid, process)| (process.tgid == tgid).then_some(*tid))
            .collect::<Vec<_>>();
        for tid in tids {
            self.processes.remove(&tid);
            self.remove_futex_waiter(tid);
        }
    }

    fn remove_futex_waiter(&mut self, tid: usize) {
        let addrs = self.futex_waiters.keys().copied().collect::<Vec<_>>();
        for addr in addrs {
            if let Some(waiters) = self.futex_waiters.get_mut(&addr) {
                waiters.retain(|waiter| *waiter != tid);
                if waiters.is_empty() {
                    self.futex_waiters.remove(&addr);
                }
            }
        }
    }

    fn find_process_by_pid(&self, pid: usize) -> KernelResult<&Process> {
        self.processes
            .get(&pid)
            .or_else(|| {
                self.processes
                    .values()
                    .find(|process| !process.is_thread && process.tgid == pid)
            })
            .ok_or(ENOENT)
    }

    fn find_process_by_pid_mut(&mut self, pid: usize) -> KernelResult<&mut Process> {
        if self.processes.contains_key(&pid) {
            return self.processes.get_mut(&pid).ok_or(ENOENT);
        }
        let tid = self
            .processes
            .iter()
            .find_map(|(tid, process)| (!process.is_thread && process.tgid == pid).then_some(*tid))
            .ok_or(ENOENT)?;
        self.processes.get_mut(&tid).ok_or(ENOENT)
    }

    fn resolve_target_tid(&self, pid: usize) -> KernelResult<usize> {
        if let Some(process) = self.processes.get(&pid) {
            if process.state != ProcessState::Exited {
                return Ok(pid);
            }
        }
        self.processes
            .values()
            .find(|process| {
                process.tgid == pid && !process.is_thread && process.state != ProcessState::Exited
            })
            .map(|process| process.tid)
            .ok_or(ESRCH)
    }
}

fn selector_from_wait_pid(pid: i32, current_pgid: usize) -> WaitSelector {
    if pid == -1 {
        WaitSelector::Any
    } else if pid == 0 {
        WaitSelector::Pgid(current_pgid)
    } else if pid > 0 {
        WaitSelector::Pid(pid as usize)
    } else {
        WaitSelector::Pgid((-pid) as usize)
    }
}

fn selector_matches(selector: WaitSelector, process: &Process) -> bool {
    match selector {
        WaitSelector::Any => true,
        WaitSelector::Pid(pid) => process.tgid == pid,
        WaitSelector::Pgid(pgid) => process.pgid == pgid,
    }
}

fn read_user_usize(process: &Process, addr: usize) -> KernelResult<usize> {
    let bytes = process
        .read_user_bytes(addr, size_of::<usize>())
        .map_err(|_| EINVAL)?;
    let mut raw = [0u8; size_of::<usize>()];
    raw.copy_from_slice(&bytes[..size_of::<usize>()]);
    Ok(usize::from_le_bytes(raw))
}

fn read_user_isize(process: &Process, addr: usize) -> KernelResult<isize> {
    let bytes = process
        .read_user_bytes(addr, size_of::<isize>())
        .map_err(|_| EINVAL)?;
    let mut raw = [0u8; size_of::<isize>()];
    raw.copy_from_slice(&bytes[..size_of::<isize>()]);
    Ok(isize::from_le_bytes(raw))
}

fn read_user_u32(process: &Process, addr: usize) -> KernelResult<u32> {
    let bytes = process.read_user_bytes(addr, 4).map_err(|_| EINVAL)?;
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[..4]);
    Ok(u32::from_le_bytes(raw))
}

fn robust_futex_addr(node: usize, futex_offset: isize) -> Option<usize> {
    if futex_offset >= 0 {
        node.checked_add(futex_offset as usize)
    } else {
        node.checked_sub((-futex_offset) as usize)
    }
}

fn maybe_mark_owner_died(process: &mut Process, futex_addr: usize, exiting_tid: usize) {
    let Ok(word) = read_user_u32(process, futex_addr) else {
        return;
    };
    if (word & FUTEX_TID_MASK) != ((exiting_tid as u32) & FUTEX_TID_MASK) {
        return;
    }
    let owner_died = (word & FUTEX_WAITERS) | FUTEX_OWNER_DIED;
    let _ = process.write_user_bytes(futex_addr, &owner_died.to_le_bytes());
}

fn push_unique_addr(addrs: &mut Vec<usize>, addr: usize) {
    if !addrs.iter().any(|existing| *existing == addr) {
        addrs.push(addr);
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessTable, SigAction, WaitSelector};

    #[test]
    fn process_accessors_expose_competition_state() {
        let mut table = ProcessTable::new();
        let pid = table.spawn_init("init", 0x1000);
        table.set_current(pid).unwrap();
        table.set_tid_address(0x4000).unwrap();
        table.setgroups_current(&[10, 20]).unwrap();
        table.send_signal(pid, 10).unwrap();
        table
            .set_sigaction(
                10,
                SigAction {
                    handler: 1,
                    flags: 2,
                    restorer: 3,
                    mask: 4,
                },
            )
            .unwrap();

        let process = table.current().unwrap();
        let creds = process.credentials();
        assert_eq!(creds.uid, 0);
        assert_eq!(creds.groups, vec![10, 20]);

        let group = process.process_group();
        assert_eq!(group.pid, pid);
        assert_eq!(group.pgid, pid);

        let signal = process.signal_state();
        assert_eq!(signal.clear_child_tid, Some(0x4000));
        assert_ne!(signal.pending_mask, 0);
        assert_eq!(table.sigaction(10).unwrap().handler, 1);
    }

    #[test]
    fn thread_clone_shares_address_space_and_waits_only_processes() {
        let mut table = ProcessTable::new();
        let leader = table.spawn_init("init", 0x1000);
        table.set_current(leader).unwrap();
        let addr = table
            .current()
            .unwrap()
            .address_space
            .map_anonymous(4096, 0b11)
            .unwrap();
        table
            .current_mut()
            .unwrap()
            .address_space
            .write_bytes(addr, b"x")
            .unwrap();

        let thread = table.clone_thread_from_current(0, None).unwrap();
        table.set_current(thread).unwrap();
        assert_eq!(
            table
                .current()
                .unwrap()
                .address_space
                .read_bytes(addr, 1)
                .unwrap(),
            b"x"
        );
        assert_eq!(table.current().unwrap().tgid, leader);
        assert_ne!(table.gettid().unwrap(), table.current_pid().unwrap());

        table.set_current(leader).unwrap();
        let child = table.fork_process_from_current().unwrap();
        table.set_current(child).unwrap();
        table.exit_current_thread(7).unwrap();
        table.set_current(leader).unwrap();
        let (waited, status) = table
            .wait_child(leader, WaitSelector::Pid(child), 0)
            .unwrap();
        assert_eq!(waited, child);
        assert_eq!(status, 7 << 8);
    }

    #[test]
    fn futex_requeue_moves_waiters() {
        let mut table = ProcessTable::new();
        let leader = table.spawn_init("init", 0x1000);
        table.set_current(leader).unwrap();
        let thread = table.clone_thread_from_current(0, None).unwrap();
        table.enqueue_futex_waiter(0x1000, leader);
        table.enqueue_futex_waiter(0x1000, thread);
        let woke = table.requeue_futex(0x1000, 0x2000, 1, 1);
        assert_eq!(woke, vec![leader]);
        assert_eq!(table.wake_futex(0x2000, 1), vec![thread]);
    }

    #[test]
    fn robust_exit_marks_owner_died_and_reports_wake_addr() {
        let mut table = ProcessTable::new();
        let leader = table.spawn_init("init", 0x1000);
        table.set_current(leader).unwrap();
        let exiting = table.clone_thread_from_current(0, None).unwrap();
        let waiter = table.clone_thread_from_current(0, None).unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .address_space
            .map_anonymous_at(0x2000, 0x2000, 0b11)
            .unwrap();

        const HEAD: usize = 0x2000;
        const NODE: usize = 0x3000;
        const FUTEX: usize = NODE + 0x20;
        let mut head = [0u8; 24];
        head[0..8].copy_from_slice(&NODE.to_le_bytes());
        head[8..16].copy_from_slice(&(0x20isize).to_le_bytes());
        head[16..24].copy_from_slice(&0usize.to_le_bytes());
        let mut node = [0u8; 8];
        node.copy_from_slice(&HEAD.to_le_bytes());

        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(HEAD, &head)
            .unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(NODE, &node)
            .unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(FUTEX, &(exiting as u32).to_le_bytes())
            .unwrap();

        table.set_current(exiting).unwrap();
        table.set_robust_list(HEAD, 24).unwrap();
        table.enqueue_futex_waiter(FUTEX, waiter);
        table.find_by_tid_mut(waiter).unwrap().futex_wait_addr = Some(FUTEX);

        let exit = table.exit_current_thread(0).unwrap();
        assert_eq!(exit.robust_futex_addrs, vec![FUTEX]);

        let futex_word = table
            .find_by_tid_mut(leader)
            .unwrap()
            .read_user_bytes(FUTEX, 4)
            .unwrap();
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&futex_word);
        let value = u32::from_le_bytes(raw);
        assert_ne!(value & super::FUTEX_OWNER_DIED, 0);
        assert_eq!(value & super::FUTEX_TID_MASK, 0);
    }

    #[test]
    fn robust_exit_cycle_scan_is_bounded() {
        let mut table = ProcessTable::new();
        let leader = table.spawn_init("init", 0x1000);
        table.set_current(leader).unwrap();
        let exiting = table.clone_thread_from_current(0, None).unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .address_space
            .map_anonymous_at(0x2000, 0x2000, 0b11)
            .unwrap();

        const HEAD: usize = 0x2100;
        const NODE: usize = 0x3100;
        const FUTEX: usize = NODE + 0x20;
        let mut head = [0u8; 24];
        head[0..8].copy_from_slice(&NODE.to_le_bytes());
        head[8..16].copy_from_slice(&(0x20isize).to_le_bytes());
        head[16..24].copy_from_slice(&0usize.to_le_bytes());
        let mut node = [0u8; 8];
        node.copy_from_slice(&NODE.to_le_bytes());

        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(HEAD, &head)
            .unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(NODE, &node)
            .unwrap();
        table
            .find_by_tid_mut(exiting)
            .unwrap()
            .write_user_bytes(FUTEX, &(exiting as u32).to_le_bytes())
            .unwrap();

        table.set_current(exiting).unwrap();
        table.set_robust_list(HEAD, 24).unwrap();
        let exit = table.exit_current_thread(0).unwrap();
        assert_eq!(exit.robust_futex_addrs, vec![FUTEX]);
    }

    #[test]
    fn descendant_cleanup_reaps_process_subtree() {
        let mut table = ProcessTable::new();
        let init = table.spawn_init("init", 0x1000);
        let shell = table.spawn("shell", Some(init), 0x1000);
        let worker = table.spawn("worker", Some(shell), 0x1000);
        let helper = table.spawn("helper", Some(worker), 0x1000);

        assert!(table.has_child_process_group(shell));
        let descendants = table.descendant_process_groups(shell);
        assert!(descendants.contains(&shell));
        assert!(descendants.contains(&worker));
        assert!(descendants.contains(&helper));

        let exits = table.force_exit_subtree(shell, 124).unwrap();
        assert_eq!(exits.len(), 3);
        assert_eq!(
            table.active_process_group_count_in(&[shell, worker, helper]),
            0
        );
        assert!(table.descendant_process_groups(shell).is_empty());
    }

    #[test]
    fn cancel_interrupt_token_is_one_shot() {
        let mut table = ProcessTable::new();
        let pid = table.spawn_init("init", 0x1000);
        table.set_current(pid).unwrap();
        let process = table.current_mut().unwrap();

        process.mark_cancel_signal_dispatched();
        assert!(process.cancel_signal_seen);
        assert!(!process.cancel_interrupt_once);

        process.arm_cancel_interrupt_once();
        assert!(!process.cancel_signal_seen);
        assert!(process.cancel_interrupt_once);
        assert!(process.consume_cancel_interrupt_once());
        assert!(!process.consume_cancel_interrupt_once());

        process.mark_cancel_signal_dispatched();
        process.arm_cancel_interrupt_once();
        process.reset_image(0x2000, None);
        assert!(!process.cancel_signal_seen);
        assert!(!process.cancel_interrupt_once);
    }
}
