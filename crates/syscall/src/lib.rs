#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use hal_api::{hal, Timespec};
use proc::ProcessTable;
use task::Scheduler;
use vfs::{FileStat, KernelVfs, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};

const EBADF: i32 = 9;
const ECHILD: i32 = 10;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENOSYS: i32 = 38;

pub const SYS_GETCWD: usize = 17;
pub const SYS_MKDIR: usize = 34;
pub const SYS_UNLINKAT: usize = 35;
pub const SYS_UMOUNT2: usize = 39;
pub const SYS_MOUNT: usize = 40;
pub const SYS_OPENAT: usize = 56;
pub const SYS_CLOSE: usize = 57;
pub const SYS_GETDENTS64: usize = 61;
pub const SYS_LSEEK: usize = 62;
pub const SYS_READ: usize = 63;
pub const SYS_WRITE: usize = 64;
pub const SYS_SCHED_YIELD: usize = 124;
pub const SYS_CLOCK_GETTIME: usize = 113;
pub const SYS_NANOSLEEP: usize = 101;
pub const SYS_EXIT: usize = 93;
pub const SYS_GETPID: usize = 172;
pub const SYS_FSTAT: usize = 80;
pub const SYS_CHDIR: usize = 49;
pub const SYS_BRK: usize = 214;
pub const SYS_MUNMAP: usize = 215;
pub const SYS_MMAP: usize = 222;
pub const SYS_MPROTECT: usize = 226;

#[derive(Clone, Copy, Debug)]
pub struct SyscallArgs(pub [usize; 6]);

pub struct SyscallDispatcher;

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
            SYS_MKDIR => self.sys_mkdir(args, procs, vfs),
            SYS_UNLINKAT => self.sys_unlinkat(args, procs, vfs),
            SYS_MOUNT => self.sys_mount(args, procs, vfs),
            SYS_UMOUNT2 => self.sys_umount(args, procs, vfs),
            SYS_OPENAT => self.sys_openat(args, procs, vfs),
            SYS_CLOSE => self.sys_close(args, procs),
            SYS_GETDENTS64 => self.sys_getdents64(args, procs, vfs),
            SYS_LSEEK => self.sys_lseek(args, procs, vfs),
            SYS_READ => self.sys_read(args, procs, vfs),
            SYS_WRITE => self.sys_write(args, procs, vfs),
            SYS_SCHED_YIELD => self.sys_sched_yield(scheduler),
            SYS_CLOCK_GETTIME => self.sys_clock_gettime(args, procs),
            SYS_NANOSLEEP => self.sys_nanosleep(args),
            SYS_EXIT => self.sys_exit(args, procs, scheduler),
            SYS_GETPID => self.sys_getpid(procs),
            SYS_FSTAT => self.sys_fstat(args, procs, vfs),
            SYS_CHDIR => self.sys_chdir(args, procs, vfs),
            SYS_BRK => self.sys_brk(args, procs),
            SYS_MMAP => self.sys_mmap(args, procs),
            SYS_MUNMAP => self.sys_munmap(args, procs),
            SYS_MPROTECT => self.sys_mprotect(args, procs),
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

#[cfg(test)]
mod tests {
    use super::{SyscallArgs, SyscallDispatcher, SYS_GETCWD, SYS_MKDIR, SYS_OPENAT, SYS_WRITE};
    use proc::ProcessTable;
    use task::Scheduler;
    use vfs::KernelVfs;

    #[test]
    fn basic_phase1_syscalls() {
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init");
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        procs.current_mut().unwrap().address_space.install_bytes(0x1000, b"/tmp\0");
        assert_eq!(
            dispatcher.dispatch(SYS_MKDIR, SyscallArgs([0x1000, 0o755, 0, 0, 0, 0]), &mut procs, &mut scheduler, &mut vfs),
            0
        );

        procs.current_mut().unwrap().address_space.install_bytes(0x2000, b"/tmp/log.txt\0");
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
}

