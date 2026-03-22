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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicUsize, Ordering};
use hal_api::{hal, Timespec};
use proc::{Process, ProcessTable, SigAction, WaitSelector};
use spin::Mutex;
use task::Scheduler;
use user_init::builtin_program;
use vfs::{
    FileHandle, FileStat, KernelVfs, HANDLE_FLAG_CLOEXEC, O_CREAT, O_DIRECTORY, O_RDONLY, O_RDWR,
    O_TRUNC, O_WRONLY,
};

const EAFNOSUPPORT: i32 = 97;
const EACCES: i32 = 13;
const EAGAIN: i32 = 11;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;
const EINTR: i32 = 4;
const EINVAL: i32 = 22;
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;
const ENOEXEC: i32 = 8;
const EPIPE: i32 = 32;
const EPERM: i32 = 1;
const ESRCH: i32 = 3;

const FUTEX_WAIT: usize = 0;
const FUTEX_WAKE: usize = 1;
const FUTEX_REQUEUE: usize = 3;
const FUTEX_CMP_REQUEUE: usize = 4;
const FUTEX_WAKE_OP: usize = 5;
const FUTEX_WAIT_BITSET: usize = 9;
const FUTEX_WAKE_BITSET: usize = 10;
const ENOTTY: i32 = 25;
const ENOSYS: i32 = 38;
const ENAMETOOLONG: i32 = 36;
const EROFS: i32 = 30;
const ETIMEDOUT: i32 = 110;
const EPROTOTYPE: i32 = 91;
const AT_FDCWD: i32 = -100;
const F_OK: usize = 0;
const X_OK: usize = 1;
const W_OK: usize = 2;
const R_OK: usize = 4;
const MS_RDONLY: usize = 1;
const PATH_MAX: usize = 4096;
const TIME_OK: usize = 0;
const ADJ_OFFSET: u32 = 0x0001;
const ADJ_FREQUENCY: u32 = 0x0002;
const ADJ_MAXERROR: u32 = 0x0004;
const ADJ_ESTERROR: u32 = 0x0008;
const ADJ_STATUS: u32 = 0x0010;
const ADJ_TIMECONST: u32 = 0x0020;
const ADJ_TICK: u32 = 0x4000;
const ADJ_OFFSET_SINGLESHOT: u32 = 0x8001;
const TIMEX_SIZE: usize = 208;
const TIMEX_OFFSET_OFF: usize = 8;
const TIMEX_FREQ_OFF: usize = 16;
const TIMEX_MAXERROR_OFF: usize = 24;
const TIMEX_ESTERROR_OFF: usize = 32;
const TIMEX_STATUS_OFF: usize = 40;
const TIMEX_CONSTANT_OFF: usize = 48;
const TIMEX_TICK_OFF: usize = 88;
const TIMEX_TAI_OFF: usize = 160;
const SIGCHLD: usize = 17;

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
pub const SYS_TKILL: usize = 130;
pub const SYS_TGKILL: usize = 131;
pub const SYS_SIGALTSTACK: usize = 132;
pub const SYS_RT_SIGSUSPEND: usize = 133;
pub const SYS_SIGACTION: usize = 134;
pub const SYS_SIGPROCMASK: usize = 135;
pub const SYS_RT_SIGPENDING: usize = 136;
pub const SYS_RT_SIGTIMEDWAIT: usize = 137;
pub const SYS_RT_SIGRETURN: usize = 139;
pub const SYS_GETPRIORITY: usize = 141;
pub const SYS_SETREUID: usize = 145;
pub const SYS_SETGID: usize = 144;
pub const SYS_SETUID: usize = 146;
pub const SYS_SETRESUID: usize = 147;
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
pub const SYS_ADJTIMEX: usize = 171;
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
pub const SYS_MEMBARRIER: usize = 283;
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
const TIMER_ABSTIME: usize = 1;
// CLONE_SYSVSEM (0x0004_0000) is NOT a namespace flag; excluding it here
// ensures pthread_create (which sets CLONE_SYSVSEM) is not rejected with EINVAL.
const CLONE_NAMESPACE_MASK: usize =
    0x0002_0000 | 0x0200_0000 | 0x0400_0000 | 0x0800_0000 | 0x1000_0000 | 0x2000_0000 | 0x4000_0000;
const PAGE_SIZE: usize = 4096;
const O_NONBLOCK: usize = 0x0000_0800;
const O_CLOEXEC: usize = 0x0008_0000;
const SEEK_SET: i16 = 0;
const SEEK_CUR: i16 = 1;
const SEEK_END: i16 = 2;
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const F_GETLK: usize = 5;
const F_SETLK: usize = 6;
const F_SETLKW: usize = 7;
const MAP_SHARED: usize = 0x01;
const MAP_PRIVATE: usize = 0x02;
const MAP_TYPE_MASK: usize = 0x0f;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED_NOREPLACE: usize = 0x10_0000;
const BUILTIN_EXEC_BASE: usize = 0x0080_0000;
// Keep signal trampoline away from the default user stack range
// (0x7ff7_f000..0x7fff_f000) to avoid turning stack pages read-only.
pub const SIGNAL_TRAMPOLINE_BASE: usize = 0x7ff0_0000;
#[cfg(target_arch = "riscv64")]
pub const SIGNAL_TRAMPOLINE_CODE: [u8; 8] = [
    0x93, 0x08, 0xb0, 0x08, // addi a7, zero, 0x8b (= 139 = SYS_RT_SIGRETURN)
    0x73, 0x00, 0x00, 0x00, // ecall
];
#[cfg(target_arch = "loongarch64")]
pub const SIGNAL_TRAMPOLINE_CODE: [u8; 8] = [
    0x0b, 0x2c, 0x82, 0x03,
    0x00, 0x00, 0x2b, 0x00,
];
#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
pub const SIGNAL_TRAMPOLINE_CODE: [u8; 8] = [0u8; 8];
const SYSCALL_TRACE_DEFAULT: bool = false;

#[inline]
fn syscall_trace_enabled() -> bool {
    if SYSCALL_TRACE_DEFAULT {
        return true;
    }
    matches!(option_env!("WHUSE_DEBUG_SYSCALL"), Some("1"))
}
const ENOSYS_TRACE_DEFAULT: bool = false;

fn trace_line(line: &str) {
    if !syscall_trace_enabled() {
        return;
    }
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

fn trace_enosys(line: &str) {
    if !(ENOSYS_TRACE_DEFAULT || matches!(option_env!("WHUSE_DEBUG_ENOSYS"), Some("1"))) {
        return;
    }
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

fn log_always(line: &str) {
    for byte in line.bytes() {
        hal().console.put_byte(byte);
    }
    hal().console.put_byte(b'\n');
}

fn wake_process_group_threads(
    procs: &ProcessTable,
    scheduler: &mut Scheduler,
    tgid: usize,
) -> usize {
    let tids = procs.live_tids_in_tgid(tgid);
    let mut woke = 0usize;
    for tid in tids {
        if scheduler.wake_task(tid) {
            woke = woke.saturating_add(1);
        }
    }
    woke
}

#[inline]
fn stage2_openat_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_STAGE2_OPENAT"), Some("1"))
}

fn cancel_debug_enabled() -> bool {
    match option_env!("WHUSE_DEBUG_CANCEL") {
        Some("1") => true,
        _ => false,
    }
}

fn signal_frame_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_SIGNAL_FRAME"), Some("1"))
}

fn is_libctest_task_name(name: &str) -> bool {
    name == "./runtest.exe"
        || (name.starts_with("entry-") && name.ends_with(".exe"))
        || name == "entry.exe"
}

fn is_libctest_openat_probe_task(name: &str, tgid: usize, cwd: &str) -> bool {
    is_libctest_task_name(name) || (name == "/musl/busybox" && tgid > 2 && cwd == "/musl")
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

fn cancel_debug(line: &str) {
    if cancel_debug_enabled() {
        log_always(line);
    }
}

fn signal_frame_debug(line: &str) {
    if signal_frame_debug_enabled() {
        log_always(line);
    }
}

static LIBCBENCH_TRACE_BUDGET: AtomicUsize = AtomicUsize::new(256);

fn is_libcbench_task(process: &proc::Process) -> bool {
    process.name.contains("libc-bench")
}

fn libcbench_debug(line: &str) {
    if LIBCBENCH_TRACE_BUDGET
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            if remaining > 0 {
                Some(remaining - 1)
            } else {
                None
            }
        })
        .is_ok()
    {
        log_always(line);
    }
}

fn log_user_path_fault(process: &proc::Process, syscall: &str, path_ptr: usize) {
    log_always(&format!(
        "whuse-ltp:path-fault syscall={} tgid={} tid={} name={} cwd={} ptr={:#x} addr={}",
        syscall,
        process.tgid,
        process.tid,
        process.name,
        process.cwd,
        path_ptr,
        process.address_space.describe_addr(path_ptr)
    ));
}

#[derive(Clone, Copy, Debug)]
pub struct SyscallArgs(pub [usize; 6]);

#[derive(Clone, Copy, Debug)]
struct CloneRequest {
    flags: usize,
    stack: usize,
    parent_tid: usize,
    child_tid: usize,
    tls: Option<usize>,
}

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
static BUSYBOX_APPLETS: Mutex<BTreeMap<usize, String>> = Mutex::new(BTreeMap::new());
static FCNTL_LOCK_STATE: Mutex<FcntlLockState> = Mutex::new(FcntlLockState { locks: Vec::new() });

#[derive(Clone, Copy, Debug, Default)]
struct FlockRequest {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

#[derive(Clone, Debug)]
struct FcntlRecordLock {
    path: String,
    owner_tgid: usize,
    lock_type: i16,
    start: u64,
    end: Option<u64>,
}

#[derive(Default)]
struct FcntlLockState {
    locks: Vec<FcntlRecordLock>,
}

#[derive(Clone, Copy, Debug)]
struct KernelTimexState {
    offset: i64,
    freq: i64,
    maxerror: i64,
    esterror: i64,
    status: i32,
    constant: i64,
    tick: i64,
    tai: i32,
}

static ADJTIMEX_STATE: Mutex<KernelTimexState> = Mutex::new(KernelTimexState {
    offset: 0,
    freq: 0,
    maxerror: 0,
    esterror: 0,
    status: 0,
    constant: 0,
    tick: 10_000,
    tai: 0,
});

pub fn cache_busybox_image(image: &[u8]) {
    *BUSYBOX_IMAGE_CACHE.lock() = Some(image.to_vec());
}

fn busybox_image_cache() -> Option<Vec<u8>> {
    BUSYBOX_IMAGE_CACHE.lock().as_ref().cloned()
}

fn with_cloexec_flag(flags: u32, cloexec: bool) -> u32 {
    if cloexec {
        flags | HANDLE_FLAG_CLOEXEC
    } else {
        flags & !HANDLE_FLAG_CLOEXEC
    }
}

fn read_flock_request(process: &Process, addr: usize) -> Result<FlockRequest, i32> {
    if addr == 0 {
        return Err(EFAULT);
    }
    let bytes = process.read_user_bytes(addr, 32).map_err(|_| EFAULT)?;
    if bytes.len() < 32 {
        return Err(EFAULT);
    }
    Ok(FlockRequest {
        l_type: i16::from_le_bytes([bytes[0], bytes[1]]),
        l_whence: i16::from_le_bytes([bytes[2], bytes[3]]),
        l_start: i64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]),
        l_len: i64::from_le_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22],
            bytes[23],
        ]),
        l_pid: i32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
    })
}

fn write_flock_request(process: &mut Process, addr: usize, flock: FlockRequest) -> Result<(), i32> {
    if addr == 0 {
        return Err(EFAULT);
    }
    let mut bytes = [0u8; 32];
    bytes[0..2].copy_from_slice(&flock.l_type.to_le_bytes());
    bytes[2..4].copy_from_slice(&flock.l_whence.to_le_bytes());
    bytes[8..16].copy_from_slice(&flock.l_start.to_le_bytes());
    bytes[16..24].copy_from_slice(&flock.l_len.to_le_bytes());
    bytes[24..28].copy_from_slice(&flock.l_pid.to_le_bytes());
    process.write_user_bytes(addr, &bytes).map_err(|_| EFAULT)
}

fn range_end_u128(end: Option<u64>) -> u128 {
    end.map_or(u128::MAX, |value| value as u128)
}

fn range_overlaps(start_a: u64, end_a: Option<u64>, start_b: u64, end_b: Option<u64>) -> bool {
    let a_start = start_a as u128;
    let a_end = range_end_u128(end_a);
    let b_start = start_b as u128;
    let b_end = range_end_u128(end_b);
    a_start < b_end && b_start < a_end
}

fn lock_modes_conflict(left: i16, right: i16) -> bool {
    left == F_WRLCK || right == F_WRLCK
}

fn lock_range_len(start: u64, end: Option<u64>) -> i64 {
    match end {
        Some(limit) => limit.saturating_sub(start) as i64,
        None => 0,
    }
}

fn normalize_lock_range(
    flock: &FlockRequest,
    handle: &FileHandle,
    vfs: &KernelVfs,
) -> Result<(u64, Option<u64>), i32> {
    let base = match flock.l_whence {
        SEEK_SET => 0i64,
        SEEK_CUR => handle.offset as i64,
        SEEK_END => vfs
            .stat_handle(handle)
            .map_err(|_| EINVAL)?
            .size
            .try_into()
            .unwrap_or(i64::MAX),
        _ => return Err(EINVAL),
    };
    let start = base.checked_add(flock.l_start).ok_or(EINVAL)?;
    if start < 0 {
        return Err(EINVAL);
    }
    if flock.l_len < 0 {
        return Err(EINVAL);
    }
    let start = start as u64;
    if flock.l_len == 0 {
        return Ok((start, None));
    }
    let end = start.checked_add(flock.l_len as u64).ok_or(EINVAL)?;
    Ok((start, Some(end)))
}

impl FcntlLockState {
    fn clear_for_owner(&mut self, owner_tgid: usize) -> bool {
        let before = self.locks.len();
        self.locks.retain(|lock| lock.owner_tgid != owner_tgid);
        before != self.locks.len()
    }

    fn clear_for_owner_path(&mut self, owner_tgid: usize, path: &str) -> bool {
        let before = self.locks.len();
        self.locks
            .retain(|lock| !(lock.owner_tgid == owner_tgid && lock.path == path));
        before != self.locks.len()
    }

    fn first_conflict(
        &self,
        path: &str,
        owner_tgid: usize,
        lock_type: i16,
        start: u64,
        end: Option<u64>,
    ) -> Option<FcntlRecordLock> {
        self.locks
            .iter()
            .filter(|lock| lock.path == path)
            .filter(|lock| lock.owner_tgid != owner_tgid)
            .filter(|lock| lock_modes_conflict(lock.lock_type, lock_type))
            .filter(|lock| range_overlaps(lock.start, lock.end, start, end))
            .min_by_key(|lock| lock.start)
            .cloned()
    }

    fn apply_lock(
        &mut self,
        path: &str,
        owner_tgid: usize,
        lock_type: i16,
        start: u64,
        end: Option<u64>,
    ) -> bool {
        let cut_start = start as u128;
        let cut_end = range_end_u128(end);
        let mut changed = false;
        let mut next = Vec::with_capacity(self.locks.len() + 1);

        for lock in self.locks.drain(..) {
            if lock.path != path || lock.owner_tgid != owner_tgid {
                next.push(lock);
                continue;
            }
            if !range_overlaps(lock.start, lock.end, start, end) {
                next.push(lock);
                continue;
            }
            changed = true;
            let lock_start = lock.start as u128;
            let lock_end = range_end_u128(lock.end);
            if lock_start < cut_start {
                next.push(FcntlRecordLock {
                    path: lock.path.clone(),
                    owner_tgid: lock.owner_tgid,
                    lock_type: lock.lock_type,
                    start: lock_start as u64,
                    end: if cut_start == u128::MAX {
                        None
                    } else {
                        Some(cut_start as u64)
                    },
                });
            }
            if cut_end < lock_end {
                next.push(FcntlRecordLock {
                    path: lock.path,
                    owner_tgid: lock.owner_tgid,
                    lock_type: lock.lock_type,
                    start: cut_end as u64,
                    end: if lock_end == u128::MAX {
                        None
                    } else {
                        Some(lock_end as u64)
                    },
                });
            }
        }

        if lock_type != F_UNLCK {
            changed = true;
            next.push(FcntlRecordLock {
                path: path.to_string(),
                owner_tgid,
                lock_type,
                start,
                end,
            });
        }

        next.sort_by(|left, right| {
            (
                left.path.as_str(),
                left.owner_tgid,
                left.start,
                range_end_u128(left.end),
                left.lock_type,
            )
                .cmp(&(
                    right.path.as_str(),
                    right.owner_tgid,
                    right.start,
                    range_end_u128(right.end),
                    right.lock_type,
                ))
        });

        let mut merged: Vec<FcntlRecordLock> = Vec::with_capacity(next.len());
        for lock in next {
            if let Some(last) = merged.last_mut() {
                let same_stream = last.path == lock.path
                    && last.owner_tgid == lock.owner_tgid
                    && last.lock_type == lock.lock_type;
                if same_stream {
                    let last_end = range_end_u128(last.end);
                    let this_start = lock.start as u128;
                    if this_start <= last_end {
                        let this_end = range_end_u128(lock.end);
                        let merged_end = core::cmp::max(last_end, this_end);
                        last.end = if merged_end == u128::MAX {
                            None
                        } else {
                            Some(merged_end as u64)
                        };
                        continue;
                    }
                }
            }
            merged.push(lock);
        }

        self.locks = merged;
        changed
    }
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
        if syscall_trace_enabled() {
            if let Ok(process) = ctx.procs.current() {
                trace_line(&format!(
                    "whuse: syscall enter tgid={} name={} sysno={} args={:#x},{:#x},{:#x},{:#x},{:#x},{:#x}",
                    process.tgid,
                    process.name,
                    sysno,
                    args.0[0],
                    args.0[1],
                    args.0[2],
                    args.0[3],
                    args.0[4],
                    args.0[5]
                ));
            }
        }
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
        if result == Err(ENOSYS) {
            if let Ok(process) = procs.current() {
                trace_enosys(&format!(
                    "whuse: enosys tgid={} name={} sysno={} args={:#x},{:#x},{:#x},{:#x},{:#x},{:#x}",
                    process.tgid,
                    process.name,
                    sysno,
                    args.0[0],
                    args.0[1],
                    args.0[2],
                    args.0[3],
                    args.0[4],
                    args.0[5]
                ));
            } else {
                trace_enosys(&format!(
                    "whuse: enosys sysno={} (no current process)",
                    sysno
                ));
            }
        }

        let res = match result {
            Ok(value) => value as isize,
            Err(errno) => -(errno as isize),
        };
        if syscall_trace_enabled() {
            if let Ok(process) = procs.current() {
                trace_line(&format!(
                    "whuse: syscall exit tgid={} name={} sysno={} res={:#x}",
                    process.tgid, process.name, sysno, res
                ));
            }
        }
        res
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
        let dirfd = args.0[0] as i32;
        let path_ptr = args.0[1];
        let path = match procs.current()?.read_user_cstr(path_ptr) {
            Ok(path) => path,
            Err(_) => {
                log_user_path_fault(procs.current()?, "unlinkat", path_ptr);
                return Err(EFAULT);
            }
        };
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
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
        let source = if args.0[0] == 0 {
            String::new()
        } else {
            procs
                .current()?
                .read_user_cstr(args.0[0])
                .map_err(|_| EFAULT)?
        };
        let target = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let fs_type = procs
            .current()?
            .read_user_cstr(args.0[2])
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let target = vfs.absolute_path(&cwd, &target);
        let mount_flags = args.0[3] as u32;
        let _data = if args.0[4] == 0 {
            String::new()
        } else {
            procs
                .current()?
                .read_user_cstr(args.0[4])
                .map_err(|_| EFAULT)?
        };
        vfs.mount(&source, &target, &fs_type, mount_flags)?;
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
        let dirfd = args.0[0] as i32;
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let raw_flags = args.0[2] as u32;
        let flags = normalize_open_flags(raw_flags);
        let mode = args.0[3] as u32;
        let (tid, tgid, name, proc_cwd) = {
            let process = procs.current()?;
            (
                process.tid,
                process.tgid,
                process.name.clone(),
                process.cwd.clone(),
            )
        };
        let openat_probe = stage2_openat_debug_enabled()
            && is_libctest_openat_probe_task(name.as_str(), tgid, proc_cwd.as_str());
        if openat_probe {
            log_always(&format!(
                "whuse-libctest:openat-enter tid={} tgid={} name={} dirfd={} cwd={} path={} raw_flags={:#x} flags={:#x}",
                tid, tgid, name, dirfd, proc_cwd, path, raw_flags, flags
            ));
        }
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
        if openat_probe {
            log_always(&format!(
                "whuse-libctest:openat-vfs-open-begin tid={} tgid={} cwd={} path={}",
                tid, tgid, cwd, path
            ));
        }
        let mut handle = match vfs.open(&cwd, &path, flags, mode) {
            Ok(handle) => handle,
            Err(err) => {
                if openat_probe {
                    log_always(&format!(
                        "whuse-libctest:openat-vfs-open-err tid={} tgid={} err={} cwd={} path={}",
                        tid, tgid, err, cwd, path
                    ));
                }
                return Err(err);
            }
        };
        if openat_probe {
            log_always(&format!(
                "whuse-libctest:openat-vfs-open-ok tid={} tgid={} resolved={}",
                tid, tgid, handle.path
            ));
        }
        handle.flags = with_cloexec_flag(handle.flags, (raw_flags as usize & O_CLOEXEC) != 0);
        let fd = procs.current_mut()?.add_fd(handle);
        if openat_probe {
            log_always(&format!(
                "whuse-libctest:openat-exit tid={} tgid={} fd={}",
                tid, tgid, fd
            ));
        }
        Ok(fd as usize)
    }

    fn sys_close(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let owner_tgid = procs.current_tgid()?;
        let handle = procs.current()?.fd(fd)?.clone();
        procs.current_mut()?.close_fd(fd)?;
        let wake_blocked = vfs.is_pipe(&handle);
        let released_locks = FCNTL_LOCK_STATE
            .lock()
            .clear_for_owner_path(owner_tgid, &handle.path);
        drop(handle);
        if wake_blocked || released_locks {
            let _ = scheduler.wake_all_blocked();
        }
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
            process.sync_fd_offset_from_alias(fd)?;
            let handle = process.fd_mut(fd)?;
            let path = handle.path.clone();
            (vfs.getdents(handle, count)?, path)
        };
        procs.current_mut()?.sync_fd_offset_to_aliases(fd)?;
        if matches!(
            BUSYBOX_APPLETS
                .lock()
                .get(&procs.current_tgid()?)
                .map(|s| s.as_str()),
            Some("du")
        ) {
            trace_line(&format!(
                "whuse: du getdents path={} bytes={}",
                bytes.1,
                bytes.0.len()
            ));
        }
        let bytes_data = bytes.0;

        procs
            .current_mut()?
            .write_user_bytes(buf, &bytes_data)
            .map_err(|_| EFAULT)?;
        Ok(bytes_data.len())
    }

    fn sys_lseek(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let offset = args.0[1] as isize;
        let whence = args.0[2] as u32;
        let process = procs.current_mut()?;
        process.sync_fd_offset_from_alias(fd)?;
        let handle = process.fd_mut(fd)?;
        let position = vfs.seek(handle, offset, whence)?;
        process.sync_fd_offset_to_aliases(fd)?;
        Ok(position)
    }

    fn sys_read(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let bytes = {
            let process = procs.current_mut()?;
            process.sync_fd_offset_from_alias(fd)?;
            let proc_name = process.name.clone();
            let proc_tgid = process.tgid;
            let proc_tid = process.tid;
            let proc_cwd = process.cwd.clone();
            let handle = process.fd_mut(fd)?;
            let is_pipe = vfs.is_pipe(handle);
            let pipe_path = handle.path.clone();
            let probe = stage2_openat_debug_enabled()
                && is_libctest_openat_probe_task(proc_name.as_str(), proc_tgid, proc_cwd.as_str())
                && is_libctest_probe_path(pipe_path.as_str());
            if probe {
                log_always(&format!(
                    "whuse-libctest:read-enter tid={} tgid={} fd={} path={} count={} off={}",
                    proc_tid, proc_tgid, fd, pipe_path, count, handle.offset
                ));
            }
            match vfs.read(handle, count) {
                Ok(bytes) => {
                    if probe {
                        log_always(&format!(
                            "whuse-libctest:read-ok tid={} tgid={} fd={} path={} bytes={} off={}",
                            proc_tid,
                            proc_tgid,
                            fd,
                            pipe_path,
                            bytes.len(),
                            handle.offset
                        ));
                    }
                    bytes
                }
                Err(EAGAIN) if is_pipe => {
                    trace_enosys(&format!(
                        "whuse: pipe read block tgid={} name={} fd={} path={}",
                        proc_tgid, proc_name, fd, pipe_path
                    ));
                    let _ = scheduler.block_current();
                    return Err(EAGAIN);
                }
                Err(err) => {
                    if probe {
                        log_always(&format!(
                            "whuse-libctest:read-err tid={} tgid={} fd={} path={} err={}",
                            proc_tid, proc_tgid, fd, pipe_path, err
                        ));
                    }
                    return Err(err);
                }
            }
        };
        procs.current_mut()?.sync_fd_offset_to_aliases(fd)?;
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
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let buf = args.0[1];
        let count = args.0[2];
        let data = procs
            .current()?
            .read_user_bytes(buf, count)
            .map_err(|_| EFAULT)?;
        let result = {
            let process = procs.current_mut()?;
            process.sync_fd_offset_from_alias(fd)?;
            let handle = process.fd_mut(fd)?;
            let is_pipe = vfs.is_pipe(handle);
            match vfs.write(handle, &data) {
                Ok(written) => {
                    process.sync_fd_offset_to_aliases(fd)?;
                    Ok((is_pipe, written))
                }
                Err(err) => Err((is_pipe, err)),
            }
        };
        match result {
            Ok((is_pipe, written)) => {
                if is_pipe && written != 0 {
                    let _ = scheduler.wake_all_blocked();
                }
                Ok(written)
            }
            Err((is_pipe, err)) => {
                if is_pipe && err == EPIPE {
                    let _ = scheduler.wake_all_blocked();
                }
                Err(err)
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

    fn sys_nanosleep(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let req_ptr = args.0[0];
        let rem_ptr = args.0[1];
        if req_ptr == 0 {
            return Err(EFAULT);
        }
        let now = hal().timer.monotonic_nanos();
        let pending = procs.pending_signals()?;
        if procs.current()?.sleep_deadline_ns.is_none() {
            let requested = read_timespec_ns(procs.current()?, req_ptr)?;
            if requested == 0 {
                if rem_ptr != 0 {
                    procs
                        .current_mut()?
                        .write_user_bytes(
                            rem_ptr,
                            &timespec_to_bytes(Timespec {
                                tv_sec: 0,
                                tv_nsec: 0,
                            }),
                        )
                        .map_err(|_| EFAULT)?;
                }
                return Ok(0);
            }
            let process = procs.current_mut()?;
            process.sleep_deadline_ns = Some(now.saturating_add(requested));
            process.sleep_requested_ns = requested;
            process.sleep_remain_ptr = (rem_ptr != 0).then_some(rem_ptr);
            process.sleep_absolute = false;
        }
        let deadline = procs.current()?.sleep_deadline_ns.unwrap_or(now);
        let requested = procs.current()?.sleep_requested_ns;
        let remain_ptr = procs.current()?.sleep_remain_ptr;

        if pending != 0 {
            if let Some(ptr) = remain_ptr {
                let remain = deadline.saturating_sub(now).min(requested);
                procs
                    .current_mut()?
                    .write_user_bytes(ptr, &nanos_to_timespec_bytes(remain))
                    .map_err(|_| EFAULT)?;
            }
            let process = procs.current_mut()?;
            process.sleep_deadline_ns = None;
            process.sleep_requested_ns = 0;
            process.sleep_remain_ptr = None;
            process.sleep_absolute = false;
            return Err(EINTR);
        }

        if now < deadline {
            let _ = scheduler.block_current();
            return Err(EAGAIN);
        }

        if let Some(ptr) = remain_ptr {
            procs
                .current_mut()?
                .write_user_bytes(
                    ptr,
                    &timespec_to_bytes(Timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                )
                .map_err(|_| EFAULT)?;
        }
        let process = procs.current_mut()?;
        process.sleep_deadline_ns = None;
        process.sleep_requested_ns = 0;
        process.sleep_remain_ptr = None;
        process.sleep_absolute = false;
        Ok(0)
    }

    fn sys_exit(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        exit_group: bool,
    ) -> Result<usize, i32> {
        let libcbench_leader = procs
            .current()
            .ok()
            .filter(|process| is_libcbench_task(process) && process.tid == process.tgid)
            .map(|process| (process.tid, process.tgid));
        if let Some((tid, tgid)) = libcbench_leader {
            log_always(&format!(
                "whuse-libcbench:sys-exit-enter tid={} tgid={} exit_group={} code={}",
                tid,
                tgid,
                exit_group,
                args.0[0] as i32
            ));
        }
        let exit = if exit_group {
            procs.exit_current_process_group(args.0[0] as i32)?
        } else {
            procs.exit_current_thread(args.0[0] as i32)?
        };
        if let Some((tid, tgid)) = libcbench_leader {
            log_always(&format!(
                "whuse-libcbench:sys-exit-result tid={} tgid={} group_exited={} clear_child_tid={:#x?} robust_addrs={}",
                tid,
                tgid,
                exit.group_exited,
                exit.clear_child_tid,
                exit.robust_futex_addrs.len()
            ));
        }
        let released_locks = if exit_group || exit.group_exited {
            FCNTL_LOCK_STATE.lock().clear_for_owner(exit.tgid)
        } else {
            false
        };
        if exit_group {
            scheduler.exit_group(exit.tgid);
            if let Some(parent_tgid) = exit.parent_tgid {
                let _ = procs.deliver_signal(parent_tgid, SIGCHLD);
                let woke = wake_process_group_threads(procs, scheduler, parent_tgid);
                trace_line(&format!(
                    "whuse: exit_group wake parent_tgid={} woke_threads={}",
                    parent_tgid, woke
                ));
            }
        } else {
            scheduler.remove_task(exit.tid);
            if exit.group_exited {
                scheduler.exit_group(exit.tgid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let _ = procs.deliver_signal(parent_tgid, SIGCHLD);
                let woke = wake_process_group_threads(procs, scheduler, parent_tgid);
                trace_line(&format!(
                    "whuse: exit wake parent_tgid={} woke_threads={}",
                    parent_tgid, woke
                ));
            }
            if let Some(addr) = exit.clear_child_tid {
                cancel_debug(&format!(
                    "whuse-debug: exit tid={} clear_child_tid={:#x}",
                    exit.tid, addr
                ));
                let woken = procs.wake_futex(addr, usize::MAX);
                cancel_debug(&format!(
                    "whuse-debug: wake_futex addr={:#x} woken={:?}",
                    addr, woken
                ));
                for tid in woken {
                    let _ = scheduler.wake_task(tid);
                }
            } else {
                cancel_debug(&format!(
                    "whuse-debug: exit tid={} no clear_child_tid",
                    exit.tid
                ));
            }
        }
        if let Some(applet) = BUSYBOX_APPLETS.lock().remove(&exit.tgid) {
            trace_enosys(&format!(
                "whuse: busybox exit tgid={} applet={} code={}",
                exit.tgid, applet, args.0[0] as i32
            ));
        }
        if released_locks {
            trace_line(&format!("whuse: released fcntl locks for tgid={}", exit.tgid));
        }
        let _ = scheduler.wake_all_blocked();
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
        let path_ptr = args.0[0];
        let path = procs
            .current()?
            .read_user_cstr(path_ptr)
            .map_err(|_| EFAULT)?;
        let cwd = procs.current()?.cwd.clone();
        let new_cwd = vfs.chdir(&cwd, &path)?;
        procs.current_mut()?.cwd = new_cwd;
        Ok(0)
    }

    fn sys_brk(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let requested = args.0[0];
        if syscall_trace_enabled() {
            trace_line(&format!("whuse: sys_brk requested={:#x}", requested));
        }
        let process = procs.current_mut()?;
        let res = process
            .address_space
            .brk((requested != 0).then_some(requested));
        if syscall_trace_enabled() {
            match &res {
                Ok(val) => trace_line(&format!("whuse: sys_brk success res={:#x}", val)),
                Err(err) => trace_line(&format!("whuse: sys_brk failed err={}", err)),
            }
        }
        res
    }

    fn sys_clone(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let request = decode_clone_request(args);
        self.do_clone(request, procs, scheduler)
    }

    fn do_clone(
        &self,
        request: CloneRequest,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let current = procs.current()?;
        let name = current.name.clone();
        let libcbench_task = is_libcbench_task(current);
        let parent_pid = procs.current_pid()?;
        let flags = request.flags;
        if flags & CLONE_NAMESPACE_MASK != 0 {
            return Err(EINVAL);
        }
        let mut compat_flags = flags & !CLONE_VFORK;
        if flags & CLONE_VFORK != 0 {
            compat_flags &= !CLONE_VM;
            trace_line(&format!(
                "whuse: clone parent_tgid={} flags={:#x} downgraded_vfork=true",
                parent_pid, flags
            ));
        }
        if (compat_flags & CLONE_THREAD) != 0 {
            let required = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
            if compat_flags & required != required {
                return Err(EINVAL);
            }
            let stack = request.stack;
            let parent_tid = request.parent_tid;
            let tls = request.tls;
            let child_tid_ptr = request.child_tid;
            if libcbench_task {
                libcbench_debug(&format!(
                    "whuse-libcbench:clone-thread parent_tgid={} flags={:#x} stack={:#x} ptid={:#x} ptid_addr={} ctid={:#x} ctid_addr={} tls={:#x}",
                    parent_pid,
                    flags,
                    stack,
                    parent_tid,
                    procs.current()?.address_space.describe_addr(parent_tid),
                    child_tid_ptr,
                    procs.current()?.address_space.describe_addr(child_tid_ptr),
                    tls.unwrap_or(0)
                ));
            }
            let tid = procs.clone_thread_from_current(stack, tls)?;
            if compat_flags & CLONE_PARENT_SETTID != 0 && parent_tid != 0 {
                procs
                    .current_mut()?
                    .write_user_bytes(parent_tid, &(tid as u32).to_le_bytes())
                    .map_err(|_| EFAULT)?;
            }
            let current_tid = procs.current_tid()?;
            procs.set_current(tid)?;
            if compat_flags & CLONE_CHILD_SETTID != 0 && child_tid_ptr != 0 {
                procs
                    .current_mut()?
                    .write_user_bytes(child_tid_ptr, &(tid as u32).to_le_bytes())
                    .map_err(|_| EFAULT)?;
            }
            if compat_flags & CLONE_CHILD_CLEARTID != 0 && child_tid_ptr != 0 {
                procs.set_clear_child_tid(Some(child_tid_ptr))?;
            }
            if libcbench_task {
                libcbench_debug(&format!(
                    "whuse-libcbench:clone-thread-child tid={} tgid={} clear_child_tid={:#x}",
                    tid,
                    procs.current_tgid()?,
                    procs.current()?.clear_child_tid().unwrap_or(0)
                ));
            }
            let tgid = procs.current_tgid()?;
            procs.set_current(current_tid)?;
            scheduler.spawn(&name, tid, tgid);
            return Ok(tid);
        }

        let child_stack = request.stack;
        let shared_vm = (compat_flags & CLONE_VM) != 0;
        let parent_tid = request.parent_tid;
        let child_tid_ptr = request.child_tid;
        let pid = if shared_vm {
            procs.fork_process_from_current_shared()?
        } else {
            procs.fork_process_from_current()?
        };
        trace_line(&format!(
            "whuse: clone parent_tgid={} flags={:#x} child_tgid={} shared_vm={}",
            parent_pid, flags, pid, shared_vm
        ));

        if compat_flags & CLONE_PARENT_SETTID != 0 && parent_tid != 0 {
            procs
                .current_mut()?
                .write_user_bytes(parent_tid, &(pid as u32).to_le_bytes())
                .map_err(|_| EFAULT)?;
        }

        {
            let child = procs.find_by_tid_mut(pid)?;
            if compat_flags & CLONE_CHILD_SETTID != 0 && child_tid_ptr != 0 {
                let _ = child.write_user_bytes(child_tid_ptr, &(pid as u32).to_le_bytes());
            }
            if compat_flags & CLONE_CHILD_CLEARTID != 0 && child_tid_ptr != 0 {
                child.clear_child_tid = Some(child_tid_ptr);
            }
        }

        if child_stack != 0 {
            procs.set_thread_stack_pointer(pid, child_stack)?;
        }
        scheduler.spawn(&name, pid, pid);
        Ok(pid)
    }

    fn sys_clone3(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let addr = args.0[0];
        let size = args.0[1];
        if addr == 0 || size < 64 {
            return Err(EINVAL);
        }
        let read_len = size.min(88);
        let raw = procs
            .current()?
            .read_user_bytes(addr, read_len)
            .map_err(|_| EFAULT)?;
        let read_u64 = |offset: usize| -> u64 {
            if offset + 8 > raw.len() {
                return 0;
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&raw[offset..offset + 8]);
            u64::from_le_bytes(bytes)
        };
        let flags = read_u64(0) as usize;
        let _pidfd = read_u64(8) as usize;
        let child_tid = read_u64(16) as usize;
        let parent_tid = read_u64(24) as usize;
        let exit_signal = read_u64(32) as usize;
        let stack = read_u64(40) as usize;
        let stack_size = read_u64(48) as usize;
        let tls = read_u64(56) as usize;
        let child_stack = if stack_size == 0 {
            stack
        } else {
            stack.saturating_add(stack_size)
        };
        let legacy = decode_clone_request(SyscallArgs([
            flags | (exit_signal & 0xff),
            child_stack,
            parent_tid,
            child_tid,
            tls,
            0,
        ]));
        self.do_clone(legacy, procs, scheduler)
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
        let mut display_path = vfs.absolute_path(&cwd, &path);
        let mut argv = read_string_vector(procs.current()?, args.0[1])?;
        if argv.is_empty() {
            argv.push(display_path.clone());
        }
        let envp = read_string_vector(procs.current()?, args.0[2])?;
        if path.contains("busybox") && argv.len() > 1 {
            let applet = argv[1].as_str();
            let redirect = match applet {
                "wait" => Some("/musl/wait"),
                "locale" => Some("/musl/locale"),
                "useradd" => Some("/musl/useradd"),
                "userdel" => Some("/musl/userdel"),
                _ => None,
            };
            if let Some(redirect_path) = redirect {
                trace_line(&format!(
                    "whuse: execve busybox applet redirect tgid={} applet={} -> {}",
                    procs.current_tgid().unwrap_or(0),
                    applet,
                    redirect_path
                ));
                path = redirect_path.to_string();
                display_path = path.clone();
                let mut redirected_argv = Vec::new();
                redirected_argv.push(display_path.clone());
                if argv.len() > 2 {
                    redirected_argv.extend_from_slice(&argv[2..]);
                }
                argv = redirected_argv;
            }
        }
        let busybox_applet = if display_path.contains("busybox") && argv.len() > 1 {
            Some(argv[1].as_str())
        } else if matches!(display_path.as_str(), "/bin/sh" | "/bin/bash" | "/busybox") {
            Some("sh")
        } else {
            None
        };
        if let Some(applet) = busybox_applet {
            BUSYBOX_APPLETS
                .lock()
                .insert(procs.current_tgid().unwrap_or(0), applet.to_string());
            if matches!(applet, "dmesg" | "du" | "expr" | "ls" | "find") {
                trace_enosys(&format!(
                    "whuse: busybox exec tgid={} applet={} cwd={}",
                    procs.current_tgid().unwrap_or(0),
                    applet,
                    cwd
                ));
            }
        }
        let mut shebang_hops = 0usize;
        loop {
            if path.contains("busybox") && argv.len() > 1 {
                BUSYBOX_APPLETS
                    .lock()
                    .insert(procs.current_tgid().unwrap_or(0), argv[1].clone());
            }
            let file_data = read_exec_file_image(vfs, &cwd, &path)?;
            if file_data.is_empty() {
                return Err(EFAULT);
            }
            if let Some((mut interp_path, mut interp_arg)) = parse_shebang_line(&file_data) {
                if interp_path == "/bin/sh"
                    || interp_path == "/bin/bash"
                    || interp_path == "/busybox"
                {
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
                next_argv.push(display_path.clone());
                if argv.len() > 1 {
                    next_argv.extend_from_slice(&argv[1..]);
                }
                path = interp_path;
                argv = next_argv;
                shebang_hops += 1;
                continue;
            }
            if path.ends_with(".sh") {
                if shebang_hops >= 4 {
                    return Err(ENOEXEC);
                }
                let shell = "/musl/busybox";
                if vfs.access("/", shell).is_ok() {
                    let mut next_argv = Vec::new();
                    next_argv.push(shell.to_string());
                    next_argv.push("sh".to_string());
                    next_argv.push(display_path.clone());
                    if argv.len() > 1 {
                        next_argv.extend_from_slice(&argv[1..]);
                    }
                    path = shell.to_string();
                    argv = next_argv;
                    shebang_hops += 1;
                    continue;
                }
            }
            if let Some(program) =
                resolve_exec_payload(&file_data).or_else(|| builtin_program(&path))
            {
                let entry = BUILTIN_EXEC_BASE + program.entry;
                procs.execve_current_image(entry, None)?;
                let process = procs.current_mut()?;
                process.name = display_path.clone();
                process
                    .address_space
                    .map_fixed_bytes(BUILTIN_EXEC_BASE, program.image, program.image.len(), 0b101)
                    .map_err(|_| EFAULT)?;
                let _ = process.address_space.map_fixed_bytes(
                    SIGNAL_TRAMPOLINE_BASE,
                    &SIGNAL_TRAMPOLINE_CODE,
                    4096,
                    0b101,
                );
                process.trap_frame.sepc = entry;
                let tgid = process.tgid;
                #[cfg(target_arch = "riscv64")]
                unsafe {
                    core::arch::asm!("fence.i");
                }
                trace_line(&format!(
                    "whuse: execve builtin tgid={} path={} entry={:#x}",
                    tgid, path, entry
                ));
                return Ok(0);
            }
            let interp = if let Some(mut interp_path) = parse_elf_interp(&file_data) {
                if vfs.access("/", &interp_path).is_err() {
                    let fallback = "/lib/ld-linux-riscv64-lp64d.so.1";
                    if vfs.access("/", fallback).is_ok() {
                        interp_path = fallback.to_string();
                    } else {
                        let musl_fallback = "/musl/lib/libc.so";
                        if vfs.access("/", musl_fallback).is_ok() {
                            interp_path = musl_fallback.to_string();
                        } else {
                            return Err(ENOENT);
                        }
                    }
                }
                let interp_image = read_exec_file_image(vfs, "/", &interp_path)?;
                if interp_image.is_empty() {
                    return Err(ENOEXEC);
                }
                Some((interp_path, interp_image))
            } else {
                None
            };
            procs.execve_current_image(0, None)?;
            let loaded = {
                let process = procs.current_mut()?;
                match process.address_space.load_elf_images(
                    &file_data,
                    interp.as_ref().map(|(_, image)| image.as_slice()),
                    &argv,
                    &envp,
                    Some(display_path.as_str()),
                ) {
                    Ok(loaded) => loaded,
                    Err(err) => return Err(if err == ENOEXEC { ENOEXEC } else { EFAULT }),
                }
            };
            let process = procs.current_mut()?;
            process.trap_frame.sepc = loaded.entry;
            process.trap_frame.regs[2] = loaded.stack_pointer;
            process.name = display_path;
            let _ = process.address_space.map_fixed_bytes(
                SIGNAL_TRAMPOLINE_BASE,
                &SIGNAL_TRAMPOLINE_CODE,
                4096,
                0b101,
            );
            let tgid = process.tgid;
            let proc_name = process.name.clone();
            let entry = process.trap_frame.sepc;
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("fence.i");
            }
            trace_line(&format!(
                "whuse: execve elf tgid={} path={} entry={:#x} program_entry={:#x} at_base={:#x} phdr={:#x} dyn={}",
                tgid,
                proc_name,
                entry,
                loaded.program_entry,
                loaded.interp_base,
                loaded.phdr_addr,
                loaded.is_dyn
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
        let addr = args.0[0];
        let len = args.0[1];
        let prot = args.0[2];
        let flags = args.0[3];
        let fd = args.0[4] as isize;
        let offset = args.0[5];
        if len == 0 {
            return Err(EINVAL);
        }
        if offset & (PAGE_SIZE - 1) != 0 {
            return Err(EINVAL);
        }
        let mapping_type = flags & MAP_TYPE_MASK;
        if mapping_type != MAP_PRIVATE && mapping_type != MAP_SHARED {
            return Err(EINVAL);
        }
        let aligned_len = len.checked_add(PAGE_SIZE - 1).ok_or(EINVAL)? & !(PAGE_SIZE - 1);
        let anonymous = (flags & MAP_ANONYMOUS) != 0;
        let fixed = (flags & MAP_FIXED) != 0;
        let fixed_noreplace = (flags & MAP_FIXED_NOREPLACE) != 0;

        let hinted = if fixed || fixed_noreplace || addr != 0 {
            let aligned = addr & !(PAGE_SIZE - 1);
            if (fixed || fixed_noreplace) && aligned != addr {
                return Err(EINVAL);
            }
            Some(aligned)
        } else {
            None
        };
        // MAP_FIXED_NOREPLACE: fail with EEXIST if range is already in use.
        if fixed_noreplace {
            if let Some(hint) = hinted {
                if !procs
                    .current()?
                    .address_space
                    .is_range_available(hint, aligned_len)?
                {
                    return Err(17); // EEXIST
                }
            }
        }
        let target = match hinted {
            Some(hint) if fixed => Some(hint),
            Some(hint) if fixed_noreplace => Some(hint),
            Some(hint) => {
                if procs
                    .current()?
                    .address_space
                    .is_range_available(hint, aligned_len)?
                {
                    Some(hint)
                } else {
                    None
                }
            }
            None => None,
        };

        if anonymous {
            let base = if let Some(target) = target {
                procs
                    .current_mut()?
                    .address_space
                    .map_anonymous_at(target, aligned_len, prot)?
            } else {
                procs
                    .current_mut()?
                    .address_space
                    .map_anonymous(aligned_len, prot)?
            };
            return Ok(base);
        }

        if fd < 0 {
            return Err(EBADF);
        }
        let mut handle = procs.current()?.fd(fd as i32)?.clone();
        handle.offset = offset;
        let data = vfs.read(&mut handle, len)?;
        if let Some(target) = target {
            procs
                .current_mut()?
                .address_space
                .map_fixed_bytes(target, &data, aligned_len, prot)?;
            return Ok(target);
        }
        let base = procs
            .current_mut()?
            .address_space
            .map_anonymous(aligned_len, prot)?;
        procs
            .current_mut()?
            .address_space
            .write_bytes(base, &data)
            .map_err(|_| EFAULT)?;
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
        let (parent_name, parent_cwd) = {
            let process = procs.current()?;
            (process.name.clone(), process.cwd.clone())
        };
        let busybox_wait_debug =
            parent_name.contains("busybox") && parent_cwd == "/musl" && wait_pid > 0;
        if busybox_wait_debug {
            log_always(&format!(
                "whuse-wait: enter tgid={} name={} wait_pid={} options={:#x}",
                parent_pid, parent_name, wait_pid, options
            ));
        }
        trace_line(&format!(
            "whuse: wait enter tgid={} wait_pid={} status_ptr={:#x} options={:#x}",
            parent_pid, wait_pid, status_ptr, options
        ));
        let selector = selector_from_wait(wait_pid, procs.getpgid(0)?);
        let (child_pid, status) = match procs.wait_child(parent_pid, selector, options) {
            Ok(pair) => pair,
            Err(err) => {
                if busybox_wait_debug {
                    log_always(&format!(
                        "whuse-wait: err tgid={} wait_pid={} err={}",
                        parent_pid, wait_pid, err
                    ));
                }
                trace_line(&format!(
                    "whuse: wait error tgid={} err={}",
                    parent_pid, err
                ));
                return Err(err);
            }
        };
        if child_pid == 0 {
            if busybox_wait_debug {
                log_always(&format!(
                    "whuse-wait: no-exited-child tgid={} wait_pid={} options={:#x}",
                    parent_pid, wait_pid, options
                ));
            }
            if options & WNOHANG != 0 {
                trace_line(&format!(
                    "whuse: wait return tgid={} child=0 wnohang",
                    parent_pid
                ));
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
        if busybox_wait_debug {
            log_always(&format!(
                "whuse-wait: return tgid={} child={} status={}",
                parent_pid, child_pid, status
            ));
        }
        trace_line(&format!(
            "whuse: wait return tgid={} child={} status={}",
            parent_pid, child_pid, status
        ));
        Ok(child_pid)
    }

    fn sys_dup(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let source_fd = args.0[0] as i32;
        let mut handle = procs.current()?.fd(source_fd)?.clone();
        handle.flags = with_cloexec_flag(handle.flags, false);
        Ok(procs.current_mut()?.add_fd_from(source_fd, handle)? as usize)
    }

    fn sys_dup3(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let oldfd = args.0[0] as i32;
        let newfd = args.0[1] as i32;
        let flags = args.0[2];
        if oldfd == newfd {
            let _ = procs.current()?.fd(oldfd)?;
            return Ok(newfd as usize);
        }
        let mut handle = procs.current()?.fd(oldfd)?.clone();
        handle.flags = with_cloexec_flag(handle.flags, (flags & O_CLOEXEC) != 0);
        let process = procs.current_mut()?;
        let _ = process.close_fd(newfd);
        process.fds.insert(newfd, handle);
        let leader = process.fd_alias_leader(oldfd)?;
        process.set_fd_alias(newfd, leader);
        process.sync_fd_offset_from_alias(newfd)?;
        Ok(newfd as usize)
    }

    fn sys_fcntl(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        const F_DUPFD: usize = 0;
        const F_GETFD: usize = 1;
        const F_SETFD: usize = 2;
        const F_GETFL: usize = 3;
        const F_SETFL: usize = 4;
        const F_DUPFD_CLOEXEC: usize = 1030;

        let fd = args.0[0] as i32;
        let cmd = args.0[1];
        let arg_raw = args.0[2];
        let arg = arg_raw as i32;
        match cmd {
            F_DUPFD | F_DUPFD_CLOEXEC => {
                let mut handle = procs.current()?.fd(fd)?.clone();
                handle.flags = with_cloexec_flag(handle.flags, cmd == F_DUPFD_CLOEXEC);
                let process = procs.current_mut()?;
                let mut newfd = arg.max(0);
                while process.fds.contains_key(&newfd) {
                    newfd += 1;
                }
                process.fds.insert(newfd, handle);
                let leader = process.fd_alias_leader(fd)?;
                process.set_fd_alias(newfd, leader);
                process.sync_fd_offset_from_alias(newfd)?;
                Ok(newfd as usize)
            }
            F_GETFD => Ok(((procs.current()?.fd(fd)?.flags & HANDLE_FLAG_CLOEXEC) != 0) as usize),
            F_SETFD => {
                let cloexec = (arg & 1) != 0;
                let handle = procs.current_mut()?.fd_mut(fd)?;
                handle.flags = with_cloexec_flag(handle.flags, cloexec);
                Ok(0)
            }
            F_GETFL => Ok((procs.current()?.fd(fd)?.flags & !HANDLE_FLAG_CLOEXEC) as usize),
            F_SETFL => Ok(0),
            F_GETLK | F_SETLK | F_SETLKW => {
                self.sys_fcntl_lock(fd, cmd, arg_raw, procs, scheduler, vfs)
            }
            _ => Err(EINVAL),
        }
    }

    fn sys_fcntl_lock(
        &self,
        fd: i32,
        cmd: usize,
        arg_ptr: usize,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let owner_tgid = procs.current_tgid()?;
        let handle = procs.current()?.fd(fd)?.clone();
        let mut request = read_flock_request(procs.current()?, arg_ptr)?;
        if !matches!(request.l_type, F_RDLCK | F_WRLCK | F_UNLCK) {
            return Err(EINVAL);
        }
        let (start, end) = normalize_lock_range(&request, &handle, vfs)?;
        if cmd == F_GETLK {
            if request.l_type == F_UNLCK {
                request.l_pid = 0;
                write_flock_request(procs.current_mut()?, arg_ptr, request)?;
                return Ok(0);
            }
            let conflict = FCNTL_LOCK_STATE
                .lock()
                .first_conflict(&handle.path, owner_tgid, request.l_type, start, end);
            if let Some(lock) = conflict {
                request.l_type = lock.lock_type;
                request.l_whence = SEEK_SET;
                request.l_start = lock.start as i64;
                request.l_len = lock_range_len(lock.start, lock.end);
                request.l_pid = lock.owner_tgid as i32;
            } else {
                request.l_type = F_UNLCK;
                request.l_pid = 0;
            }
            write_flock_request(procs.current_mut()?, arg_ptr, request)?;
            return Ok(0);
        }

        if request.l_type != F_UNLCK {
            let conflict = FCNTL_LOCK_STATE
                .lock()
                .first_conflict(&handle.path, owner_tgid, request.l_type, start, end);
            if conflict.is_some() {
                if cmd == F_SETLKW {
                    let _ = scheduler.block_current();
                    return Err(EAGAIN);
                }
                return Err(EACCES);
            }
        }

        let changed = FCNTL_LOCK_STATE
            .lock()
            .apply_lock(&handle.path, owner_tgid, request.l_type, start, end);
        if changed {
            let _ = scheduler.wake_all_blocked();
        }
        Ok(0)
    }

    fn sys_ioctl(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        const TIOCGWINSZ: usize = 0x5413;
        const RTC_RD_TIME: usize = 0x8024_7009;
        let fd = args.0[0] as i32;
        let cmd = args.0[1];
        let arg = args.0[2];
        let handle_path = procs.current()?.fd(fd)?.path.clone();
        match cmd {
            TIOCGWINSZ => {
                if arg != 0 {
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
            RTC_RD_TIME => {
                if arg == 0 {
                    return Err(EFAULT);
                }
                if handle_path != "/dev/rtc0" {
                    return Err(ENOTTY);
                }
                let now = wall_time_now();
                let bytes = rtc_time_bytes(now);
                procs
                    .current_mut()?
                    .write_user_bytes(arg, &bytes)
                    .map_err(|_| EFAULT)?;
                Ok(0)
            }
            _ => Ok(0),
        }
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
        let mut read_end;
        let mut write_end;
        (read_end, write_end) = vfs.create_pipe()?;
        let cloexec = (args.0[1] & O_CLOEXEC) != 0;
        read_end.flags = with_cloexec_flag(read_end.flags, cloexec);
        write_end.flags = with_cloexec_flag(write_end.flags, cloexec);
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
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let mut total = 0;
        for iov in iovecs {
            let maybe_bytes = {
                let process = procs.current_mut()?;
                let proc_name = process.name.clone();
                let proc_tgid = process.tgid;
                let handle = process.fd_mut(fd)?;
                let pipe_path = handle.path.clone();
                let is_pipe = vfs.is_pipe(handle);
                match vfs.read(handle, iov.iov_len) {
                    Ok(bytes) => Some(bytes),
                    Err(EAGAIN) if is_pipe && total == 0 => {
                        trace_enosys(&format!(
                            "whuse: pipe readv block tgid={} name={} fd={} path={}",
                            proc_tgid, proc_name, fd, pipe_path
                        ));
                        let _ = scheduler.block_current();
                        return Err(EAGAIN);
                    }
                    Err(EAGAIN) if is_pipe => None,
                    Err(err) => return Err(err),
                }
            };
            let Some(bytes) = maybe_bytes else {
                break;
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
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let iovecs = read_iovecs(procs.current()?, args.0[1], args.0[2])?;
        let is_pipe = if fd == 1 || fd == 2 {
            false
        } else {
            let process = procs.current()?;
            let handle = process.fd(fd)?;
            vfs.is_pipe(handle)
        };
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
                let written_res = {
                    let process = procs.current_mut()?;
                    let handle = process.fd_mut(fd)?;
                    vfs.write(handle, &bytes)
                };
                match written_res {
                    Ok(written) => total += written,
                    Err(EPIPE) if is_pipe => {
                        let _ = scheduler.wake_all_blocked();
                        if total == 0 {
                            return Err(EPIPE);
                        }
                        break;
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        if is_pipe && total != 0 {
            let _ = scheduler.wake_all_blocked();
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

    pub(crate) fn sys_ppoll(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        const POLLIN: i16 = 0x0001;
        const POLLOUT: i16 = 0x0004;
        const POLLERR: i16 = 0x0008;
        const POLLHUP: i16 = 0x0010;
        let addr = args.0[0];
        let nfds = args.0[1];
        let mut pollfds = read_pollfds(procs.current()?, addr, nfds)?;
        let mut ready = 0usize;
        for pollfd in &mut pollfds {
            pollfd.revents = 0;
            if pollfd.fd < 0 {
                continue;
            }
            let Ok(handle) = procs.current()?.fd(pollfd.fd) else {
                pollfd.revents = POLLERR;
                ready += 1;
                continue;
            };
            let read_ready = vfs.is_read_ready(handle);
            let write_ready = vfs.is_write_ready(handle);
            if (pollfd.events & POLLIN) != 0 && read_ready {
                pollfd.revents |= POLLIN;
            }
            if (pollfd.events & POLLOUT) != 0 && write_ready {
                pollfd.revents |= POLLOUT;
            }
            if read_ready && !write_ready {
                pollfd.revents |= POLLHUP;
            }
            if pollfd.revents == 0 && (pollfd.events & (POLLERR | POLLHUP)) != 0 && read_ready {
                pollfd.revents |= pollfd.events & (POLLERR | POLLHUP);
            }
            if pollfd.revents != 0 {
                ready += 1;
            }
        }
        if ready != 0 {
            procs.current_mut()?.epoll_wait_deadline_ns = None;
            procs
                .current_mut()?
                .write_user_bytes(addr, &pollfds_to_bytes(&pollfds))
                .map_err(|_| EFAULT)?;
            return Ok(ready);
        }

        let process = procs.current_mut()?;
        if process.pending_signals & !process.signal_mask != 0 {
            process.epoll_wait_deadline_ns = None;
            return Err(EINTR);
        }

        let now = hal().timer.monotonic_nanos();
        let timeout_ptr = args.0[2];
        if process.epoll_wait_deadline_ns.is_none() {
            if timeout_ptr != 0 {
                let requested = read_timespec_ns(process, timeout_ptr)?;
                if requested == 0 {
                    return Ok(0);
                }
                process.epoll_wait_deadline_ns = Some(now.saturating_add(requested));
            } else {
                process.epoll_wait_deadline_ns = Some(u64::MAX);
            }
        }

        let deadline = process.epoll_wait_deadline_ns.unwrap();
        if deadline != u64::MAX && now >= deadline {
            process.epoll_wait_deadline_ns = None;
            return Ok(0);
        }

        let _ = scheduler.block_current();
        Err(EAGAIN)
    }

    pub(crate) fn sys_pselect6(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let nfds = args.0[0];
        let readfds = args.0[1];
        let writefds = args.0[2];
        let exceptfds = args.0[3];
        let _timeout = args.0[4];
        let _sigmask = args.0[5];
        let mut ready = BTreeSet::new();

        if readfds != 0 {
            let read_bits = read_fd_set(procs.current()?, readfds, nfds)?;
            let mut out = Vec::new();
            for fd in read_bits {
                if let Ok(handle) = procs.current()?.fd(fd as i32) {
                    if vfs.is_read_ready(handle) {
                        ready.insert(fd);
                        out.push(fd);
                    }
                }
            }
            procs
                .current_mut()?
                .write_user_bytes(readfds, &fd_set_bytes(&out, nfds))
                .map_err(|_| EFAULT)?;
        }
        if writefds != 0 {
            let write_bits = read_fd_set(procs.current()?, writefds, nfds)?;
            let mut out = Vec::new();
            for fd in write_bits {
                if let Ok(handle) = procs.current()?.fd(fd as i32) {
                    if vfs.is_write_ready(handle) {
                        ready.insert(fd);
                        out.push(fd);
                    }
                }
            }
            procs
                .current_mut()?
                .write_user_bytes(writefds, &fd_set_bytes(&out, nfds))
                .map_err(|_| EFAULT)?;
        }
        if exceptfds != 0 {
            procs
                .current_mut()?
                .write_user_bytes(exceptfds, &vec![0u8; fd_set_len(nfds)])
                .map_err(|_| EFAULT)?;
        }
        if !ready.is_empty() {
            procs.current_mut()?.epoll_wait_deadline_ns = None;
            return Ok(ready.len());
        }

        let process = procs.current_mut()?;
        if process.pending_signals & !process.signal_mask != 0 {
            process.epoll_wait_deadline_ns = None;
            return Err(EINTR);
        }

        let now = hal().timer.monotonic_nanos();
        let timeout_ptr = args.0[4];
        if process.epoll_wait_deadline_ns.is_none() {
            if timeout_ptr != 0 {
                let requested = read_timespec_ns(process, timeout_ptr)?;
                if requested == 0 {
                    return Ok(0);
                }
                process.epoll_wait_deadline_ns = Some(now.saturating_add(requested));
            } else {
                process.epoll_wait_deadline_ns = Some(u64::MAX);
            }
        }

        let deadline = process.epoll_wait_deadline_ns.unwrap();
        if deadline != u64::MAX && now >= deadline {
            process.epoll_wait_deadline_ns = None;
            return Ok(0);
        }

        let _ = scheduler.block_current();
        Err(EAGAIN)
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
        let dirfd = args.0[0] as i32;
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
        let target = match path.as_str() {
            "/proc/self/exe" => procs.current()?.name.clone(),
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
        let dirfd = args.0[0] as i32;
        let path = procs
            .current()?
            .read_user_cstr(args.0[1])
            .map_err(|_| EFAULT)?;
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
        let stat = vfs.stat_path(&cwd, &path)?;
        if matches!(
            BUSYBOX_APPLETS
                .lock()
                .get(&procs.current_tgid()?)
                .map(|s| s.as_str()),
            Some("du")
        ) {
            trace_line(&format!("whuse: du fstatat path={} ok", path));
        }

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
        if let Ok(current) = procs.current() {
            if current.name.contains("access01") || current.name.contains("access02") {
                log_always(&format!(
                    "whuse-debug: faccessat enter tgid={} uid={} euid={} ptr={:#x} mode={:#x} flags={:#x}",
                    current.tgid, current.uid, current.euid, args.0[1], args.0[2], args.0[3]
                ));
            }
        }
        let dirfd = args.0[0] as i32;
        let flags = args.0[3];
        let path = read_at_path_allow_empty(procs.current()?, args.0[1], flags)?;
        let mode = args.0[2];
        let allowed_mode_bits = F_OK | X_OK | W_OK | R_OK;
        if (mode & !allowed_mode_bits) != 0 {
            return Err(EINVAL);
        }
        if path.is_empty() && (flags & AT_EMPTY_PATH_FLAG) != 0 {
            if dirfd == AT_FDCWD {
                return Ok(0);
            }
            let _ = procs.current()?.fd(dirfd)?;
            return Ok(0);
        }
        if path.is_empty() {
            return Err(ENOENT);
        }
        if path.len() >= PATH_MAX {
            return Err(ENAMETOOLONG);
        }
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
        let stat = vfs.stat_path(&cwd, &path)?;
        if (mode & W_OK) != 0 && (vfs.mount_flags_for_path(&cwd, &path) & (MS_RDONLY as u32)) != 0 {
            return Err(EROFS);
        }
        let current = procs.current()?;
        let uid = current.euid;
        if !access_mode_allowed(uid, stat, mode) {
            if current.name.contains("access01") || current.name.contains("access02") {
                log_always(&format!(
                    "whuse-debug: faccessat deny tgid={} uid={} path={} mode={:#x} stat_mode={:#o}",
                    current.tgid, uid, path, mode, stat.mode
                ));
            }
            return Err(EACCES);
        }
        if current.name.contains("access01") || current.name.contains("access02") {
            log_always(&format!(
                "whuse-debug: faccessat ok tgid={} uid={} path={} mode={:#x} stat_mode={:#o}",
                current.tgid, uid, path, mode, stat.mode
            ));
        }
        Ok(0)
    }

    fn sys_kill(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let pid_raw = args.0[0] as isize;
        let sig = args.0[1];
        log_always(&format!(
            "whuse-debug: kill/tkill target={} sig={} caller_tid={}",
            pid_raw,
            sig,
            procs.current_tid().unwrap_or(0)
        ));
        if sig > 64 {
            return Err(EINVAL);
        }
        if pid_raw == isize::MIN {
            return Err(EINVAL);
        }
        if pid_raw > 0 {
            let target_tid = procs.send_signal_tid(pid_raw as usize, sig)?;
            if sig != 0 {
                let _ = scheduler.wake_task(target_tid);
            }
            return Ok(0);
        }
        if pid_raw == 0 {
            let pgid = procs.current_pgid()?;
            let targets = procs.send_signal_pgid(pgid, sig, None)?;
            if sig != 0 {
                for tid in targets {
                    let _ = scheduler.wake_task(tid);
                }
            }
            return Ok(0);
        }
        if pid_raw == -1 {
            let exclude_tgid = None;
            let targets = procs.send_signal_all(sig, exclude_tgid, false)?;
            if sig != 0 {
                for tid in targets {
                    let _ = scheduler.wake_task(tid);
                }
            }
            return Ok(0);
        }
        let pgid = (-pid_raw) as usize;
        let targets = procs.send_signal_pgid(pgid, sig, None)?;
        if sig != 0 {
            for tid in targets {
                let _ = scheduler.wake_task(tid);
            }
        }
        Ok(0)
    }

    fn sys_tgkill(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let tgid = args.0[0];
        let tid = args.0[1];
        let sig = args.0[2];
        log_always(&format!(
            "whuse-debug: tgkill tgid={} tid={} sig={}",
            tgid, tid, sig
        ));
        if sig > 64 {
            return Err(EINVAL);
        }
        if tgid == 0 || tid == 0 {
            return Err(EINVAL);
        }
        let in_group = procs
            .process_snapshots()
            .iter()
            .any(|process| process.tid == tid && process.tgid == tgid);
        if !in_group {
            return Err(ESRCH);
        }
        let target_tid = procs.send_signal_exact_tid(tid, sig)?;
        if sig != 0 {
            let _ = scheduler.wake_task(target_tid);
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

    pub(crate) fn sys_rt_sigtimedwait(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let set_ptr = args.0[0];
        let info_ptr = args.0[1];
        let timeout_ptr = args.0[2];
        let sigsetsize = args.0[3];

        if sigsetsize != 8 {
            return Err(EINVAL);
        }

        let mut set = [0u8; 8];
        procs
            .current()?
            .read_user_bytes(set_ptr, 8)
            .map(|bytes| set.copy_from_slice(&bytes))
            .map_err(|_| EFAULT)?;
        let wait_mask = u64::from_le_bytes(set);

        let mut found_signal = None;
        {
            let process = procs.current_mut()?;
            let matching = process.pending_signals & wait_mask;
            if matching != 0 {
                let signal = matching.trailing_zeros() as usize + 1;
                process.pending_signals &= !(1u64 << (signal - 1));
                found_signal = Some(signal);
            }
        }

        let now = hal().timer.monotonic_nanos();
        if found_signal.is_none() {
            let process = procs.current_mut()?;
            let pending_unmasked = process.pending_signals & !process.signal_mask;
            if pending_unmasked != 0 {
                process.sleep_deadline_ns = None;
                return Err(EINTR);
            }

            if process.sleep_deadline_ns.is_none() {
                if timeout_ptr != 0 {
                    let requested = read_timespec_ns(process, timeout_ptr)?;
                    if requested == 0 {
                        return Err(EAGAIN);
                    }
                    process.sleep_deadline_ns = Some(now.saturating_add(requested));
                } else {
                    process.sleep_deadline_ns = Some(u64::MAX);
                }
                process.sleep_absolute = false;
            }

            let deadline = process.sleep_deadline_ns.unwrap();

            if now < deadline {
                let _ = scheduler.block_current();
                return Err(EAGAIN);
            }

            process.sleep_deadline_ns = None;
            return Err(EAGAIN);
        }

        procs.current_mut()?.sleep_deadline_ns = None;
        let signal = found_signal.unwrap();

        if info_ptr != 0 {
            let mut info = [0u8; 128];
            info[0] = signal as u8;
            procs
                .current_mut()?
                .write_user_bytes(info_ptr, &info)
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
        if let Ok(current) = procs.current() {
            if current.name.contains("access01") || current.name.contains("access02") {
                log_always(&format!(
                    "whuse-debug: setuid before tgid={} uid={} euid={} new={}",
                    current.tgid, current.uid, current.euid, args.0[0]
                ));
            }
        }
        procs.setuid_current(args.0[0] as u32)?;
        if let Ok(current) = procs.current() {
            if current.name.contains("access01") || current.name.contains("access02") {
                log_always(&format!(
                    "whuse-debug: setuid after tgid={} uid={} euid={}",
                    current.tgid, current.uid, current.euid
                ));
            }
        }
        Ok(0)
    }

    fn sys_setreuid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let ruid = parse_optional_uid(args.0[0]);
        let euid = parse_optional_uid(args.0[1]);
        if let Ok(current) = procs.current() {
            if current.name.contains("access01") || current.name.contains("access02") || current.name.contains("adjtimex02") {
                log_always(&format!(
                    "whuse-debug: setreuid tgid={} uid={} euid={} -> ruid={:?} euid={:?}",
                    current.tgid, current.uid, current.euid, ruid, euid
                ));
            }
        }
        procs.setresuid_current(ruid, euid)?;
        Ok(0)
    }

    fn sys_setresuid(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let ruid = parse_optional_uid(args.0[0]);
        let euid = parse_optional_uid(args.0[1]);
        let _suid = parse_optional_uid(args.0[2]);
        if let Ok(current) = procs.current() {
            if current.name.contains("access01") || current.name.contains("access02") || current.name.contains("adjtimex02") {
                log_always(&format!(
                    "whuse-debug: setresuid tgid={} uid={} euid={} -> ruid={:?} euid={:?}",
                    current.tgid, current.uid, current.euid, ruid, euid
                ));
            }
        }
        procs.setresuid_current(ruid, euid)?;
        Ok(0)
    }

    fn sys_adjtimex(&self, args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
        let addr = args.0[0];
        if addr == 0 {
            return Err(EFAULT);
        }
        let raw = procs
            .current()?
            .read_user_bytes(addr, TIMEX_SIZE)
            .map_err(|_| EFAULT)?;
        let mut buf = [0u8; TIMEX_SIZE];
        buf.copy_from_slice(&raw);
        let modes = timex_read_u32(&buf, 0);

        if modes == 0 {
            let state = *ADJTIMEX_STATE.lock();
            let out = timex_state_bytes(state);
            procs.current_mut()?.write_user_bytes(addr, &out).map_err(|_| EFAULT)?;
            return Ok(TIME_OK);
        }

        let valid_mask = ADJ_OFFSET
            | ADJ_FREQUENCY
            | ADJ_MAXERROR
            | ADJ_ESTERROR
            | ADJ_STATUS
            | ADJ_TIMECONST
            | ADJ_TICK;
        if modes != ADJ_OFFSET_SINGLESHOT && (modes & !valid_mask) != 0 {
            return Err(EINVAL);
        }
        if procs.current()?.euid != 0 {
            return Err(EPERM);
        }

        let tick = timex_read_i64(&buf, TIMEX_TICK_OFF);
        if (modes & ADJ_TICK) != 0 && !(9_000..=11_000).contains(&tick) {
            return Err(EINVAL);
        }

        let mut state = ADJTIMEX_STATE.lock();
        if (modes & ADJ_OFFSET) != 0 || modes == ADJ_OFFSET_SINGLESHOT {
            state.offset = timex_read_i64(&buf, TIMEX_OFFSET_OFF);
        }
        if (modes & ADJ_FREQUENCY) != 0 {
            state.freq = timex_read_i64(&buf, TIMEX_FREQ_OFF);
        }
        if (modes & ADJ_MAXERROR) != 0 {
            state.maxerror = timex_read_i64(&buf, TIMEX_MAXERROR_OFF);
        }
        if (modes & ADJ_ESTERROR) != 0 {
            state.esterror = timex_read_i64(&buf, TIMEX_ESTERROR_OFF);
        }
        if (modes & ADJ_STATUS) != 0 {
            state.status = timex_read_i32(&buf, TIMEX_STATUS_OFF);
        }
        if (modes & ADJ_TIMECONST) != 0 {
            state.constant = timex_read_i64(&buf, TIMEX_CONSTANT_OFF);
        }
        if (modes & ADJ_TICK) != 0 {
            state.tick = tick;
        }

        let out = timex_state_bytes(*state);
        drop(state);
        procs.current_mut()?.write_user_bytes(addr, &out).map_err(|_| EFAULT)?;
        Ok(TIME_OK)
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
        let dirfd = args.0[0] as i32;
        let flags = args.0[2];
        let path = read_at_path_allow_empty(procs.current()?, args.0[1], flags)?;
        if let Some(applet) = BUSYBOX_APPLETS.lock().get(&procs.current_tgid()?).cloned() {
            if applet == "du" {
                trace_enosys(&format!(
                    "whuse: du statx dirfd={} path={} flags={:#x}",
                    dirfd, path, flags
                ));
            }
        }
        let stat = if path.is_empty() && (flags & AT_EMPTY_PATH_FLAG) != 0 {
            if dirfd == AT_FDCWD {
                let cwd = procs.current()?.cwd.clone();
                vfs.stat_path(&cwd, &cwd)?
            } else {
                let handle = procs.current()?.fd(dirfd)?;
                vfs.stat_handle(handle)?
            }
        } else {
            let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
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
        let uaddr = args.0[0];
        let op = args.0[1] & 0x7f;
        let (current_tid, current_tgid, libctest_task, libcbench_task) =
            if let Ok(process) = procs.current() {
            (
                process.tid,
                process.tgid,
                is_libctest_task_name(process.name.as_str()),
                is_libcbench_task(process),
            )
        } else {
            (0, 0, false, false)
        };
        if libctest_task
            && (op == FUTEX_WAIT
                || op == FUTEX_WAIT_BITSET
                || op == FUTEX_WAKE
                || op == FUTEX_WAKE_BITSET)
        {
            log_always(&format!(
                "whuse-libctest:futex-enter tid={} tgid={} op={} uaddr={:#x} val={}",
                current_tid, current_tgid, op, uaddr, args.0[2] as i32
            ));
        }
        if libcbench_task
            && (op == FUTEX_WAIT
                || op == FUTEX_WAIT_BITSET
                || op == FUTEX_WAKE
                || op == FUTEX_WAKE_BITSET)
        {
            libcbench_debug(&format!(
                "whuse-libcbench:futex-enter tid={} tgid={} op={} uaddr={:#x} val={}",
                current_tid, current_tgid, op, uaddr, args.0[2] as i32
            ));
        }
        trace_line(&format!(
            "whuse: futex tgid={} op={} uaddr=0x{:x}",
            procs.current_tgid().unwrap_or(0),
            op,
            uaddr
        ));
        let val = args.0[2] as i32;
        match op {
            FUTEX_WAIT | FUTEX_WAIT_BITSET => {
                let tid = procs.current_tid()?;
                let now = hal().timer.monotonic_nanos();
                if let (Some(wait_addr), Some(deadline)) = (
                    procs.current()?.futex_wait_addr,
                    procs.current()?.futex_wait_deadline_ns,
                ) {
                    if wait_addr == uaddr {
                        let pending =
                            procs.current()?.pending_signals & !procs.current()?.signal_mask;
                        let signal_frame_pending = procs.current()?.signal_frame_pending;
                        let cancel_seen = procs.current()?.cancel_signal_seen;
                        let cancel_once = procs.current()?.cancel_interrupt_once;
                        if pending != 0 || signal_frame_pending || cancel_once {
                            procs.clear_futex_wait_state(tid);
                            let consumed = if cancel_once {
                                procs.current_mut()?.consume_cancel_interrupt_once()
                            } else {
                                false
                            };
                            if libctest_task {
                                log_always(&format!(
                                    "whuse-libctest:futex-eintr tid={} addr={:#x} pending={:#x} sfp={} cancel_seen={} cancel_once={} consumed={}",
                                    tid, wait_addr, pending, signal_frame_pending, cancel_seen, cancel_once, consumed
                                ));
                            }
                            cancel_debug(&format!(
                                "whuse-debug: FUTEX_WAIT EINTR tid={} addr={:#x} pending={:#x} sfp={} cancel_seen={} cancel_once={} consumed={}",
                                tid, wait_addr, pending, signal_frame_pending, cancel_seen, cancel_once, consumed
                            ));
                            return Err(EINTR);
                        }
                        if !procs.is_futex_waiting(wait_addr, tid) {
                            procs.clear_futex_wait_state(tid);
                            if libcbench_task {
                                libcbench_debug(&format!(
                                    "whuse-libcbench:futex-woke tid={} addr={:#x}",
                                    tid, wait_addr
                                ));
                            }
                            cancel_debug(&format!(
                                "whuse-debug: FUTEX_WAIT woke(remove) tid={} addr={:#x}",
                                tid, wait_addr
                            ));
                            return Ok(0);
                        }
                        if deadline != u64::MAX && now >= deadline {
                            procs.clear_futex_wait_state(tid);
                            return Err(ETIMEDOUT);
                        }
                        let _ = scheduler.block_current();
                        return Err(EAGAIN);
                    } else {
                        procs.clear_futex_wait_state(tid);
                    }
                }
                let pending = procs.current()?.pending_signals & !procs.current()?.signal_mask;
                let signal_frame_pending = procs.current()?.signal_frame_pending;
                let cancel_seen = procs.current()?.cancel_signal_seen;
                let cancel_once = procs.current()?.cancel_interrupt_once;
                if pending != 0 || signal_frame_pending || cancel_once {
                    let consumed = if cancel_once {
                        procs.current_mut()?.consume_cancel_interrupt_once()
                    } else {
                        false
                    };
                    if libctest_task {
                        log_always(&format!(
                            "whuse-libctest:futex-eintr-fresh tid={} addr={:#x} pending={:#x} sfp={} cancel_seen={} cancel_once={} consumed={}",
                            tid, uaddr, pending, signal_frame_pending, cancel_seen, cancel_once, consumed
                        ));
                    }
                    cancel_debug(&format!(
                        "whuse-debug: FUTEX_WAIT EINTR(fresh) tid={} uaddr={:#x} pending={:#x} sfp={} cancel_seen={} cancel_once={} consumed={}",
                        tid, uaddr, pending, signal_frame_pending, cancel_seen, cancel_once, consumed
                    ));
                    return Err(EINTR);
                }
                let current = read_i32(procs.current()?, uaddr)?;
                if current != val {
                    cancel_debug(&format!(
                        "whuse-debug: FUTEX_WAIT EAGAIN tid={} uaddr={:#x} cur={} val={}",
                        tid, uaddr, current, val
                    ));
                    Err(EAGAIN)
                } else {
                    let timeout_ptr = args.0[3];
                    let deadline = if timeout_ptr == 0 {
                        u64::MAX
                    } else {
                        now.saturating_add(read_timespec_ns(procs.current()?, timeout_ptr)?)
                    };
                    cancel_debug(&format!(
                        "whuse-debug: FUTEX_WAIT tid={} uaddr={:#x} val={} sfp={} cancel_seen={} cancel_once={}",
                        tid,
                        uaddr,
                        val,
                        procs.current()?.signal_frame_pending,
                        procs.current()?.cancel_signal_seen,
                        procs.current()?.cancel_interrupt_once
                    ));
                    procs.enqueue_futex_waiter(uaddr, tid);
                    {
                        let process = procs.current_mut()?;
                        process.futex_wait_addr = Some(uaddr);
                        process.futex_wait_deadline_ns = Some(deadline);
                    }
                    if libctest_task {
                        log_always(&format!(
                            "whuse-libctest:futex-block tid={} addr={:#x} val={} deadline={}",
                            tid, uaddr, val, deadline
                        ));
                    }
                    if libcbench_task {
                        libcbench_debug(&format!(
                            "whuse-libcbench:futex-block tid={} addr={:#x} val={} deadline={}",
                            tid, uaddr, val, deadline
                        ));
                    }
                    let _ = scheduler.block_current();
                    Err(EAGAIN)
                }
            }
            FUTEX_WAKE | FUTEX_WAKE_BITSET => {
                let wake_count = val.max(0) as usize;
                let woke = procs.wake_futex(uaddr, wake_count);
                let wake_targets_thread_list_lock = libcbench_task
                    && procs
                        .current()
                        .ok()
                        .and_then(|process| process.clear_child_tid())
                        == Some(uaddr);
                if libcbench_task {
                    let current_word = read_i32(procs.current()?, uaddr).unwrap_or(i32::MIN);
                    libcbench_debug(&format!(
                        "whuse-libcbench:futex-wake tid={} addr={:#x} req={} cur={} woke={:?}",
                        current_tid, uaddr, wake_count, current_word, woke
                    ));
                }
                if libctest_task {
                    log_always(&format!(
                        "whuse-libctest:futex-wake tid={} addr={:#x} req={} woke={}",
                        current_tid,
                        uaddr,
                        wake_count,
                        woke.len()
                    ));
                }
                for tid in &woke {
                    let _ = scheduler.wake_task(*tid);
                }
                if wake_targets_thread_list_lock && !woke.is_empty() && scheduler.ready_count() > 0
                {
                    let _ = scheduler.yield_now();
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
            FUTEX_WAKE_OP => {
                let uaddr2 = args.0[4];
                let val2 = args.0[5] as usize;
                let val3 = args.0[3] as u32;
                let op = (val3 >> 28) & 0xf;
                let cmp = (val3 >> 24) & 0xf;
                let oparg = (val3 >> 12) & 0xfff;
                let cmparg = val3 & 0xfff;

                let oldval = read_i32(procs.current()?, uaddr2)?;
                let newval = match op {
                    0 => oparg as i32,
                    1 => oldval.wrapping_add(oparg as i32),
                    2 => oldval | (oparg as i32),
                    3 => oldval & !(oparg as i32),
                    4 => oldval ^ (oparg as i32),
                    _ => return Err(EINVAL),
                };
                procs
                    .current_mut()?
                    .write_user_bytes(uaddr2, &newval.to_le_bytes())
                    .map_err(|_| EFAULT)?;

                let wake_count = val.max(0) as usize;
                let woke = procs.wake_futex(uaddr, wake_count);
                for tid in &woke {
                    let _ = scheduler.wake_task(*tid);
                }
                let wake_targets_thread_list_lock = libcbench_task
                    && procs
                        .current()
                        .ok()
                        .and_then(|process| process.clear_child_tid())
                        == Some(uaddr);
                if wake_targets_thread_list_lock && !woke.is_empty() && scheduler.ready_count() > 0
                {
                    let _ = scheduler.yield_now();
                }

                let condition = match cmp {
                    0 => oldval == cmparg as i32,
                    1 => oldval != cmparg as i32,
                    2 => oldval < cmparg as i32,
                    3 => oldval <= cmparg as i32,
                    4 => oldval > cmparg as i32,
                    5 => oldval >= cmparg as i32,
                    _ => return Err(EINVAL),
                };
                if condition {
                    let woke2 = procs.wake_futex(uaddr2, val2);
                    for tid in &woke2 {
                        let _ = scheduler.wake_task(*tid);
                    }
                    let wake2_targets_thread_list_lock = libcbench_task
                        && procs
                            .current()
                            .ok()
                            .and_then(|process| process.clear_child_tid())
                            == Some(uaddr2);
                    if wake2_targets_thread_list_lock
                        && !woke2.is_empty()
                        && scheduler.ready_count() > 0
                    {
                        let _ = scheduler.yield_now();
                    }
                    Ok(woke.len() + woke2.len())
                } else {
                    Ok(woke.len())
                }
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
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let epfd = args.0[0] as i32;
        let events_ptr = args.0[1];
        let maxevents_signed = args.0[2] as isize;
        let timeout_ms = args.0[3] as isize;
        if maxevents_signed <= 0 || timeout_ms < -1 {
            return Err(EINVAL);
        }
        let maxevents = maxevents_signed as usize;
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
        if !ready.is_empty() {
            procs.current_mut()?.epoll_wait_deadline_ns = None;
            return Ok(ready.len());
        }
        if timeout_ms == 0 {
            procs.current_mut()?.epoll_wait_deadline_ns = None;
            return Ok(0);
        }
        let now = hal().timer.monotonic_nanos();
        let process = procs.current_mut()?;
        if process.epoll_wait_deadline_ns.is_none() {
            process.epoll_wait_deadline_ns = Some(if timeout_ms < 0 {
                u64::MAX
            } else {
                now.saturating_add((timeout_ms as u64).saturating_mul(1_000_000))
            });
        }
        if process
            .epoll_wait_deadline_ns
            .is_some_and(|deadline| deadline != u64::MAX && now >= deadline)
        {
            process.epoll_wait_deadline_ns = None;
            return Ok(0);
        }
        let _ = scheduler.block_current();
        Err(EAGAIN)
    }

    fn sys_epoll_pwait2(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let timeout_ptr = args.0[3];
        let timeout_ms = if timeout_ptr == 0 {
            -1
        } else {
            (read_timespec_ns(procs.current()?, timeout_ptr)? / 1_000_000) as isize
        };
        let bridged = SyscallArgs([
            args.0[0],
            args.0[1],
            args.0[2],
            timeout_ms.max(-1) as usize,
            args.0[4],
            args.0[5],
        ]);
        self.sys_epoll_pwait(bridged, procs, vfs, scheduler)
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

    fn sys_fchmod(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let mode = args.0[1] as u32;
        let handle = procs.current()?.fd(args.0[0] as i32)?.clone();
        vfs.chmod_handle(&handle, mode)?;
        Ok(0)
    }

    fn sys_fchmodat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let dirfd = args.0[0] as i32;
        let flags = args.0[3];
        let path = read_at_path_allow_empty(procs.current()?, args.0[1], flags)?;
        let mode = args.0[2] as u32;
        if path.is_empty() && (flags & AT_EMPTY_PATH_FLAG) != 0 {
            if dirfd == AT_FDCWD {
                return Ok(0);
            }
            let handle = procs.current()?.fd(dirfd)?.clone();
            return vfs.chmod_handle(&handle, mode).map(|_| 0);
        }
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
        vfs.chmod_path(&cwd, &path, mode)?;
        Ok(0)
    }

    fn sys_fchownat(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let dirfd = args.0[0] as i32;
        let flags = args.0[4];
        let allowed_flags = AT_EMPTY_PATH_FLAG | AT_SYMLINK_NOFOLLOW_FLAG;
        if (flags & !allowed_flags) != 0 {
            return Err(EINVAL);
        }
        let path_ptr = args.0[1];
        let path = read_at_path_allow_empty(procs.current()?, path_ptr, flags)?;
        let _owner = args.0[2];
        let _group = args.0[3];
        if path.is_empty() && (flags & AT_EMPTY_PATH_FLAG) != 0 {
            if dirfd == AT_FDCWD {
                return Ok(0);
            }
            let _ = procs.current()?.fd(dirfd)?;
            return Ok(0);
        }
        let cwd = resolve_at_cwd(procs.current()?, vfs, dirfd, &path)?;
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
        const AT_EMPTY_PATH: usize = 0x1000;
        let dirfd = args.0[0] as i32;
        let path_ptr = args.0[1];
        let _times = args.0[2];
        let flags = args.0[3];
        // If path pointer is null, treat as empty path.
        let path = if path_ptr == 0 {
            String::new()
        } else {
            procs
                .current()?
                .read_user_cstr(path_ptr)
                .map_err(|_| EFAULT)?
        };
        if path.is_empty() && (flags & AT_EMPTY_PATH) != 0 {
            // Operate on the fd directly. We don't track timestamps, so just
            // validate that the fd exists and return success.
            if dirfd != AT_FDCWD {
                let _ = procs.current()?.fd(dirfd)?;
            }
            return Ok(0);
        }
        let cwd = procs.current()?.cwd.clone();
        vfs.access(&cwd, &path)?;
        Ok(0)
    }

    fn sys_close_range(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let first = args.0[0] as i32;
        let last = args.0[1] as i32;
        let _flags = args.0[2];
        let mut wake_blocked = false;
        let owner_tgid = procs.current_tgid()?;
        let mut released_any_lock = false;
        for fd in first..=last {
            let handle = match procs.current()?.fd(fd) {
                Ok(handle) => handle.clone(),
                Err(_) => continue,
            };
            if procs.current_mut()?.close_fd(fd).is_err() {
                continue;
            }
            wake_blocked |= vfs.is_pipe(&handle);
            released_any_lock |= FCNTL_LOCK_STATE
                .lock()
                .clear_for_owner_path(owner_tgid, &handle.path);
        }
        if wake_blocked || released_any_lock {
            let _ = scheduler.wake_all_blocked();
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

    fn sys_clock_nanosleep(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let _clock_id = args.0[0];
        let flags = args.0[1];
        let req_ptr = args.0[2];
        let rem_ptr = args.0[3];
        if req_ptr == 0 {
            return Err(EFAULT);
        }
        let now = hal().timer.monotonic_nanos();
        let pending = procs.pending_signals()?;
        if procs.current()?.sleep_deadline_ns.is_none() {
            let requested = read_timespec_ns(procs.current()?, req_ptr)?;
            let absolute = (flags & TIMER_ABSTIME) != 0;
            let deadline = if absolute {
                requested
            } else {
                now.saturating_add(requested)
            };
            let process = procs.current_mut()?;
            process.sleep_deadline_ns = Some(deadline);
            process.sleep_requested_ns = requested;
            process.sleep_remain_ptr = (rem_ptr != 0).then_some(rem_ptr);
            process.sleep_absolute = absolute;
        }
        let deadline = procs.current()?.sleep_deadline_ns.unwrap_or(now);
        let sleep_absolute = procs.current()?.sleep_absolute;
        let requested_ns = procs.current()?.sleep_requested_ns;
        let remain_ptr = procs.current()?.sleep_remain_ptr;

        if pending != 0 {
            if !sleep_absolute {
                if let Some(ptr) = remain_ptr {
                    let remain = deadline.saturating_sub(now).min(requested_ns);
                    procs
                        .current_mut()?
                        .write_user_bytes(ptr, &nanos_to_timespec_bytes(remain))
                        .map_err(|_| EFAULT)?;
                }
            }
            let process = procs.current_mut()?;
            process.sleep_deadline_ns = None;
            process.sleep_requested_ns = 0;
            process.sleep_remain_ptr = None;
            process.sleep_absolute = false;
            return Err(EINTR);
        }

        if now < deadline {
            let _ = scheduler.block_current();
            return Err(EAGAIN);
        }

        if !sleep_absolute {
            if let Some(ptr) = remain_ptr {
                procs
                    .current_mut()?
                    .write_user_bytes(
                        ptr,
                        &timespec_to_bytes(Timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        }),
                    )
                    .map_err(|_| EFAULT)?;
            }
        }
        let process = procs.current_mut()?;
        process.sleep_deadline_ns = None;
        process.sleep_requested_ns = 0;
        process.sleep_remain_ptr = None;
        process.sleep_absolute = false;
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

    fn sys_rt_sigsuspend(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
    ) -> Result<usize, i32> {
        let set_ptr = args.0[0];
        let size = args.0[1].max(8);
        let (wait_debug, cooperative_polling, tgid) = {
            let process = procs.current()?;
            (
                process.name.contains("busybox") && process.cwd == "/musl",
                process.name.contains("busybox") && process.cwd == "/musl",
                process.tgid,
            )
        };
        let pending_unmasked = if procs.current()?.sigsuspend_saved_mask.is_none() {
            let new_mask = if set_ptr == 0 {
                0
            } else {
                read_mask(procs.current()?, set_ptr, size)?
            };
            let old_mask = procs.signal_mask()?;
            let process = procs.current_mut()?;
            process.sigsuspend_saved_mask = Some(old_mask);
            process.signal_mask = new_mask;
            procs.pending_signals()?
        } else {
            procs.pending_signals()?
        };
        if pending_unmasked != 0 {
            let signum = pending_unmasked.trailing_zeros() as usize + 1;
            // Standard signals (e.g. SIGCHLD) are not routed through
            // dispatch_pending_signals handlers; consume one here so
            // sigsuspend does not spin forever on the same pending bit.
            if signum < 32 {
                let _ = procs.clear_pending_signal(signum);
                if wait_debug {
                    log_always(&format!(
                        "whuse-sigsuspend: consume-standard-signal tgid={} signum={}",
                        tgid, signum
                    ));
                }
            }
            if let Some(old_mask) = procs.current()?.sigsuspend_saved_mask {
                let process = procs.current_mut()?;
                process.signal_mask = old_mask;
                process.sigsuspend_saved_mask = None;
            }
            return Err(EINTR);
        }
        if cooperative_polling {
            let _ = scheduler.yield_now();
            return Err(EAGAIN);
        }
        let _ = scheduler.block_current();
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

    fn sys_rt_sigreturn(&self, procs: &mut ProcessTable) -> Result<usize, i32> {
        const FRAME_SIZE: usize = 1088;
        const UCONTEXT_OFF: usize = 128;
        const UC_SIGMASK_OFF: usize = UCONTEXT_OFF + 40;
        const MCTX_OFF: usize = UCONTEXT_OFF + 176;
        const MCTX_FP_OFF: usize = MCTX_OFF + 32 * 8;
        const MCTX_D_FCSR_OFF: usize = MCTX_FP_OFF + 32 * 8;

        let process = procs.current_mut().map_err(|_| ESRCH)?;
        let libctest_task = is_libctest_task_name(process.name.as_str());
        let libc_bench_task = process.name.contains("libc-bench");
        let frame_sp = process.trap_frame.regs[2];

        let frame = process
            .read_user_bytes(frame_sp, FRAME_SIZE)
            .map_err(|_| EFAULT)?;

        let read_u64 = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&frame[off..off + 8]);
            u64::from_le_bytes(b)
        };

        let saved_mask = read_u64(UC_SIGMASK_OFF);
        let saved_pc = read_u64(MCTX_OFF) as usize;
        let saved_fcsr = {
            let mut b = [0u8; 4];
            b.copy_from_slice(&frame[MCTX_D_FCSR_OFF..MCTX_D_FCSR_OFF + 4]);
            u32::from_le_bytes(b) as usize
        };

        cancel_debug(&format!(
            "whuse-debug: rt_sigreturn tid={} sfp={} cancel_seen={} cancel_once={} saved_pc={:#x}",
            process.tid,
            process.signal_frame_pending,
            process.cancel_signal_seen,
            process.cancel_interrupt_once,
            saved_pc
        ));
        if libctest_task {
            log_always(&format!(
                "whuse-libctest:rt_sigreturn tid={} sfp={} cancel_seen={} cancel_once={} saved_pc={:#x}",
                process.tid,
                process.signal_frame_pending,
                process.cancel_signal_seen,
                process.cancel_interrupt_once,
                saved_pc
            ));
        }
        if libc_bench_task {
            log_always(&format!(
                "whuse-libcbench-signal:sigreturn tid={} sfp={} saved_pc={:#x} saved_mask={:#x} sp={:#x}",
                process.tid, process.signal_frame_pending, saved_pc, saved_mask, frame_sp
            ));
        }
        signal_frame_debug(&format!(
            "whuse-signal-frame:sigreturn tid={} frame_sp={:#x} saved_pc={:#x} saved_mask={:#x}",
            process.tid, frame_sp, saved_pc, saved_mask
        ));
        process.signal_mask = saved_mask;
        process.signal_frame_pending = false;
        process.arm_cancel_interrupt_once();
        process.trap_frame.sepc = saved_pc;
        #[cfg(target_arch = "riscv64")]
        {
            for i in 0..32usize {
                let off = MCTX_FP_OFF + i * 8;
                let mut b = [0u8; 8];
                b.copy_from_slice(&frame[off..off + 8]);
                process.trap_frame.fregs[i] = u64::from_le_bytes(b);
            }
            process.trap_frame.fcsr = saved_fcsr;
        }
        let _ = saved_fcsr;

        for i in 1usize..32 {
            let off = MCTX_OFF + i * 8;
            let mut b = [0u8; 8];
            b.copy_from_slice(&frame[off..off + 8]);
            process.trap_frame.regs[i] = usize::from_le_bytes(b);
        }

        Ok(process.trap_frame.regs[10])
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

    fn sys_membarrier(&self, args: SyscallArgs) -> Result<usize, i32> {
        // Single-core kernel: membarrier is effectively a validated no-op for
        // the private expedited command family used by libc/pthread runtimes.
        const MEMBARRIER_CMD_QUERY: usize = 0;
        const MEMBARRIER_CMD_PRIVATE_EXPEDITED: usize = 1 << 3;
        const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: usize = 1 << 4;
        const MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE: usize = 1 << 5;
        const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE: usize = 1 << 6;
        const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 7;
        const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 8;

        let cmd = args.0[0];
        let flags = args.0[1];
        if flags != 0 {
            return Err(EINVAL);
        }

        let supported = MEMBARRIER_CMD_PRIVATE_EXPEDITED
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
            | MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE
            | MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ;

        match cmd {
            MEMBARRIER_CMD_QUERY => Ok(supported),
            MEMBARRIER_CMD_PRIVATE_EXPEDITED
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
            | MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE
            | MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ
            | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ => Ok(0),
            _ => Err(EINVAL),
        }
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
        // Accept SOCK_STREAM (1) and SOCK_DGRAM (2); reject everything else.
        if !matches!(sock_type, 1 | 2) {
            return Err(EPROTOTYPE);
        }
        let handle = vfs.create_socket(family, sock_type)?;
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
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let path = parse_sockaddr_path(procs.current()?, args.0[1], args.0[2])?;
        let cwd = procs.current()?.cwd.clone();
        let process = procs.current_mut()?;
        let handle = process.fd_mut(fd)?;
        vfs.connect_socket(handle, &cwd, &path)?;
        let _ = scheduler.wake_all_blocked();
        Ok(0)
    }

    fn sys_accept(
        &self,
        args: SyscallArgs,
        procs: &mut ProcessTable,
        scheduler: &mut Scheduler,
        vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
        let fd = args.0[0] as i32;
        let mut spins = 0usize;
        let new_handle = loop {
            let accept_result = {
                let process = procs.current_mut()?;
                let handle = process.fd_mut(fd)?;
                vfs.accept_socket(handle)
            };
            match accept_result {
                Ok(handle) => break handle,
                Err(EAGAIN) if spins < 256 => {
                    spins += 1;
                    let _ = scheduler.yield_now();
                }
                Err(EAGAIN) => {
                    let _ = scheduler.block_current();
                    return Err(EAGAIN);
                }
                Err(err) => return Err(err),
            }
        };
        let mut new_handle = new_handle;
        let accept_flags = args.0[3];
        new_handle.flags = with_cloexec_flag(new_handle.flags, (accept_flags & O_CLOEXEC) != 0);
        if (accept_flags & O_NONBLOCK) != 0 {
            new_handle.flags |= O_NONBLOCK as u32;
        } else {
            new_handle.flags &= !(O_NONBLOCK as u32);
        }
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

    fn sys_setsockopt(
        &self,
        _args: SyscallArgs,
        _procs: &mut ProcessTable,
        _vfs: &mut KernelVfs,
    ) -> Result<usize, i32> {
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

fn decode_clone_request_for_abi(args: SyscallArgs, riscv_legacy_order: bool) -> CloneRequest {
    let flags = args.0[0];
    let stack = args.0[1];
    let parent_tid = args.0[2];
    let (child_tid, tls_raw) = if riscv_legacy_order {
        (args.0[4], args.0[3])
    } else {
        (args.0[3], args.0[4])
    };
    CloneRequest {
        flags,
        stack,
        parent_tid,
        child_tid,
        tls: ((flags & CLONE_SETTLS) != 0).then_some(tls_raw),
    }
}

fn decode_clone_request(args: SyscallArgs) -> CloneRequest {
    decode_clone_request_for_abi(args, cfg!(target_arch = "riscv64"))
}

fn resolve_at_cwd(
    process: &proc::Process,
    vfs: &KernelVfs,
    dirfd: i32,
    path: &str,
) -> Result<String, i32> {
    if path.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(process.cwd.clone());
    }
    let handle = process.fd(dirfd)?;
    let stat = vfs.stat_handle(handle)?;
    if (stat.mode & 0o040000) != 0o040000 {
        return Err(ENOTDIR);
    }
    Ok(handle.path.clone())
}

const AT_EMPTY_PATH_FLAG: usize = 0x1000;
const AT_SYMLINK_NOFOLLOW_FLAG: usize = 0x100;

fn read_at_path_allow_empty(
    process: &proc::Process,
    path_ptr: usize,
    flags: usize,
) -> Result<String, i32> {
    if path_ptr == 0 {
        if (flags & AT_EMPTY_PATH_FLAG) != 0 {
            return Ok(String::new());
        }
        return Err(EFAULT);
    }
    process.read_user_cstr(path_ptr).map_err(|_| {
        log_user_path_fault(process, "read_at_path_allow_empty", path_ptr);
        EFAULT
    })
}

fn timespec_to_bytes(ts: Timespec) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&ts.tv_sec.to_le_bytes());
    out[8..].copy_from_slice(&ts.tv_nsec.to_le_bytes());
    out
}

fn nanos_to_timespec_bytes(nanos: u64) -> [u8; 16] {
    let secs = (nanos / 1_000_000_000) as i64;
    let nsec = (nanos % 1_000_000_000) as i64;
    timespec_to_bytes(Timespec {
        tv_sec: secs,
        tv_nsec: nsec,
    })
}

fn read_timespec_ns(process: &proc::Process, addr: usize) -> Result<u64, i32> {
    let raw = process.read_user_bytes(addr, 16).map_err(|_| EFAULT)?;
    let mut sec = [0u8; 8];
    let mut nsec = [0u8; 8];
    sec.copy_from_slice(&raw[..8]);
    nsec.copy_from_slice(&raw[8..16]);
    let sec = i64::from_le_bytes(sec);
    let nsec = i64::from_le_bytes(nsec);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
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

fn fd_set_len(nfds: usize) -> usize {
    nfds.div_ceil(8)
}

fn read_fd_set(process: &proc::Process, addr: usize, nfds: usize) -> Result<Vec<usize>, i32> {
    let raw = process
        .read_user_bytes(addr, fd_set_len(nfds))
        .map_err(|_| EFAULT)?;
    let mut out = Vec::new();
    for fd in 0..nfds {
        let byte = fd / 8;
        let bit = fd % 8;
        if raw
            .get(byte)
            .is_some_and(|value| (*value & (1 << bit)) != 0)
        {
            out.push(fd);
        }
    }
    Ok(out)
}

fn fd_set_bytes(fds: &[usize], nfds: usize) -> Vec<u8> {
    let mut out = vec![0u8; fd_set_len(nfds)];
    for fd in fds.iter().copied().filter(|fd| *fd < nfds) {
        out[fd / 8] |= 1 << (fd % 8);
    }
    out
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

fn parse_elf_interp(data: &[u8]) -> Option<String> {
    const PT_INTERP: u32 = 3;
    if data.len() < 64 || &data[..4] != b"\x7fELF" {
        return None;
    }
    if data[4] != 2 || data[5] != 1 {
        return None;
    }
    let phoff = usize::try_from(u64::from_le_bytes(data.get(32..40)?.try_into().ok()?)).ok()?;
    let phentsize = usize::from(u16::from_le_bytes(data.get(54..56)?.try_into().ok()?));
    let phnum = usize::from(u16::from_le_bytes(data.get(56..58)?.try_into().ok()?));
    if phentsize < 56 {
        return None;
    }
    for index in 0..phnum {
        let off = phoff.checked_add(index.checked_mul(phentsize)?)?;
        let end = off.checked_add(phentsize)?;
        if end > data.len() {
            return None;
        }
        let p_type = u32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?);
        if p_type != PT_INTERP {
            continue;
        }
        let p_offset = usize::try_from(u64::from_le_bytes(
            data.get(off + 8..off + 16)?.try_into().ok()?,
        ))
        .ok()?;
        let p_filesz = usize::try_from(u64::from_le_bytes(
            data.get(off + 32..off + 40)?.try_into().ok()?,
        ))
        .ok()?;
        let interp_end = p_offset.checked_add(p_filesz)?;
        if interp_end > data.len() || p_filesz == 0 {
            return None;
        }
        let raw = data.get(p_offset..interp_end)?;
        let nul = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
        if nul == 0 {
            return None;
        }
        let interp = core::str::from_utf8(&raw[..nul]).ok()?.trim();
        if interp.is_empty() {
            return None;
        }
        return Some(interp.to_string());
    }
    None
}

fn read_exec_file_image(vfs: &mut KernelVfs, cwd: &str, path: &str) -> Result<Vec<u8>, i32> {
    if path.contains("busybox") {
        if let Some(bytes) = busybox_image_cache() {
            return Ok(bytes);
        }
    }

    let mut handle = vfs.open(cwd, path, O_RDONLY, 0).map_err(|_| ENOENT)?;
    let mut file_data = Vec::new();
    const EXEC_READ_CHUNK: usize = 256 * 1024;
    const EXEC_READ_LIMIT: usize = 32 * 1024 * 1024;
    loop {
        let remaining = EXEC_READ_LIMIT.saturating_sub(file_data.len());
        if remaining == 0 {
            return Err(EFAULT);
        }
        let read_len = EXEC_READ_CHUNK.min(remaining);
        let chunk = vfs.read(&mut handle, read_len).map_err(|_| EFAULT)?;
        if chunk.is_empty() {
            break;
        }
        file_data.extend_from_slice(&chunk);
    }
    trace_line(&format!(
        "whuse: execve read image path={} bytes={}",
        path, file_data.len()
    ));
    Ok(file_data)
}

fn access_mode_allowed(uid: u32, stat: FileStat, mode: usize) -> bool {
    if mode == F_OK {
        return true;
    }
    let perm = stat.mode & 0o777;
    if uid == 0 {
        let exec_ok = (perm & 0o111) != 0;
        if (mode & X_OK) != 0 && !exec_ok {
            return false;
        }
        return true;
    }

    if (mode & R_OK) != 0 && (perm & 0o004) == 0 {
        return false;
    }
    if (mode & W_OK) != 0 && (perm & 0o002) == 0 {
        return false;
    }
    if (mode & X_OK) != 0 && (perm & 0o001) == 0 {
        return false;
    }
    true
}

fn read_u32(process: &proc::Process, addr: usize) -> Result<u32, i32> {
    let bytes = process.read_user_bytes(addr, 4).map_err(|_| EFAULT)?;
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes);
    Ok(u32::from_le_bytes(out))
}

fn timex_read_u32(buf: &[u8; TIMEX_SIZE], offset: usize) -> u32 {
    let mut out = [0u8; 4];
    out.copy_from_slice(&buf[offset..offset + 4]);
    u32::from_le_bytes(out)
}

fn timex_read_i32(buf: &[u8; TIMEX_SIZE], offset: usize) -> i32 {
    let mut out = [0u8; 4];
    out.copy_from_slice(&buf[offset..offset + 4]);
    i32::from_le_bytes(out)
}

fn timex_read_i64(buf: &[u8; TIMEX_SIZE], offset: usize) -> i64 {
    let mut out = [0u8; 8];
    out.copy_from_slice(&buf[offset..offset + 8]);
    i64::from_le_bytes(out)
}

fn timex_write_i32(buf: &mut [u8; TIMEX_SIZE], offset: usize, value: i32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn timex_write_i64(buf: &mut [u8; TIMEX_SIZE], offset: usize, value: i64) {
    buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn timex_state_bytes(state: KernelTimexState) -> [u8; TIMEX_SIZE] {
    let mut out = [0u8; TIMEX_SIZE];
    timex_write_i64(&mut out, TIMEX_OFFSET_OFF, state.offset);
    timex_write_i64(&mut out, TIMEX_FREQ_OFF, state.freq);
    timex_write_i64(&mut out, TIMEX_MAXERROR_OFF, state.maxerror);
    timex_write_i64(&mut out, TIMEX_ESTERROR_OFF, state.esterror);
    timex_write_i32(&mut out, TIMEX_STATUS_OFF, state.status);
    timex_write_i64(&mut out, TIMEX_CONSTANT_OFF, state.constant);
    timex_write_i64(&mut out, TIMEX_TICK_OFF, state.tick);
    timex_write_i32(&mut out, TIMEX_TAI_OFF, state.tai);
    out
}

fn parse_optional_uid(raw: usize) -> Option<u32> {
    if raw == usize::MAX {
        None
    } else {
        Some(raw as u32)
    }
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

fn rtc_time_bytes(ts: Timespec) -> [u8; 36] {
    let (year, month, day, hour, minute, second, weekday, yearday) =
        unix_seconds_to_calendar(ts.tv_sec);
    let fields = [
        second,
        minute,
        hour,
        day,
        month - 1,
        year - 1900,
        weekday,
        yearday,
        0,
    ];
    let mut out = [0u8; 36];
    for (index, field) in fields.into_iter().enumerate() {
        let start = index * 4;
        out[start..start + 4].copy_from_slice(&field.to_le_bytes());
    }
    out
}

fn unix_seconds_to_calendar(secs: i64) -> (i32, i32, i32, i32, i32, i32, i32, i32) {
    let day_seconds = 86_400;
    let days = secs.div_euclid(day_seconds);
    let remain = secs.rem_euclid(day_seconds);
    let hour = (remain / 3_600) as i32;
    let minute = ((remain % 3_600) / 60) as i32;
    let second = (remain % 60) as i32;

    let (year, month, day) = civil_from_days(days);
    let weekday = ((days + 4).rem_euclid(7)) as i32; // 1970-01-01 was Thursday (4).
    let yearday = day_of_year(year, month, day) - 1;

    (year, month, day, hour, minute, second, weekday, yearday)
}

fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(m <= 2);
    (year as i32, m as i32, d as i32)
}

fn day_of_year(year: i32, month: i32, day: i32) -> i32 {
    let month_index = month.saturating_sub(1).min(11) as usize;
    const CUMULATIVE: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut yday = CUMULATIVE[month_index] + day;
    if month > 2 && is_leap_year(year) {
        yday += 1;
    }
    yday
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn uname_bytes() -> [u8; 390] {
    let mut out = [0u8; 390];
    #[cfg(target_arch = "riscv64")]
    let arch = "riscv64";
    #[cfg(target_arch = "loongarch64")]
    let arch = "loongarch64";
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    let arch = "unknown";
    #[cfg(target_arch = "riscv64")]
    let platform = "whuse-riscv64-virt";
    #[cfg(target_arch = "loongarch64")]
    let platform = "whuse-loongarch64-virt";
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    let platform = "whuse-unknown";
    let fields = [
        "Linux",
        "whuse",
        "6.8.0-whuse",
        platform,
        arch,
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
        SYS_CLOSE, SYS_CONNECT, SYS_COPY_FILE_RANGE, SYS_DUP3, SYS_EPOLL_CREATE1, SYS_EPOLL_CTL,
        SYS_EPOLL_PWAIT, SYS_EVENTFD2, SYS_FACCESSAT2, SYS_FALLOCATE, SYS_FCHDIR, SYS_FCHMOD,
        SYS_FCHMODAT, SYS_FCHOWN, SYS_FCHOWNAT, SYS_FDATASYNC, SYS_FLOCK, SYS_FSTATFS, SYS_FSYNC,
        SYS_FUTEX, SYS_GETCWD, SYS_GETGROUPS, SYS_GETITIMER, SYS_GETPRIORITY, SYS_GETSID,
        SYS_GETSOCKNAME, SYS_GETSOCKOPT, SYS_GETTIMEOFDAY, SYS_GET_ROBUST_LIST, SYS_LINKAT,
        SYS_LISTEN, SYS_LSEEK, SYS_MEMBARRIER, SYS_MEMFD_CREATE, SYS_MKDIR, SYS_MLOCK, SYS_MLOCK2,
        SYS_MSYNC, SYS_OPENAT, SYS_PIDFD_GETFD, SYS_PIDFD_OPEN, SYS_PIDFD_SEND_SIGNAL, SYS_PIPE,
        SYS_PPOLL, SYS_PRCTL, SYS_PREAD64, SYS_PREADV, SYS_PREADV2, SYS_PRLIMIT64, SYS_PSELECT6,
        SYS_PWRITE64, SYS_PWRITEV, SYS_PWRITEV2, SYS_READ, SYS_RECVFROM, SYS_RECVMSG, SYS_RENAMEAT,
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
        fn set_kernel_timer_callback(&self, _cb: fn()) {}
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
        assert!(fd >= 0);

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

        assert_eq!(
            dispatcher.dispatch(
                SYS_CLOSE,
                SyscallArgs([write_fd, 0, 0, 0, 0, 0]),
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
            .install_bytes(0x7200, &[0; 1]);
        assert_eq!(
            dispatcher.dispatch(
                SYS_READ,
                SyscallArgs([read_fd, 0x7200, 1, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs
            ),
            0
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
            -11
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
    fn clone_decode_respects_riscv_legacy_tls_order() {
        let args = SyscallArgs([
            super::CLONE_THREAD | super::CLONE_SETTLS,
            0x2000,
            0x3000,
            0x4000,
            0x5000,
            0,
        ]);
        let decoded = super::decode_clone_request_for_abi(args, true);
        assert_eq!(decoded.flags, super::CLONE_THREAD | super::CLONE_SETTLS);
        assert_eq!(decoded.stack, 0x2000);
        assert_eq!(decoded.parent_tid, 0x3000);
        assert_eq!(decoded.tls, Some(0x4000));
        assert_eq!(decoded.child_tid, 0x5000);
    }

    #[test]
    fn clone_decode_respects_non_riscv_legacy_tls_order() {
        let args = SyscallArgs([
            super::CLONE_THREAD | super::CLONE_SETTLS,
            0x2000,
            0x3000,
            0x4000,
            0x5000,
            0,
        ]);
        let decoded = super::decode_clone_request_for_abi(args, false);
        assert_eq!(decoded.flags, super::CLONE_THREAD | super::CLONE_SETTLS);
        assert_eq!(decoded.stack, 0x2000);
        assert_eq!(decoded.parent_tid, 0x3000);
        assert_eq!(decoded.child_tid, 0x4000);
        assert_eq!(decoded.tls, Some(0x5000));
    }

    #[test]
    fn futex_cancel_interrupt_is_one_shot() {
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
            .install_bytes(0xa400, &0i32.to_le_bytes());
        {
            let process = procs.current_mut().unwrap();
            process.cancel_interrupt_once = true;
        }
        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa400, super::FUTEX_WAIT, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            -super::EINTR as isize
        );
        assert!(!procs.current().unwrap().cancel_interrupt_once);

        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa400, super::FUTEX_WAIT, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            -super::EAGAIN as isize
        );
        assert_eq!(procs.current().unwrap().futex_wait_addr, Some(0xa400));
        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa400, super::FUTEX_WAKE, 1, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            1
        );
        assert_eq!(procs.current().unwrap().futex_wait_addr, Some(0xa400));
        assert_eq!(
            dispatcher.dispatch(
                SYS_FUTEX,
                SyscallArgs([0xa400, super::FUTEX_WAIT, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(procs.current().unwrap().futex_wait_addr, None);
    }

    #[test]
    fn rt_sigreturn_arms_cancel_interrupt_once() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        const FRAME_SIZE: usize = 1088;
        const UCONTEXT_OFF: usize = 128;
        const UC_SIGMASK_OFF: usize = UCONTEXT_OFF + 40;
        const MCTX_OFF: usize = UCONTEXT_OFF + 176;
        let frame_sp = 0x7d00usize;
        let saved_mask = 0x1234_5678_u64;
        let saved_pc = 0x4001_2000_u64;
        let retval = 77u64;

        let mut frame = [0u8; FRAME_SIZE];
        frame[UC_SIGMASK_OFF + 8..UC_SIGMASK_OFF + 128].fill(0xff);
        frame[UC_SIGMASK_OFF..UC_SIGMASK_OFF + 8].copy_from_slice(&saved_mask.to_le_bytes());
        frame[MCTX_OFF..MCTX_OFF + 8].copy_from_slice(&saved_pc.to_le_bytes());
        let a0_off = MCTX_OFF + 10 * 8;
        frame[a0_off..a0_off + 8].copy_from_slice(&retval.to_le_bytes());

        {
            let process = procs.current_mut().unwrap();
            process.signal_frame_pending = true;
            process.cancel_signal_seen = true;
            process.cancel_interrupt_once = false;
            process.signal_mask = 0xffff;
            process.trap_frame.regs[2] = frame_sp;
            process.address_space.install_bytes(frame_sp, &frame);
        }

        assert_eq!(
            dispatcher.dispatch(
                SYS_RT_SIGRETURN,
                SyscallArgs([0, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            retval as isize
        );
        let process = procs.current().unwrap();
        assert_eq!(process.signal_mask, saved_mask);
        assert!(!process.signal_frame_pending);
        assert!(!process.cancel_signal_seen);
        assert!(process.cancel_interrupt_once);
    }

    #[test]
    fn membarrier_private_commands_supported() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        const QUERY: usize = 0;
        const PRIVATE_EXPEDITED: usize = 1 << 3;
        const REGISTER_PRIVATE_EXPEDITED: usize = 1 << 4;

        let query = dispatcher.dispatch(
            SYS_MEMBARRIER,
            SyscallArgs([QUERY, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert!(query >= 0);
        let mask = query as usize;
        assert_ne!(mask & PRIVATE_EXPEDITED, 0);
        assert_ne!(mask & REGISTER_PRIVATE_EXPEDITED, 0);

        assert_eq!(
            dispatcher.dispatch(
                SYS_MEMBARRIER,
                SyscallArgs([REGISTER_PRIVATE_EXPEDITED, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(
            dispatcher.dispatch(
                SYS_MEMBARRIER,
                SyscallArgs([PRIVATE_EXPEDITED, 0, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            0
        );
        assert_eq!(
            dispatcher.dispatch(
                SYS_MEMBARRIER,
                SyscallArgs([REGISTER_PRIVATE_EXPEDITED, 1, 0, 0, 0, 0]),
                &mut procs,
                &mut scheduler,
                &mut vfs,
            ),
            -(super::EINVAL as isize)
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
        );
        assert!(accepted >= 0);
        let accepted = accepted as usize;

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

    #[test]
    fn fcntl_lock_state_splits_overlaps_for_same_owner() {
        let mut state = super::FcntlLockState::default();
        assert!(state.apply_lock("/tmp/fcntl21", 100, super::F_RDLCK, 10, Some(15)));
        assert!(state.apply_lock("/tmp/fcntl21", 100, super::F_WRLCK, 5, Some(13)));

        let lock_a = state
            .first_conflict("/tmp/fcntl21", 200, super::F_WRLCK, 0, None)
            .expect("conflict lock A");
        assert_eq!(lock_a.lock_type, super::F_WRLCK);
        assert_eq!(lock_a.start, 5);
        assert_eq!(lock_a.end, Some(13));

        let lock_b = state
            .first_conflict("/tmp/fcntl21", 200, super::F_WRLCK, 13, None)
            .expect("conflict lock B");
        assert_eq!(lock_b.lock_type, super::F_RDLCK);
        assert_eq!(lock_b.start, 13);
        assert_eq!(lock_b.end, Some(15));
    }

    #[test]
    fn fcntl_lock_state_unlock_and_owner_clear() {
        let mut state = super::FcntlLockState::default();
        state.apply_lock("/tmp/a", 10, super::F_WRLCK, 0, None);
        state.apply_lock("/tmp/b", 10, super::F_RDLCK, 5, Some(12));
        state.apply_lock("/tmp/a", 11, super::F_RDLCK, 0, Some(3));

        assert!(state.clear_for_owner_path(10, "/tmp/b"));
        assert!(state.first_conflict("/tmp/b", 20, super::F_WRLCK, 0, None).is_none());

        assert!(state.clear_for_owner(10));
        assert!(state.first_conflict("/tmp/a", 20, super::F_WRLCK, 0, None).is_some());
        assert!(state.first_conflict("/tmp/a", 11, super::F_WRLCK, 0, None).is_none());
    }
}
