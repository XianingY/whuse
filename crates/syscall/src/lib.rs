#![cfg_attr(not(test), no_std)]

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use core::mem::size_of;
use hal_api::{hal, Timespec};
use proc::ProcessTable;
use task::Scheduler;
use user_init::builtin_program;
use vfs::{FileStat, KernelVfs, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};

const EBADF: i32 = 9;
const EAGAIN: i32 = 11;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENOENT: i32 = 2;
const ENOSYS: i32 = 38;

pub const SYS_GETCWD: usize = 17;
pub const SYS_DUP: usize = 23;
pub const SYS_DUP2: usize = 24;
pub const SYS_FCNTL: usize = 25;
pub const SYS_IOCTL: usize = 29;
pub const SYS_MKDIR: usize = 34;
pub const SYS_UNLINKAT: usize = 35;
pub const SYS_UMOUNT2: usize = 39;
pub const SYS_MOUNT: usize = 40;
pub const SYS_STATFS: usize = 43;
pub const SYS_FTRUNCATE: usize = 46;
pub const SYS_FACCESSAT: usize = 48;
pub const SYS_OPENAT: usize = 56;
pub const SYS_CLOSE: usize = 57;
pub const SYS_PIPE: usize = 59;
pub const SYS_GETDENTS64: usize = 61;
pub const SYS_LSEEK: usize = 62;
pub const SYS_READ: usize = 63;
pub const SYS_WRITE: usize = 64;
pub const SYS_READV: usize = 65;
pub const SYS_WRITEV: usize = 66;
pub const SYS_PREAD64: usize = 67;
pub const SYS_SENDFILE: usize = 71;
pub const SYS_PPOLL: usize = 73;
pub const SYS_SPLICE: usize = 76;
pub const SYS_READLINKAT: usize = 78;
pub const SYS_FSTATAT: usize = 79;
pub const SYS_SET_TID_ADDRESS: usize = 96;
pub const SYS_FUTEX: usize = 98;
pub const SYS_SET_ROBUST_LIST: usize = 99;
pub const SYS_SLEEP: usize = 101;
pub const SYS_SCHED_YIELD: usize = 124;
pub const SYS_KILL: usize = 129;
pub const SYS_CLOCK_GETTIME: usize = 113;
pub const SYS_SYSLOG: usize = 116;
pub const SYS_NANOSLEEP: usize = 115;
pub const SYS_SIGACTION: usize = 134;
pub const SYS_SIGPROCMASK: usize = 135;
pub const SYS_RT_SIGTIMEDWAIT: usize = 137;
pub const SYS_SETGID: usize = 144;
pub const SYS_SETUID: usize = 146;
pub const SYS_TIMES: usize = 153;
pub const SYS_SETPGID: usize = 154;
pub const SYS_GETPGID: usize = 155;
pub const SYS_UNAME: usize = 160;
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
pub const SYS_SOCKET: usize = 198;
pub const SYS_BIND: usize = 200;
pub const SYS_LISTEN: usize = 201;
pub const SYS_ACCEPT: usize = 202;
pub const SYS_CONNECT: usize = 203;
pub const SYS_GETSOCKNAME: usize = 204;
pub const SYS_SENDTO: usize = 206;
pub const SYS_RECVFROM: usize = 207;
pub const SYS_SETSOCKOPT: usize = 208;
pub const SYS_FSTAT: usize = 80;
pub const SYS_CHDIR: usize = 49;
pub const SYS_BRK: usize = 214;
pub const SYS_MREMAP: usize = 216;
pub const SYS_CLONE: usize = 220;
pub const SYS_FORK: usize = 220;
pub const SYS_EXECVE: usize = 221;
pub const SYS_MUNMAP: usize = 215;
pub const SYS_MMAP: usize = 222;
pub const SYS_MPROTECT: usize = 226;
pub const SYS_MADVISE: usize = 233;
pub const SYS_WAIT: usize = 260;
pub const SYS_PRLIMIT64: usize = 261;
pub const SYS_RENAMEAT2: usize = 276;
pub const SYS_GETRANDOM: usize = 278;
pub const SYS_COPY_FILE_RANGE: usize = 285;
pub const SYS_STATX: usize = 291;
pub const SYS_CLONE3: usize = 435;
pub const SYS_TGKILL: usize = 131;
pub const SYS_POWER_OFF: usize = 2024;

#[derive(Clone, Copy, Debug)]
pub struct SyscallArgs(pub [usize; 6]);

pub struct SyscallDispatcher;

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
        let result = match sysno {
            SYS_GETCWD => self.sys_getcwd(args, procs),
            SYS_DUP => self.sys_dup(args, procs),
            SYS_DUP2 => self.sys_dup2(args, procs),
            SYS_FCNTL => self.sys_fcntl(args, procs),
            SYS_IOCTL => self.sys_ioctl(args, procs),
            SYS_MKDIR => self.sys_mkdir(args, procs, vfs),
            SYS_UNLINKAT => self.sys_unlinkat(args, procs, vfs),
            SYS_MOUNT => self.sys_mount(args, procs, vfs),
            SYS_UMOUNT2 => self.sys_umount(args, procs, vfs),
            SYS_STATFS => self.sys_statfs(args, procs, vfs),
            SYS_FACCESSAT => self.sys_faccessat(args, procs, vfs),
            SYS_OPENAT => self.sys_openat(args, procs, vfs),
            SYS_CLOSE => self.sys_close(args, procs),
            SYS_PIPE => self.sys_pipe(args, procs, vfs),
            SYS_GETDENTS64 => self.sys_getdents64(args, procs, vfs),
            SYS_LSEEK => self.sys_lseek(args, procs, vfs),
            SYS_READ => self.sys_read(args, procs, vfs),
            SYS_WRITE => self.sys_write(args, procs, vfs),
            SYS_READV => self.sys_readv(args, procs, vfs),
            SYS_WRITEV => self.sys_writev(args, procs, vfs),
            SYS_PREAD64 => self.sys_pread64(args, procs, vfs),
            SYS_SENDFILE => self.sys_sendfile(args, procs, vfs),
            SYS_PPOLL => self.sys_ppoll(args, procs),
            SYS_SPLICE => self.sys_splice(args, procs, vfs),
            SYS_READLINKAT => self.sys_readlinkat(args, procs, vfs),
            SYS_FSTATAT => self.sys_fstatat(args, procs, vfs),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(args, procs),
            SYS_FUTEX => self.sys_futex(args, procs),
            SYS_SET_ROBUST_LIST => self.sys_set_robust_list(args, procs),
            SYS_SLEEP | SYS_NANOSLEEP => self.sys_nanosleep(args),
            SYS_SCHED_YIELD => self.sys_sched_yield(scheduler),
            SYS_CLOCK_GETTIME => self.sys_clock_gettime(args, procs),
            SYS_SYSLOG => self.sys_syslog(args, procs),
            SYS_KILL => self.sys_kill(args),
            SYS_SIGACTION => self.sys_sigaction(args, procs),
            SYS_SIGPROCMASK => self.sys_sigprocmask(args, procs),
            SYS_RT_SIGTIMEDWAIT => self.sys_rt_sigtimedwait(),
            SYS_SETGID => self.sys_setgid(),
            SYS_SETUID => self.sys_setuid(),
            SYS_TIMES => self.sys_times(args, procs),
            SYS_SETPGID => self.sys_setpgid(args, procs),
            SYS_GETPGID => self.sys_getpgid(args, procs),
            SYS_UNAME => self.sys_uname(args, procs),
            SYS_GETRUSAGE => self.sys_getrusage(args, procs),
            SYS_GETTIMEOFDAY => self.sys_gettimeofday(args, procs),
            SYS_EXIT => self.sys_exit(args, procs, scheduler),
            SYS_EXIT_GROUP => self.sys_exit(args, procs, scheduler),
            SYS_GETPID => self.sys_getpid(procs),
            SYS_GETPPID => self.sys_getppid(procs),
            SYS_GETUID => Ok(0),
            SYS_GETEUID => Ok(0),
            SYS_GETGID => Ok(0),
            SYS_GETEGID => Ok(0),
            SYS_GETTID => self.sys_gettid(procs),
            SYS_SYSINFO => self.sys_sysinfo(args, procs, scheduler),
            SYS_SOCKET => Err(97),
            SYS_BIND | SYS_LISTEN | SYS_ACCEPT | SYS_CONNECT | SYS_GETSOCKNAME | SYS_SENDTO
            | SYS_RECVFROM | SYS_SETSOCKOPT => Err(EBADF),
            SYS_FSTAT => self.sys_fstat(args, procs, vfs),
            SYS_CHDIR => self.sys_chdir(args, procs, vfs),
            SYS_BRK => self.sys_brk(args, procs),
            SYS_MREMAP => self.sys_mremap(args, procs),
            SYS_CLONE => self.sys_fork(procs, scheduler),
            SYS_EXECVE => self.sys_execve(args, procs),
            SYS_MMAP => self.sys_mmap(args, procs),
            SYS_MUNMAP => self.sys_munmap(args, procs),
            SYS_MPROTECT => self.sys_mprotect(args, procs),
            SYS_MADVISE => Ok(0),
            SYS_WAIT => self.sys_wait(args, procs),
            SYS_PRLIMIT64 => self.sys_prlimit64(args, procs),
            SYS_RENAMEAT2 => self.sys_renameat2(args, procs, vfs),
            SYS_GETRANDOM => self.sys_getrandom(args, procs),
            SYS_COPY_FILE_RANGE => self.sys_copy_file_range(args, procs, vfs),
            SYS_STATX => self.sys_statx(args, procs, vfs),
            SYS_FTRUNCATE => self.sys_ftruncate(args, procs, vfs),
            SYS_TGKILL => Ok(0),
            SYS_CLONE3 => Err(ENOSYS),
            SYS_POWER_OFF => Ok(0),
            _ => Err(ENOSYS),
        };

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
        let path = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.mkdir(&cwd, &path, args.0[1] as u32)?;
        Ok(0)
    }

    fn sys_unlinkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
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
        let source = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
        let target = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let fs_type = procs.current()?.read_user_cstr(args.0[2]).map_err(|_| EFAULT)?;
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
        let target = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
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
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
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
        procs.current_mut()?.write_user_bytes(buf, trimmed).map_err(|_| EFAULT)?;
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
        procs.current_mut()?.write_user_bytes(buf, &bytes).map_err(|_| EFAULT)?;
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
        let data = procs.current()?.read_user_bytes(buf, count).map_err(|_| EFAULT)?;
        match fd {
            1 | 2 => {
                for byte in data.iter().copied() {
                    hal().console.put_byte(byte);
                }
                Ok(data.len())
            }
            _ => {
                let process = procs.current_mut()?;
                let handle = process.fd_mut(fd)?;
                vfs.write(handle, &data)
            }
        }
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
    ) -> Result<usize, i32> {
        procs.exit_current(args.0[0] as i32)?;
        scheduler.exit_current();
        Ok(0)
    }

    fn sys_getpid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        Ok(procs.current()?.pid)
    }

    fn sys_getppid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        procs.getppid()
    }

    fn sys_gettid(&self, procs: &ProcessTable) -> Result<usize, i32> {
        procs.gettid()
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
            vfs.stat_path("/", &handle.path)?
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
        let path = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let new_cwd = vfs.chdir(&cwd, &path)?;
        procs.current_mut()?.cwd = new_cwd;
        Ok(0)
    }

    fn sys_brk(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let requested = args.0[0];
        let process = procs.current_mut()?;
        process.address_space.brk((requested != 0).then_some(requested))
    }

    fn sys_fork(
        &self,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let name = procs.current()?.name.clone();
        let pid = procs.fork_current()?;
        scheduler.spawn(&name, pid);
        Ok(pid)
    }

    fn sys_execve(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
        let program = builtin_program(&path).ok_or(ENOENT)?;
        let entry = program.image.as_ptr() as usize + program.entry;
        let _argv = args.0[1];
        let _envp = args.0[2];
        procs.execve_current(entry)?;
        Ok(0)
    }

    fn sys_mmap(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _addr = args.0[0];
        let len = args.0[1];
        let prot = args.0[2];
        let _flags = args.0[3];
        let _fd = args.0[4];
        let _offset = args.0[5];
        procs.current_mut()?.address_space.map_anonymous(len, prot)
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
        procs.current_mut()?.address_space.mprotect(addr, len, prot)?;
        Ok(0)
    }

    fn sys_wait(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let wait_pid = args.0[0] as i32;
        let status_ptr = args.0[1];
        let _options = args.0[2];
        let _rusage = args.0[3];
        let parent_pid = procs.current_pid()?;
        let (child_pid, status) = procs.wait(parent_pid, wait_pid)?;
        if status_ptr != 0 {
            procs
                .current_mut()?
                .write_user_bytes(status_ptr, &(status as i32).to_le_bytes())
                .map_err(|_| EFAULT)?;
        }
        Ok(child_pid)
    }

    fn sys_dup(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let handle = procs.current()?.fd(args.0[0] as i32)?.clone();
        Ok(procs.current_mut()?.add_fd(handle) as usize)
    }

    fn sys_dup2(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let oldfd = args.0[0] as i32;
        let newfd = args.0[1] as i32;
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
            procs.current_mut()?.write_user_bytes(arg, &winsz).map_err(|_| EFAULT)?;
        }
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
        process.write_user_bytes(args.0[0], &bytes).map_err(|_| EFAULT)?;
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
            procs.current_mut()?.write_user_bytes(iov.iov_base, &bytes).map_err(|_| EFAULT)?;
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
            let bytes = procs.current()?.read_user_bytes(iov.iov_base, iov.iov_len).map_err(|_| EFAULT)?;
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
        procs.current_mut()?.write_user_bytes(buf, &bytes).map_err(|_| EFAULT)?;
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

    fn sys_splice(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        self.sys_copy_between_fds(args.0[0] as i32, args.0[2] as i32, args.0[4], args.0[1], args.0[3], procs, vfs)
    }

    fn sys_readlinkat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let target = match path.as_str() {
            "/proc/self/exe" => String::from("/bin/init"),
            "/proc/self/cwd" => cwd.clone(),
            _ => {
                vfs.access(&cwd, &path)?;
                let resolved = if path.starts_with('/') {
                    path.clone()
                } else {
                    format!("{}/{}", cwd.trim_end_matches('/'), path)
                };
                vfs.cwd_string(&resolved)
            }
        };
        let bytes = target.as_bytes();
        let len = bytes.len().min(args.0[3]);
        procs.current_mut()?.write_user_bytes(args.0[2], &bytes[..len]).map_err(|_| EFAULT)?;
        Ok(len)
    }

    fn sys_fstatat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let stat = vfs.stat_path(&cwd, &path)?;
        procs.current_mut()?.write_user_bytes(args.0[2], &stat_to_bytes(stat)).map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_statfs(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[0]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        vfs.access(&cwd, &path)?;
        procs.current_mut()?.write_user_bytes(args.0[1], &statfs_bytes()).map_err(|_| EFAULT)?;
        Ok(0)
    }

    fn sys_faccessat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let _mode = args.0[2];
        let _flags = args.0[3];
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_kill(&self, args: SyscallArgs) -> Result<usize, i32> {
        let _pid = args.0[0];
        let sig = args.0[1];
        if sig > 64 {
            return Err(EINVAL);
        }
        Ok(0)
    }

    fn sys_sigaction(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _sig = args.0[0];
        let _new = args.0[1];
        let old = args.0[2];
        if old != 0 {
            procs.current_mut()?.write_user_bytes(old, &[0; 32]).map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_sigprocmask(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let _how = args.0[0];
        let _set = args.0[1];
        let old = args.0[2];
        let sigset_size = args.0[3].max(8);
        if old != 0 {
            procs
                .current_mut()?
                .write_user_bytes(old, &vec![0; sigset_size])
                .map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_rt_sigtimedwait(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_times(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        if args.0[0] != 0 {
            procs.current_mut()?.write_user_bytes(args.0[0], &[0; 32]).map_err(|_| EFAULT)?;
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
        let ts = hal().timer.monotonic_time();
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
                procs.current_mut()?.write_user_bytes(buf, &message[..written]).map_err(|_| EFAULT)?;
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

    fn sys_setgid(&self) -> Result<usize, i32> {
        Ok(0)
    }

    fn sys_setuid(&self) -> Result<usize, i32> {
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
        process.address_space.write_bytes(new_addr, &bytes).map_err(|_| EFAULT)?;
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
            procs.current_mut()?.write_user_bytes(old_limit, &bytes).map_err(|_| EFAULT)?;
        }
        Ok(0)
    }

    fn sys_renameat2(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let old_path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let new_path = procs.current()?.read_user_cstr(args.0[3]).map_err(|_| EFAULT)?;
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
        procs.current_mut()?.write_user_bytes(buf, &bytes).map_err(|_| EFAULT)?;
        Ok(len)
    }

    fn sys_copy_file_range(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        self.sys_copy_between_fds(args.0[0] as i32, args.0[2] as i32, args.0[4], args.0[1], args.0[3], procs, vfs)
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
            procs.current_mut()?.write_user_bytes(off_in_ptr, &in_handle.offset.to_le_bytes()).map_err(|_| EFAULT)?;
        } else {
            procs.current_mut()?.fd_mut(in_fd)?.offset = in_handle.offset;
        }
        if off_out_ptr != 0 {
            procs.current_mut()?.write_user_bytes(off_out_ptr, &out_handle.offset.to_le_bytes()).map_err(|_| EFAULT)?;
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
        let path = procs.current()?.read_user_cstr(args.0[1]).map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let stat = vfs.stat_path(&cwd, &path)?;
        let bytes = statx_bytes(stat);
        procs.current_mut()?.write_user_bytes(args.0[4], &bytes).map_err(|_| EFAULT)?;
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

    fn sys_futex(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        const FUTEX_WAIT: usize = 0;
        const FUTEX_WAKE: usize = 1;
        let uaddr = args.0[0];
        let op = args.0[1] & 0x7f;
        let val = args.0[2] as i32;
        match op {
            FUTEX_WAIT => {
                let current = read_i32(procs.current()?, uaddr)?;
                if current != val {
                    Err(EAGAIN)
                } else {
                    Ok(0)
                }
            }
            FUTEX_WAKE => Ok(0),
            _ => Ok(0),
        }
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
        procs.current_mut()?.write_user_bytes(args.0[0], &bytes).map_err(|_| EFAULT)?;
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

fn stat_to_bytes(stat: FileStat) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&stat.mode.to_le_bytes());
    out[4..12].copy_from_slice(&stat.size.to_le_bytes());
    out[12..].copy_from_slice(&stat.nlink.to_le_bytes());
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

fn timeval_bytes(ts: Timespec) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&ts.tv_sec.to_le_bytes());
    out[8..].copy_from_slice(&(ts.tv_nsec / 1_000).to_le_bytes());
    out
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
        SyscallArgs, SyscallDispatcher, SYS_GETCWD, SYS_LSEEK, SYS_MKDIR, SYS_OPENAT, SYS_PIPE,
        SYS_PREAD64, SYS_READ, SYS_WRITE, SYS_WRITEV,
    };
    use proc::ProcessTable;
    use task::Scheduler;
    use vfs::KernelVfs;

    #[test]
    fn basic_phase1_syscalls() {
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs.current_mut().unwrap().address_space.install_bytes(0x1000, b"/work\0");
        assert_eq!(
            dispatcher.dispatch(SYS_MKDIR, SyscallArgs([0x1000, 0o755, 0, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            0
        );

        procs.current_mut().unwrap().address_space.install_bytes(0x2000, b"/work/log.txt\0");
        let fd = dispatcher.dispatch(
            SYS_OPENAT,
            SyscallArgs([!0usize, 0x2000, (vfs::O_CREAT | vfs::O_RDWR) as usize, 0o644, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert!(fd >= 3);

        procs.current_mut().unwrap().address_space.install_bytes(0x3000, b"hello");
        assert_eq!(
            dispatcher.dispatch(SYS_WRITE, SyscallArgs([fd as usize, 0x3000, 5, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            5
        );

        procs.current_mut().unwrap().address_space.install_bytes(0x4000, &[0; 64]);
        assert_eq!(
            dispatcher.dispatch(SYS_GETCWD, SyscallArgs([0x4000, 64, 0, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            0x4000
        );
    }

    #[test]
    fn extended_syscall_smoke() {
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs.current_mut().unwrap().address_space.install_bytes(0x1000, b"/tmp/ext.txt\0");
        let fd = dispatcher.dispatch(
            SYS_OPENAT,
            SyscallArgs([!0usize, 0x1000, (vfs::O_CREAT | vfs::O_RDWR) as usize, 0o644, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;

        procs.current_mut().unwrap().address_space.install_bytes(0x2000, b"he");
        procs.current_mut().unwrap().address_space.install_bytes(0x3000, b"llo");
        let mut iov = [0u8; 32];
        iov[..8].copy_from_slice(&0x2000usize.to_le_bytes());
        iov[8..16].copy_from_slice(&2usize.to_le_bytes());
        iov[16..24].copy_from_slice(&0x3000usize.to_le_bytes());
        iov[24..32].copy_from_slice(&3usize.to_le_bytes());
        procs.current_mut().unwrap().address_space.install_bytes(0x4000, &iov);
        assert_eq!(
            dispatcher.dispatch(SYS_WRITEV, SyscallArgs([fd, 0x4000, 2, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            5
        );

        assert_eq!(
            dispatcher.dispatch(SYS_LSEEK, SyscallArgs([fd, 0, 0, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            0
        );
        procs.current_mut().unwrap().address_space.install_bytes(0x5000, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(SYS_PREAD64, SyscallArgs([fd, 0x5000, 5, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            5
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x5000, 5).unwrap(),
            b"hello"
        );

        procs.current_mut().unwrap().address_space.install_bytes(0x6000, &[0; 8]);
        assert_eq!(
            dispatcher.dispatch(SYS_PIPE, SyscallArgs([0x6000, 0, 0, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            0
        );
        let pipe_fds = procs.current().unwrap().read_user_bytes(0x6000, 8).unwrap();
        let read_fd = i32::from_le_bytes(pipe_fds[..4].try_into().unwrap()) as usize;
        let write_fd = i32::from_le_bytes(pipe_fds[4..8].try_into().unwrap()) as usize;
        procs.current_mut().unwrap().address_space.install_bytes(0x7000, b"ping");
        assert_eq!(
            dispatcher.dispatch(SYS_WRITE, SyscallArgs([write_fd, 0x7000, 4, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            4
        );
        procs.current_mut().unwrap().address_space.install_bytes(0x7100, &[0; 4]);
        assert_eq!(
            dispatcher.dispatch(SYS_READ, SyscallArgs([read_fd, 0x7100, 4, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            4
        );
        assert_eq!(
            procs.current().unwrap().read_user_bytes(0x7100, 4).unwrap(),
            b"ping"
        );
    }
}
