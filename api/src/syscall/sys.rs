use alloc::vec;
use core::ffi::c_char;

use axerrno::{LinuxError, LinuxResult};
use axfs_ng::FS_CONTEXT;
use axtask::current;
use linux_raw_sys::{
    general::{GRND_INSECURE, GRND_NONBLOCK, GRND_RANDOM},
    system::{new_utsname, sysinfo},
};
use starry_core::task::{AsThread, processes};
use starry_vm::{VmMutPtr, vm_write_slice};

pub fn sys_getuid() -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.credentials().ruid as _)
}

pub fn sys_geteuid() -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.credentials().euid as _)
}

pub fn sys_getgid() -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.credentials().rgid as _)
}

pub fn sys_getegid() -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.credentials().egid as _)
}

pub fn sys_getresuid(ruid: *mut u32, euid: *mut u32, suid: *mut u32) -> LinuxResult<isize> {
    let creds = current().as_thread().proc_data.credentials();
    ruid.vm_write(creds.ruid)?;
    euid.vm_write(creds.euid)?;
    suid.vm_write(creds.suid)?;
    Ok(0)
}

pub fn sys_getresgid(rgid: *mut u32, egid: *mut u32, sgid: *mut u32) -> LinuxResult<isize> {
    let creds = current().as_thread().proc_data.credentials();
    rgid.vm_write(creds.rgid)?;
    egid.vm_write(creds.egid)?;
    sgid.vm_write(creds.sgid)?;
    Ok(0)
}

pub fn sys_setuid(uid: u32) -> LinuxResult<isize> {
    debug!("sys_setuid <= uid: {}", uid);
    current().as_thread().proc_data.setuid(uid)?;
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> LinuxResult<isize> {
    debug!("sys_setgid <= gid: {}", gid);
    current().as_thread().proc_data.setgid(gid)?;
    Ok(0)
}

pub fn sys_setfsuid(uid: u32) -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.setfsuid(uid) as isize)
}

pub fn sys_setfsgid(gid: u32) -> LinuxResult<isize> {
    Ok(current().as_thread().proc_data.setfsgid(gid) as isize)
}

pub fn sys_getgroups(size: usize, list: *mut u32) -> LinuxResult<isize> {
    debug!("sys_getgroups <= size: {}", size);
    let gid = current().as_thread().proc_data.credentials().egid;
    if size == 0 {
        return Ok(1);
    }
    if size < 1 {
        return Err(LinuxError::EINVAL);
    }
    vm_write_slice(list, &[gid])?;
    Ok(1)
}

pub fn sys_setgroups(_size: usize, _list: *const u32) -> LinuxResult<isize> {
    Ok(0)
}

const fn pad_str(info: &str) -> [c_char; 65] {
    let mut data: [c_char; 65] = [0; 65];
    // this needs #![feature(const_copy_from_slice)]
    // data[..info.len()].copy_from_slice(info.as_bytes());
    unsafe {
        core::ptr::copy_nonoverlapping(info.as_ptr().cast(), data.as_mut_ptr(), info.len());
    }
    data
}

const UTSNAME: new_utsname = new_utsname {
    sysname: pad_str("Linux"),
    nodename: pad_str("starry"),
    release: pad_str("10.0.0"),
    version: pad_str("10.0.0"),
    machine: pad_str("riscv64"),
    domainname: pad_str("https://github.com/Starry-Mix-THU/starry-mix"),
};

pub fn sys_uname(name: *mut new_utsname) -> LinuxResult<isize> {
    name.vm_write(UTSNAME)?;
    Ok(0)
}

pub fn sys_sysinfo(info: *mut sysinfo) -> LinuxResult<isize> {
    // FIXME: Zeroable
    let mut kinfo: sysinfo = unsafe { core::mem::zeroed() };
    kinfo.procs = processes().len() as _;
    kinfo.mem_unit = 1;
    info.vm_write(kinfo)?;
    Ok(0)
}

pub fn sys_syslog(_type: i32, _buf: *mut c_char, _len: usize) -> LinuxResult<isize> {
    Ok(0)
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct GetRandomFlags: u32 {
        const NONBLOCK = GRND_NONBLOCK;
        const RANDOM = GRND_RANDOM;
        const INSECURE = GRND_INSECURE;
    }
}

pub fn sys_getrandom(buf: *mut u8, len: usize, flags: u32) -> LinuxResult<isize> {
    if len == 0 {
        return Ok(0);
    }
    let flags = GetRandomFlags::from_bits_retain(flags);

    debug!(
        "sys_getrandom <= buf: {:p}, len: {}, flags: {:?}",
        buf, len, flags
    );

    let path = if flags.contains(GetRandomFlags::RANDOM) {
        "/dev/random"
    } else {
        "/dev/urandom"
    };

    let f = FS_CONTEXT.lock().resolve(path)?;
    let mut kbuf = vec![0; len];
    let len = f.entry().as_file()?.read_at(&mut kbuf, 0)?;

    vm_write_slice(buf, &kbuf)?;

    Ok(len as _)
}

pub fn sys_seccomp(_op: u32, _flags: u32, _args: *const ()) -> LinuxResult<isize> {
    warn!("dummy sys_seccomp");
    Ok(0)
}

#[cfg(target_arch = "riscv64")]
pub fn sys_riscv_flush_icache() -> LinuxResult<isize> {
    riscv::asm::fence_i();
    Ok(0)
}
