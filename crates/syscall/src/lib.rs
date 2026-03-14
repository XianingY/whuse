#![cfg_attr(not(test), no_std)]

extern crate alloc;
mod fs_domain;
mod io_mpx_domain;
mod ipc_domain;
mod mm_domain;
mod net_domain;
mod resources_domain;
mod signal_domain;
mod sys_domain;
mod task_domain;
mod time_domain;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use hal_api::{hal, Timespec};
use mm::{BinaryLoader, ElfBinaryLoader};
use proc::{ProcessTable, SigAction, WaitSelector};
use spin::Mutex;
use task::Scheduler;
use user_init::builtin_program;
use vfs::{FileStat, KernelVfs, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};

const EAFNOSUPPORT: i32 = 97;
const EAGAIN: i32 = 11;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENOENT: i32 = 2;
const ENOSYS: i32 = 38;
const EPROTOTYPE: i32 = 91;
const ENOEXEC: i32 = 8;

pub const SYS_EVENTFD2: usize = 19;
pub const SYS_EPOLL_CREATE1: usize = 20;
pub const SYS_EPOLL_CTL: usize = 21;
pub const SYS_EPOLL_PWAIT: usize = 22;
pub const SYS_GETCWD: usize = 17;
pub const SYS_DUP: usize = 23;
pub const SYS_DUP2: usize = 24;
pub const SYS_DUP3: usize = 24;
pub const SYS_FCNTL: usize = 25;
pub const SYS_IOCTL: usize = 29;
pub const SYS_FLOCK: usize = 32;
pub const SYS_MKDIR: usize = 34;
pub const SYS_MKDIRAT: usize = 34;
pub const SYS_UNLINKAT: usize = 35;
pub const SYS_SYMLINKAT: usize = 36;
pub const SYS_LINKAT: usize = 37;
pub const SYS_RENAMEAT: usize = 38;
pub const SYS_UMOUNT2: usize = 39;
pub const SYS_MOUNT: usize = 40;
pub const SYS_STATFS: usize = 43;
pub const SYS_FSTATFS: usize = 44;
pub const SYS_TRUNCATE: usize = 45;
pub const SYS_FTRUNCATE: usize = 46;
pub const SYS_FALLOCATE: usize = 47;
pub const SYS_FACCESSAT: usize = 48;
pub const SYS_CHDIR: usize = 49;
pub const SYS_FCHDIR: usize = 50;
pub const SYS_CHROOT: usize = 51;
pub const SYS_FCHMOD: usize = 52;
pub const SYS_FCHMODAT: usize = 53;
pub const SYS_FCHOWNAT: usize = 54;
pub const SYS_FCHOWN: usize = 55;
pub const SYS_OPENAT: usize = 56;
pub const SYS_CLOSE: usize = 57;
pub const SYS_PIPE: usize = 59;
pub const SYS_PIPE2: usize = 59;
pub const SYS_GETDENTS64: usize = 61;
pub const SYS_LSEEK: usize = 62;
pub const SYS_READ: usize = 63;
pub const SYS_WRITE: usize = 64;
pub const SYS_READV: usize = 65;
pub const SYS_WRITEV: usize = 66;
pub const SYS_PREAD64: usize = 67;
pub const SYS_PWRITE64: usize = 68;
pub const SYS_PREADV: usize = 69;
pub const SYS_PWRITEV: usize = 70;
pub const SYS_SENDFILE: usize = 71;
pub const SYS_PSELECT6: usize = 72;
pub const SYS_PPOLL: usize = 73;
pub const SYS_SPLICE: usize = 76;
pub const SYS_READLINKAT: usize = 78;
pub const SYS_FSTATAT: usize = 79;
pub const SYS_FSTAT: usize = 80;
pub const SYS_SYNC: usize = 81;
pub const SYS_FSYNC: usize = 82;
pub const SYS_FDATASYNC: usize = 83;
pub const SYS_UTIMENSAT: usize = 88;
pub const SYS_SET_TID_ADDRESS: usize = 96;
pub const SYS_FUTEX: usize = 98;
pub const SYS_SET_ROBUST_LIST: usize = 99;
pub const SYS_GET_ROBUST_LIST: usize = 100;
pub const SYS_SLEEP: usize = 101;
pub const SYS_NANOSLEEP: usize = 101;
pub const SYS_GETITIMER: usize = 102;
pub const SYS_SETITIMER: usize = 103;
pub const SYS_CLOCK_GETRES: usize = 114;
pub const SYS_SCHED_YIELD: usize = 124;
pub const SYS_KILL: usize = 129;
pub const SYS_TGKILL: usize = 131;
pub const SYS_SIGALTSTACK: usize = 132;
pub const SYS_RT_SIGSUSPEND: usize = 133;
pub const SYS_SIGACTION: usize = 134;
pub const SYS_SIGPROCMASK: usize = 135;
pub const SYS_RT_SIGPENDING: usize = 136;
pub const SYS_RT_SIGTIMEDWAIT: usize = 137;
pub const SYS_RT_SIGRETURN: usize = 139;
pub const SYS_GETPRIORITY: usize = 141;
pub const SYS_SETGID: usize = 144;
pub const SYS_SETUID: usize = 146;
pub const SYS_CLOCK_GETTIME: usize = 113;
pub const SYS_CLOCK_NANOSLEEP: usize = 115;
pub const SYS_SYSLOG: usize = 116;
pub const SYS_TIMES: usize = 153;
pub const SYS_SETPGID: usize = 154;
pub const SYS_GETPGID: usize = 155;
pub const SYS_GETSID: usize = 156;
pub const SYS_SETSID: usize = 157;
pub const SYS_GETGROUPS: usize = 158;
pub const SYS_SETGROUPS: usize = 159;
pub const SYS_UNAME: usize = 160;
pub const SYS_UMASK: usize = 166;
pub const SYS_PRCTL: usize = 167;
pub const SYS_GETRUSAGE: usize = 165;
pub const SYS_GETTIMEOFDAY: usize = 169;
pub const SYS_EXIT: usize = 93;
pub const SYS_EXIT_GROUP: usize = 94;
pub const SYS_GETPID: usize = 172;
pub const SYS_GETPPID: usize = 173;
pub const SYS_GETUID: usize = 174;
pub const SYS_GETEUID: usize = 175;
pub const SYS_GETGID: usize = 176;
pub const SYS_GETEGID: usize = 177;
pub const SYS_GETTID: usize = 178;
pub const SYS_SYSINFO: usize = 179;
pub const SYS_SHMGET: usize = 194;
pub const SYS_SHMCTL: usize = 195;
pub const SYS_SHMAT: usize = 196;
pub const SYS_SHMDT: usize = 197;
pub const SYS_SOCKET: usize = 198;
pub const SYS_SOCKETPAIR: usize = 199;
pub const SYS_BIND: usize = 200;
pub const SYS_LISTEN: usize = 201;
pub const SYS_ACCEPT: usize = 202;
pub const SYS_CONNECT: usize = 203;
pub const SYS_GETSOCKNAME: usize = 204;
pub const SYS_GETPEERNAME: usize = 205;
pub const SYS_SENDTO: usize = 206;
pub const SYS_RECVFROM: usize = 207;
pub const SYS_SETSOCKOPT: usize = 208;
pub const SYS_GETSOCKOPT: usize = 209;
pub const SYS_SHUTDOWN: usize = 210;
pub const SYS_SENDMSG: usize = 211;
pub const SYS_RECVMSG: usize = 212;
pub const SYS_BRK: usize = 214;
pub const SYS_MREMAP: usize = 216;
pub const SYS_CLONE: usize = 220;
pub const SYS_FORK: usize = 220;
pub const SYS_EXECVE: usize = 221;
pub const SYS_MUNMAP: usize = 215;
pub const SYS_MMAP: usize = 222;
pub const SYS_MPROTECT: usize = 226;
pub const SYS_MSYNC: usize = 227;
pub const SYS_MLOCK: usize = 228;
pub const SYS_MADVISE: usize = 233;
pub const SYS_ACCEPT4: usize = 242;
pub const SYS_WAIT: usize = 260;
pub const SYS_RISCV_FLUSH_ICACHE: usize = 259;
pub const SYS_PRLIMIT64: usize = 261;
pub const SYS_RENAMEAT2: usize = 276;
pub const SYS_GETRANDOM: usize = 278;
pub const SYS_MEMFD_CREATE: usize = 279;
pub const SYS_MLOCK2: usize = 284;
pub const SYS_COPY_FILE_RANGE: usize = 285;
pub const SYS_PREADV2: usize = 286;
pub const SYS_PWRITEV2: usize = 287;
pub const SYS_STATX: usize = 291;
pub const SYS_PIDFD_SEND_SIGNAL: usize = 424;
pub const SYS_CLONE3: usize = 435;
pub const SYS_PIDFD_OPEN: usize = 434;
pub const SYS_CLOSE_RANGE: usize = 436;
pub const SYS_PIDFD_GETFD: usize = 438;
pub const SYS_FACCESSAT2: usize = 439;
pub const SYS_EPOLL_PWAIT2: usize = 441;
pub const SYS_FCHMODAT2: usize = 452;
pub const SYS_SECCOMP: usize = 277;
pub const SYS_POWER_OFF: usize = 2024;

const CLONE_VM: usize = 0x0000_0100;
const CLONE_FS: usize = 0x0000_0200;
const CLONE_FILES: usize = 0x0000_0400;
const CLONE_SIGHAND: usize = 0x0000_0800;
const CLONE_VFORK: usize = 0x0000_4000;
const CLONE_THREAD: usize = 0x0001_0000;
const CLONE_SETTLS: usize = 0x0008_0000;
const CLONE_PARENT_SETTID: usize = 0x0010_0000;
const CLONE_CHILD_CLEARTID: usize = 0x0020_0000;
const CLONE_CHILD_SETTID: usize = 0x0100_0000;
const CLONE_NAMESPACE_MASK: usize = 0x0002_0000
    | 0x0004_0000
    | 0x0200_0000
    | 0x0400_0000
    | 0x0800_0000
    | 0x1000_0000
    | 0x2000_0000
    | 0x4000_0000;
const BUILTIN_EXEC_BASE: usize = 0x0080_0000;

fn trace_line(line: &str) {
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

#[derive(Clone, Copy, Debug)]
pub struct SyscallArgs(pub [usize; 6]);

pub struct SyscallDispatcher;

pub(crate) struct DispatchContext<'a> {
    pub dispatcher: &'a SyscallDispatcher,
    pub procs: &'a mut ProcessTable,
    pub scheduler: &'a mut Scheduler,
    pub vfs: &'a mut KernelVfs,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct IoVec {
    iov_base: usize,
    iov_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct EpollEvent {
    events: u32,
    _pad: u32,
    data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct MsgHdr {
    msg_name: usize,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: u32,
    _pad1: u32,
}

#[derive(Default)]
struct SharedMemoryState {
    next_id: usize,
    keys: BTreeMap<i32, usize>,
    segments: BTreeMap<usize, Vec<u8>>,
}

static SHM_STATE: Mutex<SharedMemoryState> = Mutex::new(SharedMemoryState {
    next_id: 1,
    keys: BTreeMap::new(),
    segments: BTreeMap::new(),
});

static BUSYBOX_IMAGE_CACHE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

pub fn cache_busybox_image(image: &[u8]) {
    *BUSYBOX_IMAGE_CACHE.lock() = Some(image.to_vec());
}

fn busybox_image_cache() -> Option<Vec<u8>> {
    BUSYBOX_IMAGE_CACHE.lock().as_ref().cloned()
}

impl SyscallDispatcher {
    pub fn new() -> Self {
        Self
    }

    pub fn dispatch(
        &self,
        sysno: usize,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> isize {
        let mut ctx = DispatchContext {
            dispatcher: self,
            procs,
            scheduler,
            vfs,
        };
        let result = fs_domain::dispatch(&mut ctx, sysno, args)
            .or_else(|| io_mpx_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| ipc_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| mm_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| net_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| resources_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| signal_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| sys_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| task_domain::dispatch(&mut ctx, sysno, args))
            .or_else(|| time_domain::dispatch(&mut ctx, sysno, args))
            .unwrap_or(Err(ENOSYS));

        match result {
            Ok(value) => value as isize,
            Err(errno) => -(errno as isize),
        }
    }

    fn sys_getcwd(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let buf = args.0[0];
        let size = args.0[1];
        let process = procs.current_mut()?;
        let cwd = process.cwd.clone();
        trace_line(&format!(
            "whuse: getcwd tgid={} name={} cwd={} size={}",
            process.tgid, process.name, cwd, size
        ));
        let bytes = cwd.as_bytes();
        if bytes.len() + 1 > size {
            return Err(EINVAL);
        }
        let mut out = bytes.to_vec();
        out.push(0);
        process.write_user_bytes(buf, &out).map_err(|_| EFAULT)?;
        Ok(buf)
    }

    fn sys_mkdir(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        const AT_FDCWD: isize = -100;
        let first = args.0[0] as isize;
        let looks_like_dirfd = first == AT_FDCWD || (0..=1024).contains(&first);
        let (path_arg, mode) = if looks_like_dirfd {
            (args.0[1], args.0[2] as u32)
        } else {
            (args.0[0], args.0[1] as u32)
        };
        let path = procs
            .current()?
            .read_user_cstr(path_arg)
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.mkdir(&cwd, &path, mode)?;
        Ok(0)
    }

    fn sys_unlinkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _ = args.0[0];
        let _ = args.0[2];
        vfs.unlink(&cwd, &path)?;
        Ok(0)
    }

    fn sys_mount(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let source = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let target = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let fs_type = procs
            .current()?
            .read_user_cstr(args.0[2])
            .map_err(|_| EFAULT)?;
        let _ = args.0[3];
        let _ = args.0[4];
        vfs.mount(&source, &target, &fs_type)?;
        Ok(0)
    }

    fn sys_umount(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let target = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let _ = args.0[1];
        vfs.umount(&target)?;
        Ok(0)
    }

    fn sys_openat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let flags = normalize_open_flags(args.0[2] as u32);
        let mode = args.0[3] as u32;
        let cwd = procs.current()?.cwd.clone();
        let handle = vfs.open(&cwd, &path, flags, mode)?;
        let fd = procs.current_mut()?.add_fd(handle);
        Ok(fd as usize)
    }

    fn sys_close(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.current_mut()?.close_fd(args.0[0] as i32)?;
        Ok(0)
    }

    fn sys_getdents64(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let bytes = {
            let process = procs.current_mut()?;
            let handle = process.fd_mut(fd)?;
            vfs.getdents(handle)?
        };
        let trimmed = &bytes[..bytes.len().min(count)];
        procs
            .current_mut()?
            .write_user_bytes(buf, trimmed)
            .map_err(|_| EFAULT)?;
        Ok(trimmed.len())
    }

    fn sys_lseek(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let offset = args.0[1] as isize;
        let whence = args.0[2] as u32;
        let process = procs.current_mut()?;
        let handle = process.fd_mut(args.0[0] as i32)?;
        vfs.seek(handle, offset, whence)
    }

    fn sys_read(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let bytes = {
            let process = procs.current_mut()?;
            let handle = process.fd_mut(fd)?;
            vfs.read(handle, count)?
        };
        procs
            .current_mut()?
            .write_user_bytes(buf, &bytes)
            .map_err(|_| EFAULT)?;
        Ok(bytes.len())
    }

    fn sys_write(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let data = procs
            .current()?
            .read_user_bytes(buf, count)
            .map_err(|_| EFAULT)?;
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.write(handle, &data)
    }

    fn sys_sched_yield(&self, scheduler: &mut Scheduler) -> Result<usize, i32> {
        let _ = scheduler.yield_now();
        Ok(0)
    }

    fn sys_set_tid_address(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
    ) -> Result<usize, i32> {
        procs.set_tid_address(args.0[0])
    }

    fn sys_set_robust_list(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
    ) -> Result<usize, i32> {
        procs.set_robust_list(args.0[0], args.0[1])?;
        Ok(0)
    }

    fn sys_clock_gettime(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _clock_id = args.0[0];
        let buf = args.0[1];
        let ts = hal().timer.monotonic_time();
        procs
            .current_mut()?
            .write_user_bytes(buf, &timespec_to_bytes(ts))
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_nanosleep(&self, args: SyscallArgs) -> Result<usize, i32> {
        let requested = args.0[0];
        let _remaining = args.0[1];
        hal().timer.program_oneshot(requested as u64);
        Ok(0)
    }

    fn sys_exit(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        exit_group: bool,
    ) -> Result<usize, i32> {
        let exit = if exit_group {
            procs.exit_current_process_group(args.0[0] as i32)?
        } else {
            procs.exit_current_thread(args.0[0] as i32)?
        };
        if exit_group {
            scheduler.exit_group(exit.tgid);
            if let Some(parent_tgid) = exit.parent_tgid {
                let woke = scheduler.wake_task(parent_tgid);
                trace_line(&format!(
                    "whuse: exit_group wake parent_tgid={} woke={}",
                    parent_tgid, woke
                ));
            }
        } else {
            scheduler.remove_task(exit.tid);
            if exit.group_exited {
                scheduler.exit_group(exit.tgid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let woke = scheduler.wake_task(parent_tgid);
                trace_line(&format!(
                    "whuse: exit wake parent_tgid={} woke={}",
                    parent_tgid, woke
                ));
            }
            if let Some(addr) = exit.clear_child_tid {
                for tid in procs.wake_futex(addr, usize::MAX) {
                    let _ = scheduler.wake_task(tid);
                }
            }
        }
        Ok(0)
    }

    fn sys_getpid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        procs.current_pid()
    }

    fn sys_getppid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        procs.getppid()
    }

    fn sys_gettid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        procs.gettid()
    }

    fn sys_getuid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        Ok(procs.current()?.uid as usize)
    }

    fn sys_geteuid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        Ok(procs.current()?.euid as usize)
    }

    fn sys_getgid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        Ok(procs.current()?.gid as usize)
    }

    fn sys_getegid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        Ok(procs.current()?.egid as usize)
    }

    fn sys_fstat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let stat_ptr = args.0[1];
        let stat = {
            let process = procs.current()?;
            let handle = process.fd(fd)?;
            vfs.stat_handle(handle)?
        };
        procs
            .current_mut()?
            .write_user_bytes(stat_ptr, &stat_to_bytes(stat))
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_chdir(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let new_cwd = vfs.chdir(&cwd, &path)?;
        procs.current_mut()?.cwd = new_cwd;
        Ok(0)
    }

    fn sys_brk(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let requested = args.0[0];
        let process = procs.current_mut()?;
        process
            .address_space
            .brk((requested != 0).then_some(requested))
    }

    fn sys_clone(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let name = procs.current()?.name.clone();
        let parent_pid = procs.current_pid()?;
        let flags = args.0[0];
        if flags & CLONE_NAMESPACE_MASK != 0 || flags & CLONE_VFORK != 0 {
            return Err(EINVAL);
        }
        if (flags & CLONE_THREAD) != 0 {
            let required = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
            if flags & required != required {
                return Err(EINVAL);
            }
            let stack = args.0[1];
            let parent_tid = args.0[2];
            let tls = ((flags & CLONE_SETTLS) != 0).then_some(args.0[4]);
            let child_tid_ptr = args.0[3];
            let tid = procs.clone_thread_from_current(stack, tls)?;
            if flags & CLONE_PARENT_SETTID != 0 && parent_tid != 0 {
                procs
                    .current_mut()?
                    .write_user_bytes(parent_tid, &(tid as u32).to_le_bytes())
                    .map_err(|_| EFAULT)?;
            }
            let current_tid = procs.current_tid()?;
            procs.set_current(tid)?;
            if flags & CLONE_CHILD_SETTID != 0 && child_tid_ptr != 0 {
                procs
                    .current_mut()?
                    .write_user_bytes(child_tid_ptr, &(tid as u32).to_le_bytes())
                    .map_err(|_| EFAULT)?;
            }
            if flags & CLONE_CHILD_CLEARTID != 0 && child_tid_ptr != 0 {
                procs.set_clear_child_tid(Some(child_tid_ptr))?;
            }
            let tgid = procs.current_tgid()?;
            procs.set_current(current_tid)?;
            scheduler.spawn(&name, tid, tgid);
            return Ok(tid);
        }

        let child_stack = args.0[1];
        let use_shared_fork = flags == 0x11 && parent_pid == 1;
        let pid = if use_shared_fork {
            procs.fork_process_from_current_shared()?
        } else {
            procs.fork_process_from_current()?
        };
        trace_line(&format!(
            "whuse: clone parent_tgid={} flags={:#x} child_tgid={}",
            parent_pid, flags, pid
        ));
        if child_stack != 0 {
            procs.set_thread_stack_pointer(pid, child_stack)?;
        }
        scheduler.spawn(&name, pid, pid);
        Ok(pid)
    }

    fn sys_execve(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let mut path = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        trace_line(&format!(
            "whuse: execve enter tgid={} path={}",
            procs.current_tgid().unwrap_or(0),
            path
        ));
        let cwd = procs.current()?.cwd.clone();
        let mut argv = read_string_vector(procs.current()?, args.0[1])?;
        if argv.is_empty() {
            argv.push(path.clone());
        }
        let envp = read_string_vector(procs.current()?, args.0[2])?;
        let mut shebang_hops = 0usize;
        loop {
            let is_busybox = path.contains("busybox");
            let mut file_data = if is_busybox {
                if let Some(cached) = busybox_image_cache() {
                    cached
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            if file_data.is_empty() {
                let mut handle = vfs.open(&cwd, &path, O_RDONLY, 0).map_err(|_| ENOENT)?;
                const EXEC_READ_CHUNK: usize = 256 * 1024;
                const EXEC_READ_LIMIT: usize = 32 * 1024 * 1024;
                loop {
                    let remaining = EXEC_READ_LIMIT.saturating_sub(file_data.len());
                    if remaining == 0 {
                        return Err(EFAULT);
                    }
                    let read_len = EXEC_READ_CHUNK.min(remaining);
                    let chunk = match vfs.read(&mut handle, read_len) {
                        Ok(chunk) => chunk,
                        Err(_) => return Err(EFAULT),
                    };
                    if chunk.is_empty() {
                        break;
                    }
                    file_data.extend_from_slice(&chunk);
                }
                trace_line(&format!(
                    "whuse: execve read image tgid={} path={} bytes={}",
                    procs.current_tgid().unwrap_or(0),
                    path,
                    file_data.len()
                ));
            }
            if file_data.is_empty() {
                return Err(EFAULT);
            }
            if is_busybox {
                *BUSYBOX_IMAGE_CACHE.lock() = Some(file_data.clone());
            }
            if let Some((mut interp_path, mut interp_arg)) = parse_shebang_line(&file_data) {
                if interp_path == "/bin/sh" {
                    interp_path = "/musl/busybox".to_string();
                    if interp_arg.is_none() {
                        interp_arg = Some("sh".to_string());
                    }
                }
                if shebang_hops >= 4 {
                    return Err(ENOEXEC);
                }
                let mut next_argv = Vec::new();
                next_argv.push(interp_path.clone());
                if let Some(arg) = interp_arg {
                    next_argv.push(arg);
                }
                next_argv.push(path.clone());
                if argv.len() > 1 {
                    next_argv.extend_from_slice(&argv[1..]);
                }
                path = interp_path;
                argv = next_argv;
                shebang_hops += 1;
                continue;
            }
            if let Some(program) = resolve_exec_payload(&file_data).or_else(|| builtin_program(&path))
            {
                let entry = BUILTIN_EXEC_BASE + program.entry;
                procs.execve_current_image(entry, None)?;
                let process = procs.current_mut()?;
                process.name = path.clone();
                process
                    .address_space
                    .map_fixed_bytes(BUILTIN_EXEC_BASE, program.image, program.image.len(), 0b101)
                    .map_err(|_| EFAULT)?;
                process.trap_frame.sepc = entry;
                let tgid = process.tgid;
                #[cfg(target_arch = "riscv64")]
                unsafe {
                    core::arch::asm!("fence.i");
                }
                trace_line(&format!(
                    "whuse: execve builtin tgid={} path={} entry={:#x}",
                    tgid,
                    path,
                    entry
                ));
                return Ok(0);
            }
            procs.execve_current_image(0, None)?;
            let loaded = {
                let process = procs.current_mut()?;
                match ElfBinaryLoader::new().load(&process.address_space, &file_data, &argv, &envp) {
                    Ok(loaded) => loaded,
                    Err(err) => return Err(if err == ENOEXEC { ENOEXEC } else { EFAULT }),
                }
            };
            let process = procs.current_mut()?;
            process.trap_frame.sepc = loaded.entry;
            process.trap_frame.regs[2] = loaded.stack_pointer;
            process.name = path;
            let tgid = process.tgid;
            let proc_name = process.name.clone();
            let entry = process.trap_frame.sepc;
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("fence.i");
            }
            trace_line(&format!(
                "whuse: execve elf tgid={} path={} entry={:#x}",
                tgid, proc_name, entry
            ));
            return Ok(0);
        }
    }

    fn sys_mmap(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let _addr = args.0[0];
        let len = args.0[1];
        let prot = args.0[2];
        let _flags = args.0[3];
        let fd = args.0[4] as isize;
        let offset = args.0[5];
        let base = procs.current_mut()?.address_space.map_anonymous(len, prot)?;
        if fd >= 0 {
            let mut handle = procs.current()?.fd(fd as i32)?.clone();
            handle.offset = offset;
            let data = vfs.read(&mut handle, len)?;
            procs
                .current_mut()?
                .address_space
                .write_bytes(base, &data)
                .map_err(|_| EFAULT)?;
        }
        Ok(base)
    }

    fn sys_munmap(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let addr = args.0[0];
        let len = args.0[1];
        procs.current_mut()?.address_space.unmap(addr, len)?;
        Ok(0)
    }

    fn sys_mprotect(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let addr = args.0[0];
        let len = args.0[1];
        let prot = args.0[2];
        procs
            .current_mut()?
            .address_space
            .mprotect(addr, len, prot)?;
        Ok(0)
    }

    fn sys_wait(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        const WNOHANG: u32 = 1;
        let wait_pid = args.0[0] as i32;
        let status_ptr = args.0[1];
        let options = args.0[2] as u32;
        let _rusage = args.0[3];
        let parent_pid = procs.current_pid()?;
        trace_line(&format!(
            "whuse: wait enter tgid={} wait_pid={} status_ptr={:#x} options={:#x}",
            parent_pid, wait_pid, status_ptr, options
        ));
        let selector = selector_from_wait(wait_pid, procs.getpgid(0)?);
        let (child_pid, status) = match procs.wait_child(parent_pid, selector, options) {
            Ok(pair) => pair,
            Err(err) => {
                trace_line(&format!(
                    "whuse: wait error tgid={} err={}",
                    parent_pid, err
                ));
                return Err(err);
            }
        };
        if child_pid == 0 {
            if options & WNOHANG != 0 {
                trace_line(&format!("whuse: wait return tgid={} child=0 wnohang", parent_pid));
                return Ok(0);
            }
            trace_line(&format!("whuse: wait blocking tgid={}", parent_pid));
            let _ = scheduler.block_current();
            return Err(EAGAIN);
        }
        if status_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(status_ptr, &(status as i32).to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        trace_line(&format!(
            "whuse: wait return tgid={} child={} status={}",
            parent_pid, child_pid, status
        ));
        Ok(child_pid)
    }

    fn sys_dup(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let handle = procs.current()?.fd(args.0[0] as i32)?.clone();
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_dup3(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let oldfd = args.0[0] as i32;
        let newfd = args.0[1] as i32;
        let _flags = args.0[2];
        if oldfd == newfd {
            let _ = procs.current()?.fd(oldfd)?;
            return Ok(newfd as usize);
        }
        let handle = procs.current()?.fd(oldfd)?.clone();
        let process = procs.current_mut()?;
        process.fds.insert(newfd, handle);
        Ok(newfd as usize)
    }

    fn sys_fcntl(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        const F_DUPFD: usize = 0;
        const F_GETFD: usize = 1;
        const F_SETFD: usize = 2;
        const F_GETFL: usize = 3;
        const F_SETFL: usize = 4;
        const F_DUPFD_CLOEXEC: usize = 1030;

        let fd = args.0[0] as i32;
        let cmd = args.0[1];
        let arg = args.0[2] as i32;
        match cmd {
            F_DUPFD | F_DUPFD_CLOEXEC => {
                let handle = procs.current()?.fd(fd)?.clone();
                let process = procs.current_mut()?;
                let mut newfd = arg.max(0);
                while process.fds.contains_key(&newfd) {
                    newfd += 1;
                }
                process.fds.insert(newfd, handle);
                Ok(newfd as usize)
            }
            F_GETFD => Ok(0),
            F_SETFD => Ok(0),
            F_GETFL => Ok(procs.current()?.fd(fd)?.flags as usize),
            F_SETFL => Ok(0),
            _ => Err(EINVAL),
        }
    }

    fn sys_ioctl(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        const TIOCGWINSZ: usize = 0x5413;
        let _fd = args.0[0] as i32;
        let cmd = args.0[1];
        let arg = args.0[2];
        if cmd == TIOCGWINSZ && arg != 0 {
            let mut winsz = [0u8; 8];
            winsz[..2].copy_from_slice(&24u16.to_le_bytes());
            winsz[2..4].copy_from_slice(&80u16.to_le_bytes());
            procs
                .current_mut()?
                .write_user_bytes(arg, &winsz)
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_flock(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _ = procs.current()?.fd(args.0[0] as i32)?;
        let _ = args.0[1];
        Ok(0)
    }

    fn sys_pipe(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let (read_end, write_end) = vfs.create_pipe()?;
        let process = procs.current_mut()?;
        let read_fd = process.add_fd(read_end);
        let write_fd = process.add_fd(write_end);
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&read_fd.to_le_bytes());
        bytes[4..].copy_from_slice(&write_fd.to_le_bytes());
        process
            .write_user_bytes(args.0[0], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_readv(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let mut total = 0;
        for iov in iovecs {
            let bytes = {
                let process = procs.current_mut()?;
                let handle = process.fd_mut(fd)?;
                vfs.read(handle, iov.iov_len)?
            };
            procs
                .current_mut()?
                .write_user_bytes(iov.iov_base, &bytes)
                .map_err(|_| EFAULT)?;
            total += bytes.len();
            if bytes.len() < iov.iov_len {
                break;
            }
        }
        Ok(total)
    }

    fn sys_writev(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let mut total = 0;
        for iov in iovecs {
            let bytes = procs
                .current()?
                .read_user_bytes(iov.iov_base, iov.iov_len)
                .map_err(|_| EFAULT)?;
            if fd == 1 || fd == 2 {
                for byte in bytes.iter().copied() {
                    hal().console.put_byte(byte);
                }
                total += bytes.len();
            } else {
                let written = {
                    let process = procs.current_mut()?;
                    let handle = process.fd_mut(fd)?;
                    vfs.write(handle, &bytes)?
                };
                total += written;
            }
        }
        Ok(total)
    }

    fn sys_pread64(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let offset = args.0[3];
        let mut handle = procs.current()?.fd(fd)?.clone();
        handle.offset = offset;
        let bytes = vfs.read(&mut handle, count)?;
        procs
            .current_mut()?
            .write_user_bytes(buf, &bytes)
            .map_err(|_| EFAULT)?;
        Ok(bytes.len())
    }

    fn sys_sendfile(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let out_fd = args.0[0] as i32;
        let in_fd = args.0[1] as i32;
        let off_ptr = args.0[2];
        let count = args.0[3];
        let mut in_handle = procs.current()?.fd(in_fd)?.clone();
        if off_ptr != 0 {
            let offset = read_usize(procs.current()?, off_ptr)?;
            in_handle.offset = offset;
        }
        let bytes = vfs.read(&mut in_handle, count)?;
        let written = {
            let process = procs.current_mut()?;
            let handle = process.fd_mut(out_fd)?;
            vfs.write(handle, &bytes)?
        };
        if off_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(off_ptr, &in_handle.offset.to_le_bytes())
                .map_err(|_| EFAULT)?;
        } else {
            procs.current_mut()?.fd_mut(in_fd)?.offset = in_handle.offset;
        }
        Ok(written)
    }

    fn sys_ppoll(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let addr = args.0[0];
        let nfds = args.0[1];
        let mut pollfds = read_pollfds(procs.current()?, addr, nfds)?;
        let mut ready = 0usize;
        for pollfd in &mut pollfds {
            pollfd.revents = 0;
            if pollfd.fd < 0 {
                continue;
            }
            if procs.current()?.fd(pollfd.fd).is_ok() {
                pollfd.revents = pollfd.events;
                if pollfd.revents != 0 {
                    ready += 1;
                }
            }
        }
        procs
            .current_mut()?
            .write_user_bytes(addr, &pollfds_to_bytes(&pollfds))
            .map_err(|_| EFAULT)?;
        Ok(ready)
    }

    fn sys_pselect6(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _nfds = args.0[0];
        let _readfds = args.0[1];
        let _writefds = args.0[2];
        let _exceptfds = args.0[3];
        let _timeout = args.0[4];
        let _sigmask = args.0[5];
        let _ = procs.current()?;
        Ok(0)
    }

    fn sys_splice(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        self.sys_copy_between_fds(
            args.0[0] as i32,
            args.0[2] as i32,
            args.0[4],
            args.0[1],
            args.0[3],
            procs,
            vfs,
        )
    }

    fn sys_readlinkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let target = match path.as_str() {
            "/proc/self/exe" => String::from("/bin/init"),
            "/proc/self/cwd" => cwd.clone(),
            _ => vfs.read_link(&cwd, &path)?,
        };
        let bytes = target.as_bytes();
        let len = bytes.len().min(args.0[3]);
        procs
            .current_mut()?
            .write_user_bytes(args.0[2], &bytes[..len])
            .map_err(|_| EFAULT)?;
        Ok(len)
    }

    fn sys_fstatat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let stat = vfs.stat_path(&cwd, &path)?;
        procs
            .current_mut()?
            .write_user_bytes(args.0[2], &stat_to_bytes(stat))
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_statfs(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.access(&cwd, &path)?;
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &statfs_bytes())
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_faccessat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _mode = args.0[2];
        let _flags = args.0[3];
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_kill(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let pid = if args.0[0] == 0 {
            procs.current_pid()?
        } else {
            args.0[0]
        };
        let sig = args.0[1];
        if sig > 64 {
            return Err(EINVAL);
        }
        if sig != 0 {
            procs.deliver_signal(pid, sig)?;
        }
        Ok(0)
    }

    fn sys_sigaction(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let sig = args.0[0];
        let new = args.0[1];
        let old = args.0[2];
        if old != 0 {
            let current = procs.sigaction(sig)?;
            procs
                .current_mut()?
                .write_user_bytes(old, &sigaction_to_bytes(current))
                .map_err(|_| EFAULT)?;
        }
        if new != 0 {
            procs.set_sigaction(sig, read_sigaction(procs.current()?, new)?)?;
        }
        Ok(0)
    }

    fn sys_sigprocmask(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let how = args.0[0];
        let set = args.0[1];
        let old = args.0[2];
        let sigset_size = args.0[3].max(8);
        let current_mask = procs.signal_mask()?;
        if old != 0 {
            procs
                .current_mut()?
                .write_user_bytes(old, &mask_to_bytes(current_mask, sigset_size))
                .map_err(|_| EFAULT)?;
        }
        if set != 0 {
            let new_mask = read_mask(procs.current()?, set, sigset_size)?;
            let merged = match how {
                0 => new_mask,
                1 => current_mask | new_mask,
                2 => current_mask & !new_mask,
                _ => return Err(EINVAL),
            };
            procs.set_signal_mask(merged)?;
        }
        Ok(0)
    }

    fn sys_rt_sigtimedwait(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
    ) -> Result<usize, i32> {
        if args.0[2] != 0 {
            return Err(ENOSYS);
        }
        let Some(signal) = procs.dequeue_unmasked_signal()? else {
            return Err(EAGAIN);
        };
        if args.0[1] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[1], &[0; 128])
                .map_err(|_| EFAULT)?;
        }
        Ok(signal)
    }

    fn sys_times(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        if args.0[0] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[0], &[0; 32])
                .map_err(|_| EFAULT)?;
        }
        Ok(hal().timer.monotonic_time().tv_sec.max(0) as usize)
    }

    fn sys_uname(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs
            .current_mut()?
            .write_user_bytes(args.0[0], &uname_bytes())
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_gettimeofday(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let ts = wall_time_now();
        procs
            .current_mut()?
            .write_user_bytes(args.0[0], &timeval_bytes(ts))
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_getrusage(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _who = args.0[0];
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &[0; 128])
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_syslog(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        const SYSLOG_ACTION_READ_ALL: usize = 3;
        const SYSLOG_ACTION_SIZE_BUFFER: usize = 10;
        let action = args.0[0];
        let buf = args.0[1];
        let len = args.0[2];
        let message = b"whuse kernel log buffer\n";
        match action {
            SYSLOG_ACTION_SIZE_BUFFER => Ok(message.len()),
            SYSLOG_ACTION_READ_ALL => {
                let written = message.len().min(len);
                procs
                    .current_mut()?
                    .write_user_bytes(buf, &message[..written])
                    .map_err(|_| EFAULT)?;
                Ok(written)
            }
            _ => Ok(0),
        }
    }

    fn sys_getpgid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.getpgid(args.0[0])
    }

    fn sys_setpgid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.setpgid(args.0[0], args.0[1])?;
        Ok(0)
    }

    fn sys_setgid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.setgid_current(args.0[0] as u32)?;
        Ok(0)
    }

    fn sys_setuid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.setuid_current(args.0[0] as u32)?;
        Ok(0)
    }

    fn sys_mremap(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let old_addr = args.0[0];
        let old_len = args.0[1];
        let new_len = args.0[2];
        let _flags = args.0[3];
        let bytes = {
            let process = procs.current()?;
            process
                .address_space
                .read_bytes(old_addr, old_len.min(new_len))
                .map_err(|_| EFAULT)?
        };
        let process = procs.current_mut()?;
        let new_addr = process.address_space.map_anonymous(new_len, 0b11)?;
        process
            .address_space
            .write_bytes(new_addr, &bytes)
            .map_err(|_| EFAULT)?;
        process.address_space.unmap(old_addr, old_len)?;
        Ok(new_addr)
    }

    fn sys_prlimit64(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _pid = args.0[0];
        let _resource = args.0[1];
        let _new_limit = args.0[2];
        let old_limit = args.0[3];
        if old_limit != 0 {
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&usize::MAX.to_le_bytes());
            bytes[8..].copy_from_slice(&usize::MAX.to_le_bytes());
            procs
                .current_mut()?
                .write_user_bytes(old_limit, &bytes)
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_renameat2(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let old_path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let new_path = procs
            .current()?
            .read_user_cstr(args.0[3])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _flags = args.0[4];
        vfs.rename(&cwd, &old_path, &new_path)?;
        Ok(0)
    }

    fn sys_getrandom(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let buf = args.0[0];
        let len = args.0[1];
        let _flags = args.0[2];
        let mut bytes = vec![0u8; len];
        let pattern = 0x42_49_4c_47_4b_43_55_46u64.to_le_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = pattern[index % pattern.len()];
        }
        procs
            .current_mut()?
            .write_user_bytes(buf, &bytes)
            .map_err(|_| EFAULT)?;
        Ok(len)
    }

    fn sys_copy_file_range(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        self.sys_copy_between_fds(
            args.0[0] as i32,
            args.0[2] as i32,
            args.0[4],
            args.0[1],
            args.0[3],
            procs,
            vfs,
        )
    }

    fn sys_copy_between_fds(
        &self,
        in_fd: i32,
        out_fd: i32,
        len: usize,
        off_in_ptr: usize,
        off_out_ptr: usize,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let mut in_handle = procs.current()?.fd(in_fd)?.clone();
        let mut out_handle = procs.current()?.fd(out_fd)?.clone();
        if off_in_ptr != 0 {
            in_handle.offset = read_usize(procs.current()?, off_in_ptr)?;
        }
        if off_out_ptr != 0 {
            out_handle.offset = read_usize(procs.current()?, off_out_ptr)?;
        }
        let bytes = vfs.read(&mut in_handle, len)?;
        let written = vfs.write(&mut out_handle, &bytes)?;
        if off_in_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(off_in_ptr, &in_handle.offset.to_le_bytes())
                .map_err(|_| EFAULT)?;
        } else {
            procs.current_mut()?.fd_mut(in_fd)?.offset = in_handle.offset;
        }
        if off_out_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(off_out_ptr, &out_handle.offset.to_le_bytes())
                .map_err(|_| EFAULT)?;
        } else {
            procs.current_mut()?.fd_mut(out_fd)?.offset = out_handle.offset;
        }
        Ok(written)
    }

    fn sys_statx(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        const AT_EMPTY_PATH: usize = 0x1000;
        const AT_FDCWD: i32 = -100;
        let dirfd = args.0[0] as i32;
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let flags = args.0[2];
        let stat = if path.is_empty() && (flags & AT_EMPTY_PATH) != 0 {
            if dirfd == AT_FDCWD {
                let cwd = procs.current()?.cwd.clone();
                vfs.stat_path(&cwd, &cwd)?
            } else {
                let handle = procs.current()?.fd(dirfd)?;
                vfs.stat_handle(handle)?
            }
        } else {
            let cwd = procs.current()?.cwd.clone();
            vfs.stat_path(&cwd, &path)?
        };
        let bytes = statx_bytes(stat);
        procs
            .current_mut()?
            .write_user_bytes(args.0[4], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_ftruncate(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let len = args.0[1];
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.truncate(handle, len)?;
        Ok(0)
    }

    fn sys_truncate(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.truncate_path(&cwd, &path, args.0[1])?;
        Ok(0)
    }

    fn sys_fallocate(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let _mode = args.0[1];
        let offset = args.0[2];
        let len = args.0[3];
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.fallocate(handle, offset, len)?;
        Ok(0)
    }

    fn sys_futex(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        const FUTEX_WAIT: usize = 0;
        const FUTEX_WAKE: usize = 1;
        const FUTEX_REQUEUE: usize = 3;
        const FUTEX_CMP_REQUEUE: usize = 4;
        let uaddr = args.0[0];
        let op = args.0[1] & 0x7f;
        let val = args.0[2] as i32;
        match op {
            FUTEX_WAIT => {
                if args.0[3] != 0 {
                    return Err(ENOSYS);
                }
                let current = read_i32(procs.current()?, uaddr)?;
                if current != val {
                    Err(EAGAIN)
                } else {
                    let tid = procs.current_tid()?;
                    procs.enqueue_futex_waiter(uaddr, tid);
                    let _ = scheduler.block_current();
                    Ok(0)
                }
            }
            FUTEX_WAKE => {
                let wake_count = val.max(0) as usize;
                let woke = procs.wake_futex(uaddr, wake_count);
                for tid in &woke {
                    let _ = scheduler.wake_task(*tid);
                }
                Ok(woke.len())
            }
            FUTEX_REQUEUE => {
                let moved = procs.requeue_futex(uaddr, args.0[4], val.max(0) as usize, args.0[3]);
                for tid in &moved {
                    let _ = scheduler.wake_task(*tid);
                }
                Ok(moved.len())
            }
            FUTEX_CMP_REQUEUE => {
                if read_i32(procs.current()?, uaddr)? != args.0[5] as i32 {
                    return Err(EAGAIN);
                }
                let moved = procs.requeue_futex(uaddr, args.0[4], val.max(0) as usize, args.0[3]);
                for tid in &moved {
                    let _ = scheduler.wake_task(*tid);
                }
                Ok(moved.len())
            }
            _ => Err(ENOSYS),
        }
    }

    fn sys_get_robust_list(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
    ) -> Result<usize, i32> {
        let (head, len) = procs.get_robust_list(args.0[0])?;
        if args.0[1] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[1], &head.to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        if args.0[2] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[2], &len.to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_eventfd2(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let handle = vfs.create_eventfd(args.0[0] as u64)?;
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_epoll_create1(
        &self,
        _args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let handle = vfs.create_epoll()?;
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_epoll_ctl(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let epfd = args.0[0] as i32;
        let op = args.0[1] as u32;
        let fd = args.0[2] as i32;
        let event = if args.0[3] != 0 {
            read_epoll_event(procs.current()?, args.0[3])?
        } else {
            EpollEvent::default()
        };
        let process = procs.current_mut()?;
        let epoll = process.fd_mut(epfd)?;
        vfs.epoll_ctl(epoll, op, fd, event.events)?;
        Ok(0)
    }

    fn sys_epoll_pwait(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let epfd = args.0[0] as i32;
        let events_ptr = args.0[1];
        let maxevents = args.0[2];
        let watches = {
            let process = procs.current()?;
            let epoll = process.fd(epfd)?;
            vfs.epoll_watches(epoll)?
        };
        let mut ready = Vec::new();
        for watch in watches {
            let process = procs.current()?;
            let Ok(handle) = process.fd(watch.fd) else {
                continue;
            };
            let readable = vfs.is_read_ready(handle);
            let writable = vfs.is_write_ready(handle);
            if readable || writable {
                ready.push(EpollEvent {
                    events: watch.events,
                    _pad: 0,
                    data: watch.fd as u64,
                });
                if ready.len() == maxevents {
                    break;
                }
            }
        }
        if events_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(events_ptr, &epoll_events_to_bytes(&ready))
                .map_err(|_| EFAULT)?;
        }
        Ok(ready.len())
    }

    fn sys_symlinkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let target = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let linkpath = procs
            .current()?
            .read_user_cstr(args.0[2])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.create_symlink(&cwd, &linkpath, &target)?;
        Ok(0)
    }

    fn sys_linkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let old_path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let new_path = procs
            .current()?
            .read_user_cstr(args.0[3])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _flags = args.0[4];
        vfs.link(&cwd, &old_path, &new_path)?;
        Ok(0)
    }

    fn sys_renameat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let old_path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let new_path = procs
            .current()?
            .read_user_cstr(args.0[3])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.rename(&cwd, &old_path, &new_path)?;
        Ok(0)
    }

    fn sys_sync(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_fsync(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _ = procs.current()?.fd(args.0[0] as i32)?;
        Ok(0)
    }

    fn sys_fstatfs(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        _vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let _ = procs.current()?.fd(args.0[0] as i32)?;
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &statfs_bytes())
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_fchdir(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let path = procs.current()?.fd(fd)?.path.clone();
        let new_cwd = vfs.chdir("/", &path)?;
        procs.current_mut()?.cwd = new_cwd;
        Ok(0)
    }

    fn sys_chroot(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let new_root = vfs.chdir(&cwd, &path)?;
        procs.current_mut()?.cwd = new_root;
        Ok(0)
    }

    fn sys_fchmod(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _ = args.0[1];
        let _ = procs.current()?.fd(args.0[0] as i32)?;
        Ok(0)
    }

    fn sys_fchmodat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _mode = args.0[2];
        let _flags = args.0[3];
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_fchownat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _owner = args.0[2];
        let _group = args.0[3];
        let _flags = args.0[4];
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_fchown(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _owner = args.0[1];
        let _group = args.0[2];
        let _ = procs.current()?.fd(args.0[0] as i32)?;
        Ok(0)
    }

    fn sys_utimensat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _times = args.0[2];
        let _flags = args.0[3];
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_close_range(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let first = args.0[0] as i32;
        let last = args.0[1] as i32;
        let _flags = args.0[2];
        let process = procs.current_mut()?;
        for fd in first..=last {
            process.fds.remove(&fd);
        }
        Ok(0)
    }

    fn sys_pwrite64(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let offset = args.0[3];
        let data = procs
            .current()?
            .read_user_bytes(buf, count)
            .map_err(|_| EFAULT)?;
        let mut handle = procs.current()?.fd(fd)?.clone();
        handle.offset = offset;
        vfs.write(&mut handle, &data)
    }

    fn sys_preadv(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let mut handle = procs.current()?.fd(fd)?.clone();
        handle.offset = args.0[3];
        let mut total = 0;
        for iov in iovecs {
            let bytes = vfs.read(&mut handle, iov.iov_len)?;
            procs
                .current_mut()?
                .write_user_bytes(iov.iov_base, &bytes)
                .map_err(|_| EFAULT)?;
            total += bytes.len();
            if bytes.len() < iov.iov_len {
                break;
            }
        }
        Ok(total)
    }

    fn sys_pwritev(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let mut handle = procs.current()?.fd(fd)?.clone();
        handle.offset = args.0[3];
        let mut total = 0;
        for iov in iovecs {
            let bytes = procs
                .current()?
                .read_user_bytes(iov.iov_base, iov.iov_len)
                .map_err(|_| EFAULT)?;
            total += vfs.write(&mut handle, &bytes)?;
        }
        Ok(total)
    }

    fn sys_getitimer(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _which = args.0[0];
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &[0; 32])
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_setitimer(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _which = args.0[0];
        let _new = args.0[1];
        if args.0[2] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[2], &[0; 32])
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_clock_getres(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _clock_id = args.0[0];
        procs
            .current_mut()?
            .write_user_bytes(
                args.0[1],
                &timespec_to_bytes(Timespec {
                    tv_sec: 0,
                    tv_nsec: 1_000,
                }),
            )
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_clock_nanosleep(&self, args: SyscallArgs) -> Result<usize, i32> {
        let _clock_id = args.0[0];
        let _flags = args.0[1];
        let requested = args.0[2];
        let _remain = args.0[3];
        hal().timer.program_oneshot(requested as u64);
        Ok(0)
    }

    fn sys_sigaltstack(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let new_ptr = args.0[0];
        let old_ptr = args.0[1];
        let previous = procs.set_sigaltstack(if new_ptr == 0 {
            None
        } else {
            Some(read_stack_t(procs.current()?, new_ptr)?)
        })?;
        if old_ptr != 0 {
            let bytes = stack_t_to_bytes(previous.unwrap_or((0, 0, 2)));
            procs
                .current_mut()?
                .write_user_bytes(old_ptr, &bytes)
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_rt_sigsuspend(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _set = args.0[0];
        let _size = args.0[1];
        let _ = procs.current()?;
        Err(EAGAIN)
    }

    fn sys_rt_sigpending(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let pending = procs.pending_signals()?;
        procs
            .current_mut()?
            .write_user_bytes(args.0[0], &mask_to_bytes(pending, args.0[1].max(8)))
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_rt_sigreturn(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_getpriority(&self) -> Result<usize, i32> {
        Ok(20)
    }

    fn sys_getsid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.getsid(args.0[0])
    }

    fn sys_setsid(&self, procs: &mut ProcessTable) -> Result<usize, i32> {
        procs.setsid_current()
    }

    fn sys_umask(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        Ok(procs.umask_current(args.0[0] as u32)? as usize)
    }

    fn sys_prctl(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_getgroups(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let groups = procs.getgroups_current()?;
        let size = args.0[0];
        if size == 0 {
            return Ok(groups.len());
        }
        if size < groups.len() {
            return Err(EINVAL);
        }
        let mut bytes = Vec::with_capacity(groups.len() * 4);
        for group in &groups {
            bytes.extend_from_slice(&group.to_le_bytes());
        }
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(groups.len())
    }

    fn sys_setgroups(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let size = args.0[0];
        let raw = procs
            .current()?
            .read_user_bytes(args.0[1], size * 4)
            .map_err(|_| EFAULT)?;
        let mut groups = Vec::with_capacity(size);
        for chunk in raw.chunks_exact(4) {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(chunk);
            groups.push(u32::from_le_bytes(bytes));
        }
        procs.setgroups_current(&groups)?;
        Ok(0)
    }

    fn sys_shmget(&self, args: SyscallArgs) -> Result<usize, i32> {
        let key = args.0[0] as i32;
        let size = args.0[1];
        let _flags = args.0[2];
        let mut state = SHM_STATE.lock();
        if let Some(id) = state.keys.get(&key).copied() {
            return Ok(id);
        }
        let id = state.next_id;
        state.next_id += 1;
        state.keys.insert(key, id);
        state.segments.insert(id, vec![0; size.max(1)]);
        Ok(id)
    }

    fn sys_shmat(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let id = args.0[0];
        let _addr = args.0[1];
        let _flags = args.0[2];
        let data = SHM_STATE.lock().segments.get(&id).cloned().ok_or(ENOENT)?;
        let addr = procs
            .current_mut()?
            .address_space
            .map_anonymous(data.len(), 0b11)?;
        procs
            .current_mut()?
            .address_space
            .write_bytes(addr, &data)
            .map_err(|_| EFAULT)?;
        Ok(addr)
    }

    fn sys_shmctl(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let id = args.0[0];
        let cmd = args.0[1] as i32;
        let buf = args.0[2];
        match cmd {
            0 => {
                SHM_STATE.lock().segments.remove(&id).ok_or(ENOENT)?;
            }
            2 => {
                if buf != 0 {
                    procs
                        .current_mut()?
                        .write_user_bytes(buf, &[0; 128])
                        .map_err(|_| EFAULT)?;
                }
            }
            _ => {}
        }
        Ok(0)
    }

    fn sys_shmdt(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_socket(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let family = args.0[0];
        let sock_type = args.0[1] & 0xf;
        if !matches!(family, 1 | 2 | 10) {
            return Err(EAFNOSUPPORT);
        }
        if sock_type != 1 {
            return Err(EPROTOTYPE);
        }
        let handle = vfs.create_socket()?;
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_socketpair(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        if args.0[0] != 1 {
            return Err(EAFNOSUPPORT);
        }
        let (left, right) = vfs.create_socketpair()?;
        let process = procs.current_mut()?;
        let left_fd = process.add_fd(left);
        let right_fd = process.add_fd(right);
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&left_fd.to_le_bytes());
        bytes[4..].copy_from_slice(&right_fd.to_le_bytes());
        process
            .write_user_bytes(args.0[3], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_bind(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let path = parse_sockaddr_path(procs.current()?, args.0[1], args.0[2])?;
        let cwd = procs.current()?.cwd.clone();
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.bind_socket(handle, &cwd, &path)?;
        Ok(0)
    }

    fn sys_listen(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let process = procs.current_mut()?;
        let handle = process.fd_mut(args.0[0] as i32)?;
        vfs.listen_socket(handle, args.0[1] as i32)?;
        Ok(0)
    }

    fn sys_connect(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let path = parse_sockaddr_path(procs.current()?, args.0[1], args.0[2])?;
        let cwd = procs.current()?.cwd.clone();
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.connect_socket(handle, &cwd, &path)?;
        Ok(0)
    }

    fn sys_accept(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let new_handle = {
            let process = procs.current_mut()?;
            let handle = process.fd_mut(fd)?;
            vfs.accept_socket(handle)?
        };
        let new_fd = procs.current_mut()?.add_fd(new_handle.clone());
        if args.0[1] != 0 && args.0[2] != 0 {
            write_sockaddr(procs.current_mut()?, args.0[1], args.0[2], &new_handle.path)?;
        }
        Ok(new_fd as usize)
    }

    fn sys_getsockname(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let path = procs.current()?.fd(fd)?.path.clone();
        write_sockaddr(procs.current_mut()?, args.0[1], args.0[2], &path)?;
        Ok(0)
    }

    fn sys_sendto(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let _flags = args.0[3];
        let data = procs
            .current()?
            .read_user_bytes(buf, count)
            .map_err(|_| EFAULT)?;
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.write(handle, &data)
    }

    fn sys_recvfrom(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let bytes = {
            let process = procs.current_mut()?;
            let handle = process.fd_mut(fd)?;
            vfs.read(handle, args.0[2])?
        };
        procs
            .current_mut()?
            .write_user_bytes(args.0[1], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(bytes.len())
    }

    fn sys_setsockopt(&self, _args: SyscallArgs) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_getsockopt(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _fd = args.0[0];
        let _level = args.0[1];
        let _opt = args.0[2];
        if args.0[3] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[3], &0u32.to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        if args.0[4] != 0 {
            procs
                .current_mut()?
                .write_user_bytes(args.0[4], &4u32.to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_shutdown(&self, _args: SyscallArgs) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_sendmsg(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let msg = read_msghdr(procs.current()?, args.0[1])?;
        let iovecs = read_iovecs(procs.current()?, msg.msg_iov, msg.msg_iovlen)?;
        let mut total = 0;
        for iov in iovecs {
            let bytes = procs
                .current()?
                .read_user_bytes(iov.iov_base, iov.iov_len)
                .map_err(|_| EFAULT)?;
            let process = procs.current_mut()?;
            let handle = process.fd_mut(args.0[0] as i32)?;
            total += vfs.write(handle, &bytes)?;
        }
        Ok(total)
    }

    fn sys_recvmsg(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let msg = read_msghdr(procs.current()?, args.0[1])?;
        let iovecs = read_iovecs(procs.current()?, msg.msg_iov, msg.msg_iovlen)?;
        let mut total = 0;
        for iov in iovecs {
            let bytes = {
                let process = procs.current_mut()?;
                let handle = process.fd_mut(args.0[0] as i32)?;
                vfs.read(handle, iov.iov_len)?
            };
            procs
                .current_mut()?
                .write_user_bytes(iov.iov_base, &bytes)
                .map_err(|_| EFAULT)?;
            total += bytes.len();
            if bytes.len() < iov.iov_len {
                break;
            }
        }
        Ok(total)
    }

    fn sys_memfd_create(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let name = procs
            .current()?
            .read_user_cstr(args.0[0])
            .map_err(|_| EFAULT)?;
        let _flags = args.0[1];
        let handle = vfs.create_memfd(&name)?;
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_pidfd_open(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let pid = args.0[0];
        let _flags = args.0[1];
        if !procs.has_pid(pid) {
            return Err(ENOENT);
        }
        let handle = vfs.create_pidfd(pid)?;
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_pidfd_getfd(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let pid = {
            let process = procs.current()?;
            let handle = process.fd(args.0[0] as i32)?;
            vfs.pidfd_pid(handle)?
        };
        let target_handle = procs.duplicate_fd_from(pid, args.0[1] as i32)?;
        Ok(procs.current_mut()?.add_fd(target_handle) as usize)
    }

    fn sys_pidfd_send_signal(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let pid = {
            let process = procs.current()?;
            let handle = process.fd(args.0[0] as i32)?;
            vfs.pidfd_pid(handle)?
        };
        let signal = args.0[1];
        let _info = args.0[2];
        let _flags = args.0[3];
        if signal != 0 {
            procs.send_signal(pid, signal)?;
        }
        Ok(0)
    }

    fn sys_sysinfo(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &Scheduler,
    ) -> Result<usize, i32> {
        let mut bytes = [0u8; 112];
        bytes[64..72].copy_from_slice(&(scheduler.current().is_some() as u64).to_le_bytes());
        bytes[80..82].copy_from_slice(&(procs.process_count() as u16).to_le_bytes());
        bytes[104..108].copy_from_slice(&1u32.to_le_bytes());
        procs
            .current_mut()?
            .write_user_bytes(args.0[0], &bytes)
            .map_err(|_| EFAULT)?;
        Ok(0)
    }
}

fn normalize_open_flags(flags: u32) -> u32 {
    let mut out = flags & (O_CREAT | O_TRUNC | O_DIRECTORY);
    out |= match flags & 0b11 {
        0 => O_RDONLY,
        1 => O_WRONLY,
        _ => O_RDWR,
    };
    out
}

fn timespec_to_bytes(ts: Timespec) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&ts.tv_sec.to_le_bytes());
    out[8..].copy_from_slice(&ts.tv_nsec.to_le_bytes());
    out
}

fn stat_to_bytes(stat: FileStat) -> [u8; 128] {
    // Linux-compatible struct kstat layout used by OS COMP basic tests.
    let mut out = [0u8; 128];
    out[16..20].copy_from_slice(&stat.mode.to_le_bytes());
    out[20..24].copy_from_slice(&stat.nlink.to_le_bytes());
    out[48..56].copy_from_slice(&stat.size.to_le_bytes());
    out[56..60].copy_from_slice(&(4096u32).to_le_bytes());
    out[64..72].copy_from_slice(&(stat.size / 512).to_le_bytes());
    out
}

fn read_iovecs(process: &proc::Process, addr: usize, count: usize) -> Result<Vec<IoVec>, i32> {
    let raw = process
        .read_user_bytes(addr, count * size_of::<IoVec>())
        .map_err(|_| EFAULT)?;
    let mut out = Vec::with_capacity(count);
    for chunk in raw.chunks_exact(size_of::<IoVec>()) {
        let mut base = [0u8; size_of::<usize>()];
        let mut len = [0u8; size_of::<usize>()];
        base.copy_from_slice(&chunk[..size_of::<usize>()]);
        len.copy_from_slice(&chunk[size_of::<usize>()..size_of::<IoVec>()]);
        out.push(IoVec {
            iov_base: usize::from_le_bytes(base),
            iov_len: usize::from_le_bytes(len),
        });
    }
    Ok(out)
}

fn read_pollfds(process: &proc::Process, addr: usize, count: usize) -> Result<Vec<PollFd>, i32> {
    let raw = process
        .read_user_bytes(addr, count * size_of::<PollFd>())
        .map_err(|_| EFAULT)?;
    let mut out = Vec::with_capacity(count);
    for chunk in raw.chunks_exact(size_of::<PollFd>()) {
        let mut fd = [0u8; 4];
        let mut events = [0u8; 2];
        let mut revents = [0u8; 2];
        fd.copy_from_slice(&chunk[..4]);
        events.copy_from_slice(&chunk[4..6]);
        revents.copy_from_slice(&chunk[6..8]);
        out.push(PollFd {
            fd: i32::from_le_bytes(fd),
            events: i16::from_le_bytes(events),
            revents: i16::from_le_bytes(revents),
        });
    }
    Ok(out)
}

fn pollfds_to_bytes(pollfds: &[PollFd]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pollfds.len() * size_of::<PollFd>());
    for pollfd in pollfds {
        out.extend_from_slice(&pollfd.fd.to_le_bytes());
        out.extend_from_slice(&pollfd.events.to_le_bytes());
        out.extend_from_slice(&pollfd.revents.to_le_bytes());
    }
    out
}

fn epoll_events_to_bytes(events: &[EpollEvent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(events.len() * size_of::<EpollEvent>());
    for event in events {
        out.extend_from_slice(&event.events.to_le_bytes());
        out.extend_from_slice(&event._pad.to_le_bytes());
        out.extend_from_slice(&event.data.to_le_bytes());
    }
    out
}

fn read_usize(process: &proc::Process, addr: usize) -> Result<usize, i32> {
    let bytes = process
        .read_user_bytes(addr, size_of::<usize>())
        .map_err(|_| EFAULT)?;
    let mut out = [0u8; size_of::<usize>()];
    out.copy_from_slice(&bytes);
    Ok(usize::from_le_bytes(out))
}

fn read_i32(process: &proc::Process, addr: usize) -> Result<i32, i32> {
    let bytes = process.read_user_bytes(addr, 4).map_err(|_| EFAULT)?;
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes);
    Ok(i32::from_le_bytes(out))
}

fn read_epoll_event(process: &proc::Process, addr: usize) -> Result<EpollEvent, i32> {
    let bytes = process
        .read_user_bytes(addr, size_of::<EpollEvent>())
        .map_err(|_| EFAULT)?;
    let mut events = [0u8; 4];
    let mut data = [0u8; 8];
    events.copy_from_slice(&bytes[..4]);
    data.copy_from_slice(&bytes[8..16]);
    Ok(EpollEvent {
        events: u32::from_le_bytes(events),
        _pad: 0,
        data: u64::from_le_bytes(data),
    })
}

fn read_msghdr(process: &proc::Process, addr: usize) -> Result<MsgHdr, i32> {
    let bytes = process
        .read_user_bytes(addr, size_of::<MsgHdr>())
        .map_err(|_| EFAULT)?;
    let mut word = [0u8; size_of::<usize>()];
    let mut word32 = [0u8; 4];
    word.copy_from_slice(&bytes[0..size_of::<usize>()]);
    let msg_name = usize::from_le_bytes(word);
    word32.copy_from_slice(&bytes[8..12]);
    let msg_namelen = u32::from_le_bytes(word32);
    word.copy_from_slice(&bytes[16..16 + size_of::<usize>()]);
    let msg_iov = usize::from_le_bytes(word);
    word.copy_from_slice(&bytes[24..24 + size_of::<usize>()]);
    let msg_iovlen = usize::from_le_bytes(word);
    word.copy_from_slice(&bytes[32..32 + size_of::<usize>()]);
    let msg_control = usize::from_le_bytes(word);
    word.copy_from_slice(&bytes[40..40 + size_of::<usize>()]);
    let msg_controllen = usize::from_le_bytes(word);
    word32.copy_from_slice(&bytes[48..52]);
    let msg_flags = u32::from_le_bytes(word32);
    Ok(MsgHdr {
        msg_name,
        msg_namelen,
        _pad0: 0,
        msg_iov,
        msg_iovlen,
        msg_control,
        msg_controllen,
        msg_flags,
        _pad1: 0,
    })
}

fn read_mask(process: &proc::Process, addr: usize, size: usize) -> Result<u64, i32> {
    let bytes = process.read_user_bytes(addr, size).map_err(|_| EFAULT)?;
    let mut out = [0u8; 8];
    out[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
    Ok(u64::from_le_bytes(out))
}

fn read_string_vector(process: &proc::Process, addr: usize) -> Result<Vec<String>, i32> {
    if addr == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for index in 0..64usize {
        let ptr = read_usize(process, addr + index * size_of::<usize>())?;
        if ptr == 0 {
            break;
        }
        out.push(process.read_user_cstr(ptr).map_err(|_| EFAULT)?);
    }
    Ok(out)
}

fn selector_from_wait(pid: i32, current_pgid: usize) -> WaitSelector {
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

fn read_sigaction(process: &proc::Process, addr: usize) -> Result<SigAction, i32> {
    let bytes = process.read_user_bytes(addr, 32).map_err(|_| EFAULT)?;
    let handler = usize::from_le_bytes(bytes[0..8].try_into().unwrap());
    let flags = usize::from_le_bytes(bytes[8..16].try_into().unwrap());
    let restorer = usize::from_le_bytes(bytes[16..24].try_into().unwrap());
    let mask = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    Ok(SigAction {
        handler,
        flags,
        restorer,
        mask,
    })
}

fn sigaction_to_bytes(action: SigAction) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&action.handler.to_le_bytes());
    out[8..16].copy_from_slice(&action.flags.to_le_bytes());
    out[16..24].copy_from_slice(&action.restorer.to_le_bytes());
    out[24..32].copy_from_slice(&action.mask.to_le_bytes());
    out
}

fn mask_to_bytes(mask: u64, size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size];
    let bytes = mask.to_le_bytes();
    out[..size.min(bytes.len())].copy_from_slice(&bytes[..size.min(bytes.len())]);
    out
}

fn read_stack_t(process: &proc::Process, addr: usize) -> Result<(usize, usize, u32), i32> {
    let bytes = process.read_user_bytes(addr, 24).map_err(|_| EFAULT)?;
    let mut sp = [0u8; 8];
    let mut flags = [0u8; 4];
    let mut size = [0u8; 8];
    sp.copy_from_slice(&bytes[..8]);
    flags.copy_from_slice(&bytes[8..12]);
    size.copy_from_slice(&bytes[16..24]);
    Ok((
        usize::from_le_bytes(sp),
        usize::from_le_bytes(size),
        u32::from_le_bytes(flags),
    ))
}

fn stack_t_to_bytes(stack: (usize, usize, u32)) -> [u8; 24] {
    let mut out = [0u8; 24];
    out[..8].copy_from_slice(&stack.0.to_le_bytes());
    out[8..12].copy_from_slice(&stack.2.to_le_bytes());
    out[16..24].copy_from_slice(&stack.1.to_le_bytes());
    out
}

fn parse_sockaddr_path(process: &proc::Process, addr: usize, len: usize) -> Result<String, i32> {
    if addr == 0 || len < 2 {
        return Err(EFAULT);
    }
    let bytes = process.read_user_bytes(addr, len).map_err(|_| EFAULT)?;
    let family = u16::from_le_bytes([bytes[0], bytes[1]]);
    match family {
        1 => {
            let end = bytes[2..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|pos| pos + 2)
                .unwrap_or(len);
            String::from_utf8(bytes[2..end].to_vec()).map_err(|_| EINVAL)
        }
        2 => {
            if len < 8 {
                return Err(EINVAL);
            }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            Ok(format!(
                "/inet/{:03}.{:03}.{:03}.{:03}:{}",
                bytes[4], bytes[5], bytes[6], bytes[7], port
            ))
        }
        10 => {
            if len < 28 {
                return Err(EINVAL);
            }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            let mut groups = [0u16; 8];
            for (index, group) in groups.iter_mut().enumerate() {
                let offset = 8 + index * 2;
                *group = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            }
            Ok(format!(
                "/inet6/{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{}",
                groups[0],
                groups[1],
                groups[2],
                groups[3],
                groups[4],
                groups[5],
                groups[6],
                groups[7],
                port
            ))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

fn write_sockaddr(
    process: &mut proc::Process,
    addr_ptr: usize,
    len_ptr: usize,
    path: &str,
) -> Result<(), i32> {
    if addr_ptr == 0 || len_ptr == 0 {
        return Ok(());
    }
    let max_len = read_u32(process, len_ptr)? as usize;
    let bytes = if let Some(rest) = path.strip_prefix("/inet/") {
        encode_sockaddr_in(rest)?
    } else if let Some(rest) = path.strip_prefix("/inet6/") {
        encode_sockaddr_in6(rest)?
    } else {
        let mut bytes = Vec::with_capacity(2 + path.len() + 1);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes
    };
    let used = bytes.len().min(max_len);
    process
        .write_user_bytes(addr_ptr, &bytes[..used])
        .map_err(|_| EFAULT)?;
    process
        .write_user_bytes(len_ptr, &(bytes.len() as u32).to_le_bytes())
        .map_err(|_| EFAULT)?;
    Ok(())
}

fn encode_sockaddr_in(spec: &str) -> Result<Vec<u8>, i32> {
    let (ip, port) = spec.rsplit_once(':').ok_or(EINVAL)?;
    let mut bytes = vec![0u8; 16];
    bytes[..2].copy_from_slice(&2u16.to_le_bytes());
    bytes[2..4].copy_from_slice(&port.parse::<u16>().map_err(|_| EINVAL)?.to_be_bytes());
    let octets = parse_ipv4(ip)?;
    bytes[4..8].copy_from_slice(&octets);
    Ok(bytes)
}

fn encode_sockaddr_in6(spec: &str) -> Result<Vec<u8>, i32> {
    let (ip, port) = spec.rsplit_once(':').ok_or(EINVAL)?;
    let mut bytes = vec![0u8; 28];
    bytes[..2].copy_from_slice(&10u16.to_le_bytes());
    bytes[2..4].copy_from_slice(&port.parse::<u16>().map_err(|_| EINVAL)?.to_be_bytes());
    let groups = parse_ipv6(ip)?;
    for (index, group) in groups.iter().enumerate() {
        let offset = 8 + index * 2;
        bytes[offset..offset + 2].copy_from_slice(&group.to_be_bytes());
    }
    Ok(bytes)
}

fn parse_ipv4(ip: &str) -> Result<[u8; 4], i32> {
    let mut out = [0u8; 4];
    let mut count = 0;
    for (index, segment) in ip.split('.').enumerate() {
        if index >= 4 {
            return Err(EINVAL);
        }
        out[index] = segment.parse::<u8>().map_err(|_| EINVAL)?;
        count += 1;
    }
    if count != 4 {
        return Err(EINVAL);
    }
    Ok(out)
}

fn parse_ipv6(ip: &str) -> Result<[u16; 8], i32> {
    let mut groups = [0u16; 8];
    let mut filled = 0usize;
    let mut compressed = None;
    for segment in ip.split(':') {
        if segment.is_empty() {
            if compressed.is_some() {
                continue;
            }
            compressed = Some(filled);
            continue;
        }
        if filled >= 8 {
            return Err(EINVAL);
        }
        groups[filled] = u16::from_str_radix(segment, 16).map_err(|_| EINVAL)?;
        filled += 1;
    }
    if let Some(pos) = compressed {
        let tail_len = filled.saturating_sub(pos);
        for index in 0..tail_len {
            groups[7 - index] = groups[pos + tail_len - 1 - index];
            groups[pos + tail_len - 1 - index] = 0;
        }
    } else if filled != 8 {
        return Err(EINVAL);
    }
    Ok(groups)
}

fn resolve_exec_payload(data: &[u8]) -> Option<user_init::BuiltinProgram> {
    let text = core::str::from_utf8(data).ok()?.trim();
    if let Some(path) = text.strip_prefix("builtin ") {
        return builtin_program(path.trim());
    }
    match text {
        "builtin-init" => builtin_program("/sbin/init"),
        "builtin-child" => builtin_program("/bin/child"),
        _ => None,
    }
}

fn parse_shebang_line(data: &[u8]) -> Option<(String, Option<String>)> {
    if !data.starts_with(b"#!") {
        return None;
    }
    let line = data
        .get(2..)?
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|raw| core::str::from_utf8(raw).ok())?
        .trim();
    let mut parts = line.split_ascii_whitespace();
    let interp = parts.next()?.to_string();
    let arg = parts.next().map(|value| value.to_string());
    Some((interp, arg))
}

fn read_u32(process: &proc::Process, addr: usize) -> Result<u32, i32> {
    let bytes = process.read_user_bytes(addr, 4).map_err(|_| EFAULT)?;
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes);
    Ok(u32::from_le_bytes(out))
}

fn timeval_bytes(ts: Timespec) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&ts.tv_sec.to_le_bytes());
    out[8..].copy_from_slice(&(ts.tv_nsec / 1_000).to_le_bytes());
    out
}

fn wall_time_now() -> Timespec {
    // Temporary wall-time anchor until RTC-backed realtime is wired in.
    const WALL_TIME_BASE_SEC: i64 = 1_735_689_600; // 2025-01-01T00:00:00Z
    let mono = hal().timer.monotonic_time();
    let sec = WALL_TIME_BASE_SEC.saturating_add(mono.tv_sec.max(0));
    let nsec = mono.tv_nsec.clamp(0, 999_999_999);
    Timespec {
        tv_sec: sec,
        tv_nsec: nsec,
    }
}

fn uname_bytes() -> [u8; 390] {
    let mut out = [0u8; 390];
    let fields = [
        "Linux",
        "whuse",
        "6.8.0-whuse",
        "whuse-riscv64-virt",
        "riscv64",
        "localdomain",
    ];
    for (index, field) in fields.iter().enumerate() {
        let start = index * 65;
        let bytes = field.as_bytes();
        out[start..start + bytes.len()].copy_from_slice(bytes);
    }
    out
}

fn statfs_bytes() -> [u8; 120] {
    let mut out = [0u8; 120];
    out[8..16].copy_from_slice(&4096u64.to_le_bytes());
    out[80..88].copy_from_slice(&255u64.to_le_bytes());
    out[88..96].copy_from_slice(&4096u64.to_le_bytes());
    out
}

fn statx_bytes(stat: FileStat) -> [u8; 256] {
    let mut out = [0u8; 256];
    out[..4].copy_from_slice(&0x1ffu32.to_le_bytes());
    out[28..32].copy_from_slice(&stat.mode.to_le_bytes());
    out[40..48].copy_from_slice(&stat.size.to_le_bytes());
    out[16..20].copy_from_slice(&stat.nlink.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::{
        SyscallArgs, SyscallDispatcher, SYS_ACCEPT, SYS_BIND, SYS_CLOCK_GETRES, SYS_CLONE,
        SYS_CONNECT, SYS_COPY_FILE_RANGE, SYS_DUP3, SYS_EPOLL_CREATE1, SYS_EPOLL_CTL,
        SYS_EPOLL_PWAIT, SYS_EVENTFD2, SYS_FACCESSAT2, SYS_FALLOCATE, SYS_FCHDIR, SYS_FCHMOD,
        SYS_FCHMODAT, SYS_FCHOWN, SYS_FCHOWNAT, SYS_FDATASYNC, SYS_FLOCK, SYS_FSTATFS, SYS_FSYNC,
        SYS_FUTEX, SYS_GETCWD, SYS_GETGROUPS, SYS_GETITIMER, SYS_GETPRIORITY, SYS_GETSID,
        SYS_GETSOCKNAME, SYS_GETSOCKOPT, SYS_GETTIMEOFDAY, SYS_GET_ROBUST_LIST, SYS_LINKAT,
        SYS_LISTEN, SYS_LSEEK, SYS_MEMFD_CREATE, SYS_MKDIR, SYS_MLOCK, SYS_MLOCK2, SYS_MSYNC,
        SYS_OPENAT, SYS_PIDFD_GETFD, SYS_PIDFD_OPEN, SYS_PIDFD_SEND_SIGNAL, SYS_PIPE, SYS_PPOLL,
        SYS_PRCTL, SYS_PREAD64, SYS_PREADV, SYS_PREADV2, SYS_PRLIMIT64, SYS_PSELECT6, SYS_PWRITE64,
        SYS_PWRITEV, SYS_PWRITEV2, SYS_READ, SYS_RECVFROM, SYS_RECVMSG, SYS_RENAMEAT,
        SYS_RENAMEAT2, SYS_RISCV_FLUSH_ICACHE, SYS_RT_SIGPENDING, SYS_RT_SIGRETURN,
        SYS_RT_SIGSUSPEND, SYS_RT_SIGTIMEDWAIT, SYS_SECCOMP, SYS_SENDMSG, SYS_SENDTO, SYS_SETGID,
        SYS_SETGROUPS, SYS_SETITIMER, SYS_SETSID, SYS_SETSOCKOPT, SYS_SETUID, SYS_SET_ROBUST_LIST,
        SYS_SET_TID_ADDRESS, SYS_SHMAT, SYS_SHMCTL, SYS_SHMDT, SYS_SHMGET, SYS_SHUTDOWN,
        SYS_SIGACTION, SYS_SIGALTSTACK, SYS_SIGPROCMASK, SYS_SOCKET, SYS_SOCKETPAIR, SYS_SPLICE,
        SYS_STATX, SYS_SYMLINKAT, SYS_TIMES, SYS_TRUNCATE, SYS_UMASK, SYS_UNAME, SYS_UTIMENSAT,
        SYS_WAIT, SYS_WRITE, SYS_WRITEV,
    };
    use hal_api::{
        register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalInterrupt, HalMemory,
        HalNetDevice, HalPlatform, HalPlatformLifecycle, HalTimer, MemoryRegion, PlatformArch,
        Timespec, TrapFrame, VmSpaceToken,
    };
    use proc::ProcessTable;
    use spin::Once;
    use task::Scheduler;
    use vfs::KernelVfs;

    struct TestCpu;
    struct TestMemory;
    struct TestTimer;
    struct TestConsole;
    struct TestPlatform;
    struct TestInterrupt;
    struct TestLifecycle;
    struct TestNet;

    static TEST_CPU: TestCpu = TestCpu;
    static TEST_MEMORY: TestMemory = TestMemory;
    static TEST_TIMER: TestTimer = TestTimer;
    static TEST_CONSOLE: TestConsole = TestConsole;
    static TEST_PLATFORM: TestPlatform = TestPlatform;
    static TEST_INTERRUPT: TestInterrupt = TestInterrupt;
    static TEST_LIFECYCLE: TestLifecycle = TestLifecycle;
    static TEST_NET: TestNet = TestNet;
    static TEST_REGIONS: [MemoryRegion; 1] = [MemoryRegion {
        start: 0x8000_0000,
        size: 0x100000,
        usable: true,
    }];
    static TEST_BLOCKS: [&'static dyn HalBlockDevice; 0] = [];
    static TEST_NETS: [&'static dyn HalNetDevice; 1] = [&TEST_NET];
    static INIT_HAL: Once<()> = Once::new();

    impl HalCpu for TestCpu {
        fn cpu_id(&self) -> usize {
            0
        }
        fn enable_interrupts(&self) {}
        fn disable_interrupts(&self) {}
        fn interrupts_enabled(&self) -> bool {
            false
        }
        fn switch_address_space(&self, _token: VmSpaceToken) {}
        fn wait_for_interrupt(&self) {}
        fn run_user(&self, _frame: &mut TrapFrame) {}
    }

    impl HalMemory for TestMemory {
        fn memory_regions(&self) -> &'static [MemoryRegion] {
            &TEST_REGIONS
        }
        fn phys_to_virt(&self, phys: usize) -> usize {
            phys
        }
        fn virt_to_phys(&self, virt: usize) -> usize {
            virt
        }
        fn mmio_base(&self) -> usize {
            0x1000_0000
        }
    }

    impl HalTimer for TestTimer {
        fn monotonic_time(&self) -> Timespec {
            Timespec {
                tv_sec: 1,
                tv_nsec: 0,
            }
        }
        fn monotonic_nanos(&self) -> u64 {
            1_000_000_000
        }
        fn program_oneshot(&self, _deadline_nanos: u64) {}
    }

    impl HalCharDevice for TestConsole {
        fn name(&self) -> &'static str {
            "test-console"
        }
        fn put_byte(&self, _byte: u8) {}
        fn get_byte(&self) -> Option<u8> {
            None
        }
    }

    impl HalPlatform for TestPlatform {
        fn platform_name(&self) -> &'static str {
            "test-platform"
        }
        fn architecture(&self) -> PlatformArch {
            PlatformArch::Riscv64
        }
    }

    impl HalPlatformLifecycle for TestLifecycle {
        fn supports_userspace(&self) -> bool {
            true
        }

        fn idle(&self) -> ! {
            panic!("test lifecycle idle should never be entered");
        }
    }

    impl HalInterrupt for TestInterrupt {
        fn name(&self) -> &'static str {
            "test-interrupt"
        }
        fn enable_irq(&self, _irq: usize) {}
        fn disable_irq(&self, _irq: usize) {}
        fn ack_irq(&self, _irq: usize) {}
        fn next_pending(&self) -> Option<usize> {
            None
        }
    }

    impl HalNetDevice for TestNet {
        fn name(&self) -> &'static str {
            "test-net"
        }
        fn mac_address(&self) -> [u8; 6] {
            [0, 1, 2, 3, 4, 5]
        }
        fn mtu(&self) -> usize {
            1500
        }
        fn can_send(&self) -> bool {
            false
        }
        fn can_recv(&self) -> bool {
            false
        }
        fn send_frame(&self, _frame: &[u8]) -> Result<usize, i32> {
            Err(95)
        }
        fn recv_frame(&self, _frame: &mut [u8]) -> Result<usize, i32> {
            Err(11)
        }
    }

    fn ensure_test_hal() {
        INIT_HAL.call_once(|| {
            let _ = register_hal(HalBundle {
                platform: &TEST_PLATFORM,
                lifecycle: &TEST_LIFECYCLE,
                interrupt: &TEST_INTERRUPT,
                cpu: &TEST_CPU,
                memory: &TEST_MEMORY,
                timer: &TEST_TIMER,
                console: &TEST_CONSOLE,
                block_devices: &TEST_BLOCKS,
                net_devices: &TEST_NETS,
            });
        });
    }

    #[test]
    fn basic_phase1_syscalls() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x1000, b"/work\0");
        assert_eq!(
            dispatcher.dispatch(
                SYS_MKDIR,
                SyscallArgs([0x1000, 0o755, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x2000, b"/work/log.txt\0");
        let fd = dispatcher.dispatch(
            SYS_OPENAT,
            SyscallArgs([
                !0usize,
                0x2000,
                (vfs::O_CREAT | vfs::O_RDWR) as usize,
                0o644,
                0,
                0,
            ]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert!(fd >= 3);

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x3000, b"hello");
        assert_eq!(
            dispatcher.dispatch(
                SYS_WRITE,
                SyscallArgs([fd as usize, 0x3000, 5, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            5
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x4000, &[0; 64]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_GETCWD,
                SyscallArgs([0x4000, 64, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0x4000
        );
    }

    #[test]
    fn extended_syscall_smoke() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x1000, b"/tmp/ext.txt\0");
        let fd = dispatcher.dispatch(
            SYS_OPENAT,
            SyscallArgs([
                !0usize,
                0x1000,
                (vfs::O_CREAT | vfs::O_RDWR) as usize,
                0o644,
                0,
                0,
            ]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x2000, b"he");
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x3000, b"llo");
        let mut iov = [0u8; 32];
        iov[..8].copy_from_slice(&0x2000usize.to_le_bytes());
        iov[8..16].copy_from_slice(&2usize.to_le_bytes());
        iov[16..24].copy_from_slice(&0x3000usize.to_le_bytes());
        iov[24..32].copy_from_slice(&3usize.to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x4000, &iov);
        assert_eq!(
            dispatcher.dispatch(
                SYS_WRITEV,
                SyscallArgs([fd, 0x4000, 2, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            5
        );

        assert_eq!(
            dispatcher.dispatch(
                SYS_LSEEK,
                SyscallArgs([fd, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
        );
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x5000, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_PREAD64,
                SyscallArgs([fd, 0x5000, 5, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            5
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x5000, 5).unwrap(),
            b"hello"
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x6000, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_PIPE,
                SyscallArgs([0x6000, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
        );
        let pipe_fds = procs.current().unwrap().read_user_bytes(0x6000, 8).unwrap();
        let read_fd = i32::from_le_bytes(pipe_fds[..4].try_into().unwrap()) as usize;
        let write_fd = i32::from_le_bytes(pipe_fds[4..8].try_into().unwrap()) as usize;
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x7000, b"ping");
        assert_eq!(
            dispatcher.dispatch(
                SYS_WRITE,
                SyscallArgs([write_fd, 0x7000, 4, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            4
        );
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x7100, &[0; 4]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_READ,
                SyscallArgs([read_fd, 0x7100, 4, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            4
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x7100, 4).unwrap(),
            b"ping"
        );
    }

    #[test]
    fn eventfd_epoll_and_socketpair_smoke() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        let eventfd = dispatcher.dispatch(
            SYS_EVENTFD2,
            SyscallArgs([0, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8000, &1u64.to_le_bytes());
        assert_eq!(
            dispatcher.dispatch(
                SYS_WRITE,
                SyscallArgs([eventfd, 0x8000, 8, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            8
        );

        let epfd = dispatcher.dispatch(
            SYS_EPOLL_CREATE1,
            SyscallArgs([0, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        let mut event = [0u8; 16];
        event[..4].copy_from_slice(&1u32.to_le_bytes());
        event[8..16].copy_from_slice(&(eventfd as u64).to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8100, &event);
        assert_eq!(
            dispatcher.dispatch(
                SYS_EPOLL_CTL,
                SyscallArgs([epfd, 1, eventfd, 0x8100, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
        );
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8200, &[0; 16]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_EPOLL_PWAIT,
                SyscallArgs([epfd, 0x8200, 4, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            1
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8300, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_READ,
                SyscallArgs([eventfd, 0x8300, 8, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            8
        );
        assert_eq!(
            u64::from_le_bytes(
                procs
                    .current()
                    .unwrap()
                    .read_user_bytes(0x8300, 8)
                    .unwrap()
                    .try_into()
                    .unwrap()
            ),
            1
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8400, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_SOCKETPAIR,
                SyscallArgs([1, 1, 0, 0x8400, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
        );
        let pair = procs.current().unwrap().read_user_bytes(0x8400, 8).unwrap();
        let left = i32::from_le_bytes(pair[..4].try_into().unwrap()) as usize;
        let right = i32::from_le_bytes(pair[4..8].try_into().unwrap()) as usize;
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8500, b"pong");
        assert_eq!(
            dispatcher.dispatch(
                SYS_SENDTO,
                SyscallArgs([left, 0x8500, 4, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            4
        );
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x8600, &[0; 4]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_RECVFROM,
                SyscallArgs([right, 0x8600, 4, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            4
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x8600, 4).unwrap(),
            b"pong"
        );
    }

    #[test]
    fn clone_wait_signal_and_futex_semantics() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xa000, &[0u8; 4]);
        let thread_flags = super::CLONE_VM
            | super::CLONE_FS
            | super::CLONE_FILES
            | super::CLONE_SIGHAND
            | super::CLONE_THREAD
            | super::CLONE_CHILD_CLEARTID;
        let thread_tid = dispatcher.dispatch(
            SYS_CLONE,
            SyscallArgs([thread_flags, 0, 0, 0xa000, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert!(thread_tid > init);

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xa100, &[0; 32]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_SIGACTION,
                SyscallArgs([10, 0, 0xa100, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(
            procs
                .current()
                .unwrap()
                .read_user_bytes(0xa100, 32)
                .unwrap(),
            [0u8; 32]
        );

        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa000, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(scheduler.current_thread_id(), Some(thread_tid));
        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa000, 1, 1, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            1
        );

        let child = dispatcher.dispatch(
            SYS_CLONE,
            SyscallArgs([0, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert!(child > thread_tid);
        procs.set_current(child).unwrap();
        let _ = procs.exit_current_thread(5).unwrap();
        scheduler.remove_task(child);
        procs.set_current(init).unwrap();

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xa200, &[0; 4]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_WAIT,
                SyscallArgs([child, 0xa200, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            child as isize
        );
        assert_eq!(
            i32::from_le_bytes(
                procs
                    .current()
                    .unwrap()
                    .read_user_bytes(0xa200, 4)
                    .unwrap()
                    .try_into()
                    .unwrap()
            ),
            5 << 8
        );

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xa300, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_RT_SIGPENDING,
                SyscallArgs([0xa300, 8, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
    }

    #[test]
    fn inet_loopback_socket_smoke() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        let mut sockaddr = [0u8; 16];
        sockaddr[..2].copy_from_slice(&2u16.to_le_bytes());
        sockaddr[2..4].copy_from_slice(&7000u16.to_be_bytes());
        sockaddr[4..8].copy_from_slice(&[127, 0, 0, 1]);
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x9000, &sockaddr);
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x9100, &[0; 16]);
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x9200, &16u32.to_le_bytes());

        let server = dispatcher.dispatch(
            SYS_SOCKET,
            SyscallArgs([2, 1, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert_eq!(
            dispatcher.dispatch(
                SYS_BIND,
                SyscallArgs([server, 0x9000, 16, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(
            dispatcher.dispatch(
                SYS_LISTEN,
                SyscallArgs([server, 1, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );

        let client = dispatcher.dispatch(
            SYS_SOCKET,
            SyscallArgs([2, 1, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert_eq!(
            dispatcher.dispatch(
                SYS_CONNECT,
                SyscallArgs([client, 0x9000, 16, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        let accepted = dispatcher.dispatch(
            SYS_ACCEPT,
            SyscallArgs([server, 0x9100, 0x9200, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert!(accepted >= 3);

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x9300, b"tcp");
        assert_eq!(
            dispatcher.dispatch(
                SYS_SENDTO,
                SyscallArgs([client, 0x9300, 3, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            3
        );
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x9400, &[0; 3]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_RECVFROM,
                SyscallArgs([accepted, 0x9400, 3, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            3
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x9400, 3).unwrap(),
            b"tcp"
        );

        assert_eq!(
            dispatcher.dispatch(
                SYS_GETSOCKNAME,
                SyscallArgs([server, 0x9100, 0x9200, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        let returned = procs.current().unwrap().read_user_bytes(0x9100, 8).unwrap();
        assert_eq!(u16::from_le_bytes([returned[0], returned[1]]), 2);
        assert_eq!(u16::from_be_bytes([returned[2], returned[3]]), 7000);
    }

    #[test]
    fn starry_riscv_syscalls_have_handlers() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0x1000, &[0; 4096]);
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1000, b"/tmp/x\0")
            .unwrap();
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1100, b"/tmp/y\0")
            .unwrap();
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1200, b"memfd\0")
            .unwrap();
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1380, &128u32.to_le_bytes())
            .unwrap();
        let mut sockaddr = [0u8; 32];
        sockaddr[..2].copy_from_slice(&1u16.to_le_bytes());
        sockaddr[2..12].copy_from_slice(b"socktest\0\0");
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1600, &sockaddr)
            .unwrap();
        let mut iov = [0u8; 16];
        iov[..8].copy_from_slice(&0x1700usize.to_le_bytes());
        iov[8..16].copy_from_slice(&4usize.to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1680, &iov)
            .unwrap();
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1700, b"data")
            .unwrap();
        let mut msghdr = [0u8; 56];
        msghdr[16..24].copy_from_slice(&0x1680usize.to_le_bytes());
        msghdr[24..32].copy_from_slice(&1usize.to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x1780, &msghdr)
            .unwrap();
        let mut epoll_event = [0u8; 16];
        epoll_event[..4].copy_from_slice(&1u32.to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .write_user_bytes(0x17c0, &epoll_event)
            .unwrap();

        let syscalls = [
            SYS_GETCWD,
            SYS_DUP3,
            SYS_FLOCK,
            SYS_MKDIR,
            SYS_SYMLINKAT,
            SYS_LINKAT,
            SYS_RENAMEAT,
            SYS_RENAMEAT2,
            SYS_FSTATFS,
            SYS_TRUNCATE,
            SYS_FALLOCATE,
            SYS_FCHDIR,
            SYS_FCHMOD,
            SYS_FCHMODAT,
            SYS_FCHOWNAT,
            SYS_FCHOWN,
            SYS_PWRITE64,
            SYS_PREADV,
            SYS_PWRITEV,
            SYS_PREADV2,
            SYS_PWRITEV2,
            SYS_PSELECT6,
            SYS_FSYNC,
            SYS_FDATASYNC,
            SYS_UTIMENSAT,
            SYS_SET_TID_ADDRESS,
            SYS_GET_ROBUST_LIST,
            SYS_SET_ROBUST_LIST,
            SYS_GETITIMER,
            SYS_SETITIMER,
            SYS_CLOCK_GETRES,
            SYS_SIGALTSTACK,
            SYS_RT_SIGSUSPEND,
            SYS_RT_SIGPENDING,
            SYS_RT_SIGRETURN,
            SYS_GETPRIORITY,
            SYS_GETSID,
            SYS_SETSID,
            SYS_GETGROUPS,
            SYS_SETGROUPS,
            SYS_UMASK,
            SYS_PRCTL,
            SYS_SHMGET,
            SYS_SHMAT,
            SYS_SHMCTL,
            SYS_SHMDT,
            SYS_SOCKET,
            SYS_SOCKETPAIR,
            SYS_BIND,
            SYS_LISTEN,
            SYS_ACCEPT,
            SYS_CONNECT,
            SYS_GETSOCKNAME,
            SYS_SENDTO,
            SYS_RECVFROM,
            SYS_SETSOCKOPT,
            SYS_GETSOCKOPT,
            SYS_SHUTDOWN,
            SYS_SENDMSG,
            SYS_RECVMSG,
            SYS_MSYNC,
            SYS_MLOCK,
            SYS_MLOCK2,
            SYS_MEMFD_CREATE,
            SYS_PIDFD_OPEN,
            SYS_PIDFD_SEND_SIGNAL,
            SYS_PIDFD_GETFD,
            SYS_FACCESSAT2,
            SYS_EPOLL_CREATE1,
            SYS_EPOLL_CTL,
            SYS_EPOLL_PWAIT,
            SYS_SECCOMP,
            SYS_RISCV_FLUSH_ICACHE,
            SYS_WAIT,
            SYS_PPOLL,
            SYS_TIMES,
            SYS_UNAME,
            SYS_GETTIMEOFDAY,
            SYS_PRLIMIT64,
            SYS_STATX,
            SYS_COPY_FILE_RANGE,
            SYS_SPLICE,
            SYS_SETUID,
            SYS_SETGID,
            SYS_SIGACTION,
            SYS_SIGPROCMASK,
            SYS_RT_SIGTIMEDWAIT,
        ];

        for sysno in syscalls {
            let args = match sysno {
                SYS_BIND | SYS_CONNECT => SyscallArgs([3, 0x1600, 32, 0, 0, 0]),
                SYS_GETSOCKNAME | SYS_GETSOCKOPT => {
                    SyscallArgs([3, 0x1800, 0x1380, 0x1800, 0x1380, 0])
                }
                SYS_SENDTO | SYS_RECVFROM => SyscallArgs([3, 0x1700, 4, 0, 0x1600, 32]),
                SYS_SENDMSG | SYS_RECVMSG => SyscallArgs([3, 0x1780, 0, 0, 0, 0]),
                SYS_PREADV | SYS_PWRITEV | SYS_PREADV2 | SYS_PWRITEV2 => {
                    SyscallArgs([3, 0x1680, 1, 0, 0, 0])
                }
                SYS_PWRITE64 | SYS_PREAD64 => SyscallArgs([3, 0x1700, 4, 0, 0, 0]),
                SYS_SETGROUPS | SYS_GETGROUPS => SyscallArgs([1, 0x1800, 0, 0, 0, 0]),
                SYS_EPOLL_CTL => SyscallArgs([3, 1, 4, 0x17c0, 0, 0]),
                SYS_PPOLL => SyscallArgs([0x1800, 1, 0, 0, 0, 0]),
                SYS_RT_SIGPENDING => SyscallArgs([0x1800, 8, 0, 0, 0, 0]),
                SYS_SIGPROCMASK => SyscallArgs([0, 0, 0x1800, 8, 0, 0]),
                SYS_RT_SIGTIMEDWAIT => SyscallArgs([0x1800, 0x1900, 0, 8, 0, 0]),
                _ => SyscallArgs([0x1000, 0x1100, 0x1300, 0x1380, 0x1400, 0x1500]),
            };
            let rc = dispatcher.dispatch(sysno, args, &mut procs, &mut scheduler, &mut vfs);
            assert_ne!(
                rc,
                -(super::ENOSYS as isize),
                "syscall {} fell through to ENOSYS",
                sysno
            );
        }
    }
}
