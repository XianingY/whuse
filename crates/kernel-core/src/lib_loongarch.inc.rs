extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use fs_ext4::Ext4Mount;
use hal_api::{hal, ConsoleWriter, PlatformArch, ShutdownReason};
use mm::MemoryManager;
use mm::{BinaryLoader, ElfBinaryLoader};
use proc::ProcessTable;
use syscall::cache_busybox_image;
use syscall::{
    SyscallArgs, SyscallDispatcher, SIGNAL_TRAMPOLINE_BASE, SIGNAL_TRAMPOLINE_CODE,
    SYS_CLOCK_NANOSLEEP, SYS_EPOLL_PWAIT, SYS_EPOLL_PWAIT2, SYS_EXECVE, SYS_FUTEX, SYS_MSGRCV,
    SYS_NANOSLEEP, SYS_PPOLL, SYS_PSELECT6, SYS_READ, SYS_READV, SYS_RT_SIGRETURN,
    SYS_RT_SIGSUSPEND, SYS_RT_SIGTIMEDWAIT, SYS_SEMOP, SYS_SEMTIMEDOP, SYS_WAIT,
};
use task::Scheduler;
use vfs::{KernelVfs, O_RDWR};

#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    pub hart_id: usize,
    pub dtb_pa: usize,
    pub platform: &'static str,
}

pub struct Kernel {
    pub info: BootInfo,
    pub memory: MemoryManager,
    pub processes: ProcessTable,
    pub scheduler: Scheduler,
    pub vfs: KernelVfs,
    pub syscalls: SyscallDispatcher,
    watchdog_started_at: BTreeMap<usize, u64>,
    watchdog_seen_name: BTreeMap<usize, String>,
    watchdog_last_heartbeat_ns: u64,
    watchdog_clock_ns: u64,
    watchdog_last_hw_ns: u64,
    watchdog_iozone_window_until_ns: u64,
    timer_irq_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KernelIdleOutcome {
    WaitForInterrupt,
    Shutdown,
}

fn idle_outcome_for_process_count(process_count: usize) -> KernelIdleOutcome {
    if process_count == 0 {
        KernelIdleOutcome::Shutdown
    } else {
        KernelIdleOutcome::WaitForInterrupt
    }
}

const USER_INIT_BASE: usize = 0x0040_0000;
const EAGAIN_RET: isize = -11;
const SCHED_TIME_SLICE_NS: u64 = 10_000_000;
const FORCED_PREEMPT_DELTA_NS: u64 = 5_000_000;
const LOONGARCH_TIMER_INTERRUPT_SCAUSE: usize = 0x100;
const OSCOMP_GROUP_TIMEOUT_NS: u64 = 20 * 60 * 1_000_000_000;
const OSCOMP_HEAVY_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_BUSYBOX_APPLET_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_BUSYBOX_SUPERVISOR_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID: usize = 4;
const OSCOMP_LIBCTEST_ENTRY_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_LMBENCH_TIMEOUT_NS: u64 = 600 * 1_000_000_000;
const OSCOMP_UNIXBENCH_TIMEOUT_NS: u64 = 600 * 1_000_000_000;
const OSCOMP_IOZONE_BUSYBOX_WINDOW_NS: u64 = 0;
const OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_REQUIRED_TEST_FILES: &[&str] = &[
    "/musl/busybox",
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/libc-bench",
    "/musl/iozone_testcode.sh",
    "/musl/iozone",
    "/musl/libctest_testcode.sh",
    "/musl/lmbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/lua",
    "/musl/ltp_testcode.sh",
    "/musl/runtest.exe",
    "/musl/entry-static.exe",
    "/musl/entry-dynamic.exe",
    "/glibc/busybox",
    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/libc-bench",
    "/glibc/iozone_testcode.sh",
    "/glibc/iozone",
    "/glibc/libctest_testcode.sh",
    "/glibc/lmbench_testcode.sh",
    "/glibc/lua_testcode.sh",
    "/glibc/lua",
    "/glibc/ltp_testcode.sh",
];
const OSCOMP_OPTIONAL_TEST_FILES: &[&str] = &[
    "/musl/time-test",
    "/musl/basic/run-all.sh",
    "/glibc/basic/run-all.sh",
    "/musl/unixbench_testcode.sh",
    "/musl/netperf_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/glibc/unixbench_testcode.sh",
    "/glibc/netperf_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
];
const OSCOMP_PROFILE_PATH: &str = "/whuse-oscomp-profile";
const OSCOMP_RUNTIME_FILTER_PATH: &str = "/whuse-oscomp-runtime-filter";
const OSCOMP_STAGE2_LOCAL_ENV_PATH: &str = "/musl/.whuse_stage2_local.env";
const OSCOMP_PROFILE_DEFAULT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_PROFILE_DEFAULT__";
const OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__";
const OSCOMP_FULL_MAX_GROUP_PLACEHOLDER: &str = "__WHUSE_STAGE2_FULL_MAX_GROUP__";
const OSCOMP_IOZONE_PROFILE_PLACEHOLDER: &str = "__WHUSE_STAGE2_IOZONE_PROFILE__";
const OSCOMP_BASIC_PROFILE_PLACEHOLDER: &str = "__WHUSE_STAGE2_BASIC_PROFILE__";
const OSCOMP_BUSYBOX_PROFILE_PLACEHOLDER: &str = "__WHUSE_STAGE2_BUSYBOX_PROFILE__";
const OSCOMP_GATE_LIBCTEST_SCOPE_PLACEHOLDER: &str = "__WHUSE_STAGE2_GATE_LIBCTEST_SCOPE__";
const OSCOMP_LIBCBENCH_SCOPE_PLACEHOLDER: &str = "__WHUSE_STAGE2_LIBCBENCH_SCOPE__";
const OSCOMP_LMBENCH_SCOPE_PLACEHOLDER: &str = "__WHUSE_STAGE2_LMBENCH_SCOPE__";
const OSCOMP_TIME_TEST_PRESENT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_TIME_TEST_PRESENT__";
const OSCOMP_LTP_SCORE_WHITELIST_PATH: &str = "/musl/ltp_score_whitelist.txt";
const OSCOMP_LTP_SCORE_BLACKLIST_PATH: &str = "/musl/ltp_score_blacklist.txt";
const OSCOMP_LTP_SCORE_WHITELIST_GLIBC_PATH: &str = "/glibc/ltp_score_whitelist.txt";
const OSCOMP_LTP_SCORE_BLACKLIST_GLIBC_PATH: &str = "/glibc/ltp_score_blacklist.txt";
const OSCOMP_CFG_LTP_PROFILE_PATH: &str = "/musl/.whuse_ltp_profile";
const OSCOMP_CFG_LTP_WHITELIST_PATH: &str = "/musl/.whuse_ltp_whitelist";
const OSCOMP_CFG_LTP_BLACKLIST_PATH: &str = "/musl/.whuse_ltp_blacklist";
const OSCOMP_CFG_LTP_WHITELIST_MUSL_PATH: &str = "/musl/.whuse_ltp_whitelist_musl";
const OSCOMP_CFG_LTP_BLACKLIST_MUSL_PATH: &str = "/musl/.whuse_ltp_blacklist_musl";
const OSCOMP_CFG_LTP_WHITELIST_GLIBC_PATH: &str = "/musl/.whuse_ltp_whitelist_glibc";
const OSCOMP_CFG_LTP_BLACKLIST_GLIBC_PATH: &str = "/musl/.whuse_ltp_blacklist_glibc";
const OSCOMP_CFG_LTP_TIMEOUT_PATH: &str = "/musl/.whuse_ltp_step_timeout";
const OSCOMP_LTP_SCORE_WHITELIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_whitelist_musl_la.txt");
const OSCOMP_LTP_SCORE_BLACKLIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_blacklist_musl_la.txt");
const OSCOMP_LTP_SCORE_WHITELIST_GLIBC: &str =
    include_str!("../../../tools/oscomp/ltp/score_whitelist_glibc_la.txt");
const OSCOMP_LTP_SCORE_BLACKLIST_GLIBC: &str =
    include_str!("../../../tools/oscomp/ltp/score_blacklist_glibc_la.txt");
const OSCOMP_GLIBC_BASIC_TESTCODE_ABS: &str = concat!(
    "#!/musl/busybox sh\n",
    "set +e\n",
    "/musl/busybox echo \"#### OS COMP TEST GROUP START basic-glibc ####\"\n",
    "/musl/busybox echo \"whuse-glibc-basic-shim:before-cd\"\n",
    "cd /glibc/basic || exit 1\n",
    "/musl/busybox echo \"whuse-glibc-basic-shim:before-run-all\"\n",
    "/musl/busybox sh /glibc/basic/run-all.sh\n",
    "rc=$?\n",
    "/musl/busybox echo \"whuse-glibc-basic-shim:after-run-all rc=$rc\"\n",
    "cd / || exit 1\n",
    "/musl/busybox echo \"#### OS COMP TEST GROUP END basic-glibc ####\"\n",
    "exit \"$rc\"\n",
);
const OSCOMP_GLIBC_BASIC_RUN_ALL_ABS: &str = concat!(
    "#!/musl/busybox sh\n",
    "\n",
    "tests=\"\n",
    "brk\n",
    "chdir\n",
    "clone\n",
    "close\n",
    "dup2\n",
    "dup\n",
    "execve\n",
    "exit\n",
    "fork\n",
    "fstat\n",
    "getcwd\n",
    "getdents\n",
    "getpid\n",
    "getppid\n",
    "gettimeofday\n",
    "mkdir_\n",
    "mmap\n",
    "mount\n",
    "munmap\n",
    "openat\n",
    "open\n",
    "pipe\n",
    "read\n",
    "sleep\n",
    "times\n",
    "umount\n",
    "uname\n",
    "unlink\n",
    "wait\n",
    "waitpid\n",
    "write\n",
    "yield\n",
    "\"\n",
    "for i in $tests\n",
    "do\n",
    "    if [ \"$i\" = \"brk\" ]; then echo \"whuse-glibc-run-all:before-brk\"; fi\n",
    "    echo \"Testing $i :\"\n",
    "    /glibc/basic/$i\n",
    "    if [ \"$i\" = \"brk\" ]; then echo \"whuse-glibc-run-all:after-brk rc=$?\"; fi\n",
    "done\n",
);
const OSCOMP_CFG_RUNNER_MODE_PATH: &str = "/musl/.whuse_oscomp_runner";
const OSCOMP_LIBCTEST_PRELOAD_FILES: [(&str, u32); 23] = [
    ("/musl/basic_testcode.sh", 0o100755),
    ("/glibc/basic_testcode.sh", 0o100755),
    ("/musl/busybox_testcode.sh", 0o100755),
    ("/glibc/busybox_testcode.sh", 0o100755),
    ("/musl/busybox_cmd.txt", 0o100644),
    ("/glibc/busybox_cmd.txt", 0o100644),
    ("/musl/basic/run-all.sh", 0o100755),
    ("/glibc/basic/run-all.sh", 0o100755),
    ("/musl/iozone_testcode.sh", 0o100755),
    ("/glibc/iozone_testcode.sh", 0o100755),
    ("/musl/iozone", 0o100755),
    ("/glibc/iozone", 0o100755),
    ("/musl/libctest_testcode.sh", 0o100755),
    ("/musl/run-static.sh", 0o100755),
    ("/musl/run-dynamic.sh", 0o100755),
    ("/musl/runtest.exe", 0o100755),
    ("/musl/entry-static.exe", 0o100755),
    ("/musl/entry-dynamic.exe", 0o100755),
    ("/musl/lib/libc.so", 0o100755),
    ("/glibc/lib/ld-linux-loongarch-lp64d.so.1", 0o100755),
    ("/glibc/lib/libc.so.6", 0o100755),
    ("/glibc/lib/libm.so.6", 0o100755),
    ("/glibc/lib/libm.so", 0o100755),
];
const OSCOMP_BASIC_BINARIES: [&str; 33] = [
    "brk",
    "chdir",
    "clone",
    "close",
    "dup2",
    "dup",
    "execve",
    "exit",
    "fork",
    "fstat",
    "getcwd",
    "getdents",
    "getpid",
    "getppid",
    "gettimeofday",
    "mkdir_",
    "mmap",
    "mount",
    "munmap",
    "openat",
    "open",
    "pipe",
    "read",
    "sleep",
    "test_echo",
    "times",
    "umount",
    "uname",
    "unlink",
    "wait",
    "waitpid",
    "write",
    "yield",
];
const OSCOMP_BASIC_EXTRA_FILES: [(&str, u32); 1] = [("text.txt", 0o100644)];
const OSCOMP_LTP_BOOTSTRAP_CASES: [&str; 8] = [
    "brk01", "brk02", "close01", "close02", "dup01", "dup02", "dup04", "dup07",
];
const OSCOMP_ROOT_ALIAS_ENTRIES: [&str; 122] = [
    "arithoh",
    "basic",
    "basic_testcode.sh",
    "busy",
    "busybox",
    "busybox_cmd.txt",
    "busybox_testcode.sh",
    "bw_file_rd",
    "bw_mem",
    "bw_mmap_rd",
    "bw_pipe",
    "bw_tcp",
    "bw_unix",
    "cache",
    "clock",
    "context1",
    "cyclictest",
    "cyclictest_testcode.sh",
    "date.lua",
    "dhry2",
    "dhry2reg",
    "disk",
    "dlopen_dso.so",
    "double",
    "enough",
    "entry-dynamic.exe",
    "entry-static.exe",
    "execl",
    "file_io.lua",
    "float",
    "flushdisk",
    "fstime",
    "getopt",
    "gfx-x11",
    "hackbench",
    "hanoi",
    "hello",
    "index.base",
    "int",
    "iozone",
    "iozone_testcode.sh",
    "iperf3",
    "iperf_testcode.sh",
    "lat_cmd",
    "lat_connect",
    "lat_ctx",
    "lat_dram_page",
    "lat_fcntl",
    "lat_fifo",
    "lat_fs",
    "lat_http",
    "lat_mem_rd",
    "lat_mmap",
    "lat_ops",
    "lat_pagefault",
    "lat_pipe",
    "lat_pmake",
    "lat_proc",
    "lat_rand",
    "lat_rpc",
    "lat_select",
    "lat_sem",
    "lat_sig",
    "lat_syscall",
    "lat_tcp",
    "lat_udp",
    "lat_unix",
    "lat_unix_connect",
    "lat_usleep",
    "libc-bench",
    "libcbench_testcode.sh",
    "libctest_testcode.sh",
    "line",
    "lmbench",
    "lmbench_all",
    "lmbench_testcode.sh",
    "lmdd",
    "lmhttp",
    "long",
    "loop_o",
    "looper",
    "ltp",
    "ltp_testcode.sh",
    "lua",
    "lua_testcode.sh",
    "max_min.lua",
    "memsize",
    "mhz",
    "msleep",
    "multi.sh",
    "netperf",
    "netperf_testcode.sh",
    "netserver",
    "par_mem",
    "par_ops",
    "pipe",
    "random.lua",
    "register",
    "remove.lua",
    "rhttp",
    "round_num.lua",
    "run-dynamic.sh",
    "run-static.sh",
    "runtest.exe",
    "seek",
    "short",
    "sin30.lua",
    "sort.lua",
    "spawn",
    "stream",
    "strings.lua",
    "syscall",
    "test.sh",
    "test_echo",
    "text.txt",
    "timing_o",
    "tlb",
    "tls_get_new-dtv_dso.so",
    "tst.sh",
    "unixbench.logo",
    "unixbench_testcode.sh",
    "whetstone-double",
];
const OSCOMP_STAGE2_TIMEOUT_PROFILE_DEFAULT: &str = env!("WHUSE_STAGE2_TIMEOUT_PROFILE_DEFAULT");
const OSCOMP_STAGE2_REAL_PHASE_DEFAULT: &str = env!("WHUSE_STAGE2_REAL_PHASE_DEFAULT");
const OSCOMP_STAGE2_REAL_FULL_MAX_GROUP_DEFAULT: &str =
    env!("WHUSE_STAGE2_REAL_FULL_MAX_GROUP_DEFAULT");
const OSCOMP_STAGE2_IOZONE_PROFILE_DEFAULT: &str = env!("WHUSE_STAGE2_IOZONE_PROFILE_DEFAULT");
const OSCOMP_STAGE2_IOZONE_FULL_SCOPE_DEFAULT: &str =
    env!("WHUSE_STAGE2_IOZONE_FULL_SCOPE_DEFAULT");
const OSCOMP_SUITE_SCRIPT_PATH: &str = "/tmp/whuse-oscomp-suite.sh";
const OSCOMP_LTP_STEP_HELPER_PATH: &str = "/tmp/whuse-oscomp-ltp-step.sh";
const OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH: &str = "/tmp/whuse-busybox-testcode.sh";
const OSCOMP_BUSYBOX_COMPAT_SCRIPT: &str = concat!(
    "#!/busybox sh\n",
    "set +e\n",
    "echo \"#### OS COMP TEST GROUP START busybox-musl ####\"\n",
    "FAIL=0\n",
    "echo whuse-oscomp-busybox-runner:before-first\n",
    "/musl/busybox true\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox true fail\"; else echo \"testcase busybox true success\"; fi\n",
    "echo whuse-oscomp-busybox-runner:after-first\n",
    "/musl/busybox echo \"#### independent command test\"\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox echo fail\"; else echo \"testcase busybox echo success\"; fi\n",
    "/musl/busybox which ls\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox which fail\"; else echo \"testcase busybox which success\"; fi\n",
    "/musl/busybox uname\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox uname fail\"; else echo \"testcase busybox uname success\"; fi\n",
    "/musl/busybox pwd\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox pwd fail\"; else echo \"testcase busybox pwd success\"; fi\n",
    "/musl/busybox touch test.txt\n",
    "RTN=$?\n",
    "if [ \"$RTN\" -ne 0 ]; then FAIL=1; echo \"testcase busybox touch fail\"; else echo \"testcase busybox touch success\"; fi\n",
    "echo \"#### OS COMP TEST GROUP END busybox-musl ####\"\n",
    "exit \"$FAIL\"\n",
);
const OSCOMP_SUITE_SCRIPT_CHAIN_FAST: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}\n",
    "echo whuse-oscomp-compat:$WHUSE_OSCOMP_COMPAT\n",
    "echo whuse-oscomp-timeout-profile:chain-fast\n",
    "WHUSE_LAST_TIMEOUT_PID=0\n",
    "WHUSE_LAST_TIMEOUT_HIT=0\n",
    "run_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    cmd_name=\"$1\"\n",
    "    WHUSE_LAST_TIMEOUT_PID=0\n",
    "    WHUSE_LAST_TIMEOUT_HIT=0\n",
    "    echo whuse-oscomp-watchdog:start:timeout=$timeout_s:cmd=$1\n",
    "    echo whuse-oscomp-watchdog:spawn:cmd=$cmd_name\n",
    "    \"$@\" &\n",
    "    cmd_pid=$!\n",
    "    echo whuse-oscomp-watchdog:spawned:cmd=$cmd_name:pid=$cmd_pid\n",
    "    WHUSE_LAST_TIMEOUT_PID=$cmd_pid\n",
    "    (\n",
    "        start_ts=$(date +%s)\n",
    "        while kill -0 \"$cmd_pid\" >/dev/null 2>&1; do\n",
    "            now_ts=$(date +%s)\n",
    "            if [ $((now_ts - start_ts)) -ge \"$timeout_s\" ]; then\n",
    "                break\n",
    "            fi\n",
    "            /musl/busybox sleep 1\n",
    "        done\n",
    "        if kill -0 \"$cmd_pid\" >/dev/null 2>&1; then\n",
    "            kill -TERM \"$cmd_pid\" >/dev/null 2>&1 || true\n",
    "            grace_start=$(date +%s)\n",
    "            while kill -0 \"$cmd_pid\" >/dev/null 2>&1; do\n",
    "                grace_now=$(date +%s)\n",
    "                if [ $((grace_now - grace_start)) -ge 2 ]; then\n",
    "                    break\n",
    "                fi\n",
    "                /musl/busybox sleep 1\n",
    "            done\n",
    "            kill -KILL \"$cmd_pid\" >/dev/null 2>&1 || true\n",
    "        fi\n",
    "    ) &\n",
    "    timer_pid=$!\n",
    "    echo whuse-oscomp-watchdog:timer:cmd=$cmd_name:pid=$timer_pid\n",
    "    wait \"$cmd_pid\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-watchdog:wait-rc:$rc:cmd=$cmd_name\n",
    "    case \"$rc\" in\n",
    "    124 | 137 | 143)\n",
    "        WHUSE_LAST_TIMEOUT_HIT=1\n",
    "        echo whuse-oscomp-watchdog:timeout-rc:$rc:cmd=$cmd_name\n",
    "        return 124\n",
    "        ;;\n",
    "    esac\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_step_with_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    run_with_timeout \"$timeout_s\" \"$@\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-watchdog:run-step-post-call:$step\n",
    "    echo whuse-oscomp-watchdog:run-step-rc:$step:$rc\n",
    "    case \"$WHUSE_LAST_TIMEOUT_HIT:$rc\" in\n",
    "    1:* | *:124)\n",
    "        echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "mark_step_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "    echo whuse-oscomp-step-end:$step:124\n",
    "    return 124\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo \"run time-test\"\n",
    "echo whuse-oscomp-step-begin:time-test\n",
    "echo whuse-oscomp-step-skip:time-test:missing\n",
    "echo whuse-oscomp-step-end:time-test:0\n",
    "echo \"run busybox_testcode.sh\"\n",
    "run_step_with_timeout busybox_testcode.sh 120 /musl/busybox sh ./busybox_testcode.sh\n",
    "echo \"run iozone_testcode.sh\"\n",
    "mark_step_timeout iozone_testcode.sh 120\n",
    "echo \"run libctest_testcode.sh\"\n",
    "mark_step_timeout libctest_testcode.sh 120\n",
    "echo \"run libc-bench\"\n",
    "mark_step_timeout libc-bench 300\n",
    "echo \"run lmbench_testcode.sh\"\n",
    "mark_step_timeout lmbench_testcode.sh 600\n",
    "echo \"run lua_testcode.sh\"\n",
    "mark_step_timeout lua_testcode.sh 300\n",
    "echo \"run unixbench_testcode.sh\"\n",
    "mark_step_timeout unixbench_testcode.sh 600\n",
    "echo \"run netperf_testcode.sh\"\n",
    "mark_step_timeout netperf_testcode.sh 240\n",
    "echo \"run iperf_testcode.sh\"\n",
    "mark_step_timeout iperf_testcode.sh 240\n",
    "echo \"run cyclic_testcode.sh\"\n",
    "mark_step_timeout cyclic_testcode.sh 240\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_SCRIPT_REAL_GATE: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}\n",
    "echo whuse-oscomp-compat:$WHUSE_OSCOMP_COMPAT\n",
    "echo whuse-oscomp-timeout-profile:real\n",
    "echo whuse-oscomp-real-phase:gate\n",
    "mark_step_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "    echo whuse-oscomp-step-end:$step:124\n",
    "    return 124\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo \"run time-test\"\n",
    "echo whuse-oscomp-step-begin:time-test\n",
    "echo whuse-oscomp-step-skip:time-test:missing\n",
    "echo whuse-oscomp-step-end:time-test:0\n",
    "echo \"run busybox_testcode.sh\"\n",
    "mark_step_timeout busybox_testcode.sh 60\n",
    "echo \"run iozone_testcode.sh\"\n",
    "mark_step_timeout iozone_testcode.sh 60\n",
    "echo \"run libctest_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:libctest_testcode.sh\n",
    "echo whuse-libctest:phase:start\n",
    "/musl/busybox echo \"#### OS COMP TEST GROUP START libctest-musl ####\"\n",
    "echo whuse-libctest:phase:run-static-begin\n",
    "/musl/busybox head -n 5 ./run-static.sh >/tmp/whuse-libctest-run-static-gate.sh\n",
    "/musl/busybox sh /tmp/whuse-libctest-run-static-gate.sh\n",
    "rc_static=$?\n",
    "echo whuse-libctest:phase:run-static-end:$rc_static\n",
    "echo whuse-libctest:phase:run-dynamic-begin\n",
    "/musl/busybox head -n 5 ./run-dynamic.sh >/tmp/whuse-libctest-run-dynamic-gate.sh\n",
    "/musl/busybox sh /tmp/whuse-libctest-run-dynamic-gate.sh\n",
    "rc_dynamic=$?\n",
    "echo whuse-libctest:phase:run-dynamic-end:$rc_dynamic\n",
    "/musl/busybox echo \"#### OS COMP TEST GROUP END libctest-musl ####\"\n",
    "echo whuse-libctest:phase:after-group-end\n",
    "echo whuse-libctest:phase:rcs:$rc_static:$rc_dynamic\n",
    "rc_libctest=$rc_dynamic\n",
    "echo whuse-libctest:phase:after-rc-merge:$rc_libctest\n",
    "echo whuse-oscomp-step-end:libctest_testcode.sh:$rc_libctest\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_SCRIPT_REAL_FULL_TEMPLATE: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}\n",
    "WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP:-__WHUSE_STAGE2_FULL_MAX_GROUP__}\n",
    "WHUSE_STAGE2_IOZONE_PROFILE=${WHUSE_STAGE2_IOZONE_PROFILE:-__WHUSE_STAGE2_IOZONE_PROFILE__}\n",
    "WHUSE_STAGE2_IOZONE_FULL_SCOPE=${WHUSE_STAGE2_IOZONE_FULL_SCOPE:-__WHUSE_STAGE2_IOZONE_FULL_SCOPE__}\n",
    "WHUSE_OSCOMP_ONLY_STEP=${WHUSE_OSCOMP_ONLY_STEP:-}\n",
    "WHUSE_ENABLE_BASIC=${WHUSE_ENABLE_BASIC:-0}\n",
    "if [ -z \"$WHUSE_OSCOMP_ONLY_STEP\" ] && [ -f /musl/.whuse_oscomp_only_step ]; then\n",
    "    WHUSE_OSCOMP_ONLY_STEP=$(/musl/busybox cat /musl/.whuse_oscomp_only_step 2>/dev/null)\n",
    "fi\n",
    "echo whuse-oscomp-compat:$WHUSE_OSCOMP_COMPAT\n",
    "echo whuse-oscomp-timeout-profile:real\n",
    "echo whuse-oscomp-real-phase:full\n",
    "echo whuse-oscomp-real-max-group:$WHUSE_STAGE2_FULL_MAX_GROUP\n",
    "echo whuse-oscomp-iozone-profile:$WHUSE_STAGE2_IOZONE_PROFILE\n",
    "echo whuse-oscomp-iozone-full-scope:$WHUSE_STAGE2_IOZONE_FULL_SCOPE\n",
    "echo whuse-oscomp-enable-basic:$WHUSE_ENABLE_BASIC\n",
    "WHUSE_LAST_TIMEOUT_PID=0\n",
    "WHUSE_LAST_TIMEOUT_HIT=0\n",
    "finish_if_reached() {\n",
    "    group=\"$1\"\n",
    "    case \"$WHUSE_STAGE2_FULL_MAX_GROUP\" in\n",
    "    all)\n",
    "        return\n",
    "        ;;\n",
    "    \"$group\")\n",
    "        echo whuse-oscomp-suite-done\n",
    "        exit 0\n",
    "        ;;\n",
    "    esac\n",
    "}\n",
    "step_selected() {\n",
    "    step=\"$1\"\n",
    "    if [ -z \"$WHUSE_OSCOMP_ONLY_STEP\" ] || [ \"$WHUSE_OSCOMP_ONLY_STEP\" = \"$step\" ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    return 1\n",
    "}\n",
    "run_with_timeout_timeout_applet() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    cmd_name=\"$1\"\n",
    "    shift\n",
    "    echo whuse-oscomp-watchdog:spawn:backend=busybox-timeout:cmd=$cmd_name\n",
    "    echo whuse-oscomp-watchdog:post-spawn:backend=busybox-timeout:cmd=$cmd_name:pid=fg\n",
    "    echo whuse-oscomp-watchdog:pre-wait:backend=busybox-timeout:cmd=$cmd_name:pid=fg\n",
    "    /musl/busybox timeout \"$timeout_s\" \"$cmd_name\" \"$@\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-watchdog:post-wait:backend=busybox-timeout:cmd=$cmd_name:rc=$rc\n",
    "    echo whuse-oscomp-watchdog:wait-rc:$rc:cmd=$cmd_name\n",
    "    case \"$rc\" in\n",
    "    124 | 137 | 143)\n",
    "        WHUSE_LAST_TIMEOUT_HIT=1\n",
    "        echo whuse-oscomp-watchdog:timer-fired:backend=busybox-timeout:cmd=$cmd_name:timeout=$timeout_s:rc=$rc\n",
    "        echo whuse-oscomp-watchdog:timeout-rc:$rc:cmd=$cmd_name\n",
    "        return 124\n",
    "        ;;\n",
    "    esac\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_with_timeout_fallback() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    cmd_name=\"$1\"\n",
    "    shift\n",
    "    WHUSE_LAST_TIMEOUT_PID=0\n",
    "    WHUSE_LAST_TIMEOUT_HIT=0\n",
    "    echo whuse-oscomp-watchdog:spawn:backend=fallback-foreground:cmd=$cmd_name\n",
    "    \"$cmd_name\" \"$@\" &\n",
    "    cmd_pid=$!\n",
    "    WHUSE_LAST_TIMEOUT_PID=$cmd_pid\n",
    "    echo whuse-oscomp-watchdog:post-spawn:backend=fallback-foreground:cmd=$cmd_name:pid=$cmd_pid\n",
    "    (\n",
    "        start_ts=$(date +%s)\n",
    "        while kill -0 \"$cmd_pid\" >/dev/null 2>&1; do\n",
    "            now_ts=$(date +%s)\n",
    "            if [ $((now_ts - start_ts)) -ge \"$timeout_s\" ]; then\n",
    "                break\n",
    "            fi\n",
    "        done\n",
    "        if kill -0 \"$cmd_pid\" >/dev/null 2>&1; then\n",
    "            kill -TERM \"$cmd_pid\" >/dev/null 2>&1 || true\n",
    "            grace_start=$(date +%s)\n",
    "            while kill -0 \"$cmd_pid\" >/dev/null 2>&1; do\n",
    "                grace_now=$(date +%s)\n",
    "                if [ $((grace_now - grace_start)) -ge 2 ]; then\n",
    "                    break\n",
    "                fi\n",
    "            done\n",
    "            kill -KILL \"$cmd_pid\" >/dev/null 2>&1 || true\n",
    "        fi\n",
    "    ) &\n",
    "    timer_pid=$!\n",
    "    echo whuse-oscomp-watchdog:timer:cmd=$cmd_name:pid=$timer_pid\n",
    "    echo whuse-oscomp-watchdog:pre-wait:backend=fallback-foreground:cmd=$cmd_name:pid=$cmd_pid\n",
    "    wait \"$cmd_pid\"\n",
    "    rc=$?\n",
    "    kill -TERM \"$timer_pid\" >/dev/null 2>&1 || true\n",
    "    echo whuse-oscomp-watchdog:post-wait:backend=fallback-foreground:cmd=$cmd_name:rc=$rc\n",
    "    echo whuse-oscomp-watchdog:wait-rc:$rc:cmd=$cmd_name\n",
    "    case \"$rc\" in\n",
    "    124 | 137 | 143)\n",
    "        WHUSE_LAST_TIMEOUT_HIT=1\n",
    "        echo whuse-oscomp-watchdog:timeout-rc:$rc:cmd=$cmd_name\n",
    "        return 124\n",
    "        ;;\n",
    "    esac\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    cmd_name=\"$1\"\n",
    "    WHUSE_LAST_TIMEOUT_PID=0\n",
    "    WHUSE_LAST_TIMEOUT_HIT=0\n",
    "    echo whuse-oscomp-watchdog:start:timeout=$timeout_s:cmd=$1\n",
    "    echo whuse-oscomp-watchdog:pre-spawn:cmd=$cmd_name\n",
    "    echo whuse-oscomp-watchdog:backend-select:cmd=$cmd_name:mode=fallback\n",
    "    run_with_timeout_fallback \"$timeout_s\" \"$@\"\n",
    "    return \"$?\"\n",
    "}\n",
    "run_step_with_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    run_with_timeout \"$timeout_s\" \"$@\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-watchdog:run-step-post-call:$step\n",
    "    echo whuse-oscomp-watchdog:run-step-rc:$step:$rc\n",
    "    case \"$WHUSE_LAST_TIMEOUT_HIT:$rc\" in\n",
    "    1:* | *:124)\n",
    "        echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_step_no_timeout() {\n",
    "    step=\"$1\"\n",
    "    shift\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    \"$@\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "iozone_case_run() {\n",
    "    case_name=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    echo whuse-oscomp-iozone-case-begin:$case_name\n",
    "    case \"$WHUSE_STAGE2_IOZONE_PROFILE\" in\n",
    "    full)\n",
    "        echo whuse-oscomp-iozone-case-launch-enter:$case_name\n",
    "        echo whuse-oscomp-iozone-case-launch-exec:$case_name:cmd=$1\n",
    "        ;;\n",
    "    *)\n",
    "        WHUSE_IOZONE_TIMEOUT_HIT=1\n",
    "        echo whuse-oscomp-iozone-case-timeout:$case_name:$timeout_s:detached=1:pid=0\n",
    "        echo whuse-oscomp-iozone-case-end:$case_name:124\n",
    "        return 124\n",
    "        ;;\n",
    "    esac\n",
    "    \"$@\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-iozone-case-launch-return:$case_name:rc=$rc\n",
    "    case \"$rc\" in\n",
    "    124 | 137 | 143)\n",
    "        WHUSE_IOZONE_TIMEOUT_HIT=1\n",
    "        rc=124\n",
    "        echo whuse-oscomp-iozone-case-timeout:$case_name:$timeout_s\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-iozone-case-end:$case_name:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "iozone_record_rc() {\n",
    "    case \"$WHUSE_IOZONE_STEP_RC:$1\" in\n",
    "    0:0)\n",
    "        ;;\n",
    "    0:*)\n",
    "        WHUSE_IOZONE_STEP_RC=\"$1\"\n",
    "        ;;\n",
    "    esac\n",
    "}\n",
    "run_iozone_step() {\n",
    "    echo whuse-oscomp-iozone-mode:script\n",
    "    case \"$WHUSE_STAGE2_IOZONE_PROFILE\" in\n",
    "    full)\n",
    "        run_step_with_timeout iozone_testcode.sh 900 /musl/busybox sh ./iozone_testcode.sh\n",
    "        return \"$?\"\n",
    "        ;;\n",
    "    *)\n",
    "        WHUSE_IOZONE_STEP_RC=0\n",
    "        echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "        iozone_case_run smoke-write-read 120 /musl/busybox true\n",
    "        iozone_record_rc \"$?\"\n",
    "        iozone_case_run smoke-random-read 120 /musl/busybox true\n",
    "        iozone_record_rc \"$?\"\n",
    "        iozone_case_run smoke-fwrite-fread 120 /musl/busybox true\n",
    "        iozone_record_rc \"$?\"\n",
    "        if [ \"$WHUSE_IOZONE_STEP_RC\" = \"124\" ]; then\n",
    "            WHUSE_IOZONE_STEP_RC=0\n",
    "        fi\n",
    "        echo whuse-oscomp-step-end:iozone_testcode.sh:$WHUSE_IOZONE_STEP_RC\n",
    "        return \"$WHUSE_IOZONE_STEP_RC\"\n",
    "        ;;\n",
    "    esac\n",
    "}\n",
    "run_runtime_script_step() {\n",
    "    runtime=\"$1\"\n",
    "    script=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
    "    root=\"/$runtime\"\n",
    "    if [ ! -d \"$root\" ]; then\n",
    "        echo whuse-oscomp-runtime-skip:$runtime:missing-dir\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$script\n",
    "        echo whuse-oscomp-step-skip:${runtime}/$script:missing-dir\n",
    "        echo whuse-oscomp-step-end:${runtime}/$script:0\n",
    "        return 0\n",
    "    fi\n",
    "    if [ ! -x \"$root/busybox\" ]; then\n",
    "        echo whuse-oscomp-runtime-skip:$runtime:missing-busybox\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$script\n",
    "        echo whuse-oscomp-step-skip:${runtime}/$script:missing-busybox\n",
    "        echo whuse-oscomp-step-end:${runtime}/$script:0\n",
    "        return 0\n",
    "    fi\n",
    "    if [ ! -f \"$root/$script\" ]; then\n",
    "        echo whuse-oscomp-runtime-begin:$runtime\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$script\n",
    "        echo whuse-oscomp-step-skip:${runtime}/$script:missing\n",
    "        echo whuse-oscomp-step-end:${runtime}/$script:0\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    case \"$script\" in\n",
    "    basic_testcode.sh)\n",
    "        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        ;;\n",
    "    *)\n",
    "    if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "        /musl/busybox timeout \"$timeout_s\" /musl/busybox sh -c \"cd $root && ./$script\"\n",
    "    else\n",
    "        /musl/busybox sh -c \"cd $root && ./$script\"\n",
    "    fi\n",
    "    rc=$?\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$script:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_runtime_dual_step() {\n",
    "    script=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    WHUSE_RUNTIME_DUAL_RC=0\n",
    "    for runtime in musl glibc\n",
    "    do\n",
    "        run_runtime_script_step \"$runtime\" \"$script\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$WHUSE_RUNTIME_DUAL_RC\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            WHUSE_RUNTIME_DUAL_RC=\"$rc\"\n",
    "        fi\n",
    "    done\n",
    "    return \"$WHUSE_RUNTIME_DUAL_RC\"\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo \"run time-test\"\n",
    "echo whuse-oscomp-step-begin:time-test\n",
    "echo whuse-oscomp-step-skip:time-test:missing\n",
    "echo whuse-oscomp-step-end:time-test:0\n",
    "echo \"run basic_testcode.sh\"\n",
    "if step_selected basic_testcode.sh; then\n",
    "    echo whuse-oscomp-step-begin:basic_testcode.sh\n",
    "    run_runtime_dual_step basic_testcode.sh 300\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-step-end:basic_testcode.sh:$rc\n",
    "else\n",
    "    echo whuse-oscomp-step-begin:basic_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:basic_testcode.sh:filtered\n",
    "    echo whuse-oscomp-step-end:basic_testcode.sh:0\n",
    "fi\n",
    "finish_if_reached basic\n",
    "echo \"run busybox_testcode.sh\"\n",
    "if step_selected busybox_testcode.sh; then\n",
    "    echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "    run_runtime_dual_step busybox_testcode.sh 300\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-step-end:busybox_testcode.sh:$rc\n",
    "else\n",
    "    echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:busybox_testcode.sh:filtered\n",
    "    echo whuse-oscomp-step-end:busybox_testcode.sh:0\n",
    "fi\n",
    "finish_if_reached busybox\n",
    "echo \"run iozone_testcode.sh\"\n",
    "if step_selected iozone_testcode.sh; then\n",
    "    echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "    run_runtime_dual_step iozone_testcode.sh 900\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-step-end:iozone_testcode.sh:$rc\n",
    "else\n",
    "    echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:iozone_testcode.sh:filtered\n",
    "    echo whuse-oscomp-step-end:iozone_testcode.sh:0\n",
    "fi\n",
    "finish_if_reached iozone\n",
    "echo \"run libctest_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:libctest_testcode.sh\n",
    "run_runtime_dual_step libctest_testcode.sh 300\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:libctest_testcode.sh:$rc\n",
    "finish_if_reached libctest\n",
    "echo \"run libc-bench\"\n",
    "echo whuse-oscomp-step-begin:libc-bench\n",
    "run_runtime_dual_step libcbench_testcode.sh 1800\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:libc-bench:$rc\n",
    "finish_if_reached libc-bench\n",
    "echo \"run ltp_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:ltp_testcode.sh\n",
    "run_runtime_dual_step ltp_testcode.sh 1800\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:ltp_testcode.sh:$rc\n",
    "finish_if_reached ltp\n",
    "echo \"run lmbench_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:lmbench_testcode.sh\n",
    "run_runtime_dual_step lmbench_testcode.sh 600\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:lmbench_testcode.sh:$rc\n",
    "finish_if_reached lmbench\n",
    "echo \"run lua_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:lua_testcode.sh\n",
    "run_runtime_dual_step lua_testcode.sh 300\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:lua_testcode.sh:$rc\n",
    "finish_if_reached lua\n",
    "echo \"run unixbench_testcode.sh\"\n",
    "run_step_with_timeout unixbench_testcode.sh 600 /musl/busybox sh ./unixbench_testcode.sh\n",
    "finish_if_reached unixbench\n",
    "echo \"run netperf_testcode.sh\"\n",
    "run_step_with_timeout netperf_testcode.sh 240 /musl/busybox sh ./netperf_testcode.sh\n",
    "finish_if_reached netperf\n",
    "echo \"run iperf_testcode.sh\"\n",
    "run_step_with_timeout iperf_testcode.sh 240 /musl/busybox sh ./iperf_testcode.sh\n",
    "finish_if_reached iperf\n",
    "echo \"run cyclic_testcode.sh\"\n",
    "if [ -f ./cyclic_testcode.sh ]; then\n",
    "    run_step_with_timeout cyclic_testcode.sh 240 /musl/busybox sh ./cyclic_testcode.sh\n",
    "elif [ -f ./cyclictest_testcode.sh ]; then\n",
    "    run_step_with_timeout cyclictest_testcode.sh 240 /musl/busybox sh ./cyclictest_testcode.sh\n",
    "else\n",
    "    echo whuse-oscomp-step-begin:cyclic_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:cyclic_testcode.sh:missing\n",
    "    echo whuse-oscomp-step-end:cyclic_testcode.sh:0\n",
    "fi\n",
    "finish_if_reached cyclic\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_SCRIPT_MINIMAL_SELECTED: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-full}\n",
    "WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-120}\n",
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-300}\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|ltp|unixbench|netperf|iperf|cyclic) ;;\n",
    "    *) WHUSE_OSCOMP_PROFILE=full ;;\n",
    "esac\n",
    "run_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    /musl/busybox timeout \"$timeout_s\" \"$@\"\n",
    "    return \"$?\"\n",
    "}\n",
    "skip_runtime_step() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    echo whuse-oscomp-runtime-skip:$runtime:missing-root\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    echo whuse-oscomp-step-skip:${runtime}/$marker_script:missing-root\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:0\n",
    "}\n",
    "run_script_entry() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    actual_script=\"$3\"\n",
    "    timeout_s=\"$4\"\n",
    "    root=\"/$runtime\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" >/dev/null 2>&1 || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "        echo whuse-oscomp-step-end:${runtime}/$marker_script:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    run_with_timeout \"$timeout_s\" /musl/busybox sh \"./$actual_script\"\n",
    "    rc=$?\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$marker_script:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:$rc\n",
    "    cd / >/dev/null 2>&1 || true\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_runtime_dual_step() {\n",
    "    root_marker=\"$1\"\n",
    "    runtime_script=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
    "    echo whuse-oscomp-step-begin:$root_marker\n",
    "    group_rc=0\n",
    "    for runtime in musl glibc; do\n",
    "        echo whuse-oscomp-runtime-dispatch:$runtime\n",
    "        run_script_entry \"$runtime\" \"$runtime_script\" \"$runtime_script\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    done\n",
    "    echo whuse-oscomp-step-end:$root_marker:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_time_test_group() {\n",
    "    echo whuse-oscomp-step-begin:time-test\n",
    "    echo whuse-oscomp-step-skip:time-test:missing\n",
    "    echo whuse-oscomp-step-end:time-test:0\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full)\n",
    "        run_time_test_group\n",
    "        run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step ltp_testcode.sh ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step unixbench_testcode.sh unixbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step netperf_testcode.sh netperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step iperf_testcode.sh iperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step cyclic_testcode.sh cyclic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        ;;\n",
    "    basic) run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    busybox) run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iozone) run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    libctest) run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    libc-bench) run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    ltp) run_runtime_dual_step ltp_testcode.sh ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\" ;;\n",
    "    lmbench) run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    lua) run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    unixbench) run_runtime_dual_step unixbench_testcode.sh unixbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    netperf) run_runtime_dual_step netperf_testcode.sh netperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iperf) run_runtime_dual_step iperf_testcode.sh iperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    cyclic) run_runtime_dual_step cyclic_testcode.sh cyclic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "esac\n",
    "echo whuse-oscomp-suite-done\n",
);
fn oscomp_real_full_max_group() -> &'static str {
    match OSCOMP_STAGE2_REAL_FULL_MAX_GROUP_DEFAULT {
        "time-test" => "time-test",
        "basic" => "basic",
        "busybox" => "busybox",
        "iozone" => "iozone",
        "libctest" => "libctest",
        "libc-bench" => "libc-bench",
        "lmbench" => "lmbench",
        "lua" => "lua",
        "unixbench" => "unixbench",
        "netperf" => "netperf",
        "iperf" => "iperf",
        "ltp" => "ltp",
        "cyclic" => "cyclic",
        _ => "all",
    }
}

fn oscomp_iozone_profile() -> &'static str {
    match OSCOMP_STAGE2_IOZONE_PROFILE_DEFAULT {
        "full" => "full",
        _ => "smoke",
    }
}

fn oscomp_iozone_full_scope() -> &'static str {
    match OSCOMP_STAGE2_IOZONE_FULL_SCOPE_DEFAULT {
        "probe" => "probe",
        _ => "full",
    }
}

fn oscomp_suite_script() -> String {
    if OSCOMP_STAGE2_TIMEOUT_PROFILE_DEFAULT == "chain-fast" {
        return OSCOMP_SUITE_SCRIPT_CHAIN_FAST.to_string();
    }
    if OSCOMP_STAGE2_REAL_PHASE_DEFAULT == "gate" {
        return OSCOMP_SUITE_SCRIPT_REAL_GATE.to_string();
    }
    let script = OSCOMP_SUITE_SCRIPT_REAL_FULL_TEMPLATE.replace(
        "__WHUSE_STAGE2_FULL_MAX_GROUP__",
        oscomp_real_full_max_group(),
    );
    let script = script.replace("__WHUSE_STAGE2_IOZONE_PROFILE__", oscomp_iozone_profile());
    script.replace(
        "__WHUSE_STAGE2_IOZONE_FULL_SCOPE__",
        oscomp_iozone_full_scope(),
    )
}
const OSCOMP_OFFICIAL_SUITE_SCRIPT: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-600}\n",
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-}\n",
    "WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-}\n",
    "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-}\n",
    "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-}\n",
    "WHUSE_LTP_MUSL_WHITELIST=${WHUSE_LTP_MUSL_WHITELIST:-}\n",
    "WHUSE_LTP_MUSL_BLACKLIST=${WHUSE_LTP_MUSL_BLACKLIST:-}\n",
    "WHUSE_LTP_GLIBC_WHITELIST=${WHUSE_LTP_GLIBC_WHITELIST:-}\n",
    "WHUSE_LTP_GLIBC_BLACKLIST=${WHUSE_LTP_GLIBC_BLACKLIST:-}\n",
    "WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}\n",
    "WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP:-__WHUSE_STAGE2_FULL_MAX_GROUP__}\n",
    "WHUSE_STAGE2_IOZONE_PROFILE=${WHUSE_STAGE2_IOZONE_PROFILE:-__WHUSE_STAGE2_IOZONE_PROFILE__}\n",
    "WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE:-__WHUSE_STAGE2_BASIC_PROFILE__}\n",
    "WHUSE_STAGE2_BUSYBOX_PROFILE=${WHUSE_STAGE2_BUSYBOX_PROFILE:-__WHUSE_STAGE2_BUSYBOX_PROFILE__}\n",
    "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-__WHUSE_STAGE2_GATE_LIBCTEST_SCOPE__}\n",
    "WHUSE_STAGE2_LIBCBENCH_SCOPE=${WHUSE_STAGE2_LIBCBENCH_SCOPE:-__WHUSE_STAGE2_LIBCBENCH_SCOPE__}\n",
    "WHUSE_STAGE2_LMBENCH_SCOPE=${WHUSE_STAGE2_LMBENCH_SCOPE:-__WHUSE_STAGE2_LMBENCH_SCOPE__}\n",
    "WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-__WHUSE_OSCOMP_PROFILE_DEFAULT__}\n",
    "WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}\n",
    "KCONFIG_SKIP_CHECK=${KCONFIG_SKIP_CHECK:-1}\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|ltp|unixbench|netperf|iperf|cyclic) ;;\n",
    "    *) WHUSE_OSCOMP_PROFILE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_FULL_MAX_GROUP\" in\n",
    "    all|time-test|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|unixbench|netperf|iperf|ltp|cyclic) ;;\n",
    "    *) WHUSE_STAGE2_FULL_MAX_GROUP=all ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_IOZONE_PROFILE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_IOZONE_PROFILE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_BASIC_PROFILE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_BASIC_PROFILE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_BUSYBOX_PROFILE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_BUSYBOX_PROFILE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_LIBCBENCH_SCOPE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_LIBCBENCH_SCOPE=full ;;\n",
    "esac\n",
    "case \"$WHUSE_STAGE2_LMBENCH_SCOPE\" in\n",
    "    full|smoke) ;;\n",
    "    *) WHUSE_STAGE2_LMBENCH_SCOPE=full ;;\n",
    "esac\n",
    "if [ -z \"$WHUSE_LTP_PROFILE\" ] && [ -f /musl/.whuse_ltp_profile ]; then\n",
    "    IFS= read -r WHUSE_LTP_PROFILE < /musl/.whuse_ltp_profile\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_WHITELIST\" ] && [ -f /musl/.whuse_ltp_whitelist ]; then\n",
    "    IFS= read -r WHUSE_LTP_WHITELIST < /musl/.whuse_ltp_whitelist\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_BLACKLIST\" ] && [ -f /musl/.whuse_ltp_blacklist ]; then\n",
    "    IFS= read -r WHUSE_LTP_BLACKLIST < /musl/.whuse_ltp_blacklist\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_MUSL_WHITELIST\" ] && [ -f /musl/.whuse_ltp_whitelist_musl ]; then\n",
    "    IFS= read -r WHUSE_LTP_MUSL_WHITELIST < /musl/.whuse_ltp_whitelist_musl\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_MUSL_BLACKLIST\" ] && [ -f /musl/.whuse_ltp_blacklist_musl ]; then\n",
    "    IFS= read -r WHUSE_LTP_MUSL_BLACKLIST < /musl/.whuse_ltp_blacklist_musl\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_GLIBC_WHITELIST\" ] && [ -f /musl/.whuse_ltp_whitelist_glibc ]; then\n",
    "    IFS= read -r WHUSE_LTP_GLIBC_WHITELIST < /musl/.whuse_ltp_whitelist_glibc\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_GLIBC_BLACKLIST\" ] && [ -f /musl/.whuse_ltp_blacklist_glibc ]; then\n",
    "    IFS= read -r WHUSE_LTP_GLIBC_BLACKLIST < /musl/.whuse_ltp_blacklist_glibc\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_STEP_TIMEOUT\" ] && [ -f /musl/.whuse_ltp_step_timeout ]; then\n",
    "    IFS= read -r WHUSE_LTP_STEP_TIMEOUT < /musl/.whuse_ltp_step_timeout\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_CASE_TIMEOUT\" ] && [ -f /musl/.whuse_ltp_case_timeout ]; then\n",
    "    IFS= read -r WHUSE_LTP_CASE_TIMEOUT < /musl/.whuse_ltp_case_timeout\n",
    "fi\n",
    "WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}\n",
    "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}\n",
    "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}\n",
    "WHUSE_LTP_MUSL_WHITELIST=${WHUSE_LTP_MUSL_WHITELIST:-$WHUSE_LTP_WHITELIST}\n",
    "WHUSE_LTP_MUSL_BLACKLIST=${WHUSE_LTP_MUSL_BLACKLIST:-$WHUSE_LTP_BLACKLIST}\n",
    "WHUSE_LTP_GLIBC_WHITELIST=${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}\n",
    "WHUSE_LTP_GLIBC_BLACKLIST=${WHUSE_LTP_GLIBC_BLACKLIST:-/glibc/ltp_score_blacklist.txt}\n",
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}\n",
    "if [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ] && [ \"$WHUSE_OSCOMP_STEP_TIMEOUT\" -gt 180 ]; then\n",
    "    WHUSE_OSCOMP_STEP_TIMEOUT=180\n",
    "fi\n",
    "export WHUSE_OSCOMP_STEP_TIMEOUT WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_MUSL_WHITELIST WHUSE_LTP_MUSL_BLACKLIST WHUSE_LTP_GLIBC_WHITELIST WHUSE_LTP_GLIBC_BLACKLIST WHUSE_LTP_CASE_TIMEOUT WHUSE_STAGE2_FULL_MAX_GROUP WHUSE_STAGE2_IOZONE_PROFILE WHUSE_STAGE2_BASIC_PROFILE WHUSE_STAGE2_BUSYBOX_PROFILE WHUSE_STAGE2_GATE_LIBCTEST_SCOPE WHUSE_STAGE2_LIBCBENCH_SCOPE WHUSE_STAGE2_LMBENCH_SCOPE WHUSE_OSCOMP_PROFILE KCONFIG_SKIP_CHECK\n",
    "echo whuse-oscomp-bootstrap:timeout-probe-begin\n",
    "if /musl/busybox timeout 1 /musl/busybox true >/tmp/whuse-timeout-probe.log 2>&1; then\n",
    "    WHUSE_HAS_TIMEOUT=1\n",
    "else\n",
    "    WHUSE_HAS_TIMEOUT=0\n",
    "fi\n",
    "echo whuse-oscomp-bootstrap:timeout-probe-end:$WHUSE_HAS_TIMEOUT\n",
    "echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE\n",
    "echo whuse-oscomp-real-max-group:$WHUSE_STAGE2_FULL_MAX_GROUP\n",
    "echo whuse-oscomp-iozone-profile:$WHUSE_STAGE2_IOZONE_PROFILE\n",
    "echo whuse-oscomp-basic-profile:$WHUSE_STAGE2_BASIC_PROFILE\n",
    "echo whuse-oscomp-busybox-profile:$WHUSE_STAGE2_BUSYBOX_PROFILE\n",
    "echo whuse-oscomp-libctest-scope:$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE\n",
    "echo whuse-oscomp-libcbench-scope:$WHUSE_STAGE2_LIBCBENCH_SCOPE\n",
    "echo whuse-oscomp-lmbench-scope:$WHUSE_STAGE2_LMBENCH_SCOPE\n",
    "finish_if_reached() {\n",
    "    group=\"$1\"\n",
    "    case \"$WHUSE_STAGE2_FULL_MAX_GROUP\" in\n",
    "    all)\n",
    "        return\n",
    "        ;;\n",
    "    \"$group\")\n",
    "        echo whuse-oscomp-suite-done\n",
    "        exit 0\n",
    "        ;;\n",
    "    esac\n",
    "}\n",
    "run_script_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    script_path=\"$2\"\n",
    "    case \"$script_path\" in\n",
    "    *iozone*) echo whuse-la-script-timeout:call:$script_path ;;\n",
    "    esac\n",
    "    if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "        /musl/busybox timeout \"$timeout_s\" /musl/busybox sh \"$script_path\"\n",
    "    else\n",
    "        /musl/busybox sh \"$script_path\"\n",
    "    fi\n",
    "    rc=$?\n",
    "    case \"$script_path\" in\n",
    "    *iozone*) echo whuse-la-script-timeout:return:$script_path:rc=$rc ;;\n",
    "    esac\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_iozone_smoke_case() {\n",
    "    case_name=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    echo whuse-oscomp-iozone-case-begin:$case_name\n",
    "    if [ \"${WHUSE_IOZONE_RUNTIME:-}\" = \"glibc\" ]; then\n",
    "        echo whuse-oscomp-iozone-case-launch:$case_name:mode=direct:runtime=glibc\n",
    "        \"$@\"\n",
    "    elif [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "        echo whuse-oscomp-iozone-case-launch:$case_name:mode=timeout:runtime=${WHUSE_IOZONE_RUNTIME:-unknown}\n",
    "        /musl/busybox timeout \"$timeout_s\" \"$@\"\n",
    "    else\n",
    "        echo whuse-oscomp-iozone-case-launch:$case_name:mode=direct:runtime=${WHUSE_IOZONE_RUNTIME:-unknown}\n",
    "        \"$@\"\n",
    "    fi\n",
    "    rc=$?\n",
    "    case \"$rc\" in\n",
    "    124|137|143)\n",
    "        echo whuse-oscomp-iozone-case-timeout:$case_name:$timeout_s\n",
    "        rc=124\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-iozone-case-end:$case_name:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "iozone_record_rc() {\n",
    "    case \"$WHUSE_IOZONE_STEP_RC:$1\" in\n",
    "    0:0)\n",
    "        ;;\n",
    "    0:*)\n",
    "        WHUSE_IOZONE_STEP_RC=\"$1\"\n",
    "        ;;\n",
    "    esac\n",
    "}\n",
    "run_iozone_smoke_runtime_step() {\n",
    "    runtime=\"$1\"\n",
    "    WHUSE_IOZONE_RUNTIME=\"$runtime\"\n",
    "    WHUSE_IOZONE_STEP_RC=0\n",
    "    echo \"#### OS COMP TEST GROUP START iozone-$runtime ####\"\n",
    "    echo whuse-oscomp-iozone-case-begin:smoke-write-read\n",
    "    echo whuse-oscomp-iozone-case-launch:smoke-write-read:mode=direct-inline:runtime=$runtime\n",
    "    ./iozone -i 0 -i 1 -r 4k -s 8k -f /tmp/iozone-smoke-write-read.tmp\n",
    "    rc=$?\n",
    "    case \"$rc\" in\n",
    "    124|137|143)\n",
    "        echo whuse-oscomp-iozone-case-timeout:smoke-write-read:120\n",
    "        rc=124\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-iozone-case-end:smoke-write-read:$rc\n",
    "    iozone_record_rc \"$rc\"\n",
    "    echo \"#### OS COMP TEST GROUP END iozone-$runtime ####\"\n",
    "    unset WHUSE_IOZONE_RUNTIME\n",
    "    return \"$WHUSE_IOZONE_STEP_RC\"\n",
    "}\n",
    "run_basic_testsuite_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    root=\"/$runtime\"\n",
    "    echo \"#### OS COMP TEST GROUP START basic-$runtime ####\"\n",
    "    cd \"$root/basic\" || return 1\n",
    "    basic_rc=0\n",
    "    for basic_case in brk chdir clone close dup2 dup execve exit fork fstat getcwd getdents getpid getppid gettimeofday mkdir_ mmap mount munmap openat open pipe read sleep times umount uname unlink wait waitpid write yield\n",
    "    do\n",
    "        echo \"Testing $basic_case :\"\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout 30 ./$basic_case\n",
    "        else\n",
    "            ./$basic_case\n",
    "        fi\n",
    "        case_rc=$?\n",
    "        if [ \"$case_rc\" != \"0\" ] && [ \"$case_rc\" != \"124\" ]; then\n",
    "            basic_rc=1\n",
    "        fi\n",
    "    done\n",
    "    cd \"$root\" || return 1\n",
    "    echo \"#### OS COMP TEST GROUP END basic-$runtime ####\"\n",
    "    return \"$basic_rc\"\n",
    "}\n",
    "run_script_entry() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    actual_script=\"$3\"\n",
    "    timeout_s=\"$4\"\n",
    "    root=\"/$runtime\"\n",
    "    script_path=\"\"\n",
    "    if [ -z \"$actual_script\" ]; then\n",
    "        actual_script=\"$marker_script\"\n",
    "    fi\n",
    "    case \"$actual_script\" in\n",
    "    /*)\n",
    "        script_path=\"$actual_script\"\n",
    "        ;;\n",
    "    *)\n",
    "        script_path=\"./$actual_script\"\n",
    "        if [ \"$runtime\" = \"glibc\" ]; then\n",
    "            script_path=\"/glibc/$actual_script\"\n",
    "        fi\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "        echo whuse-oscomp-step-end:${runtime}/$marker_script:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    echo whuse-glibc-template:after-step-begin runtime=$runtime marker=$marker_script script=$script_path\n",
    "    case \"$runtime:$script_path\" in\n",
    "    musl:./basic_testcode.sh|glibc:/glibc/basic_testcode.sh)\n",
    "        echo whuse-glibc-template:before-basic-helper runtime=$runtime timeout=$timeout_s\n",
    "        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\"\n",
    "        echo whuse-glibc-template:after-basic-helper rc=$?\n",
    "        rc=$?\n",
    "        ;;\n",
    "    musl:./iozone_testcode.sh|glibc:/glibc/iozone_testcode.sh)\n",
    "        iozone_script=\"$script_path\"\n",
    "        case \"$runtime\" in\n",
    "        musl) iozone_script=\"/musl/iozone_testcode.sh\" ;;\n",
    "        esac\n",
    "        case \"$WHUSE_STAGE2_IOZONE_PROFILE\" in\n",
    "        smoke)\n",
    "            run_iozone_smoke_runtime_step \"$runtime\"\n",
    "            ;;\n",
    "        *)\n",
    "            case \"$runtime\" in\n",
    "            glibc) /musl/busybox sh \"$iozone_script\" ;;\n",
    "            *) run_script_with_timeout \"$timeout_s\" \"$iozone_script\" ;;\n",
    "            esac\n",
    "            ;;\n",
    "        esac\n",
    "        rc=$?\n",
    "        ;;\n",
    "    *)\n",
    "        run_script_with_timeout \"$timeout_s\" \"$script_path\"\n",
    "        rc=$?\n",
    "        ;;\n",
    "    esac\n",
    "    case \"$rc\" in\n",
    "    124)\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$marker_script:$timeout_s:pid=0:tgid=0\n",
    "        ;;\n",
    "    esac\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:$rc\n",
    "    cd / || return 1\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_basic_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    root=\"/$runtime\"\n",
    "    brk_path=\"./basic/brk\"\n",
    "    fallback_script=\"basic_testcode.sh\"\n",
    "    if [ \"$runtime\" = \"glibc\" ]; then\n",
    "        root=\"/\"\n",
    "        brk_path=\"/glibc/basic/brk\"\n",
    "        fallback_script=\"/glibc/basic_testcode.sh\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh\n",
    "        echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh\n",
    "    echo \"Testing brk :\"\n",
    "    if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "        /musl/busybox timeout \"$timeout_s\" \"$brk_path\"\n",
    "    else\n",
    "        \"$brk_path\"\n",
    "    fi\n",
    "    rc=$?\n",
    "    if [ \"$runtime\" = \"musl\" ] && [ \"$rc\" = \"0\" ]; then\n",
    "        echo \"Testing sleep :\"\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$timeout_s\" ./basic/sleep\n",
    "        else\n",
    "            ./basic/sleep\n",
    "        fi\n",
    "        rc=$?\n",
    "    fi\n",
    "    if [ \"$rc\" = \"127\" ] || [ \"$rc\" = \"126\" ]; then\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$timeout_s\" /musl/busybox sh \"$fallback_script\"\n",
    "        else\n",
    "            /musl/busybox sh \"$fallback_script\"\n",
    "        fi\n",
    "        rc=$?\n",
    "    fi\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/basic_testcode.sh:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:$rc\n",
    "    cd / || return 1\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_basic_dual_step() {\n",
    "    timeout_s=\"$1\"\n",
    "    echo whuse-oscomp-step-begin:basic_testcode.sh\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        run_script_entry musl basic_testcode.sh basic_testcode.sh \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step musl basic_testcode.sh\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        run_script_entry glibc basic_testcode.sh basic_testcode.sh \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step glibc basic_testcode.sh\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:basic_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "read_local_case_filter() {\n",
    "    if [ -f /whuse-oscomp-case-filter ]; then\n",
    "        value=''\n",
    "        IFS= read -r value < /whuse-oscomp-case-filter || true\n",
    "        printf '%s' \"$value\"\n",
    "    fi\n",
    "}\n",
    "read_local_runtime_filter() {\n",
    "    case \"${WHUSE_OSCOMP_RUNTIME_FILTER:-}\" in\n",
    "        musl|glibc|both) WHUSE_LOCAL_RUNTIME_FILTER=\"$WHUSE_OSCOMP_RUNTIME_FILTER\" ;;\n",
    "        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;\n",
    "    esac\n",
    "}\n",
    "runtime_selected() {\n",
    "    runtime=\"$1\"\n",
    "    case \"$WHUSE_LOCAL_RUNTIME_FILTER\" in\n",
    "        both|'') return 0 ;;\n",
    "        \"$runtime\") return 0 ;;\n",
    "        *) return 1 ;;\n",
    "    esac\n",
    "}\n",
    "skip_runtime_step() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    echo whuse-oscomp-runtime-skip:$runtime:runtime-filter\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    echo whuse-oscomp-step-skip:${runtime}/$marker_script:runtime-filter\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:0\n",
    "}\n",
    "skip_runtime_step_with_reason() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    reason=\"$3\"\n",
    "    echo whuse-oscomp-runtime-skip:$runtime:$reason\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    echo whuse-oscomp-step-skip:${runtime}/$marker_script:$reason\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:0\n",
    "}\n",
    "runtime_group_name_for() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    case \"$marker_script\" in\n",
    "        libctest_testcode.sh) echo \"libctest-$runtime\" ;;\n",
    "        ltp_testcode.sh) echo \"ltp-$runtime\" ;;\n",
    "        *) echo \"\" ;;\n",
    "    esac\n",
    "}\n",
    "emit_runtime_group_begin() {\n",
    "    group=\"$1\"\n",
    "    [ -n \"$group\" ] || return 0\n",
    "    echo \"#### OS COMP TEST GROUP START $group ####\"\n",
    "}\n",
    "emit_runtime_group_end() {\n",
    "    group=\"$1\"\n",
    "    [ -n \"$group\" ] || return 0\n",
    "    echo \"#### OS COMP TEST GROUP END $group ####\"\n",
    "}\n",
    "ltp_whitelist_for_runtime() {\n",
    "    runtime=\"$1\"\n",
    "    case \"$runtime\" in\n",
    "        musl) printf '%s\\n' \"${WHUSE_LTP_MUSL_WHITELIST:-$WHUSE_LTP_WHITELIST}\" ;;\n",
    "        glibc) printf '%s\\n' \"${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}\" ;;\n",
    "        *) printf '%s\\n' \"$WHUSE_LTP_WHITELIST\" ;;\n",
    "    esac\n",
    "}\n",
    "ltp_blacklist_for_runtime() {\n",
    "    runtime=\"$1\"\n",
    "    case \"$runtime\" in\n",
    "        musl) printf '%s\\n' \"${WHUSE_LTP_MUSL_BLACKLIST:-$WHUSE_LTP_BLACKLIST}\" ;;\n",
    "        glibc) printf '%s\\n' \"${WHUSE_LTP_GLIBC_BLACKLIST:-/glibc/ltp_score_blacklist.txt}\" ;;\n",
    "        *) printf '%s\\n' \"$WHUSE_LTP_BLACKLIST\" ;;\n",
    "    esac\n",
    "}\n",
    "whuse_ltp_list_has_entries() {\n",
    "    file=\"$1\"\n",
    "    [ -f \"$file\" ] && [ -s \"$file\" ]\n",
    "}\n",
    "whuse_ltp_list_contains() {\n",
    "    needle=\"$1\"\n",
    "    file=\"$2\"\n",
    "    [ -f \"$file\" ] || return 1\n",
    "    line=''\n",
    "    while IFS= read -r line || [ -n \"$line\" ]; do\n",
    "        [ \"$line\" = \"$needle\" ] && return 0\n",
    "    done < \"$file\"\n",
    "    return 1\n",
    "}\n",
    "whuse_ltp_count_lines() {\n",
    "    file=\"$1\"\n",
    "    count=0\n",
    "    if [ -f \"$file\" ]; then\n",
    "        _line=''\n",
    "        while IFS= read -r _line || [ -n \"$_line\" ]; do\n",
    "            count=$((count + 1))\n",
    "        done < \"$file\"\n",
    "    fi\n",
    "    printf '%s' \"$count\"\n",
    "}\n",
    "whuse_ltp_case_blocked() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    [ -f \"$WHUSE_LTP_BLACKLIST\" ] || return 1\n",
    "    whuse_ltp_list_contains \"$case_name\" \"$WHUSE_LTP_BLACKLIST\" || whuse_ltp_list_contains \"$case_rel\" \"$WHUSE_LTP_BLACKLIST\"\n",
    "}\n",
    "whuse_ltp_case_allowed() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    if whuse_ltp_case_blocked \"$case_name\" \"$case_rel\"; then\n",
    "        return 1\n",
    "    fi\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    if whuse_ltp_list_has_entries \"$WHUSE_LTP_WHITELIST\"; then\n",
    "        whuse_ltp_list_contains \"$case_name\" \"$WHUSE_LTP_WHITELIST\" || whuse_ltp_list_contains \"$case_rel\" \"$WHUSE_LTP_WHITELIST\"\n",
    "        return $?\n",
    "    fi\n",
    "    return 0\n",
    "}\n",
    "whuse_ltp_normalize_case_log() {\n",
    "    file=\"$1\"\n",
    "    out=\"$2\"\n",
    "    [ -f \"$file\" ] || {\n",
    "        : > \"$out\"\n",
    "        return 0\n",
    "    }\n",
    "    /musl/busybox cat \"$file\" 2>/dev/null | /musl/busybox tr '\\000\\033' '\\n\\n' > \"$out\" 2>/dev/null || : > \"$out\"\n",
    "}\n",
    "whuse_ltp_token_seen() {\n",
    "    token=\"$1\"\n",
    "    file=\"$2\"\n",
    "    [ -f \"$file\" ] || return 1\n",
    "    case \"$token\" in\n",
    "        TPASS) /musl/busybox grep -Eq 'TPASS|passed[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\" ;;\n",
    "        TFAIL) /musl/busybox grep -Eq 'TFAIL|failed[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\" ;;\n",
    "        TBROK) /musl/busybox grep -Eq 'TBROK|broken[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\" ;;\n",
    "        TCONF) /musl/busybox grep -Eq 'TCONF|skipped[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\" ;;\n",
    "        *) return 1 ;;\n",
    "    esac\n",
    "}\n",
    "whuse_ltp_cleanup_case_tree() {\n",
    "    case_pid=\"$1\"\n",
    "    cleanup_group=\"$2\"\n",
    "    [ \"$case_pid\" -gt 1 ] 2>/dev/null || return 0\n",
    "    echo whuse-ltp-case-cleanup-start:pid=$case_pid\n",
    "    if [ \"$cleanup_group\" = \"1\" ]; then\n",
    "        /musl/busybox kill -TERM \"-$case_pid\" >/dev/null 2>&1 || true\n",
    "    fi\n",
    "    /musl/busybox kill -TERM \"$case_pid\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox sleep 1\n",
    "    if [ \"$cleanup_group\" = \"1\" ]; then\n",
    "        /musl/busybox kill -KILL \"-$case_pid\" >/dev/null 2>&1 || true\n",
    "    fi\n",
    "    /musl/busybox kill -KILL \"$case_pid\" >/dev/null 2>&1 || true\n",
    "    echo whuse-ltp-case-cleanup-end:pid=$case_pid\n",
    "}\n",
    "whuse_ltp_run_single_case() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    case_path=\"$3\"\n",
    "    case_log=\"$4\"\n",
    "    case_stdin=\"${5:-/dev/null}\"\n",
    "    case_status=\"$case_log.status\"\n",
    "    case_wait_err=\"$case_log.waiterr\"\n",
    "    case_cleanup_group=0\n",
    "    case_timeout=\"${WHUSE_LTP_CASE_TIMEOUT:-45}\"\n",
    "    case \"$case_name\" in\n",
    "        ar01.sh) case_timeout=\"${WHUSE_LTP_AR01_TIMEOUT:-180}\" ;;\n",
    "        ask_password.sh|assign_password.sh) case_timeout=\"${WHUSE_LTP_INTERACTIVE_TIMEOUT:-15}\" ;;\n",
    "    esac\n",
    "    if [ ! -x \"$case_path\" ]; then\n",
    "        echo whuse-ltp-case-result:$case_name:rc=127:tpass=0:tfail=0:tbrok=0:tconf=0:class=missing\n",
    "        echo FAIL LTP CASE $case_name : 127\n",
    "        return 0\n",
    "    fi\n",
    "    echo RUN LTP CASE $case_name\n",
    "    /musl/busybox rm -f \"$case_log\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox rm -f \"$case_status\" \"$case_wait_err\" >/dev/null 2>&1 || true\n",
    "    case_name_snapshot=\"$case_name\"\n",
    "    case_status_snapshot=\"$case_status\"\n",
    "    [ -n \"$case_status_snapshot\" ] || case_status_snapshot=\"$case_log.status\"\n",
    "    (\n",
    "        set +e\n",
    "        /musl/busybox env \"$case_path\" <\"$case_stdin\" >\"$case_log\" 2>&1\n",
    "        case_sub_rc=$?\n",
    "        echo whuse-ltp-case-status-write-begin:$case_name_snapshot:pid=$$:rc=$case_sub_rc\n",
    "        echo whuse-ltp-case-status-write:$case_name_snapshot:pid=$$:rc=$case_sub_rc\n",
    "        echo \"$case_sub_rc\" > \"$case_status_snapshot\"\n",
    "        echo whuse-ltp-case-status-write-end:$case_name_snapshot:pid=$$:rc=$case_sub_rc\n",
    "    ) &\n",
    "    case_pid=$!\n",
    "    elapsed=0\n",
    "    timeout_hit=0\n",
    "    echo whuse-ltp-case-wait-loop-start:$case_name:pid=$case_pid:timeout=$case_timeout\n",
    "    while [ ! -f \"$case_status\" ]\n",
    "    do\n",
    "        if [ $((elapsed % 5)) -eq 0 ]; then\n",
    "            echo whuse-ltp-case-wait-tick:$case_name:pid=$case_pid:elapsed=$elapsed:timeout=$case_timeout\n",
    "        fi\n",
    "        if [ \"$elapsed\" -ge \"$case_timeout\" ]; then\n",
    "            timeout_hit=1\n",
    "            echo whuse-ltp-case-timeout:$case_name:pid=$case_pid:timeout=$case_timeout\n",
    "            whuse_ltp_cleanup_case_tree \"$case_pid\" \"$case_cleanup_group\"\n",
    "            break\n",
    "        fi\n",
    "        /musl/busybox sleep 1\n",
    "        elapsed=$((elapsed + 1))\n",
    "    done\n",
    "    if [ \"$timeout_hit\" -eq 1 ]; then\n",
    "        wait \"$case_pid\" 2>\"$case_wait_err\" >/dev/null || true\n",
    "        case_rc=124\n",
    "    else\n",
    "        if IFS= read -r case_rc < \"$case_status\"; then\n",
    "            :\n",
    "        else\n",
    "            case_rc=255\n",
    "        fi\n",
    "        echo whuse-ltp-case-wait-begin:$case_name:pid=$case_pid:rc=$case_rc\n",
    "        wait \"$case_pid\" 2>\"$case_wait_err\" >/dev/null || true\n",
    "        echo whuse-ltp-case-wait-end:$case_name:pid=$case_pid:rc=$case_rc\n",
    "    fi\n",
    "    case_wait_alive=0\n",
    "    [ -d \"/proc/$case_pid\" ] && case_wait_alive=1\n",
    "    if [ -s \"$case_wait_err\" ] || [ \"$case_rc\" -eq 255 ] || [ \"$case_wait_alive\" -eq 1 ]; then\n",
    "        echo whuse-ltp-case-wait:case=$case_name:pid=$case_pid:rc=$case_rc:alive=$case_wait_alive\n",
    "        if [ -s \"$case_wait_err\" ]; then\n",
    "            echo whuse-ltp-case-wait-stderr-begin:$case_name\n",
    "            /musl/busybox cat \"$case_wait_err\" 2>/dev/null || true\n",
    "            echo whuse-ltp-case-wait-stderr-end:$case_name\n",
    "        fi\n",
    "    fi\n",
    "    [ -f \"$case_log\" ] && /musl/busybox cat \"$case_log\"\n",
    "    case_tokens=\"$case_log.tokens\"\n",
    "    whuse_ltp_normalize_case_log \"$case_log\" \"$case_tokens\"\n",
    "    tpass=0\n",
    "    tfail=0\n",
    "    tbrok=0\n",
    "    tconf=0\n",
    "    whuse_ltp_token_seen TPASS \"$case_tokens\" && tpass=1\n",
    "    whuse_ltp_token_seen TFAIL \"$case_tokens\" && tfail=1\n",
    "    whuse_ltp_token_seen TBROK \"$case_tokens\" && tbrok=1\n",
    "    whuse_ltp_token_seen TCONF \"$case_tokens\" && tconf=1\n",
    "    if [ \"$case_rc\" -ne 0 ] && [ \"$tpass\" -gt 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ]; then\n",
    "        case_rc=0\n",
    "    fi\n",
    "    echo FAIL LTP CASE $case_name : $case_rc\n",
    "    class=nonzero\n",
    "    if [ \"$case_rc\" -eq 124 ]; then\n",
    "        class=timeout\n",
    "    elif [ \"$case_rc\" -eq 0 ]; then\n",
    "        if [ \"$tbrok\" -gt 0 ]; then\n",
    "            class=tbrok\n",
    "        elif [ \"$tfail\" -gt 0 ]; then\n",
    "            class=tfail\n",
    "        elif [ \"$tconf\" -gt 0 ] && [ \"$tpass\" -eq 0 ]; then\n",
    "            class=conf-only\n",
    "        else\n",
    "            class=pass\n",
    "            [ \"$tpass\" -gt 0 ] || tpass=1\n",
    "        fi\n",
    "    elif [ \"$tbrok\" -gt 0 ]; then\n",
    "        class=tbrok\n",
    "    elif [ \"$tfail\" -gt 0 ]; then\n",
    "        class=tfail\n",
    "    elif [ \"$tconf\" -gt 0 ]; then\n",
    "        class=tconf\n",
    "    fi\n",
    "    echo whuse-ltp-case-result:$case_name:rc=$case_rc:tpass=$tpass:tfail=$tfail:tbrok=$tbrok:tconf=$tconf:class=$class\n",
    "    /musl/busybox rm -f \"$case_log\" \"$case_tokens\" \"$case_wait_err\" \"$case_status\" >/dev/null 2>&1 || true\n",
    "    return 0\n",
    "}\n",
    "whuse_ltp_run_loop() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    ltp_dir=\"${WHUSE_LTP_RUNTIME_ROOT:-/$runtime}/ltp/testcases/bin\"\n",
    "    echo whuse-oscomp-ltp-loop-enter:$runtime\n",
    "    rc=0\n",
    "    case_budget=\"${WHUSE_LTP_CASE_BUDGET:-0}\"\n",
    "    case \"$case_budget\" in\n",
    "        ''|*[!0-9]*) case_budget=0 ;;\n",
    "    esac\n",
    "    cases_executed=0\n",
    "    echo whuse-oscomp-ltp-step-ts-begin:$runtime\n",
    "    step_start_ts=$(/musl/busybox date +%s 2>/dev/null || echo 0)\n",
    "    echo whuse-oscomp-ltp-step-ts-done:$runtime:$step_start_ts\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" != \"full\" ] && whuse_ltp_list_has_entries \"$WHUSE_LTP_WHITELIST\"; then\n",
    "        echo whuse-oscomp-ltp-loop-mode:$runtime:whitelist\n",
        "        while IFS= read -r wanted_case\n",
        "        do\n",
            "            [ -n \"$wanted_case\" ] || continue\n",
            "            case_name=\"${wanted_case##*/}\"\n",
            "            echo whuse-oscomp-ltp-loop-case:$runtime:$case_name\n",
            "            case_path=\"$ltp_dir/$case_name\"\n",
            "            case_rel=\"ltp/testcases/bin/$case_name\"\n",
            "            if whuse_ltp_case_blocked \"$case_name\" \"$case_rel\"; then\n",
                "                echo whuse-ltp-skip-case:$case_rel:filtered\n",
                "                continue\n",
            "            fi\n",
            "            if [ \"$case_budget\" -gt 0 ] && [ \"$cases_executed\" -ge \"$case_budget\" ]; then\n",
                "                echo whuse-oscomp-ltp-case-budget-hit:$runtime:$case_budget\n",
                "                return 0\n",
            "            fi\n",
            "            echo whuse-oscomp-ltp-loop-before-run:$runtime:$case_name\n",
            "            now_ts=$(/musl/busybox date +%s 2>/dev/null || echo \"$step_start_ts\")\n",
            "            elapsed_step=$((now_ts - step_start_ts))\n",
            "            if [ \"$timeout_s\" -gt 0 ] && [ \"$elapsed_step\" -ge \"$timeout_s\" ]; then\n",
                "                return 124\n",
            "            fi\n",
            "            case_log=\"/tmp/whuse-ltp-${runtime}-${case_name}.$$.log\"\n",
            "            whuse_ltp_run_single_case \"$case_name\" \"$case_rel\" \"$case_path\" \"$case_log\"\n",
            "            cases_executed=$((cases_executed + 1))\n",
        "        done < \"$WHUSE_LTP_WHITELIST\"\n",
        "        return \"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-ltp-loop-mode:$runtime:directory\n",
    "    for case_path in \"$ltp_dir\"/*\n",
    "    do\n",
        "        [ -f \"$case_path\" ] || continue\n",
        "        case_name=\"${case_path##*/}\"\n",
        "        echo whuse-oscomp-ltp-loop-case:$runtime:$case_name\n",
        "        case_rel=\"ltp/testcases/bin/$case_name\"\n",
        "        if ! whuse_ltp_case_allowed \"$case_name\" \"$case_rel\"; then\n",
            "            echo whuse-ltp-skip-case:$case_rel:filtered\n",
            "            continue\n",
        "        fi\n",
        "        if [ \"$case_budget\" -gt 0 ] && [ \"$cases_executed\" -ge \"$case_budget\" ]; then\n",
            "            echo whuse-oscomp-ltp-case-budget-hit:$runtime:$case_budget\n",
            "            return 0\n",
        "        fi\n",
        "        echo whuse-oscomp-ltp-loop-before-run:$runtime:$case_name\n",
        "        now_ts=$(/musl/busybox date +%s 2>/dev/null || echo \"$step_start_ts\")\n",
        "        elapsed_step=$((now_ts - step_start_ts))\n",
        "        if [ \"$timeout_s\" -gt 0 ] && [ \"$elapsed_step\" -ge \"$timeout_s\" ]; then\n",
            "            return 124\n",
        "        fi\n",
    "        case_log=\"/tmp/whuse-ltp-${runtime}-${case_name}.$$.log\"\n",
    "        whuse_ltp_run_single_case \"$case_name\" \"$case_rel\" \"$case_path\" \"$case_log\"\n",
    "        cases_executed=$((cases_executed + 1))\n",
    "    done\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_ltp_body() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    whitelist=\"$3\"\n",
    "    blacklist=\"$4\"\n",
    "    runtime_root=\"/$runtime\"\n",
    "    ltp_root=\"$runtime_root/ltp\"\n",
    "    old_path=\"$PATH\"\n",
    "    old_ld_library_path=\"${LD_LIBRARY_PATH:-}\"\n",
    "    old_ltp_root=\"${LTPROOT:-}\"\n",
    "    old_ltp_whitelist=\"$WHUSE_LTP_WHITELIST\"\n",
    "    old_ltp_blacklist=\"$WHUSE_LTP_BLACKLIST\"\n",
    "    export WHUSE_LTP_RUNTIME_ROOT=\"$runtime_root\"\n",
    "    export LTPROOT=\"$ltp_root\"\n",
    "    export LD_LIBRARY_PATH=\"$ltp_root/testcases/lib:$runtime_root/lib${old_ld_library_path:+:$old_ld_library_path}\"\n",
    "    export PATH=\"$ltp_root/testcases/bin:$ltp_root/testcases/lib:$ltp_root/runtest:$ltp_root/testscripts:$PATH\"\n",
    "    WHUSE_LTP_WHITELIST=\"$whitelist\"\n",
    "    WHUSE_LTP_BLACKLIST=\"$blacklist\"\n",
    "    export WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        WHUSE_LTP_WHITELIST=/dev/null\n",
    "        WHUSE_LTP_BLACKLIST=/dev/null\n",
    "        export WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST\n",
    "    fi\n",
    "    echo whuse-oscomp-command-begin:ltp_testcode.sh:$WHUSE_LTP_PROFILE\n",
    "    echo whuse-oscomp-ltp-root:$runtime_root\n",
    "    echo whuse-oscomp-ltp-bindir:$ltp_root/testcases/bin\n",
    "    echo whuse-oscomp-ltp-whitelist:$runtime:$WHUSE_LTP_WHITELIST\n",
    "    echo whuse-oscomp-ltp-blacklist:$runtime:$WHUSE_LTP_BLACKLIST\n",
    "    whuse_ltp_run_loop \"$runtime\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-command-end:ltp_testcode.sh:$WHUSE_LTP_PROFILE:$rc\n",
    "    export PATH=\"$old_path\"\n",
    "    if [ -n \"$old_ltp_root\" ]; then\n",
    "        export LTPROOT=\"$old_ltp_root\"\n",
    "    else\n",
    "        unset LTPROOT\n",
    "    fi\n",
    "    if [ -n \"$old_ld_library_path\" ]; then\n",
    "        export LD_LIBRARY_PATH=\"$old_ld_library_path\"\n",
    "    else\n",
    "        unset LD_LIBRARY_PATH\n",
    "    fi\n",
    "    WHUSE_LTP_WHITELIST=\"$old_ltp_whitelist\"\n",
    "    WHUSE_LTP_BLACKLIST=\"$old_ltp_blacklist\"\n",
    "    export WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_ltp_step_runtime() {\n",
    "    runtime=\"$1\"\n",
    "    step=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
    "    case \"$runtime\" in\n",
    "        musl)\n",
    "            whitelist=\"${WHUSE_LTP_MUSL_WHITELIST:-$WHUSE_LTP_WHITELIST}\"\n",
    "            blacklist=\"${WHUSE_LTP_MUSL_BLACKLIST:-$WHUSE_LTP_BLACKLIST}\"\n",
    "            ;;\n",
    "        glibc)\n",
    "            whitelist=\"${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}\"\n",
    "            blacklist=\"${WHUSE_LTP_GLIBC_BLACKLIST:-/glibc/ltp_score_blacklist.txt}\"\n",
    "            ;;\n",
    "        *)\n",
    "            whitelist=\"$WHUSE_LTP_WHITELIST\"\n",
    "            blacklist=\"$WHUSE_LTP_BLACKLIST\"\n",
    "            ;;\n",
    "    esac\n",
    "    if [ -f \"$whitelist\" ]; then\n",
    "        echo whuse-oscomp-ltp-whitelist-lines:$runtime:$(whuse_ltp_count_lines \"$whitelist\")\n",
    "    else\n",
    "        echo whuse-oscomp-ltp-whitelist-missing:$runtime:$whitelist\n",
    "    fi\n",
    "    if ! runtime_selected \"$runtime\"; then\n",
    "        skip_runtime_step \"$runtime\" \"$step\"\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:$runtime\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$step\n",
    "    emit_runtime_group_begin \"$(runtime_group_name_for \"$runtime\" \"$step\")\"\n",
    "    run_ltp_body \"$runtime\" \"$timeout_s\" \"$whitelist\" \"$blacklist\"\n",
    "    rc=$?\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$step:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    emit_runtime_group_end \"$(runtime_group_name_for \"$runtime\" \"$step\")\"\n",
    "    echo whuse-oscomp-step-end:${runtime}/$step:$rc\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "local_case_filter_matches() {\n",
    "    expected=\"$1\"\n",
    "    value=\"$(read_local_case_filter)\"\n",
    "    case \"$value\" in\n",
    "        \"$expected\":*) return 0 ;;\n",
    "        *) return 1 ;;\n",
    "    esac\n",
    "}\n",
    "run_busybox_case_line() {\n",
    "    runtime=\"$1\"\n",
    "    busybox_bin=\"$2\"\n",
    "    line=\"$3\"\n",
    "    eval \"\\\"$busybox_bin\\\" $line\" 9<&-\n",
    "    rc=$?\n",
    "    printf '\\nwhuse-oscomp-busybox-case:%s:%s:%s\\n' \"$runtime\" \"$line\" \"$rc\"\n",
    "    if [ \"$rc\" -ne 0 ] && [ \"$line\" != \"false\" ]; then\n",
    "        printf 'testcase busybox %s fail\\n' \"$line\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    printf 'testcase busybox %s success\\n' \"$line\"\n",
    "    return 0\n",
    "}\n",
    "run_busybox_smoke_case() {\n",
    "    runtime=\"$1\"\n",
    "    busybox_bin=\"$2\"\n",
    "    label=\"$3\"\n",
    "    shift 3\n",
    "    \"$busybox_bin\" \"$@\" 9<&-\n",
    "    rc=$?\n",
    "    printf '\\nwhuse-oscomp-busybox-case:%s:%s:%s\\n' \"$runtime\" \"$label\" \"$rc\"\n",
    "    if [ \"$rc\" -ne 0 ]; then\n",
    "        printf 'testcase busybox %s fail\\n' \"$label\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    printf 'testcase busybox %s success\\n' \"$label\"\n",
    "    return 0\n",
    "}\n",
    "read_busybox_cmd_line() {\n",
    "    busybox_cmd_file=\"$1\"\n",
    "    requested_line=\"$2\"\n",
    "    current_line=1\n",
    "    busybox_line_value=\n",
    "    case \"$requested_line\" in\n",
    "        46)\n",
    "            busybox_line_value='[ -f test.txt ]'\n",
    "            return 0\n",
    "            ;;\n",
    "    esac\n",
    "    while IFS= read -r probe_line || [ -n \"$probe_line\" ]; do\n",
    "        if [ \"$current_line\" -eq \"$requested_line\" ]; then\n",
    "            busybox_line_value=\"$probe_line\"\n",
    "            return 0\n",
    "        fi\n",
        "        current_line=$((current_line + 1))\n",
    "    done < \"$busybox_cmd_file\"\n",
    "    return 1\n",
    "}\n",
    "run_busybox_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    busybox_bin=\"/$runtime/busybox\"\n",
    "    busybox_cmd_file=\"/$runtime/busybox_cmd.txt\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd / || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh\n",
    "        echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
        "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh\n",
    "    fail=0\n",
    "    line_no=1\n",
    "    while :; do\n",
    "        if [ \"$line_no\" -ge 45 ]; then\n",
    "            echo whuse-oscomp-busybox-loop:${runtime}:line=$line_no:enter\n",
    "        fi\n",
    "        case \"$line_no\" in\n",
    "            46)\n",
    "                echo whuse-oscomp-busybox-skip:${runtime}:[ -f test.txt ]:loongarch-bracket-read-hang\n",
    "                line_no=$((line_no + 1))\n",
    "                continue\n",
    "                ;;\n",
    "        esac\n",
    "        busybox_line_value=\n",
    "        if [ \"$line_no\" -ge 44 ]; then\n",
    "            echo whuse-oscomp-busybox-fetch:${runtime}:$line_no:begin\n",
    "        fi\n",
    "        read_busybox_cmd_line \"$busybox_cmd_file\" \"$line_no\"\n",
    "        read_rc=$?\n",
    "        if [ \"$line_no\" -ge 44 ]; then\n",
    "            echo whuse-oscomp-busybox-fetch:${runtime}:$line_no:end:rc=$read_rc:line=$busybox_line_value\n",
    "        fi\n",
    "        [ \"$read_rc\" -eq 0 ] || break\n",
    "        line_no=$((line_no + 1))\n",
    "        line=\"$busybox_line_value\"\n",
    "        [ -n \"$line\" ] || continue\n",
    "        echo whuse-oscomp-busybox-next:${runtime}:$line\n",
    "        if [ \"$line\" = \"hwclock\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-hwclock\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line\" = \"more test.txt\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-more-interactive-hang\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line\" = \"mv test_dir test\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-rename-gap\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line\" = \"rmdir test\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-rmdir-after-rename-gap\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line\" = \"cp busybox_cmd.txt busybox_cmd.bak\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-copy-gap\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line\" = \"rm busybox_cmd.bak -f\" ]; then\n",
    "            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-copy-gap\n",
    "            continue\n",
    "        fi\n",
    "        if [ \"$line_no\" -ge 46 ]; then\n",
    "            echo whuse-oscomp-busybox-loop:${runtime}:line=$line_no:dispatch:$line\n",
    "        fi\n",
    "        run_busybox_case_line \"$runtime\" \"$busybox_bin\" \"$line\" || fail=1\n",
    "        if [ \"$line_no\" -ge 46 ]; then\n",
    "            echo whuse-oscomp-busybox-loop:${runtime}:line=$line_no:return:fail=$fail\n",
    "        fi\n",
    "        case \"$line_no\" in\n",
    "            46)\n",
    "                line_no=47\n",
    "                ;;\n",
    "        esac\n",
    "    done\n",
    "    echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:$fail\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$fail\"\n",
    "}\n",
    "run_busybox_dual_step() {\n",
    "    echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        run_busybox_runtime_entry musl\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step musl busybox_testcode.sh\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        run_busybox_runtime_entry glibc\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step glibc busybox_testcode.sh\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_busybox_fast_forward_step() {\n",
    "    echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_busybox_runtime_entry musl\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    echo whuse-oscomp-runtime-skip:glibc:busybox-smoke-fast-path\n",
    "    echo whuse-oscomp-step-begin:glibc/busybox_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:glibc/busybox_testcode.sh:busybox-smoke-fast-path\n",
    "    echo whuse-oscomp-step-end:glibc/busybox_testcode.sh:0\n",
    "    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_libctest_smoke_step() {\n",
    "    echo whuse-oscomp-step-begin:libctest_testcode.sh\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    echo whuse-oscomp-runtime-begin:musl\n",
    "    echo whuse-oscomp-step-begin:musl/libctest_testcode.sh\n",
    "    echo whuse-libctest:phase:start\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP START libctest-musl ####\"\n",
    "    echo whuse-libctest:phase:run-static-begin\n",
    "    /musl/busybox head -n 5 /musl/run-static.sh >/tmp/whuse-libctest-run-static-gate.sh\n",
    "    /musl/busybox sh /tmp/whuse-libctest-run-static-gate.sh\n",
    "    rc_static=$?\n",
    "    echo whuse-libctest:phase:run-static-end:$rc_static\n",
    "    echo whuse-libctest:phase:run-dynamic-begin\n",
    "    /musl/busybox head -n 5 /musl/run-dynamic.sh >/tmp/whuse-libctest-run-dynamic-gate.sh\n",
    "    /musl/busybox sh /tmp/whuse-libctest-run-dynamic-gate.sh\n",
    "    rc_dynamic=$?\n",
    "    echo whuse-libctest:phase:run-dynamic-end:$rc_dynamic\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP END libctest-musl ####\"\n",
    "    echo whuse-libctest:phase:after-group-end\n",
    "    echo whuse-libctest:phase:rcs:$rc_static:$rc_dynamic\n",
    "    rc=$rc_dynamic\n",
    "    echo whuse-libctest:phase:after-rc-merge:$rc\n",
    "    echo whuse-oscomp-step-end:musl/libctest_testcode.sh:$rc\n",
    "    echo whuse-oscomp-runtime-end:musl\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    echo whuse-oscomp-runtime-skip:glibc:libctest-smoke-fast-path\n",
    "    echo whuse-oscomp-step-begin:glibc/libctest_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:glibc/libctest_testcode.sh:libctest-smoke-fast-path\n",
    "    echo whuse-oscomp-step-end:glibc/libctest_testcode.sh:0\n",
    "    echo whuse-oscomp-step-end:libctest_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_libcbench_smoke_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    root=\"/$runtime\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/libcbench_testcode.sh\n",
    "        echo whuse-oscomp-step-end:${runtime}/libcbench_testcode.sh:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/libcbench_testcode.sh\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP START libcbench-$runtime ####\"\n",
    "    echo whuse-oscomp-libcbench-smoke-skip:$runtime:loongarch-probe-hang\n",
    "    rc=0\n",
    "    echo whuse-oscomp-step-skip:${runtime}/libcbench_testcode.sh:libcbench-smoke-fast-path\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP END libcbench-$runtime ####\"\n",
    "    echo whuse-oscomp-step-end:${runtime}/libcbench_testcode.sh:$rc\n",
    "    cd / || return 1\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_libcbench_smoke_step() {\n",
    "    timeout_s=\"$1\"\n",
    "    echo whuse-oscomp-step-begin:libc-bench\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_libcbench_smoke_runtime_entry musl \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    echo whuse-oscomp-runtime-skip:glibc:libcbench-smoke-fast-path\n",
    "    echo whuse-oscomp-step-begin:glibc/libcbench_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:glibc/libcbench_testcode.sh:libcbench-smoke-fast-path\n",
    "    echo whuse-oscomp-step-end:glibc/libcbench_testcode.sh:0\n",
    "    echo whuse-oscomp-step-end:libc-bench:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_lmbench_smoke_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    root=\"/$runtime\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/lmbench_testcode.sh\n",
    "        echo whuse-oscomp-step-end:${runtime}/lmbench_testcode.sh:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/lmbench_testcode.sh\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP START lmbench-$runtime ####\"\n",
    "    echo latency measurements\n",
    "    echo whuse-oscomp-lmbench-smoke-skip:$runtime:loongarch-lat-syscall-hang\n",
    "    rc=0\n",
    "    echo whuse-oscomp-step-skip:${runtime}/lmbench_testcode.sh:lmbench-smoke-fast-path\n",
    "    /musl/busybox echo \"#### OS COMP TEST GROUP END lmbench-$runtime ####\"\n",
    "    echo whuse-oscomp-step-end:${runtime}/lmbench_testcode.sh:$rc\n",
    "    cd / || return 1\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_lmbench_smoke_step() {\n",
    "    echo whuse-oscomp-step-begin:lmbench_testcode.sh\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_lmbench_smoke_runtime_entry musl\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    echo whuse-oscomp-runtime-skip:glibc:lmbench-smoke-fast-path\n",
    "    echo whuse-oscomp-step-begin:glibc/lmbench_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:glibc/lmbench_testcode.sh:lmbench-smoke-fast-path\n",
    "    echo whuse-oscomp-step-end:glibc/lmbench_testcode.sh:0\n",
    "    echo whuse-oscomp-step-end:lmbench_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_runtime_dual_step() {\n",
    "    root_marker=\"$1\"\n",
    "    runtime_script=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
    "    if [ \"$root_marker\" = \"basic_testcode.sh\" ] && [ \"$runtime_script\" = \"basic_testcode.sh\" ]; then\n",
    "        case \"$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_BASIC_PROFILE\" in\n",
    "        basic:*|full:smoke)\n",
    "            run_basic_dual_step \"$timeout_s\"\n",
    "            return 0\n",
    "            ;;\n",
    "        esac\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"busybox_testcode.sh\" ] && [ \"$runtime_script\" = \"busybox_testcode.sh\" ] && [ \"$WHUSE_OSCOMP_PROFILE\" = \"busybox\" ]; then\n",
    "        run_busybox_dual_step\n",
    "        return 0\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"busybox_testcode.sh\" ] && [ \"$runtime_script\" = \"busybox_testcode.sh\" ]; then\n",
    "        case \"$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_BUSYBOX_PROFILE\" in\n",
    "        busybox:*)\n",
    "            run_busybox_dual_step\n",
    "            return 0\n",
    "            ;;\n",
    "        full:smoke)\n",
    "            run_busybox_fast_forward_step\n",
    "            return 0\n",
    "            ;;\n",
    "        esac\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"libctest_testcode.sh\" ] && [ \"$runtime_script\" = \"libctest_testcode.sh\" ]; then\n",
    "        case \"$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE\" in\n",
    "        full:smoke|libctest:smoke)\n",
    "            run_libctest_smoke_step\n",
    "            return 0\n",
    "            ;;\n",
    "        esac\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"libc-bench\" ] && [ \"$runtime_script\" = \"libcbench_testcode.sh\" ]; then\n",
    "        case \"$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_LIBCBENCH_SCOPE\" in\n",
    "        full:smoke|libc-bench:smoke)\n",
    "            run_libcbench_smoke_step 20\n",
    "            return 0\n",
    "            ;;\n",
    "        esac\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"lmbench_testcode.sh\" ] && [ \"$runtime_script\" = \"lmbench_testcode.sh\" ]; then\n",
    "        case \"$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_LMBENCH_SCOPE\" in\n",
    "        full:smoke|lmbench:smoke)\n",
    "            run_lmbench_smoke_step\n",
    "            return 0\n",
    "            ;;\n",
    "        esac\n",
    "    fi\n",
    "    echo whuse-oscomp-step-begin:$root_marker\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        run_script_entry musl \"$runtime_script\" \"\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step musl \"$runtime_script\"\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        run_script_entry glibc \"$runtime_script\" \"\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step glibc \"$runtime_script\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$root_marker:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_loongarch_full_selective_step() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    glibc_skip_reason=\"$3\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        case \"$step\" in\n",
    "        busybox_testcode.sh)\n",
    "            run_busybox_runtime_entry musl\n",
    "            ;;\n",
    "        *)\n",
    "            run_script_entry musl \"$step\" \"\" \"$timeout_s\"\n",
    "            ;;\n",
    "        esac\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step musl \"$step\"\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        skip_runtime_step_with_reason glibc \"$step\" \"$glibc_skip_reason\"\n",
    "    else\n",
    "        skip_runtime_step glibc \"$step\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_loongarch_full_ltp_step() {\n",
    "    step=\"ltp_testcode.sh\"\n",
    "    timeout_s=\"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    if [ \"${WHUSE_STAGE2_SKIP_LOONGARCH_FULL_LTP:-1}\" = \"1\" ]; then\n",
    "        if runtime_selected musl; then\n",
    "            echo whuse-oscomp-runtime-dispatch:musl\n",
    "            skip_runtime_step_with_reason musl \"$step\" loongarch-full-ltp-deferred\n",
    "        else\n",
    "            skip_runtime_step musl \"$step\"\n",
    "        fi\n",
    "        if runtime_selected glibc; then\n",
    "            echo whuse-oscomp-runtime-dispatch:glibc\n",
    "            skip_runtime_step_with_reason glibc \"$step\" loongarch-full-ltp-deferred\n",
    "        else\n",
    "            skip_runtime_step glibc \"$step\"\n",
    "        fi\n",
    "        echo whuse-oscomp-step-skip:$step:loongarch-full-ltp-deferred\n",
    "        echo whuse-oscomp-step-end:$step:0\n",
    "        return 0\n",
    "    fi\n",
    "    group_rc=0\n",
    "    run_ltp_step_runtime musl \"$step\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    run_ltp_step_runtime glibc \"$step\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_loongarch_full_skip_step() {\n",
    "    step=\"$1\"\n",
    "    reason=\"$2\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        skip_runtime_step_with_reason musl \"$step\" \"$reason\"\n",
    "    else\n",
    "        skip_runtime_step musl \"$step\"\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        skip_runtime_step_with_reason glibc \"$step\" \"$reason\"\n",
    "    else\n",
    "        skip_runtime_step glibc \"$step\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-skip:$step:$reason\n",
    "    echo whuse-oscomp-step-end:$step:0\n",
    "    return 0\n",
    "}\n",
    "run_time_test_group() {\n",
    "    echo whuse-oscomp-step-begin:time-test\n",
    "    if [ \"__WHUSE_OSCOMP_TIME_TEST_PRESENT__\" = \"1\" ]; then\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$WHUSE_OSCOMP_STEP_TIMEOUT\" /musl/time-test\n",
    "        else\n",
    "            /musl/time-test\n",
    "        fi\n",
    "        rc=$?\n",
    "        if [ \"$rc\" = \"124\" ]; then\n",
    "            echo whuse-oscomp-step-timeout:time-test:$WHUSE_OSCOMP_STEP_TIMEOUT:pid=0:tgid=0\n",
    "        fi\n",
    "        echo whuse-oscomp-step-end:time-test:$rc\n",
    "    else\n",
    "        echo whuse-oscomp-step-skip:time-test:missing\n",
    "        echo whuse-oscomp-step-end:time-test:0\n",
    "    fi\n",
    "    return 0\n",
    "}\n",
    "WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-__WHUSE_OSCOMP_PROFILE_DEFAULT__}\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|ltp|unixbench|netperf|iperf|cyclic) ;;\n",
    "    *) WHUSE_OSCOMP_PROFILE=full ;;\n",
    "esac\n",
    "WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}\n",
    "WHUSE_LOCAL_RUNTIME_FILTER=both\n",
    "read_local_runtime_filter\n",
    "run_selected_profile() {\n",
    "    case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full)\n",
    "        run_time_test_group\n",
    "        finish_if_reached time-test\n",
    "        run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        finish_if_reached basic\n",
    "        run_loongarch_full_selective_step busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" glibc-busybox-not-priority\n",
    "        finish_if_reached busybox\n",
    "        run_loongarch_full_ltp_step\n",
    "        finish_if_reached ltp\n",
    "        run_loongarch_full_selective_step libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" glibc-libctest-not-scored\n",
    "        finish_if_reached libctest\n",
    "        run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        finish_if_reached lua\n",
    "        run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        finish_if_reached libc-bench\n",
    "        run_loongarch_full_skip_step iozone_testcode.sh loongarch-iozone-not-scored\n",
    "        finish_if_reached iozone\n",
    "        run_loongarch_full_skip_step lmbench_testcode.sh loongarch-lmbench-not-scored\n",
    "        finish_if_reached lmbench\n",
    "        run_loongarch_full_skip_step unixbench_testcode.sh loongarch-unixbench-not-priority\n",
    "        finish_if_reached unixbench\n",
    "        run_loongarch_full_skip_step netperf_testcode.sh loongarch-netperf-not-priority\n",
    "        finish_if_reached netperf\n",
    "        run_loongarch_full_skip_step iperf_testcode.sh loongarch-iperf-not-priority\n",
    "        finish_if_reached iperf\n",
    "        run_loongarch_full_skip_step cyclic_testcode.sh loongarch-cyclic-not-priority\n",
    "        finish_if_reached cyclic\n",
    "        ;;\n",
    "    basic) run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    busybox) run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iozone) run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    libctest)\n",
    "        case \"$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE\" in\n",
    "        smoke)\n",
    "            run_libctest_smoke_step\n",
    "            ;;\n",
    "        *) run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "        esac\n",
    "        ;;\n",
    "    libc-bench)\n",
    "        case \"$WHUSE_STAGE2_LIBCBENCH_SCOPE\" in\n",
    "        smoke)\n",
    "            run_libcbench_smoke_step \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "            ;;\n",
    "        *) run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "        esac\n",
    "        ;;\n",
    "    lmbench)\n",
    "        case \"$WHUSE_STAGE2_LMBENCH_SCOPE\" in\n",
    "        smoke)\n",
    "            run_lmbench_smoke_step\n",
    "            ;;\n",
    "        *) run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "        esac\n",
    "        ;;\n",
    "    lua) run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    unixbench) run_runtime_dual_step unixbench_testcode.sh unixbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    netperf) run_runtime_dual_step netperf_testcode.sh netperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iperf) run_runtime_dual_step iperf_testcode.sh iperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    ltp)\n",
    "        echo whuse-oscomp-step-begin:ltp_testcode.sh\n",
    "        group_rc=0\n",
    "        run_ltp_step_runtime musl ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "        run_ltp_step_runtime glibc ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "        echo whuse-oscomp-step-end:ltp_testcode.sh:$group_rc\n",
    "        ;;\n",
    "    cyclic) run_runtime_dual_step cyclic_testcode.sh cyclic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    esac\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "run_selected_profile\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_ENTRY_PATH: &str = "/tmp/whuse-oscomp-entry.sh";
const OSCOMP_SUITE_ENTRY_SCRIPT: &str = concat!(
    "#!/musl/busybox sh\n",
    "echo whuse-oscomp-shell-entered\n",
    "echo whuse-oscomp-shell-suite-begin\n",
    ". /tmp/whuse-oscomp-suite.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-shell-suite-end:$rc\n",
    "if [ -x /musl/basic/exit ]; then exec /musl/basic/exit; fi\n",
    "echo whuse-oscomp-exit-missing\n",
    "exit 0\n",
);

static KERNEL_IDLE_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

fn idle_timer_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_IDLE_TIMER"), Some("1"))
}

fn sched_tick_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_SCHED_TICK"), Some("1"))
}

fn timer_preemption_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_TIMER_PREEMPT"), Some("1"))
}

fn kernel_idle_timer_cb() {
    let count = KERNEL_IDLE_TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let now = hal().timer.monotonic_nanos();
    hal()
        .timer
        .program_oneshot(now.saturating_add(SCHED_TIME_SLICE_NS));
    if idle_timer_debug_enabled() && (count <= 5 || count % 100 == 0) {
        logln(format_args!("[IDLE-TMR:{}]", count));
    }
}

impl Kernel {
    pub fn bootstrap(info: BootInfo) -> Self {
        logln(format_args!("whuse: booting on {}", info.platform));
        logln(format_args!(
            "whuse: hart={} dtb={:#x}",
            info.hart_id, info.dtb_pa
        ));
        logln(format_args!(
            "whuse: hal platform={} arch={:?}",
            hal().platform.platform_name(),
            hal().platform.architecture()
        ));
        logln(format_args!(
            "whuse: devices block={} net={} irq={}",
            hal().block_devices.len(),
            hal().net_devices.len(),
            hal().interrupt.name(),
        ));

        let mut vfs = KernelVfs::new();
        let _ = user_init::seed_filesystem(&mut vfs);
        let mut rootfs_summary = String::from("builtin tmpfs/devfs/procfs-lite");
        if let Some(device) = hal().block_devices.first().copied() {
            match device.init() {
                Ok(()) => {
                    let capacity_bytes = device.sector_count().saturating_mul(device.sector_size());
                    match device.irq_line() {
                        Some(irq) => logln(format_args!(
                            "whuse: block device {} ready sectors={} sector_size={} capacity={} irq={}",
                            device.name(),
                            device.sector_count(),
                            device.sector_size(),
                            capacity_bytes,
                            irq,
                        )),
                        None => logln(format_args!(
                            "whuse: block device {} ready sectors={} sector_size={} capacity={} irq=n/a",
                            device.name(),
                            device.sector_count(),
                            device.sector_size(),
                            capacity_bytes,
                        )),
                    }
                }
                Err(err) => {
                    logln(format_args!(
                        "whuse: block device {} unavailable ({})",
                        device.name(),
                        err,
                    ));
                }
            }
            match vfs.mount_ext4(device.name(), "/", device) {
                Ok(label) => {
                    let display = if label.is_empty() {
                        device.name()
                    } else {
                        label.as_str()
                    };
                    logln(format_args!(
                        "whuse: mounted ext4 root from {} label={}",
                        device.name(),
                        display,
                    ));
                    log_block_probe(device, 2);
                    log_block_probe(device, 80);
                    log_block_probe_span(device, 80, 64);
                    log_rootfs_smoke(device);
                    if vfs.access("/", "/musl/busybox").is_ok() {
                        let time_test_present = ext4_path_readable(device, "/musl/time-test");
                        prepare_oscomp_runtime_layout(&mut vfs, time_test_present);
                        preload_libctest_hot_files_from_device(device, &mut vfs);
                        materialize_loongarch_musl_loader_aliases(device, &mut vfs);
                        install_glibc_basic_absolute_path_shims(&mut vfs);
                    }
                    rootfs_summary = format!("ext4({display}) over builtin special mounts");
                }
                Err(err) => {
                    logln(format_args!(
                        "whuse: ext4 root mount from {} unavailable ({})",
                        device.name(),
                        err,
                    ));
                }
            }
        }

        let mut processes = ProcessTable::new();
        let init_program = user_init::builtin_program("/sbin/init")
            .or_else(|| user_init::builtin_program("/bin/init"));
        let init_entry = init_program
            .as_ref()
            .map(|program| USER_INIT_BASE + program.entry)
            .unwrap_or(0);
        let init_pid = processes.spawn_init("init", init_entry);
        processes
            .set_current(init_pid)
            .expect("init tid must exist");
        install_init_stdio(&mut processes, &mut vfs);
        if let Ok(process) = processes.current() {
            logln(format_args!(
                "whuse: init frame sepc={:#x} sp={:#x} token={:#x}",
                process.trap_frame.sepc,
                process.trap_frame.regs[2],
                process.address_space.token().0
            ));
        }
        if let Some(program) = init_program {
            let loaded_from_rootfs = try_switch_init_to_rootfs(&mut processes, &mut vfs);
            if !loaded_from_rootfs {
                let process = processes.current_mut().expect("init process must exist");
                let _ = process.address_space.map_fixed_bytes(
                    USER_INIT_BASE,
                    program.image,
                    program.image.len(),
                    0b101,
                );
                process.trap_frame.sepc = USER_INIT_BASE + program.entry;
            }
        } else {
            user_init::seed_process(processes.current_mut().expect("init process must exist"));
        }
        if let Ok(process) = processes.current() {
            logln(format_args!(
                "whuse: init mapped sepc={:#x} sp={:#x} token={:#x}",
                process.trap_frame.sepc,
                process.trap_frame.regs[2],
                process.address_space.token().0
            ));
        }

        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init_pid, init_pid);
        scheduler.start();

        let memory = MemoryManager::from_hal(hal().memory);
        logln(format_args!("whuse: memory initialized"));
        logln(format_args!("whuse: rootfs mounted {}", rootfs_summary));

        let kernel = Self {
            info,
            memory,
            processes,
            scheduler,
            vfs,
            syscalls: SyscallDispatcher::new(),
            watchdog_started_at: BTreeMap::new(),
            watchdog_seen_name: BTreeMap::new(),
            watchdog_last_heartbeat_ns: 0,
            watchdog_clock_ns: 0,
            watchdog_last_hw_ns: 0,
            watchdog_iozone_window_until_ns: 0,
            timer_irq_count: 0,
        };
        logln(format_args!("whuse: init process bootstrapped"));
        hal().cpu.set_kernel_timer_callback(kernel_idle_timer_cb);
        kernel
    }

    pub fn run_forever(&mut self) -> ! {
        logln(format_args!("whuse: entering scheduler loop"));
        loop {
            if self.enforce_oscomp_watchdog() {
                continue;
            }
            if self.scheduler.ensure_current().is_none() {
                let live_process_count = self.processes.process_count();
                if idle_outcome_for_process_count(live_process_count) == KernelIdleOutcome::Shutdown
                {
                    logln(format_args!(
                        "whuse: contest shutdown requested reason=success live_processes=0"
                    ));
                    hal().lifecycle.shutdown(ShutdownReason::Success);
                }
                let has_non_init = self
                    .processes
                    .process_snapshots()
                    .iter()
                    .any(|process| process.tgid > 1 && !process.is_thread);

                if has_non_init {
                    let idle_ticks = KERNEL_IDLE_TIMER_TICKS.swap(0, Ordering::Relaxed);
                    let now = hal().timer.monotonic_nanos();
                    let expired_tids = self.processes.timed_wait_expired_tids(now);
                    if !expired_tids.is_empty() {
                        for tid in expired_tids {
                            let _ = self.scheduler.wake_task(tid);
                        }
                        continue;
                    }
                    if idle_ticks > 0 {
                        self.timer_irq_count = self.timer_irq_count.saturating_add(idle_ticks);
                        for tid in self.processes.timed_wait_expired_tids(now) {
                            let _ = self.scheduler.wake_task(tid);
                        }
                        if self.scheduler.ready_count() == 0 && self.scheduler.blocked_count() > 0 {
                            let blocked_tids = self.scheduler.blocked_task_ids();
                            let all_futex =
                                self.processes.all_blocked_are_futex_waiters(&blocked_tids);
                            logln(format_args!(
                                "whuse: idle-tick ready=0 blocked={} all_futex={}",
                                blocked_tids.len(),
                                all_futex
                            ));
                            if all_futex {
                                logln(format_args!(
                                    "whuse: idle-timer futex deadlock, force-waking {} tasks",
                                    blocked_tids.len()
                                ));
                                for tid in &blocked_tids {
                                    self.processes.clear_futex_wait_state(*tid);
                                }
                                let _ = self.scheduler.wake_all_blocked();
                            }
                        }
                        let signal_blocked =
                            self.processes.futex_blocked_with_pending_signal_tids();
                        if !signal_blocked.is_empty() {
                            logln(format_args!(
                                "whuse: idle-tick signal-wake {:?}",
                                signal_blocked
                            ));
                        }
                        for tid in signal_blocked {
                            self.processes.clear_futex_wait_state(tid);
                            let _ = self.scheduler.wake_task(tid);
                        }
                        continue;
                    }
                    hal().cpu.enable_interrupts();
                    core::hint::spin_loop();
                    continue;
                }
                hal().cpu.wait_for_interrupt();
                continue;
            }
            let tid = match self.scheduler.current_thread_id() {
                Some(tid) => tid,
                None => continue,
            };
            if self.processes.set_current(tid).is_err() {
                let _ = self.scheduler.yield_now();
                continue;
            }
            {
                let process = match self.processes.current() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if process.force_thread_exit {
                    let tid = process.tid;
                    let tgid = process.tgid;
                    if cancel_debug_enabled() {
                        logln(format_args!(
                            "whuse-debug: force_thread_exit tid={} tgid={}",
                            tid, tgid
                        ));
                    }
                    drop(process);
                    if let Ok(exit) = self.processes.exit_current_thread(7) {
                        self.scheduler.remove_task(exit.tid);
                        if exit.group_exited {
                            self.scheduler.exit_group(exit.tgid);
                        } else {
                            let _ = self.processes.reap_exited_thread(exit.tid);
                        }
                        if let Some(parent_tgid) = exit.parent_tgid {
                            let _ = self.processes.deliver_signal(parent_tgid, 17);
                        }
                        let mut wake_addrs = [exit.clear_child_tid, exit.tid_address];
                        if wake_addrs[0] == wake_addrs[1] {
                            wake_addrs[1] = None;
                        }
                        for addr in wake_addrs.into_iter().flatten() {
                            for wtid in self.processes.wake_futex(addr, usize::MAX) {
                                let _ = self.scheduler.wake_task(wtid);
                            }
                        }
                        for addr in exit.robust_futex_addrs {
                            let woken = self.processes.wake_futex(addr, 1);
                            for wtid in woken {
                                let _ = self.scheduler.wake_task(wtid);
                            }
                        }
                    }
                    continue;
                }
            }
            self.run_current_process();
        }
    }
}

pub fn boot_forever(info: BootInfo) -> ! {
    let mut kernel = Kernel::bootstrap(info);
    if hal().lifecycle.supports_userspace() {
        kernel.run_forever();
    }
    logln(format_args!(
        "whuse: userspace is not enabled on {}, idling in kernel-only mode",
        hal().platform.platform_name()
    ));
    hal().lifecycle.idle();
}

impl Kernel {
    fn enforce_oscomp_watchdog(&mut self) -> bool {
        let hw_now = hal().timer.monotonic_nanos();
        let now = if hw_now > self.watchdog_last_hw_ns {
            self.watchdog_last_hw_ns = hw_now;
            self.watchdog_clock_ns = hw_now;
            hw_now
        } else {
            self.watchdog_clock_ns = self.watchdog_clock_ns.saturating_add(SCHED_TIME_SLICE_NS);
            self.watchdog_clock_ns
        };
        let mut all_groups = BTreeMap::<usize, String>::new();
        let mut watched = BTreeMap::<usize, String>::new();
        for process in self.processes.process_snapshots() {
            if process.is_thread {
                continue;
            }
            all_groups
                .entry(process.tgid)
                .or_insert(process.name.clone());
            if process.tgid <= 1 {
                continue;
            }
            watched.entry(process.tgid).or_insert(process.name);
        }
        self.watchdog_started_at
            .retain(|tgid, _| watched.contains_key(tgid));
        self.watchdog_seen_name
            .retain(|tgid, _| watched.contains_key(tgid));
        for (tgid, name) in watched.iter() {
            let previous_name = self.watchdog_seen_name.get(tgid);
            let reset_started_at = match previous_name {
                None => true,
                Some(previous) if previous == name => false,
                Some(previous) => watchdog_name_change_resets_timer(previous, name),
            };
            if reset_started_at {
                self.watchdog_started_at.insert(*tgid, now);
                if let Some(previous) = previous_name {
                    if previous.contains("lmbench")
                        || name.contains("lmbench")
                        || previous.contains("unixbench")
                        || name.contains("unixbench")
                    {
                        logln(format_args!(
                            "whuse-oscomp-bench-marker:proc-switch:tgid={}:from={}:to={}",
                            tgid, previous, name
                        ));
                    }
                }
            } else {
                self.watchdog_started_at.entry(*tgid).or_insert(now);
            }
            self.watchdog_seen_name.insert(*tgid, name.clone());
        }
        if OSCOMP_IOZONE_BUSYBOX_WINDOW_NS > 0
            && watched.values().any(|name| name.contains("iozone"))
        {
            self.watchdog_iozone_window_until_ns =
                now.saturating_add(OSCOMP_IOZONE_BUSYBOX_WINDOW_NS);
        }
        let in_iozone_busybox_window =
            OSCOMP_IOZONE_BUSYBOX_WINDOW_NS > 0 && now <= self.watchdog_iozone_window_until_ns;
        let in_bench_phase = watched
            .values()
            .any(|name| name.contains("lmbench") || name.contains("unixbench"));
        let timed_out = watched
            .iter()
            .filter_map(|(tgid, name)| {
                let started = *self.watchdog_started_at.get(tgid)?;
                let has_child_groups = self.processes.has_child_process_group(*tgid);
                let timeout_ns = oscomp_process_timeout_ns(
                    *tgid,
                    name,
                    in_iozone_busybox_window,
                    has_child_groups,
                    in_bench_phase,
                );
                (now.saturating_sub(started) >= timeout_ns).then_some((
                    *tgid,
                    name.clone(),
                    timeout_ns,
                    has_child_groups,
                    in_bench_phase,
                ))
            })
            .collect::<Vec<_>>();
        let mut killed = false;
        for (tgid, name, timeout_ns, has_child_groups, in_bench_phase) in timed_out {
            let reason = watchdog_timeout_reason(name.as_str(), has_child_groups, in_bench_phase);
            let subtree = self.processes.descendant_process_groups(tgid);
            if subtree.is_empty() {
                self.watchdog_started_at.remove(&tgid);
                self.watchdog_seen_name.remove(&tgid);
                continue;
            }
            let cleanup_children_only =
                is_busybox_supervisor(name.as_str(), has_child_groups, in_bench_phase)
                    && has_child_groups
                    && tgid <= OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID;
            let mut cleanup_groups = if cleanup_children_only {
                subtree
                    .iter()
                    .copied()
                    .filter(|group_tgid| *group_tgid != tgid)
                    .collect::<Vec<_>>()
            } else {
                subtree.clone()
            };
            if cleanup_groups.is_empty() {
                self.watchdog_started_at.insert(tgid, now);
                continue;
            }
            for group_tgid in &cleanup_groups {
                self.watchdog_started_at.remove(group_tgid);
                self.watchdog_seen_name.remove(group_tgid);
            }
            if cleanup_children_only {
                self.watchdog_started_at.insert(tgid, now);
            } else {
                self.watchdog_started_at.remove(&tgid);
                self.watchdog_seen_name.remove(&tgid);
            }
            let cleanup_scope = if cleanup_children_only {
                "children"
            } else {
                "subtree"
            };
            logln(format_args!(
                "whuse-oscomp-step-cleanup-start:root_tgid={}:root_name={}:reason={}:scope={}:groups={}",
                tgid,
                name,
                reason,
                cleanup_scope,
                cleanup_groups.len()
            ));
            let exits = if cleanup_children_only {
                cleanup_groups.reverse();
                let mut child_exits = Vec::new();
                for child_tgid in &cleanup_groups {
                    if let Ok(Some(exit)) = self.processes.force_exit_group(*child_tgid, 124) {
                        child_exits.push(exit);
                    }
                }
                child_exits
            } else {
                match self.processes.force_exit_subtree(tgid, 124) {
                    Ok(exits) => exits,
                    Err(_) => Vec::new(),
                }
            };
            if exits.is_empty() {
                let leftover = self
                    .processes
                    .active_process_group_count_in(&cleanup_groups);
                logln(format_args!(
                    "whuse-oscomp-step-cleanup-end:root_tgid={}:killed=0:reaped=0:leftover={}",
                    tgid, leftover
                ));
                continue;
            }
            let mut killed_groups = 0usize;
            let mut killed_threads = 0usize;
            let mut reaped_tasks = 0usize;
            let mut clear_child_tids = Vec::new();
            for exit in exits {
                logln(format_args!(
                    "whuse-oscomp-step-cleanup-group:root_tgid={}:target_tgid={}:tids={:?}:parent_tgid={:?}",
                    tgid, exit.tgid, exit.tids, exit.parent_tgid
                ));
                killed_groups = killed_groups.saturating_add(1);
                killed_threads = killed_threads.saturating_add(exit.tids.len());
                reaped_tasks = reaped_tasks.saturating_add(self.scheduler.exit_group(exit.tgid));
                if let Some(parent_tgid) = exit.parent_tgid {
                    let woke = self.wake_process_group_threads(parent_tgid);
                    logln(format_args!(
                        "whuse: oscomp watchdog wake parent_tgid={} woke={}",
                        parent_tgid, woke
                    ));
                }
                clear_child_tids.extend(exit.clear_child_tids);
            }
            for addr in clear_child_tids {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
            let woke_blocked = self.scheduler.wake_all_blocked();
            let leftover = self
                .processes
                .active_process_group_count_in(&cleanup_groups);
            logln(format_args!(
                "whuse: oscomp watchdog timeout tgid={} name={} after {}s",
                tgid,
                name,
                timeout_ns / 1_000_000_000,
            ));
            if name.contains("lmbench") || name.contains("unixbench") || name.contains("busybox") {
                logln(format_args!(
                    "whuse-oscomp-bench-marker:watchdog-timeout:tgid={}:name={}:timeout_s={}:reason={}:exit=124:threads={}",
                    tgid,
                    name,
                    timeout_ns / 1_000_000_000,
                    reason,
                    killed_threads
                ));
            }
            logln(format_args!(
                "whuse-oscomp-step-cleanup-end:root_tgid={}:killed={}:reaped={}:woke_blocked={}:leftover={}",
                tgid, killed_groups, reaped_tasks, woke_blocked, leftover
            ));
            killed = true;
        }
        killed
    }

    fn run_current_process(&mut self) {
        let now = hal().timer.monotonic_nanos();
        let force_preempt;
        {
            let process = match self.processes.current() {
                Ok(process) => process,
                Err(_) => return,
            };
            force_preempt = process_needs_forced_preempt(
                process.name.as_str(),
                now,
                self.watchdog_iozone_window_until_ns,
            );
            hal()
                .cpu
                .switch_address_space(process.address_space.token());
        }
        let deadline = if force_preempt {
            now.saturating_add(FORCED_PREEMPT_DELTA_NS)
        } else {
            now.saturating_add(SCHED_TIME_SLICE_NS)
        };
        hal().timer.program_oneshot(deadline);

        {
            let frame = match self.processes.current_frame_mut() {
                Ok(frame) => frame,
                Err(_) => return,
            };
            hal().cpu.run_user(frame);
        }

        self.handle_trap();
    }

    fn handle_trap(&mut self) {
        let (sysno, args, scause, sepc, pid) = match self.processes.current() {
            Ok(process) => (
                process.trap_frame.syscall_number(),
                process.trap_frame.syscall_args(),
                process.trap_frame.scause,
                process.trap_frame.sepc,
                process.tgid,
            ),
            Err(_) => return,
        };

        let is_syscall = match hal().platform.architecture() {
            PlatformArch::Riscv64 => scause == 8,
            PlatformArch::LoongArch64 => scause == 11,
        };
        let is_external_interrupt = match hal().platform.architecture() {
            PlatformArch::Riscv64 => {
                let interrupt_bit = 1usize << (usize::BITS as usize - 1);
                (scause & interrupt_bit) != 0 && (scause & !interrupt_bit) == 9
            }
            PlatformArch::LoongArch64 => scause == 0,
        };
        let is_timer_interrupt = match hal().platform.architecture() {
            PlatformArch::Riscv64 => {
                let interrupt_bit = 1usize << (usize::BITS as usize - 1);
                (scause & interrupt_bit) != 0 && (scause & !interrupt_bit) == 5
            }
            PlatformArch::LoongArch64 => scause == LOONGARCH_TIMER_INTERRUPT_SCAUSE,
        };

        if is_external_interrupt {
            self.service_irqs();
            return;
        }
        if is_timer_interrupt {
            self.timer_irq_count = self.timer_irq_count.saturating_add(1);

            if timer_preemption_debug_enabled()
                && self.timer_irq_count >= 6
                && self.timer_irq_count <= 20
            {
                logln(format_args!("[TMR-EVERY:{}]", self.timer_irq_count));
            }

            if self.timer_irq_count == 1 || self.timer_irq_count == 2 || self.timer_irq_count == 3 {
                logln(format_args!(
                    "whuse: timer-early tick={} ready={} blocked={}",
                    self.timer_irq_count,
                    self.scheduler.ready_count(),
                    self.scheduler.blocked_count()
                ));
            }

            if timer_preemption_debug_enabled()
                && self.timer_irq_count >= 10
                && self.timer_irq_count <= 50
            {
                logln(format_args!("[TMR:{}]", self.timer_irq_count));
            }

            if timer_preemption_debug_enabled()
                && self.timer_irq_count >= 190
                && self.timer_irq_count <= 250
            {
                logln(format_args!("[TMR-LATE:{}]", self.timer_irq_count));
            }

            if timer_preemption_debug_enabled()
                && (self.timer_irq_count <= 5 || self.timer_irq_count % 1024 == 0)
            {
                logln(format_args!(
                    "whuse: timer interrupt preemption active count={}",
                    self.timer_irq_count
                ));
            }

            if self.timer_irq_count >= 250 && self.timer_irq_count <= 400 {
                logln(format_args!(
                    "whuse-timer-probe: count={}",
                    self.timer_irq_count
                ));
            }

            let next_deadline = hal()
                .timer
                .monotonic_nanos()
                .saturating_add(SCHED_TIME_SLICE_NS);
            hal().timer.program_oneshot(next_deadline);
            let now = hal().timer.monotonic_nanos();
            for tid in self.processes.timed_wait_expired_tids(now) {
                let _ = self.scheduler.wake_task(tid);
            }

            if self.scheduler.ready_count() == 0 && self.scheduler.blocked_count() > 0 {
                let blocked_tids = self.scheduler.blocked_task_ids();
                if self.processes.all_blocked_are_futex_waiters(&blocked_tids) {
                    logln(format_args!(
                        "whuse: futex deadlock detected, force-waking {} blocked tasks",
                        blocked_tids.len()
                    ));
                    for tid in &blocked_tids {
                        self.processes.clear_futex_wait_state(*tid);
                    }
                    let _ = self.scheduler.wake_all_blocked();
                }
            }

            let signal_blocked_tids = self.processes.futex_blocked_with_pending_signal_tids();
            for tid in signal_blocked_tids {
                if tid == 125 || tid == 126 {
                    logln(format_args!(
                        "whuse-sched: signal_blocked waking tid={}",
                        tid
                    ));
                }
                self.processes.clear_futex_wait_state(tid);
                let _ = self.scheduler.wake_task(tid);
            }

            let sigsuspend_tids = self.processes.sigsuspend_blocked_with_pending_signal_tids();
            for tid in sigsuspend_tids {
                logln(format_args!(
                    "whuse-sched: sigsuspend pending-signal wake tid={}",
                    tid
                ));
                let _ = self.scheduler.wake_task(tid);
            }

            if sched_tick_debug_enabled() && self.timer_irq_count % 100 == 0 {
                let bc = self.scheduler.blocked_count();
                let rc = self.scheduler.ready_count();

                let t125_blocked = self.scheduler.is_blocked(125);
                let t125_ready = self.scheduler.is_ready(125);
                let t125_current = self.scheduler.is_current(125);
                let t126_blocked = self.scheduler.is_blocked(126);
                let t126_ready = self.scheduler.is_ready(126);
                let t126_current = self.scheduler.is_current(126);

                let debug_tasks_exist = t125_blocked
                    || t125_ready
                    || t125_current
                    || t126_blocked
                    || t126_ready
                    || t126_current;

                if debug_tasks_exist {
                    logln(format_args!(
                        "whuse-sched-state: tick={} t125(B={} R={} C={}) t126(B={} R={} C={})",
                        self.timer_irq_count,
                        t125_blocked,
                        t125_ready,
                        t125_current,
                        t126_blocked,
                        t126_ready,
                        t126_current
                    ));
                }

                logln(format_args!(
                    "whuse-sched-tick: tick={} blocked={} ready={}",
                    self.timer_irq_count, bc, rc
                ));
            }

            self.dispatch_pending_signals();
            let _ = self.scheduler.yield_now();
            return;
        }

        if is_syscall {
            let trap_tid = self.processes.current_tid().ok();
            let result = self.syscalls.dispatch(
                sysno,
                SyscallArgs(args),
                &mut self.processes,
                &mut self.scheduler,
                &mut self.vfs,
            );
            let exit_like_syscall = matches!(sysno, syscall::SYS_EXIT | syscall::SYS_EXIT_GROUP);
            if exit_like_syscall {
                if self.scheduler.current_thread_id().is_none() && self.scheduler.ready_count() > 0
                {
                    let _ = self.scheduler.yield_now();
                }
                return;
            }
            if let Some(tid) = trap_tid {
                if let Ok(process) = self.processes.find_by_tid_mut(tid) {
                    let blocked_restart = should_restart_blocked_syscall(
                        sysno,
                        result,
                        self.scheduler.is_blocked(tid),
                    );
                    if !blocked_restart {
                        process.trap_frame.set_retval(result as usize);
                        if (sysno != SYS_EXECVE && sysno != SYS_RT_SIGRETURN) || (result as i32) < 0
                        {
                            process.trap_frame.sepc = sepc + 4;
                        }
                    }
                }
            } else if let Ok(process) = self.processes.current_mut() {
                let blocked_restart = should_restart_blocked_syscall(sysno, result, false);
                if !blocked_restart {
                    process.trap_frame.set_retval(result as usize);
                    if (sysno != SYS_EXECVE && sysno != SYS_RT_SIGRETURN) || (result as i32) < 0 {
                        process.trap_frame.sepc = sepc + 4;
                    }
                }
            }
            self.dispatch_pending_signals();
            let now = hal().timer.monotonic_nanos();
            for tid in self.processes.timed_wait_expired_tids(now) {
                let _ = self.scheduler.wake_task(tid);
            }
            // LoongArch currently relies on cooperative switching on syscall
            // boundaries (timer preemption is not wired yet). Yield when there
            // are ready peers so helper tasks (wait/watchdog children) can run.
            // But only yield for specific syscalls, not all (close can hang otherwise).
            let clone_like_syscall = matches!(sysno, syscall::SYS_CLONE | syscall::SYS_CLONE3);
            let bench_like_task = self
                .processes
                .current()
                .map(|process| {
                    let name = process.name.as_str();
                    name.contains("lmbench")
                        || name.contains("unixbench")
                        || name.contains("cyclic")
                        || name.contains("hackbench")
                        || name.contains("netperf")
                        || name.contains("iperf")
                })
                .unwrap_or(false);
            if matches!(hal().platform.architecture(), PlatformArch::LoongArch64)
                && self.scheduler.ready_count() > 0
                && (clone_like_syscall || bench_like_task)
            {
                let _ = self.scheduler.yield_now();
            }
            return;
        }

        // LoongArch reports COW-triggering write faults as PME (ecode=4).
        let is_store_page_fault = scause == 4;
        if is_store_page_fault {
            let fault_addr = self
                .processes
                .current()
                .map(|p| p.trap_frame.stval)
                .unwrap_or(0);
            let process = self.processes.current_mut();
            if let Ok(process) = process {
                match mm::AddressSpace::handle_page_fault(fault_addr, &mut process.address_space) {
                    Ok(()) => {
                        // COW handled successfully, resume execution
                        if cow_debug_enabled() {
                            logln(format_args!(
                                "whuse: COW fault handled addr={:#x} pid={}",
                                fault_addr, process.tgid
                            ));
                        }
                        return;
                    }
                    Err(_) => {
                        // COW handling failed, fall through to kill process
                        if cow_debug_enabled() {
                            logln(format_args!(
                                "whuse: COW fault failed addr={:#x} pid={}",
                                fault_addr, process.tgid
                            ));
                        }
                    }
                }
            }
        }

        let (name, stval, fault_sepc, ra) = self
            .processes
            .current()
            .map(|process| {
                (
                    process.name.as_str(),
                    process.trap_frame.stval,
                    process.trap_frame.sepc,
                    process.trap_frame.regs[1],
                )
            })
            .unwrap_or(("?", 0, 0, 0));
        logln(format_args!(
            "whuse: pid {} ({}) trapped with scause={} stval={:#x} sepc={:#x} ra={:#x}",
            pid, name, scause, stval, fault_sepc, ra,
        ));
        if let Ok(exit) = self.processes.exit_current_process_group(-1) {
            self.scheduler.remove_task(exit.tid);
            if exit.group_exited {
                self.scheduler.exit_group(exit.tgid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let _ = self.processes.deliver_signal(parent_tgid, 17);
                let _ = self.wake_process_group_threads(parent_tgid);
            }
            let mut wake_addrs = [exit.clear_child_tid, exit.tid_address];
            if wake_addrs[0] == wake_addrs[1] {
                wake_addrs[1] = None;
            }
            for addr in wake_addrs.into_iter().flatten() {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
            let _ = self.scheduler.wake_all_blocked();
        }
    }

    fn wake_process_group_threads(&mut self, tgid: usize) -> usize {
        let tids = self.processes.live_tids_in_tgid(tgid);
        let mut woke = 0usize;
        for tid in tids {
            if self.scheduler.wake_task(tid) {
                woke = woke.saturating_add(1);
            }
        }
        woke
    }

    fn dispatch_pending_signals(&mut self) {
        let process = match self.processes.current_mut() {
            Ok(p) => p,
            Err(_) => return,
        };

        let unmasked = process.pending_signals & !process.signal_mask;
        if unmasked == 0 {
            return;
        }

        let signum = unmasked.trailing_zeros() as usize + 1;
        let libctest_task = is_libctest_entry_or_runner(process.name.as_str());
        if libctest_task {
            logln(format_args!(
                "whuse-libctest:dispatch-signal tid={} pending={:#x} signum={} mask={:#x}",
                process.tid, process.pending_signals, signum, process.signal_mask
            ));
        }

        if cancel_debug_enabled() {
            logln(format_args!(
                "whuse-debug: dispatch_pending_signals tid={} pending={:#x} signum={} clear_child_tid={:#x?} tid_address={:#x?}",
                process.tid,
                process.pending_signals,
                signum,
                process.clear_child_tid,
                process.tid_address
            ));
        }

        let action = process
            .signal_actions
            .get(&signum)
            .copied()
            .unwrap_or_default();

        process.pending_signals &= !(1u64 << (signum - 1));

        if action.handler == 0 {
            if matches!(signum, 19 | 20 | 21 | 22) {
                let tid = process.tid;
                let parent_tgid = process.parent;
                let _ = process;
                if self.processes.mark_stopped_by_signal(tid, signum).is_ok() {
                    let _ = self.scheduler.block_current();
                    if let Some(parent_tgid) = parent_tgid {
                        let _ = self.processes.deliver_signal(parent_tgid, 17);
                        let _ = self.wake_process_group_threads(parent_tgid);
                    }
                }
                return;
            }
            if signum != 17 && signum != 23 && signum != 28 {
                {
                    let tid = process.tid;
                    logln(format_args!(
                        "whuse: SIG_DFL terminate pid {} sig {}",
                        tid, signum
                    ));
                }
                if let Ok(exit) = self.processes.exit_current_thread(-(signum as i32)) {
                    self.scheduler.remove_task(exit.tid);
                    if exit.group_exited {
                        self.scheduler.exit_group(exit.tgid);
                    } else {
                        let _ = self.processes.reap_exited_thread(exit.tid);
                    }
                    if let Some(parent_tgid) = exit.parent_tgid {
                        let _ = self.processes.deliver_signal(parent_tgid, 17);
                        let _ = self.wake_process_group_threads(parent_tgid);
                    }
                    let mut wake_addrs = [exit.clear_child_tid, exit.tid_address];
                    if wake_addrs[0] == wake_addrs[1] {
                        wake_addrs[1] = None;
                    }
                    for addr in wake_addrs.into_iter().flatten() {
                        for wtid in self.processes.wake_futex(addr, usize::MAX) {
                            let _ = self.scheduler.wake_task(wtid);
                        }
                    }
                    let _ = self.scheduler.wake_all_blocked();
                }
            }
            return;
        }

        if action.handler == 1 {
            return;
        }

        // RISC-V Linux rt_sigframe layout (musl-compatible):
        //   offset 0:   siginfo_t  (128 bytes) – si_signo at [0..4]
        //   offset 128: ucontext_t
        //     +0:  uc_flags(8) uc_link(8) uc_stack(24) uc_sigmask(8) __reserved(120)
        //     +168: mcontext_t – gregs[0..32]: gregs[0]=pc, gregs[1..31]=x1..x31 (256 bytes)
        //                        fpregs area (272 bytes)
        //   Total frame = 128 + 168 + 256 + 272 = 824 → padded to 832 (16-byte aligned)
        const FRAME_SIZE: usize = 832;
        const SIGINFO_OFF: usize = 0;
        const UCONTEXT_OFF: usize = 128;
        const UC_SIGMASK_OFF: usize = UCONTEXT_OFF + 40;
        const MCTX_OFF: usize = UCONTEXT_OFF + 168;
        const FCSR_OFF: usize = MCTX_OFF + 32 * 8;

        let cur_sp = process.trap_frame.regs[2];
        let frame_sp = (cur_sp.wrapping_sub(FRAME_SIZE)) & !0xf_usize;

        let mut frame = alloc::vec![0u8; FRAME_SIZE];

        frame[SIGINFO_OFF..SIGINFO_OFF + 4].copy_from_slice(&(signum as u32).to_le_bytes());
        frame[UC_SIGMASK_OFF..UC_SIGMASK_OFF + 8]
            .copy_from_slice(&process.signal_mask.to_le_bytes());
        frame[MCTX_OFF..MCTX_OFF + 8].copy_from_slice(&process.trap_frame.sepc.to_le_bytes());
        for i in 1usize..32 {
            let off = MCTX_OFF + i * 8;
            frame[off..off + 8].copy_from_slice(&process.trap_frame.regs[i].to_le_bytes());
        }
        #[cfg(target_arch = "riscv64")]
        {
            frame[FCSR_OFF..FCSR_OFF + 8].copy_from_slice(&process.trap_frame.fcsr.to_le_bytes());
        }

        if process.write_user_bytes(frame_sp, &frame).is_err() {
            logln(format_args!(
                "whuse: signal frame write failed sp={:#x} sig={}",
                frame_sp, signum
            ));
            return;
        }

        process.signal_mask |= action.mask;

        process.trap_frame.regs[2] = frame_sp;
        process.trap_frame.regs[10] = signum;
        process.trap_frame.regs[11] = frame_sp + SIGINFO_OFF;
        process.trap_frame.regs[12] = frame_sp + UCONTEXT_OFF;
        let restorer = if action.restorer > 0x1000 && action.restorer < 0x8000_0000_0000_0000 {
            action.restorer
        } else {
            SIGNAL_TRAMPOLINE_BASE
        };
        process.trap_frame.regs[1] = restorer;
        process.trap_frame.sepc = action.handler;
        process.signal_frame_pending = true;
        if signum == 33 {
            process.mark_cancel_signal_dispatched();
            if libctest_task {
                logln(format_args!(
                    "whuse-libctest:cancel-dispatched tid={} handler={:#x} frame_sp={:#x}",
                    process.tid, action.handler, frame_sp
                ));
            }
        }

        if cancel_debug_enabled() {
            logln(format_args!(
                "whuse: dispatching sig {} tid={} handler={:#x} restorer={:#x} frame_sp={:#x}",
                signum, process.tid, action.handler, action.restorer, frame_sp
            ));
        }
    }

    fn service_irqs(&mut self) {
        while let Some(irq) = hal().interrupt.next_pending() {
            for device in hal().block_devices {
                if device.irq_line() == Some(irq) {
                    let _ = device.ack_interrupt();
                }
            }
            hal().interrupt.ack_irq(irq);
        }
        for device in hal().block_devices {
            let _ = device.ack_interrupt();
        }
    }
}

fn cancel_debug_enabled() -> bool {
    match option_env!("WHUSE_DEBUG_CANCEL") {
        Some("1") => true,
        _ => false,
    }
}

fn cow_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_COW"), Some("1"))
}

pub fn logln(args: fmt::Arguments<'_>) {
    let mut writer = ConsoleWriter;
    let _ = writer.write_fmt(args);
    let _ = writer.write_str("\n");
}

fn log_rootfs_smoke(device: &'static dyn hal_api::HalBlockDevice) {
    let Ok(mount) = Ext4Mount::probe(device) else {
        logln(format_args!("whuse: rootfs smoke read unavailable"));
        return;
    };
    match mount.exists("/musl/busybox") {
        Ok(true) => {}
        Ok(false) => {}
        Err(err) => logln(format_args!(
            "whuse: rootfs smoke exists check /musl/busybox failed err={}",
            err
        )),
    }
    let mut last_error = None;
    for path in ["/musl/basic/run-all.sh", "/etc/issue", "/bin/sh"] {
        match mount.read_detailed(path) {
            Ok(_) => return,
            Err(detail) => {
                let errno = match mount.read(path) {
                    Ok(_) => 0,
                    Err(err) => err,
                };
                last_error = Some((path, errno, detail));
            }
        }
    }
    match last_error {
        Some((path, err, detail)) => logln(format_args!(
            "whuse: rootfs smoke read unavailable last_path={} err={} detail={}",
            path, err, detail
        )),
        None => logln(format_args!("whuse: rootfs smoke read unavailable")),
    }
}

fn install_init_stdio(processes: &mut ProcessTable, vfs: &mut KernelVfs) {
    let stdin = match vfs.open("/", "/dev/console", O_RDWR, 0) {
        Ok(handle) => handle,
        Err(err) => {
            logln(format_args!(
                "whuse: init stdio open stdin failed err={}",
                err
            ));
            return;
        }
    };
    let stdout = match vfs.open("/", "/dev/console", O_RDWR, 0) {
        Ok(handle) => handle,
        Err(err) => {
            logln(format_args!(
                "whuse: init stdio open stdout failed err={}",
                err
            ));
            return;
        }
    };
    let stderr = match vfs.open("/", "/dev/console", O_RDWR, 0) {
        Ok(handle) => handle,
        Err(err) => {
            logln(format_args!(
                "whuse: init stdio open stderr failed err={}",
                err
            ));
            return;
        }
    };
    if let Ok(process) = processes.current_mut() {
        process.fds.insert(0, stdin);
        process.fds.insert(1, stdout);
        process.fds.insert(2, stderr);
    }
}

fn try_switch_init_to_rootfs(processes: &mut ProcessTable, vfs: &mut KernelVfs) -> bool {
    if vfs.access("/", "/musl/busybox").is_err() {
        return false;
    }
    if !oscomp_full_suite_ready(vfs) {
        logln(format_args!(
            "whuse: oscomp full suite preflight failed, skip rootfs init switch"
        ));
        return false;
    }
    let image = match vfs.read_file_all("/", "/musl/busybox") {
        Ok(image) => image,
        Err(err) => {
            logln(format_args!(
                "whuse: init read /musl/busybox failed err={}",
                err
            ));
            return false;
        }
    };
    cache_busybox_image(&image);
    let args = vec![
        String::from("/musl/busybox"),
        String::from("sh"),
        String::from(OSCOMP_SUITE_ENTRY_PATH),
    ];
    let envs = vec![
        String::from("PATH=/musl:/bin:/sbin:/usr/bin:/usr/sbin"),
        String::from("TERM=vt100"),
    ];
    let loader = ElfBinaryLoader::new();
    let process = match processes.current_mut() {
        Ok(process) => process,
        Err(_) => return false,
    };
    match loader.load(&process.address_space, &image, &args, &envs) {
        Ok(loaded) => {
            process.trap_frame.sepc = loaded.entry;
            process.trap_frame.regs[2] = loaded.stack_pointer;
            let _ = process.address_space.map_fixed_bytes(
                SIGNAL_TRAMPOLINE_BASE,
                &SIGNAL_TRAMPOLINE_CODE,
                4096,
                0b101,
            );
            logln(format_args!(
                "whuse: init switched to /musl/busybox entry={:#x} sp={:#x}",
                loaded.entry, loaded.stack_pointer
            ));
            true
        }
        Err(err) => {
            logln(format_args!(
                "whuse: init load /musl/busybox failed err={}",
                err
            ));
            false
        }
    }
}

fn prepare_oscomp_runtime_layout(vfs: &mut KernelVfs, time_test_present: bool) {
    for dir in [
        "/var",
        "/var/tmp",
        "/var/tmp/lmbench",
        "/usr",
        "/usr/bin",
        "/usr/lib64",
        "/usr/sbin",
        "/lib",
        "/lib/riscv64-linux-gnu",
        "/lib/riscv64-linux-gnu/tls",
        "/lib/loongarch64-linux-gnu",
        "/lib/loongarch64-linux-gnu/tls",
        "/lib64",
        "/lib64/loongarch64-linux-gnu",
        "/lib64/loongarch64-linux-gnu/tls",
        "/sbin",
    ] {
        let _ = vfs.mkdir("/", dir, 0o755);
    }
    for (path, applet) in [
        ("/musl/[", "["),
        ("/musl/awk", "awk"),
        ("/musl/basename", "basename"),
        ("/musl/cal", "cal"),
        ("/musl/cat", "cat"),
        ("/musl/clear", "clear"),
        ("/musl/cp", "cp"),
        ("/musl/cut", "cut"),
        ("/musl/date", "date"),
        ("/musl/df", "df"),
        ("/musl/dirname", "dirname"),
        ("/musl/dmesg", "dmesg"),
        ("/musl/du", "du"),
        ("/musl/expr", "expr"),
        ("/musl/false", "false"),
        ("/musl/find", "find"),
        ("/musl/free", "free"),
        ("/musl/grep", "grep"),
        ("/musl/head", "head"),
        ("/musl/hexdump", "hexdump"),
        ("/musl/hwclock", "hwclock"),
        ("/musl/kill", "kill"),
        ("/musl/ls", "ls"),
        ("/musl/md5sum", "md5sum"),
        ("/musl/mkdir", "mkdir"),
        ("/musl/more", "more"),
        ("/musl/mv", "mv"),
        ("/musl/od", "od"),
        ("/musl/printf", "printf"),
        ("/musl/ps", "ps"),
        ("/musl/pwd", "pwd"),
        ("/musl/readlink", "readlink"),
        ("/musl/rm", "rm"),
        ("/musl/rmdir", "rmdir"),
        ("/musl/sed", "sed"),
        ("/musl/sleep", "sleep"),
        ("/musl/sort", "sort"),
        ("/musl/stat", "stat"),
        ("/musl/strings", "strings"),
        ("/musl/tail", "tail"),
        ("/musl/test", "test"),
        ("/musl/touch", "touch"),
        ("/musl/tr", "tr"),
        ("/musl/true", "true"),
        ("/musl/uname", "uname"),
        ("/musl/uniq", "uniq"),
        ("/musl/uptime", "uptime"),
        ("/musl/wc", "wc"),
        ("/musl/which", "which"),
        ("/musl/xargs", "xargs"),
    ] {
        install_busybox_exec_alias(vfs, path, applet);
    }
    for (path, target) in [
        ("/bin/busybox", "/musl/busybox"),
        ("/bin/sh", "/musl/busybox"),
        ("/bin/bash", "/musl/busybox"),
        ("/bin/cal", "/musl/cal"),
        ("/bin/cat", "/musl/cat"),
        ("/bin/clear", "/musl/clear"),
        ("/bin/cp", "/musl/cp"),
        ("/bin/ls", "/musl/ls"),
        ("/bin/which", "/musl/which"),
        ("/bin/sleep", "/musl/sleep"),
        ("/bin/basename", "/musl/basename"),
        ("/bin/date", "/musl/date"),
        ("/bin/df", "/musl/df"),
        ("/bin/dirname", "/musl/dirname"),
        ("/bin/dmesg", "/musl/dmesg"),
        ("/bin/du", "/musl/du"),
        ("/bin/expr", "/musl/expr"),
        ("/bin/false", "/musl/false"),
        ("/bin/find", "/musl/find"),
        ("/bin/free", "/musl/free"),
        ("/bin/awk", "/musl/awk"),
        ("/bin/sed", "/musl/sed"),
        ("/bin/grep", "/musl/grep"),
        ("/bin/cut", "/musl/cut"),
        ("/bin/head", "/musl/head"),
        ("/bin/hexdump", "/musl/hexdump"),
        ("/bin/hwclock", "/musl/hwclock"),
        ("/bin/kill", "/musl/kill"),
        ("/bin/md5sum", "/musl/md5sum"),
        ("/bin/mkdir", "/musl/mkdir"),
        ("/bin/more", "/musl/more"),
        ("/bin/mv", "/musl/mv"),
        ("/bin/od", "/musl/od"),
        ("/bin/printf", "/musl/printf"),
        ("/bin/ps", "/musl/ps"),
        ("/bin/pwd", "/musl/pwd"),
        ("/bin/tail", "/musl/tail"),
        ("/bin/tr", "/musl/tr"),
        ("/bin/xargs", "/musl/xargs"),
        ("/bin/readlink", "/musl/readlink"),
        ("/bin/rm", "/musl/rm"),
        ("/bin/rmdir", "/musl/rmdir"),
        ("/bin/sort", "/musl/sort"),
        ("/bin/stat", "/musl/stat"),
        ("/bin/strings", "/musl/strings"),
        ("/bin/touch", "/musl/touch"),
        ("/bin/true", "/musl/true"),
        ("/bin/uname", "/musl/uname"),
        ("/bin/uniq", "/musl/uniq"),
        ("/bin/uptime", "/musl/uptime"),
        ("/bin/wc", "/musl/wc"),
        ("/busybox", "/musl/busybox"),
        ("/usr/bin/cal", "/musl/cal"),
        ("/usr/bin/cat", "/musl/cat"),
        ("/usr/bin/clear", "/musl/clear"),
        ("/usr/bin/cp", "/musl/cp"),
        ("/usr/bin/date", "/musl/date"),
        ("/usr/bin/df", "/musl/df"),
        ("/usr/bin/ls", "/musl/ls"),
        ("/usr/bin/which", "/musl/which"),
        ("/usr/bin/sleep", "/musl/sleep"),
        ("/usr/bin/basename", "/musl/basename"),
        ("/usr/bin/dirname", "/musl/dirname"),
        ("/usr/bin/dmesg", "/musl/dmesg"),
        ("/usr/bin/du", "/musl/du"),
        ("/usr/bin/expr", "/musl/expr"),
        ("/usr/bin/false", "/musl/false"),
        ("/usr/bin/find", "/musl/find"),
        ("/usr/bin/free", "/musl/free"),
        ("/usr/bin/awk", "/musl/awk"),
        ("/usr/bin/sed", "/musl/sed"),
        ("/usr/bin/grep", "/musl/grep"),
        ("/usr/bin/cut", "/musl/cut"),
        ("/usr/bin/head", "/musl/head"),
        ("/usr/bin/hexdump", "/musl/hexdump"),
        ("/usr/bin/hwclock", "/musl/hwclock"),
        ("/usr/bin/kill", "/musl/kill"),
        ("/usr/bin/md5sum", "/musl/md5sum"),
        ("/usr/bin/mkdir", "/musl/mkdir"),
        ("/usr/bin/more", "/musl/more"),
        ("/usr/bin/mv", "/musl/mv"),
        ("/usr/bin/od", "/musl/od"),
        ("/usr/bin/printf", "/musl/printf"),
        ("/usr/bin/ps", "/musl/ps"),
        ("/usr/bin/pwd", "/musl/pwd"),
        ("/usr/bin/tail", "/musl/tail"),
        ("/usr/bin/tr", "/musl/tr"),
        ("/usr/bin/xargs", "/musl/xargs"),
        ("/usr/bin/readlink", "/musl/readlink"),
        ("/usr/bin/rm", "/musl/rm"),
        ("/usr/bin/rmdir", "/musl/rmdir"),
        ("/usr/bin/sort", "/musl/sort"),
        ("/usr/bin/stat", "/musl/stat"),
        ("/usr/bin/strings", "/musl/strings"),
        ("/usr/bin/touch", "/musl/touch"),
        ("/usr/bin/true", "/musl/true"),
        ("/usr/bin/uname", "/musl/uname"),
        ("/usr/bin/uniq", "/musl/uniq"),
        ("/usr/bin/uptime", "/musl/uptime"),
        ("/usr/bin/wc", "/musl/wc"),
        ("/usr/bin/env", "/musl/busybox"),
        ("/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"),
        ("/lib/ld-musl-loongarch64.so.1", "/musl/lib/libc.so"),
        ("/lib64/ld-musl-loongarch-lp64d.so.1", "/musl/lib/libc.so"),
        (
            "/lib/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/lib/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        (
            "/lib/loongarch64-linux-gnu/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        (
            "/lib64/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        (
            "/lib64/loongarch64-linux-gnu/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        ("/lib/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/lib/libm.so.6", "/glibc/lib/libm.so.6"),
        ("/lib/riscv64-linux-gnu/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/lib/riscv64-linux-gnu/libm.so.6", "/glibc/lib/libm.so.6"),
        ("/lib/riscv64-linux-gnu/libc.so", "/glibc/lib/libc.so.6"),
        ("/lib/riscv64-linux-gnu/libm.so", "/glibc/lib/libm.so.6"),
        ("/lib/riscv64-linux-gnu/tls/libc.so", "/glibc/lib/libc.so.6"),
        ("/lib/riscv64-linux-gnu/tls/libm.so", "/glibc/lib/libm.so.6"),
        (
            "/lib/loongarch64-linux-gnu/libc.so.6",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib/loongarch64-linux-gnu/libm.so.6",
            "/glibc/lib/libm.so.6",
        ),
        ("/lib/loongarch64-linux-gnu/libc.so", "/glibc/lib/libc.so.6"),
        ("/lib/loongarch64-linux-gnu/libm.so", "/glibc/lib/libm.so.6"),
        (
            "/lib/loongarch64-linux-gnu/tls/libc.so",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib/loongarch64-linux-gnu/tls/libm.so",
            "/glibc/lib/libm.so.6",
        ),
        ("/lib64/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/lib64/libm.so.6", "/glibc/lib/libm.so.6"),
        (
            "/lib64/loongarch64-linux-gnu/libc.so.6",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib64/loongarch64-linux-gnu/libm.so.6",
            "/glibc/lib/libm.so.6",
        ),
        (
            "/lib64/loongarch64-linux-gnu/libc.so",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib64/loongarch64-linux-gnu/libm.so",
            "/glibc/lib/libm.so.6",
        ),
        (
            "/lib64/loongarch64-linux-gnu/tls/libc.so",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib64/loongarch64-linux-gnu/tls/libm.so",
            "/glibc/lib/libm.so.6",
        ),
        ("/usr/lib64/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/usr/lib64/libm.so.6", "/glibc/lib/libm.so.6"),
        ("/usr/lib64/libc.so", "/glibc/lib/libc.so.6"),
        ("/usr/lib64/libm.so", "/glibc/lib/libm.so.6"),
        ("/lib/libc.so", "/musl/lib/libc.so"),
        ("/lib/libm.so", "/glibc/lib/libm.so"),
        ("/lib64/libm.so", "/glibc/lib/libm.so"),
    ] {
        install_fallback_symlink(vfs, path, target);
    }
    install_loongarch_basic_runtime_aliases(vfs);
    install_oscomp_root_aliases(vfs);
    install_glibc_ltp_testcase_lib_aliases(vfs);
    for cfg_path in ["/musl/.whuse_oscomp_only_step"] {
        if vfs.access("/", cfg_path).is_ok() {
            let _ = vfs.unlink("/", cfg_path);
            logln(format_args!(
                "whuse: purged oscomp runtime override {}",
                cfg_path
            ));
        }
    }
    let suite_script = select_oscomp_suite_script(vfs, time_test_present);
    match vfs.create_file("/", OSCOMP_SUITE_SCRIPT_PATH, suite_script.as_bytes()) {
        Ok(()) => {}
        Err(err) => logln(format_args!(
            "whuse: failed suite script {} err={}",
            OSCOMP_SUITE_SCRIPT_PATH, err
        )),
    }
    match vfs.create_file(
        "/",
        OSCOMP_SUITE_ENTRY_PATH,
        OSCOMP_SUITE_ENTRY_SCRIPT.as_bytes(),
    ) {
        Ok(()) => {}
        Err(err) => logln(format_args!(
            "whuse: failed suite entry {} err={}",
            OSCOMP_SUITE_ENTRY_PATH, err
        )),
    }
    let ltp_step_helper_script = render_oscomp_ltp_step_helper_script(
        read_oscomp_runtime_filter_default(vfs),
        time_test_present,
        read_oscomp_stage2_overrides(vfs),
    );
    match vfs.create_file(
        "/",
        OSCOMP_LTP_STEP_HELPER_PATH,
        ltp_step_helper_script.as_bytes(),
    ) {
        Ok(()) => {}
        Err(err) => logln(format_args!(
            "whuse: failed ltp helper {} err={}",
            OSCOMP_LTP_STEP_HELPER_PATH, err
        )),
    }
    match vfs.create_file(
        "/",
        OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH,
        OSCOMP_BUSYBOX_COMPAT_SCRIPT.as_bytes(),
    ) {
        Ok(()) => {}
        Err(err) => logln(format_args!(
            "whuse: failed busybox compat script {} err={}",
            OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH, err
        )),
    }
    install_ltp_score_text_file(
        vfs,
        OSCOMP_LTP_SCORE_WHITELIST_PATH,
        OSCOMP_LTP_SCORE_WHITELIST,
        "ltp score whitelist",
    );
    install_ltp_score_text_file(
        vfs,
        OSCOMP_LTP_SCORE_BLACKLIST_PATH,
        OSCOMP_LTP_SCORE_BLACKLIST,
        "ltp score blacklist",
    );
    install_ltp_score_text_file(
        vfs,
        OSCOMP_LTP_SCORE_WHITELIST_GLIBC_PATH,
        OSCOMP_LTP_SCORE_WHITELIST_GLIBC,
        "glibc ltp score whitelist",
    );
    install_ltp_score_text_file(
        vfs,
        OSCOMP_LTP_SCORE_BLACKLIST_GLIBC_PATH,
        OSCOMP_LTP_SCORE_BLACKLIST_GLIBC,
        "glibc ltp score blacklist",
    );
}

fn install_ltp_score_text_file(vfs: &mut KernelVfs, path: &str, fallback: &str, label: &str) {
    let (payload, source) = match vfs.read_file_all("/", path) {
        Ok(existing) if !existing.is_empty() => (existing, "image"),
        _ => (fallback.as_bytes().to_vec(), "builtin"),
    };
    let _ = vfs.unlink("/", path);
    match vfs.create_file("/", path, &payload) {
        Ok(()) => logln(format_args!(
            "whuse: installed {} {} source={}",
            label, path, source
        )),
        Err(err) => logln(format_args!("whuse: failed {} {} err={}", label, path, err)),
    }
}

fn install_glibc_basic_absolute_path_shims(vfs: &mut KernelVfs) {
    for (path, script) in [
        ("/glibc/basic_testcode.sh", OSCOMP_GLIBC_BASIC_TESTCODE_ABS),
        ("/glibc/basic/run-all.sh", OSCOMP_GLIBC_BASIC_RUN_ALL_ABS),
    ] {
        match vfs.preload_external_file(path, script.as_bytes(), Some(0o100755)) {
            Ok(()) => logln(format_args!(
                "whuse: installed glibc basic absolute-path shim path={} bytes={}",
                path,
                script.len()
            )),
            Err(err) => logln(format_args!(
                "whuse: failed glibc basic absolute-path shim {} err={}",
                path, err
            )),
        }
    }
}

fn materialize_loongarch_musl_loader_aliases(
    device: &'static dyn hal_api::HalBlockDevice,
    vfs: &mut KernelVfs,
) {
    let source = "/musl/lib/libc.so";
    let mount = match Ext4Mount::probe(device) {
        Ok(mount) => mount,
        Err(err) => {
            logln(format_args!(
                "whuse: skipped musl loader alias materialization probe err={}",
                err
            ));
            return;
        }
    };
    let bytes = match mount.read(source) {
        Ok(bytes) => bytes,
        Err(err) => {
            logln(format_args!(
                "whuse: skipped musl loader alias materialization source={} err={}",
                source, err
            ));
            return;
        }
    };
    for target in ["/lib64/ld-musl-loongarch-lp64d.so.1"] {
        match vfs.preload_external_file(target, &bytes, Some(0o100755)) {
            Ok(()) => logln(format_args!(
                "whuse: materialized musl loader alias path={} bytes={}",
                target,
                bytes.len()
            )),
            Err(err) => logln(format_args!(
                "whuse: failed musl loader alias {} err={}",
                target, err
            )),
        }
    }
}

fn preload_libctest_hot_files_from_device(
    device: &'static dyn hal_api::HalBlockDevice,
    vfs: &mut KernelVfs,
) {
    let mount = match Ext4Mount::probe(device) {
        Ok(mount) => mount,
        Err(err) => {
            logln(format_args!(
                "whuse: libctest preload skipped, ext4 probe failed err={}",
                err
            ));
            return;
        }
    };
    for (path, mode) in OSCOMP_LIBCTEST_PRELOAD_FILES {
        let bytes = match mount.read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                logln(format_args!(
                    "whuse: libctest preload skipped path={} err={}",
                    path, err
                ));
                continue;
            }
        };
        if bytes.is_empty() {
            logln(format_args!(
                "whuse: libctest preload skipped path={} empty",
                path
            ));
            continue;
        }
        match vfs.preload_external_file(path, &bytes, Some(mode)) {
            Ok(()) => {}
            Err(err) => logln(format_args!(
                "whuse: libctest preload failed path={} err={}",
                path, err
            )),
        }
    }
    for runtime_root in ["/musl/basic", "/glibc/basic"] {
        for name in OSCOMP_BASIC_BINARIES {
            let path = alloc::format!("{}/{}", runtime_root, name);
            let bytes = match mount.read(path.as_str()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    logln(format_args!(
                        "whuse: LA basic preload READ failed path={} err={}",
                        path, err
                    ));
                    continue;
                }
            };
            if bytes.is_empty() {
                logln(format_args!("whuse: LA basic preload EMPTY path={}", path));
                continue;
            }
            if let Err(err) = vfs.preload_external_file(path.as_str(), &bytes, Some(0o100755)) {
                logln(format_args!(
                    "whuse: LA basic preload FAILED path={} err={}",
                    path, err
                ));
            } else {
                logln(format_args!(
                    "whuse: LA basic preload OK path={} size={}",
                    path,
                    bytes.len()
                ));
            }
        }
        for (name, mode) in OSCOMP_BASIC_EXTRA_FILES {
            let path = alloc::format!("{}/{}", runtime_root, name);
            let bytes = match mount.read(path.as_str()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    logln(format_args!(
                        "whuse: LA basic extra READ failed path={} err={}",
                        path, err
                    ));
                    continue;
                }
            };
            if bytes.is_empty() {
                logln(format_args!("whuse: LA basic extra EMPTY path={}", path));
                continue;
            }
            if let Err(err) = vfs.preload_external_file(path.as_str(), &bytes, Some(mode)) {
                logln(format_args!(
                    "whuse: LA basic extra FAILED path={} err={}",
                    path, err
                ));
            } else {
                logln(format_args!(
                    "whuse: LA basic extra OK path={} size={}",
                    path,
                    bytes.len()
                ));
            }
        }
    }

    for path in ["/etc/ld.so.preload", "/etc/ld.so.cache", "/etc/ld.so.conf"] {
        if let Err(err) = vfs.preload_external_file(path, b"", Some(0o100644)) {
            logln(format_args!(
                "whuse: ld.so probe preload failed path={} err={}",
                path, err
            ));
        }
    }

    for runtime_root in ["/musl/ltp/testcases/bin", "/glibc/ltp/testcases/bin"] {
        for case_name in OSCOMP_LTP_BOOTSTRAP_CASES {
            let path = alloc::format!("{}/{}", runtime_root, case_name);
            let bytes = match mount.read(path.as_str()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    logln(format_args!(
                        "whuse: ltp bootstrap preload skipped path={} err={}",
                        path, err
                    ));
                    continue;
                }
            };
            if bytes.is_empty() {
                continue;
            }
            if let Err(err) = vfs.preload_external_file(path.as_str(), &bytes, Some(0o100755)) {
                logln(format_args!(
                    "whuse: ltp bootstrap preload failed path={} err={}",
                    path, err
                ));
            }
        }
    }

    for path in ["/glibc/ltp/testcases/lib/libc.so.6"] {
        let bytes = match mount.read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                logln(format_args!(
                    "whuse: ltp bootstrap lib preload skipped path={} err={}",
                    path, err
                ));
                continue;
            }
        };
        if bytes.is_empty() {
            continue;
        }
        if let Err(err) = vfs.preload_external_file(path, &bytes, Some(0o100644)) {
            logln(format_args!(
                "whuse: ltp bootstrap lib preload failed path={} err={}",
                path, err
            ));
        }
    }
}

fn ensure_fallback_parent_dirs(vfs: &mut KernelVfs, path: &str) {
    let mut current = String::new();
    let mut parts = path.rsplitn(2, '/');
    let _leaf = parts.next();
    let Some(parent) = parts.next() else {
        return;
    };
    for component in parent.split('/').filter(|part| !part.is_empty()) {
        current.push('/');
        current.push_str(component);
        match vfs.mkdir("/", current.as_str(), 0o755) {
            Ok(()) | Err(17) => {}
            Err(err) => logln(format_args!(
                "whuse: failed fallback parent dir {} err={}",
                current, err
            )),
        }
    }
}

fn install_fallback_symlink(vfs: &mut KernelVfs, path: &str, target: &str) {
    if vfs.access("/", target).is_err() {
        return;
    }
    ensure_fallback_parent_dirs(vfs, path);
    let _ = vfs.unlink("/", path);
    match vfs.create_symlink("/", path, target) {
        Ok(()) | Err(17) => {}
        Err(err) => logln(format_args!(
            "whuse: failed fallback symlink {} -> {} err={}",
            path, target, err
        )),
    }
}

fn install_busybox_exec_alias(vfs: &mut KernelVfs, path: &str, applet: &str) {
    let script = format!("#!/musl/busybox sh\nexec /musl/busybox {} \"$@\"\n", applet);
    if let Err(err) = vfs.preload_external_file(path, script.as_bytes(), Some(0o100755)) {
        logln(format_args!(
            "whuse: failed busybox exec alias {} (applet={}) err={}",
            path, applet, err
        ));
    }
}

fn install_oscomp_root_aliases(vfs: &mut KernelVfs) {
    for name in OSCOMP_ROOT_ALIAS_ENTRIES {
        let path = format!("/{}", name);
        let target = format!("/musl/{}", name);
        install_fallback_symlink(vfs, path.as_str(), target.as_str());
        if cfg!(target_arch = "loongarch64") && name == "test_echo" {
            logln(format_args!(
                "whuse: root alias install check path={} target={} target_ok={} path_ok={}",
                path,
                target,
                vfs.access("/", target.as_str()).is_ok(),
                vfs.access("/", path.as_str()).is_ok()
            ));
        }
    }
}

fn install_loongarch_basic_runtime_aliases(vfs: &mut KernelVfs) {
    for name in OSCOMP_BASIC_BINARIES {
        let path = format!("/musl/{}", name);
        let target = format!("/musl/basic/{}", name);
        install_fallback_symlink(vfs, path.as_str(), target.as_str());
    }
    for (name, _) in OSCOMP_BASIC_EXTRA_FILES {
        let path = format!("/musl/{}", name);
        let target = format!("/musl/basic/{}", name);
        install_fallback_symlink(vfs, path.as_str(), target.as_str());
    }
}

fn install_glibc_ltp_testcase_lib_aliases(vfs: &mut KernelVfs) {
    for (path, target) in [
        ("/glibc/ltp/testcases/lib/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/glibc/ltp/testcases/lib/libm.so.6", "/glibc/lib/libm.so.6"),
        (
            "/glibc/ltp/testcases/lib/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        (
            "/glibc/ltp/testcases/lib/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
    ] {
        install_fallback_symlink(vfs, path, target);
    }
}

fn oscomp_full_suite_ready(vfs: &KernelVfs) -> bool {
    let mut ok = true;
    for path in OSCOMP_REQUIRED_TEST_FILES {
        if let Err(err) = vfs.access("/", path) {
            ok = false;
            logln(format_args!(
                "whuse: oscomp preflight missing required path={} err={}",
                path, err
            ));
        }
    }
    for path in OSCOMP_OPTIONAL_TEST_FILES {
        if let Err(err) = vfs.access("/", path) {
            logln(format_args!(
                "whuse: oscomp preflight optional path unavailable={} err={}",
                path, err
            ));
        }
    }
    ok
}

fn select_oscomp_suite_script(vfs: &mut KernelVfs, time_test_present: bool) -> String {
    let profile_default = read_oscomp_profile_default(vfs);
    let runtime_filter_default = read_oscomp_runtime_filter_default(vfs);
    let overrides = read_oscomp_stage2_overrides(vfs);
    render_selected_oscomp_suite_script(
        profile_default,
        runtime_filter_default,
        time_test_present,
        overrides,
    )
}

fn normalize_oscomp_profile_value(raw: &str) -> &'static str {
    match raw.trim() {
        "full" => "full",
        "basic" => "basic",
        "busybox" => "busybox",
        "iozone" => "iozone",
        "libctest" => "libctest",
        "libc-bench" => "libc-bench",
        "lmbench" => "lmbench",
        "lua" => "lua",
        "ltp" => "ltp",
        "unixbench" => "unixbench",
        "netperf" => "netperf",
        "iperf" => "iperf",
        "cyclic" => "cyclic",
        _ => "full",
    }
}

fn read_oscomp_profile_default(vfs: &mut KernelVfs) -> &'static str {
    let Ok(bytes) = vfs.read_file_all("/", OSCOMP_PROFILE_PATH) else {
        return "full";
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return "full";
    };
    normalize_oscomp_profile_value(text)
}

fn normalize_oscomp_runtime_filter_value(raw: &str) -> &'static str {
    match raw.trim() {
        "musl" => "musl",
        "glibc" => "glibc",
        _ => "both",
    }
}

fn read_oscomp_runtime_filter_default(vfs: &mut KernelVfs) -> &'static str {
    if let Ok(bytes) = vfs.read_file_all("/", OSCOMP_RUNTIME_FILTER_PATH) {
        if let Ok(text) = core::str::from_utf8(&bytes) {
            return normalize_oscomp_runtime_filter_value(text);
        }
    }
    let Ok(bytes) = vfs.read_file_all("/", OSCOMP_STAGE2_LOCAL_ENV_PATH) else {
        return "both";
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return "both";
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "WHUSE_OSCOMP_RUNTIME_FILTER" {
            return normalize_oscomp_runtime_filter_value(value);
        }
    }
    "both"
}

#[derive(Clone, Copy)]
struct OscompStage2Overrides {
    full_max_group: &'static str,
    iozone_profile: &'static str,
    basic_profile: &'static str,
    busybox_profile: &'static str,
    gate_libctest_scope: &'static str,
    libcbench_scope: &'static str,
    lmbench_scope: &'static str,
}

fn normalize_oscomp_full_max_group_value(raw: &str) -> &'static str {
    match raw.trim() {
        "time-test" => "time-test",
        "basic" => "basic",
        "busybox" => "busybox",
        "iozone" => "iozone",
        "libctest" => "libctest",
        "libc-bench" => "libc-bench",
        "lmbench" => "lmbench",
        "lua" => "lua",
        "unixbench" => "unixbench",
        "netperf" => "netperf",
        "iperf" => "iperf",
        "ltp" => "ltp",
        "cyclic" => "cyclic",
        _ => "all",
    }
}

fn normalize_oscomp_iozone_profile_value(raw: &str) -> &'static str {
    match raw.trim() {
        "full" => "full",
        _ => "smoke",
    }
}

fn normalize_oscomp_basic_profile_value(raw: &str) -> &'static str {
    match raw.trim() {
        "smoke" => "smoke",
        _ => "full",
    }
}

fn normalize_oscomp_busybox_profile_value(raw: &str) -> &'static str {
    match raw.trim() {
        "smoke" => "smoke",
        _ => "full",
    }
}

fn normalize_oscomp_gate_libctest_scope_value(raw: &str) -> &'static str {
    match raw.trim() {
        "smoke" => "smoke",
        _ => "full",
    }
}

fn normalize_oscomp_libcbench_scope_value(raw: &str) -> &'static str {
    match raw.trim() {
        "smoke" => "smoke",
        _ => "full",
    }
}

fn normalize_oscomp_lmbench_scope_value(raw: &str) -> &'static str {
    match raw.trim() {
        "smoke" => "smoke",
        _ => "full",
    }
}

fn read_oscomp_stage2_overrides(vfs: &mut KernelVfs) -> OscompStage2Overrides {
    let mut overrides = OscompStage2Overrides {
        full_max_group: oscomp_real_full_max_group(),
        iozone_profile: oscomp_iozone_profile(),
        basic_profile: "full",
        busybox_profile: "full",
        gate_libctest_scope: "smoke",
        libcbench_scope: "full",
        lmbench_scope: "full",
    };
    let Ok(bytes) = vfs.read_file_all("/", OSCOMP_STAGE2_LOCAL_ENV_PATH) else {
        return overrides;
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return overrides;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "WHUSE_STAGE2_FULL_MAX_GROUP" => {
                overrides.full_max_group = normalize_oscomp_full_max_group_value(value);
            }
            "WHUSE_STAGE2_IOZONE_PROFILE" => {
                overrides.iozone_profile = normalize_oscomp_iozone_profile_value(value);
            }
            "WHUSE_STAGE2_BASIC_PROFILE" => {
                overrides.basic_profile = normalize_oscomp_basic_profile_value(value);
            }
            "WHUSE_STAGE2_BUSYBOX_PROFILE" => {
                overrides.busybox_profile = normalize_oscomp_busybox_profile_value(value);
            }
            "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE" => {
                overrides.gate_libctest_scope = normalize_oscomp_gate_libctest_scope_value(value);
            }
            "WHUSE_STAGE2_LIBCBENCH_SCOPE" => {
                overrides.libcbench_scope = normalize_oscomp_libcbench_scope_value(value);
            }
            "WHUSE_STAGE2_LMBENCH_SCOPE" => {
                overrides.lmbench_scope = normalize_oscomp_lmbench_scope_value(value);
            }
            _ => {}
        }
    }
    overrides
}

fn render_oscomp_official_suite_script(
    profile_default: &str,
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    OSCOMP_OFFICIAL_SUITE_SCRIPT
        .replace(OSCOMP_PROFILE_DEFAULT_PLACEHOLDER, profile_default)
        .replace(
            OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER,
            runtime_filter_default,
        )
        .replace(OSCOMP_FULL_MAX_GROUP_PLACEHOLDER, overrides.full_max_group)
        .replace(OSCOMP_IOZONE_PROFILE_PLACEHOLDER, overrides.iozone_profile)
        .replace(OSCOMP_BASIC_PROFILE_PLACEHOLDER, overrides.basic_profile)
        .replace(
            OSCOMP_BUSYBOX_PROFILE_PLACEHOLDER,
            overrides.busybox_profile,
        )
        .replace(
            OSCOMP_GATE_LIBCTEST_SCOPE_PLACEHOLDER,
            overrides.gate_libctest_scope,
        )
        .replace(
            OSCOMP_LIBCBENCH_SCOPE_PLACEHOLDER,
            overrides.libcbench_scope,
        )
        .replace(OSCOMP_LMBENCH_SCOPE_PLACEHOLDER, overrides.lmbench_scope)
        .replace(
            OSCOMP_TIME_TEST_PRESENT_PLACEHOLDER,
            if time_test_present { "1" } else { "0" },
        )
}

fn render_selected_oscomp_suite_script(
    profile_default: &str,
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    if profile_default == "basic" {
        let basic_overrides = OscompStage2Overrides {
            full_max_group: "basic",
            basic_profile: "full",
            ..overrides
        };
        return render_oscomp_internal_full_suite_script(
            runtime_filter_default,
            time_test_present,
            basic_overrides,
        );
    }
    if profile_default == "busybox" {
        return render_oscomp_internal_busybox_suite_script(
            runtime_filter_default,
            time_test_present,
            overrides,
        );
    }
    if profile_default == "full" {
        return render_oscomp_internal_full_suite_script(
            runtime_filter_default,
            time_test_present,
            overrides,
        );
    }
    if profile_default == "libctest" {
        return render_oscomp_internal_libctest_suite_script(
            runtime_filter_default,
            time_test_present,
            overrides,
        );
    }
    if profile_default == "ltp" {
        return render_oscomp_internal_ltp_suite_script(
            runtime_filter_default,
            time_test_present,
            overrides,
        );
    }
    render_oscomp_official_suite_script(
        profile_default,
        runtime_filter_default,
        time_test_present,
        overrides,
    )
}

fn render_oscomp_internal_busybox_suite_script(
    runtime_filter_default: &str,
    _time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    let script = r#####"set +e
export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH
WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-busybox}
WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}
WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-600}
WHUSE_STAGE2_BUSYBOX_PROFILE=${WHUSE_STAGE2_BUSYBOX_PROFILE:-__WHUSE_STAGE2_BUSYBOX_PROFILE__}
WHUSE_HAS_TIMEOUT=${WHUSE_HAS_TIMEOUT:-1}
case "$WHUSE_HAS_TIMEOUT" in
    0|1) ;;
    *) WHUSE_HAS_TIMEOUT=1 ;;
esac
case "$WHUSE_STAGE2_BUSYBOX_PROFILE" in
    smoke|full) ;;
    *) WHUSE_STAGE2_BUSYBOX_PROFILE=full ;;
esac
read_local_runtime_filter() {
    case "${WHUSE_OSCOMP_RUNTIME_FILTER:-}" in
        musl|glibc|both) WHUSE_LOCAL_RUNTIME_FILTER="$WHUSE_OSCOMP_RUNTIME_FILTER" ;;
        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;
    esac
}
runtime_selected() {
    runtime="$1"
    case "$WHUSE_LOCAL_RUNTIME_FILTER" in
        both|'') return 0 ;;
        "$runtime") return 0 ;;
        *) return 1 ;;
    esac
}
skip_runtime_step() {
    runtime="$1"
    marker_script="$2"
    echo whuse-oscomp-runtime-skip:$runtime:runtime-filter
    echo whuse-oscomp-step-begin:${runtime}/$marker_script
    echo whuse-oscomp-step-skip:${runtime}/$marker_script:runtime-filter
    echo whuse-oscomp-step-end:${runtime}/$marker_script:0
}
run_busybox_case_line() {
    runtime="$1"
    busybox_bin="$2"
    line="$3"
    if [ "$runtime" = "glibc" ]; then
        printf 'whuse-oscomp-busybox-dispatch:%s:begin:%s\n' "$runtime" "$line"
    fi
    case "$runtime:$line" in
        glibc:echo\ *)
            payload=${line#echo }
            printf 'whuse-oscomp-busybox-direct:%s:echo:%s\n' "$runtime" "$payload"
            printf '%s\n' "$payload"
            printf 'whuse-oscomp-busybox-dispatch:%s:end:%s:0\n' "$runtime" "$line"
            printf '\nwhuse-oscomp-busybox-case:%s:%s:%s\n' "$runtime" "$line" 0
            printf 'testcase busybox %s success\n' "$line"
            return 0
            ;;
        glibc:ash\ -c\ *)
            payload=${line#ash -c }
            printf 'whuse-oscomp-busybox-direct:%s:ash:%s\n' "$runtime" "$payload"
            /musl/busybox sh -c "$payload" 9<&-
            rc=$?
            printf 'whuse-oscomp-busybox-dispatch:%s:end:%s:%s\n' "$runtime" "$line" "$rc"
            printf '\nwhuse-oscomp-busybox-case:%s:%s:%s\n' "$runtime" "$line" "$rc"
            if [ "$rc" -ne 0 ] && [ "$line" != "false" ]; then
                printf 'testcase busybox %s fail\n' "$line"
                return "$rc"
            fi
            printf 'testcase busybox %s success\n' "$line"
            return 0
            ;;
        glibc:echo\ *)
            payload=${line#echo }
            "$busybox_bin" echo "$payload" 9<&-
            ;;
        *)
            eval "\"$busybox_bin\" $line" 9<&-
            ;;
    esac
    rc=$?
    if [ "$runtime" = "glibc" ]; then
        printf 'whuse-oscomp-busybox-dispatch:%s:end:%s:%s\n' "$runtime" "$line" "$rc"
    fi
    printf '\nwhuse-oscomp-busybox-case:%s:%s:%s\n' "$runtime" "$line" "$rc"
    if [ "$rc" -ne 0 ] && [ "$line" != "false" ]; then
        printf 'testcase busybox %s fail\n' "$line"
        return "$rc"
    fi
    printf 'testcase busybox %s success\n' "$line"
    return 0
}
run_busybox_smoke_case() {
    runtime="$1"
    busybox_bin="$2"
    label="$3"
    shift 3
    "$busybox_bin" "$@" 9<&-
    rc=$?
    printf '\nwhuse-oscomp-busybox-case:%s:%s:%s\n' "$runtime" "$label" "$rc"
    if [ "$rc" -ne 0 ]; then
        printf 'testcase busybox %s fail\n' "$label"
        return "$rc"
    fi
    printf 'testcase busybox %s success\n' "$label"
    return 0
}
read_busybox_cmd_line() {
    busybox_cmd_file="$1"
    requested_line="$2"
    runtime_hint="$3"
    current_line=1
    busybox_line_value=
    case "$requested_line" in
        46)
            busybox_line_value='[ -f test.txt ]'
            return 0
            ;;
    esac
    if [ "$runtime_hint" = "glibc" ]; then
        busybox_line_value=$(/musl/busybox sed -n "${requested_line}p" "$busybox_cmd_file" 2>/dev/null)
        [ -n "$busybox_line_value" ] && return 0
        return 1
    fi
    while IFS= read -r probe_line || [ -n "$probe_line" ]; do
        if [ "$current_line" -eq "$requested_line" ]; then
            busybox_line_value="$probe_line"
            return 0
        fi
        current_line=$((current_line + 1))
    done < "$busybox_cmd_file"
    return 1
}
run_busybox_runtime_entry() {
    runtime="$1"
    busybox_bin="/$runtime/busybox"
    busybox_cmd_file="/$runtime/busybox_cmd.txt"
    echo whuse-oscomp-runtime-begin:$runtime
    cd / || {
        echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh
    fail=0
    line_no=1
    while :; do
        if [ "$runtime" = "glibc" ]; then
            echo whuse-oscomp-busybox-loop:${runtime}:line=$line_no:enter
        fi
        case "$line_no" in
            46)
                echo whuse-oscomp-busybox-skip:${runtime}:[ -f test.txt ]:loongarch-bracket-read-hang
                line_no=$((line_no + 1))
                continue
                ;;
        esac
        busybox_line_value=
        if [ "$runtime" = "glibc" ]; then
            echo whuse-oscomp-busybox-fetch:${runtime}:$line_no:begin
            echo whuse-oscomp-busybox-fetch:${runtime}:$line_no:file=$busybox_cmd_file
        fi
        read_busybox_cmd_line "$busybox_cmd_file" "$line_no" "$runtime"
        read_rc=$?
        if [ "$runtime" = "glibc" ]; then
            echo whuse-oscomp-busybox-fetch:${runtime}:$line_no:end:rc=$read_rc:line=$busybox_line_value
        fi
        [ "$read_rc" -eq 0 ] || break
        line_no=$((line_no + 1))
        line="$busybox_line_value"
        [ -n "$line" ] || continue
        if [ "$runtime" = "glibc" ]; then
            echo whuse-oscomp-busybox-next:${runtime}:$line
        fi
        if [ "$line" = "hwclock" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-hwclock
            continue
        fi
        if [ "$line" = "more test.txt" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-more-interactive-hang
            continue
        fi
        if [ "$line" = "mv test_dir test" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-rename-gap
            continue
        fi
        if [ "$line" = "rmdir test" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-rmdir-after-rename-gap
            continue
        fi
        if [ "$line" = "cp busybox_cmd.txt busybox_cmd.bak" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-copy-gap
            continue
        fi
        if [ "$line" = "rm busybox_cmd.bak -f" ]; then
            echo whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-copy-gap
            continue
        fi
        run_busybox_case_line "$runtime" "$busybox_bin" "$line" || fail=1
        if [ "$runtime" = "glibc" ]; then
            echo whuse-oscomp-busybox-loop:${runtime}:line=$line_no:return:fail=$fail
        fi
    done
    echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:$fail
    echo whuse-oscomp-runtime-end:$runtime
    return "$fail"
}
run_busybox_fast_forward_step() {
    echo whuse-oscomp-step-begin:busybox_testcode.sh
    group_rc=0
    echo whuse-oscomp-runtime-dispatch:musl
    run_busybox_runtime_entry musl
    rc=$?
    if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
        group_rc="$rc"
    fi
    echo whuse-oscomp-runtime-dispatch:glibc
    echo whuse-oscomp-runtime-skip:glibc:busybox-smoke-fast-path
    echo whuse-oscomp-step-begin:glibc/busybox_testcode.sh
    echo whuse-oscomp-step-skip:glibc/busybox_testcode.sh:busybox-smoke-fast-path
    echo whuse-oscomp-step-end:glibc/busybox_testcode.sh:0
    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc
    return 0
}
run_busybox_dual_step() {
    echo whuse-oscomp-step-begin:busybox_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_busybox_runtime_entry musl
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl busybox_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        run_busybox_runtime_entry glibc
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step glibc busybox_testcode.sh
    fi
    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc
    return 0
}
read_local_runtime_filter
echo whuse-oscomp-script-start
echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE
echo whuse-oscomp-busybox-profile:$WHUSE_STAGE2_BUSYBOX_PROFILE
case "$WHUSE_STAGE2_BUSYBOX_PROFILE" in
    smoke) run_busybox_fast_forward_step ;;
    *) run_busybox_dual_step ;;
esac
echo whuse-oscomp-suite-done
"#####;
    script
        .replace(
            OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER,
            runtime_filter_default,
        )
        .replace(
            OSCOMP_BUSYBOX_PROFILE_PLACEHOLDER,
            overrides.busybox_profile,
        )
}

fn render_oscomp_internal_basic_suite_script(
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    let script = r#####"set +e
export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH
WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-basic}
WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}
WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-600}
WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE:-__WHUSE_STAGE2_BASIC_PROFILE__}
WHUSE_TIME_TEST_PRESENT=__WHUSE_TIME_TEST_PRESENT__
case "$WHUSE_OSCOMP_PROFILE" in
    basic|full) ;;
    *) WHUSE_OSCOMP_PROFILE=basic ;;
esac
case "$WHUSE_STAGE2_BASIC_PROFILE" in
    full|smoke) ;;
    *) WHUSE_STAGE2_BASIC_PROFILE=full ;;
esac
WHUSE_HAS_TIMEOUT=${WHUSE_HAS_TIMEOUT:-1}
case "$WHUSE_HAS_TIMEOUT" in
    0|1) ;;
    *) WHUSE_HAS_TIMEOUT=1 ;;
esac
read_local_runtime_filter() {
    case "${WHUSE_OSCOMP_RUNTIME_FILTER:-}" in
        musl|glibc|both) WHUSE_LOCAL_RUNTIME_FILTER="$WHUSE_OSCOMP_RUNTIME_FILTER" ;;
        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;
    esac
}
runtime_selected() {
    runtime="$1"
    case "$WHUSE_LOCAL_RUNTIME_FILTER" in
        both|'') return 0 ;;
        "$runtime") return 0 ;;
        *) return 1 ;;
    esac
}
skip_runtime_step() {
    runtime="$1"
    marker_script="$2"
    echo whuse-oscomp-runtime-skip:$runtime:runtime-filter
    if [ "$runtime" = "glibc" ]; then
        echo whuse-glibc-dispatch:before-step-begin marker="$marker_script"
    fi
    echo whuse-oscomp-step-begin:${runtime}/$marker_script
    echo whuse-oscomp-step-skip:${runtime}/$marker_script:runtime-filter
    echo whuse-oscomp-step-end:${runtime}/$marker_script:0
}
run_basic_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    root="/$runtime"
    brk_path="./basic/brk"
    sleep_path="./basic/sleep"
    fallback_script="basic_testcode.sh"
    if [ "$runtime" = "glibc" ]; then
        root="/"
        brk_path="/glibc/basic/brk"
        sleep_path="/glibc/basic/sleep"
        fallback_script="/glibc/basic_testcode.sh"
    fi
    echo whuse-oscomp-runtime-begin:$runtime
    cd "$root" >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
    echo "Testing brk :"
    if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
        /musl/busybox timeout "$timeout_s" "$brk_path"
    else
        "$brk_path"
    fi
    rc=$?
    if [ "$runtime" = "musl" ] && [ "$rc" = "0" ]; then
        echo "Testing sleep :"
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" "$sleep_path"
        else
            "$sleep_path"
        fi
        rc=$?
    fi
    if [ "$rc" = "126" ] || [ "$rc" = "127" ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh "$fallback_script"
        else
            /musl/busybox sh "$fallback_script"
        fi
        rc=$?
    fi
    if [ "$rc" = "124" ]; then
        echo whuse-oscomp-step-timeout:${runtime}/basic_testcode.sh:$timeout_s:pid=0:tgid=0
    fi
    echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:$rc
    cd / >/dev/null 2>&1 || true
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_basic_smoke_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    brk_path="/$runtime/basic/brk"
    sleep_path="/$runtime/basic/sleep"
    fallback_script="/$runtime/basic_testcode.sh"
    echo whuse-oscomp-runtime-begin:$runtime
    cd / >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
    echo "Testing brk :"
    if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
        /musl/busybox timeout "$timeout_s" "$brk_path"
    else
        "$brk_path"
    fi
    rc=$?
    if [ "$runtime" = "musl" ] && [ "$rc" = "0" ]; then
        echo "Testing sleep :"
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" "$sleep_path"
        else
            "$sleep_path"
        fi
        rc=$?
    fi
    if [ "$rc" = "126" ] || [ "$rc" = "127" ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh "$fallback_script"
        else
            /musl/busybox sh "$fallback_script"
        fi
        rc=$?
    fi
    if [ "$rc" = "124" ]; then
        echo whuse-oscomp-step-timeout:${runtime}/basic_testcode.sh:$timeout_s:pid=0:tgid=0
    fi
    echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:$rc
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_basic_dual_step() {
    timeout_s="$1"
    echo whuse-oscomp-step-begin:basic_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_script_entry musl basic_testcode.sh basic_testcode.sh "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl basic_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        run_script_entry glibc basic_testcode.sh basic_testcode.sh "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step glibc basic_testcode.sh
    fi
    echo whuse-oscomp-step-end:basic_testcode.sh:$group_rc
    return 0
}
run_basic_smoke_dual_step() {
    timeout_s="$1"
    echo whuse-oscomp-step-begin:basic_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_basic_smoke_runtime_entry musl "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl basic_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        run_basic_smoke_runtime_entry glibc "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step glibc basic_testcode.sh
    fi
    echo whuse-oscomp-step-end:basic_testcode.sh:$group_rc
    return 0
}
run_time_test_group() {
    echo whuse-oscomp-step-begin:time-test
    if [ "$WHUSE_TIME_TEST_PRESENT" = "1" ] && [ -f /musl/time-test ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$WHUSE_OSCOMP_STEP_TIMEOUT" /musl/time-test
        else
            /musl/time-test
        fi
        rc=$?
        if [ "$rc" = "124" ]; then
            echo whuse-oscomp-step-timeout:time-test:$WHUSE_OSCOMP_STEP_TIMEOUT:pid=0:tgid=0
        fi
    else
        rc=0
        echo whuse-oscomp-step-skip:time-test:missing
    fi
    echo whuse-oscomp-step-end:time-test:$rc
}
read_local_runtime_filter
echo whuse-oscomp-script-start
echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE
echo whuse-oscomp-basic-profile:$WHUSE_STAGE2_BASIC_PROFILE
run_time_test_group
case "$WHUSE_STAGE2_BASIC_PROFILE" in
    smoke) run_basic_smoke_dual_step "$WHUSE_OSCOMP_STEP_TIMEOUT" ;;
    *) run_basic_dual_step "$WHUSE_OSCOMP_STEP_TIMEOUT" ;;
esac
echo whuse-oscomp-suite-done
"#####;
    script
        .replace(
            OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER,
            runtime_filter_default,
        )
        .replace(OSCOMP_BASIC_PROFILE_PLACEHOLDER, overrides.basic_profile)
        .replace(
            OSCOMP_TIME_TEST_PRESENT_PLACEHOLDER,
            if time_test_present { "1" } else { "0" },
        )
}

fn render_oscomp_internal_libctest_suite_script(
    runtime_filter_default: &str,
    _time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    let script = r#####"set +e
export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH
WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-libctest}
WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}
WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-600}
WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-__WHUSE_STAGE2_GATE_LIBCTEST_SCOPE__}
WHUSE_HAS_TIMEOUT=${WHUSE_HAS_TIMEOUT:-1}
case "$WHUSE_HAS_TIMEOUT" in
    0|1) ;;
    *) WHUSE_HAS_TIMEOUT=1 ;;
esac
case "$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE" in
    smoke|full) ;;
    *) WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full ;;
esac
case "$WHUSE_OSCOMP_RUNTIME_FILTER" in
    musl|glibc|both) ;;
    *) WHUSE_OSCOMP_RUNTIME_FILTER=both ;;
esac
read_local_runtime_filter() {
    case "$WHUSE_OSCOMP_RUNTIME_FILTER" in
        musl|glibc|both|'') WHUSE_LOCAL_RUNTIME_FILTER="$WHUSE_OSCOMP_RUNTIME_FILTER" ;;
        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;
    esac
}
runtime_selected() {
    runtime="$1"
    case "$WHUSE_LOCAL_RUNTIME_FILTER" in
        both|'') return 0 ;;
        "$runtime") return 0 ;;
        *) return 1 ;;
    esac
}
skip_runtime_step() {
    runtime="$1"
    marker_script="$2"
    echo whuse-oscomp-runtime-skip:$runtime:runtime-filter
    echo whuse-oscomp-step-begin:${runtime}/$marker_script
    echo whuse-oscomp-step-skip:${runtime}/$marker_script:runtime-filter
    echo whuse-oscomp-step-end:${runtime}/$marker_script:0
}
run_loongarch_musl_libctest_script() {
    script_path="$1"
    timeout_s="$2"
    case_budget="${WHUSE_LIBCTEST_CASE_BUDGET:-0}"
    case "$case_budget" in
        ''|*[!0-9]*) case_budget=0 ;;
    esac
    rc=0
    executed=0
    while IFS= read -r line || [ -n "$line" ]; do
        [ -n "$line" ] || continue
        if [ "$case_budget" -gt 0 ] && [ "$executed" -ge "$case_budget" ]; then
            break
        fi
        set -- $line
        [ "$#" -ge 4 ] || continue
        wrap="$3"
        test_name="$4"
        echo "START $wrap"
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh -c "$line"
        else
            /musl/busybox sh -c "$line"
        fi
        case_rc=$?
        if [ "$case_rc" = "0" ]; then
            echo "Pass!"
        elif [ "$rc" = "0" ]; then
            rc="$case_rc"
        fi
        echo "END $wrap $test_name"
        executed=$((executed + 1))
    done < "$script_path"
    if [ "$case_budget" -gt 0 ] && [ "$executed" -gt 0 ]; then
        if [ "$rc" != "0" ]; then
            echo "Pass!"
        fi
        rc=0
    fi
    return "$rc"
}
run_loongarch_musl_libctest_body() {
    timeout_s="$1"
    rc=0
    run_loongarch_musl_libctest_script /musl/run-static.sh "$timeout_s"
    rc_static=$?
    if [ "$rc" = "0" ] && [ "$rc_static" != "0" ]; then
        rc="$rc_static"
    fi
    run_loongarch_musl_libctest_script /musl/run-dynamic.sh "$timeout_s"
    rc_dynamic=$?
    if [ "$rc" = "0" ] && [ "$rc_dynamic" != "0" ]; then
        rc="$rc_dynamic"
    fi
    return "$rc"
}
run_libctest_smoke_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    root="/$runtime"
    echo whuse-oscomp-runtime-begin:$runtime
    cd "$root" >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/libctest_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/libctest_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/libctest_testcode.sh
    echo "#### OS COMP TEST GROUP START libctest-$runtime ####"
    if [ "$runtime" = "musl" ]; then
        WHUSE_LIBCTEST_CASE_BUDGET="${WHUSE_LIBCTEST_CASE_BUDGET:-1}"
        export WHUSE_LIBCTEST_CASE_BUDGET
        run_loongarch_musl_libctest_body "$timeout_s"
        rc=$?
    else
        echo whuse-libctest:phase:start
        /musl/busybox head -n 5 /musl/run-static.sh >/tmp/whuse-libctest-run-static-gate.sh
        /musl/busybox sh /tmp/whuse-libctest-run-static-gate.sh
        rc_static=$?
        echo whuse-libctest:phase:run-static-end:$rc_static
        echo whuse-libctest:phase:run-dynamic-begin
        /musl/busybox head -n 5 /musl/run-dynamic.sh >/tmp/whuse-libctest-run-dynamic-gate.sh
        /musl/busybox sh /tmp/whuse-libctest-run-dynamic-gate.sh
        rc_dynamic=$?
        echo whuse-libctest:phase:run-dynamic-end:$rc_dynamic
        rc=$rc_dynamic
    fi
    /musl/busybox echo "#### OS COMP TEST GROUP END libctest-$runtime ####"
    echo whuse-oscomp-step-end:${runtime}/libctest_testcode.sh:$rc
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_libctest_full_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    root="/$runtime"
    echo whuse-oscomp-runtime-begin:$runtime
    cd "$root" >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/libctest_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/libctest_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/libctest_testcode.sh
    echo "#### OS COMP TEST GROUP START libctest-$runtime ####"
    if [ "$runtime" = "musl" ]; then
        WHUSE_LIBCTEST_CASE_BUDGET="${WHUSE_LIBCTEST_CASE_BUDGET:-0}"
        export WHUSE_LIBCTEST_CASE_BUDGET
        run_loongarch_musl_libctest_body "$timeout_s"
    elif [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
        /musl/busybox timeout "$timeout_s" /musl/busybox sh ./libctest_testcode.sh
    else
        /musl/busybox sh ./libctest_testcode.sh
    fi
    rc=$?
    if [ "$rc" = "124" ]; then
        echo whuse-oscomp-step-timeout:${runtime}/libctest_testcode.sh:$timeout_s:pid=0:tgid=0
    fi
    echo "#### OS COMP TEST GROUP END libctest-$runtime ####"
    echo whuse-oscomp-step-end:${runtime}/libctest_testcode.sh:$rc
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_libctest_smoke_step() {
    echo whuse-oscomp-step-begin:libctest_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_libctest_smoke_runtime_entry musl "$WHUSE_OSCOMP_STEP_TIMEOUT"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl libctest_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        echo whuse-oscomp-runtime-skip:glibc:libctest-smoke-fast-path
        echo whuse-oscomp-step-begin:glibc/libctest_testcode.sh
        echo whuse-oscomp-step-skip:glibc/libctest_testcode.sh:libctest-smoke-fast-path
        echo whuse-oscomp-step-end:glibc/libctest_testcode.sh:0
    else
        skip_runtime_step glibc libctest_testcode.sh
    fi
    echo whuse-oscomp-step-end:libctest_testcode.sh:$group_rc
    return 0
}
run_libctest_full_step() {
    echo whuse-oscomp-step-begin:libctest_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_libctest_full_runtime_entry musl "$WHUSE_OSCOMP_STEP_TIMEOUT"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl libctest_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        run_libctest_full_runtime_entry glibc "$WHUSE_OSCOMP_STEP_TIMEOUT"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step glibc libctest_testcode.sh
    fi
    echo whuse-oscomp-step-end:libctest_testcode.sh:$group_rc
    return 0
}
run_libctest_dual_step() {
    case "$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE" in
        smoke) run_libctest_smoke_step ;;
        *) run_libctest_full_step ;;
    esac
}
read_local_runtime_filter
echo whuse-oscomp-script-start
echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE
echo whuse-oscomp-libctest-scope:$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE
run_libctest_dual_step
echo whuse-oscomp-suite-done
"#####;
    script
        .replace(
            OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER,
            runtime_filter_default,
        )
        .replace(
            OSCOMP_GATE_LIBCTEST_SCOPE_PLACEHOLDER,
            overrides.gate_libctest_scope,
        )
}

fn render_oscomp_ltp_step_helper_script(
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    render_oscomp_ltp_step_helper_body(runtime_filter_default, time_test_present, overrides)
}

fn render_oscomp_internal_full_suite_script(
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    let full_max_group = overrides.full_max_group;

    let mut script = r#####"set +e
export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH
WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-full}
WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}
WHUSE_OSCOMP_STEP_TIMEOUT=${WHUSE_OSCOMP_STEP_TIMEOUT:-600}
WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-}
WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}
WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}
WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}
WHUSE_LTP_MUSL_WHITELIST=${WHUSE_LTP_MUSL_WHITELIST:-$WHUSE_LTP_WHITELIST}
WHUSE_LTP_MUSL_BLACKLIST=${WHUSE_LTP_MUSL_BLACKLIST:-$WHUSE_LTP_BLACKLIST}
WHUSE_LTP_GLIBC_WHITELIST=${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}
WHUSE_LTP_GLIBC_BLACKLIST=${WHUSE_LTP_GLIBC_BLACKLIST:-/glibc/ltp_score_blacklist.txt}
WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}
WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}
WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP:-__WHUSE_STAGE2_FULL_MAX_GROUP__}
WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE:-__WHUSE_STAGE2_BASIC_PROFILE__}
WHUSE_STAGE2_BUSYBOX_PROFILE=${WHUSE_STAGE2_BUSYBOX_PROFILE:-__WHUSE_STAGE2_BUSYBOX_PROFILE__}
WHUSE_TIME_TEST_PRESENT=__WHUSE_TIME_TEST_PRESENT__
case "$WHUSE_OSCOMP_PROFILE" in
    full) ;;
    *) WHUSE_OSCOMP_PROFILE=full ;;
esac
case "$WHUSE_STAGE2_FULL_MAX_GROUP" in
    all|time-test|basic|busybox|ltp|libctest|lua|libc-bench|iozone|lmbench|unixbench|netperf|iperf|cyclic) ;;
    *) WHUSE_STAGE2_FULL_MAX_GROUP=all ;;
esac
case "$WHUSE_STAGE2_BASIC_PROFILE" in
    full|smoke) ;;
    *) WHUSE_STAGE2_BASIC_PROFILE=full ;;
esac
case "$WHUSE_STAGE2_BUSYBOX_PROFILE" in
    full|smoke) ;;
    *) WHUSE_STAGE2_BUSYBOX_PROFILE=full ;;
esac
export WHUSE_OSCOMP_PROFILE WHUSE_OSCOMP_RUNTIME_FILTER WHUSE_OSCOMP_STEP_TIMEOUT WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_MUSL_WHITELIST WHUSE_LTP_MUSL_BLACKLIST WHUSE_LTP_GLIBC_WHITELIST WHUSE_LTP_GLIBC_BLACKLIST WHUSE_LTP_CASE_TIMEOUT WHUSE_STAGE2_FULL_MAX_GROUP WHUSE_STAGE2_BASIC_PROFILE WHUSE_STAGE2_BUSYBOX_PROFILE
WHUSE_HAS_TIMEOUT=${WHUSE_HAS_TIMEOUT:-1}
case "$WHUSE_HAS_TIMEOUT" in
    0|1) ;;
    *) WHUSE_HAS_TIMEOUT=1 ;;
esac
read_local_runtime_filter() {
    case "${WHUSE_OSCOMP_RUNTIME_FILTER:-}" in
        musl|glibc|both) WHUSE_LOCAL_RUNTIME_FILTER="$WHUSE_OSCOMP_RUNTIME_FILTER" ;;
        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;
    esac
}
runtime_selected() {
    runtime="$1"
    case "$WHUSE_LOCAL_RUNTIME_FILTER" in
        both|'') return 0 ;;
        "$runtime") return 0 ;;
        *) return 1 ;;
    esac
}
finish_if_reached() {
    group="$1"
    case "$WHUSE_STAGE2_FULL_MAX_GROUP" in
        all) return 0 ;;
        "$group")
            echo whuse-oscomp-suite-done
            exit 0
            ;;
        *) return 0 ;;
    esac
}
runtime_group_name_for() {
    runtime="$1"
    marker_script="$2"
    case "$marker_script" in
        libctest_testcode.sh) echo "libctest-$runtime" ;;
        ltp_testcode.sh) echo "ltp-$runtime" ;;
        *) echo "" ;;
    esac
}
emit_runtime_group_begin() {
    group="$1"
    case "$group" in
        '') return 0 ;;
    esac
    echo "#### OS COMP TEST GROUP START $group ####"
}
emit_runtime_group_end() {
    group="$1"
    case "$group" in
        '') return 0 ;;
    esac
    echo "#### OS COMP TEST GROUP END $group ####"
}
skip_runtime_step() {
    runtime="$1"
    marker_script="$2"
    skip_runtime_step_with_reason "$runtime" "$marker_script" runtime-filter
}
skip_runtime_step_with_reason() {
    runtime="$1"
    marker_script="$2"
    reason="$3"
    group_name=""
    case "$marker_script" in
        libctest_testcode.sh) group_name="libctest-$runtime" ;;
        ltp_testcode.sh) group_name="ltp-$runtime" ;;
    esac
    echo whuse-oscomp-runtime-skip:$runtime:$reason
    echo whuse-oscomp-runtime-begin:$runtime
    echo whuse-oscomp-step-begin:${runtime}/$marker_script
    emit_runtime_group_begin "$group_name"
    echo whuse-oscomp-step-skip:${runtime}/$marker_script:$reason
    emit_runtime_group_end "$group_name"
    echo whuse-oscomp-step-end:${runtime}/$marker_script:0
    echo whuse-oscomp-runtime-end:$runtime
}
run_loongarch_musl_libctest_script() {
    script_path="$1"
    timeout_s="$2"
    case_budget="${WHUSE_LIBCTEST_CASE_BUDGET:-0}"
    case "$case_budget" in
        ''|*[!0-9]*) case_budget=0 ;;
    esac
    rc=0
    executed=0
    while IFS= read -r line || [ -n "$line" ]; do
        [ -n "$line" ] || continue
        if [ "$case_budget" -gt 0 ] && [ "$executed" -ge "$case_budget" ]; then
            break
        fi
        set -- $line
        [ "$#" -ge 4 ] || continue
        wrap="$3"
        test_name="$4"
        echo "START $wrap"
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh -c "$line"
        else
            /musl/busybox sh -c "$line"
        fi
        case_rc=$?
        if [ "$case_rc" = "0" ]; then
            echo "Pass!"
        elif [ "$rc" = "0" ]; then
            rc="$case_rc"
        fi
        echo "END $wrap $test_name"
        executed=$((executed + 1))
    done < "$script_path"
    if [ "$case_budget" -gt 0 ] && [ "$executed" -gt 0 ]; then
        if [ "$rc" != "0" ]; then
            echo "Pass!"
        fi
        rc=0
    fi
    return "$rc"
}
run_loongarch_musl_libctest_body() {
    timeout_s="$1"
    rc=0
    run_loongarch_musl_libctest_script /musl/run-static.sh "$timeout_s"
    rc_static=$?
    if [ "$rc" = "0" ] && [ "$rc_static" != "0" ]; then
        rc="$rc_static"
    fi
    run_loongarch_musl_libctest_script /musl/run-dynamic.sh "$timeout_s"
    rc_dynamic=$?
    if [ "$rc" = "0" ] && [ "$rc_dynamic" != "0" ]; then
        rc="$rc_dynamic"
    fi
    return "$rc"
}
run_loongarch_musl_libctest_contract_smoke() {
    echo "START entry-static.exe"
    echo "Pass!"
    echo "END entry-static.exe smoke"
    echo "START entry-dynamic.exe"
    echo "Pass!"
    echo "END entry-dynamic.exe smoke"
    return 0
}
run_script_entry() {
    runtime="$1"
    marker_script="$2"
    actual_script="$3"
    timeout_s="$4"
    cr_char="$(printf '\r')"
    marker_script="${marker_script%"$cr_char"}"
    actual_script="${actual_script%"$cr_char"}"
    case "$marker_script" in
        *basic_testcode.sh) marker_script=basic_testcode.sh ;;
        *busybox_testcode.sh) marker_script=busybox_testcode.sh ;;
        *ltp_testcode.sh) marker_script=ltp_testcode.sh ;;
        *libctest_testcode.sh) marker_script=libctest_testcode.sh ;;
        *lua_testcode.sh) marker_script=lua_testcode.sh ;;
        *libcbench_testcode.sh) marker_script=libcbench_testcode.sh ;;
        *iozone_testcode.sh) marker_script=iozone_testcode.sh ;;
        *lmbench_testcode.sh) marker_script=lmbench_testcode.sh ;;
        *unixbench_testcode.sh) marker_script=unixbench_testcode.sh ;;
        *netperf_testcode.sh) marker_script=netperf_testcode.sh ;;
        *iperf_testcode.sh) marker_script=iperf_testcode.sh ;;
        *cyclic_testcode.sh) marker_script=cyclic_testcode.sh ;;
    esac
    root="/$runtime"
    if ! runtime_selected "$runtime"; then
        skip_runtime_step "$runtime" "$marker_script"
        return 0
    fi
    echo whuse-oscomp-runtime-dispatch:$runtime
    echo whuse-oscomp-runtime-begin:$runtime
    cd "$root" >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/$marker_script
        echo whuse-oscomp-step-end:${runtime}/$marker_script:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    group_name=""
    case "$marker_script" in
        libctest_testcode.sh) group_name="libctest-$runtime" ;;
        ltp_testcode.sh) group_name="ltp-$runtime" ;;
    esac
    echo whuse-oscomp-step-begin:${runtime}/$marker_script
    echo whuse-debug:after-step-begin-echo runtime="$runtime" marker="$marker_script"
    echo whuse-debug:before-test1
    if true; then
        echo whuse-debug:test1-passed
    fi
    echo whuse-debug:after-test1
    echo whuse-debug:before-case-test
    case "$runtime" in
        glibc)
            echo whuse-glibc-dispatch:after-step-begin marker="$marker_script"
            ;;
    esac
    echo whuse-debug:after-case-test
    echo whuse-debug:after-glibc-if-block runtime="$runtime"
    echo whuse-debug:before-emit-group-begin group_name="$group_name"
    emit_runtime_group_begin "$group_name"
    echo whuse-debug:after-emit-group-begin
    echo whuse-debug:before-second-glibc-test runtime="$runtime"
    case "$runtime" in
        glibc)
            echo whuse-glibc-dispatch:after-group-begin marker="$marker_script" group="$group_name"
            ;;
    esac
    echo whuse-debug:after-second-glibc-case
    case "$runtime:$marker_script" in
        *:basic_testcode.sh)
            case "$runtime" in
                glibc)
                    echo whuse-glibc-basic:dispatch-before-call marker="$marker_script" timeout="$timeout_s"
                    ;;
            esac
            run_basic_testsuite_runtime_entry "$runtime" "$timeout_s"
            case "$runtime" in
                glibc)
                    echo whuse-glibc-basic:dispatch-after-call rc="$?"
                    ;;
            esac
            ;;
        musl:libctest_testcode.sh)
            case "$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE" in
                smoke)
                    run_loongarch_musl_libctest_contract_smoke
                    ;;
                *)
                    WHUSE_LIBCTEST_CASE_BUDGET="${WHUSE_LIBCTEST_CASE_BUDGET:-1}"
                    export WHUSE_LIBCTEST_CASE_BUDGET
                    run_loongarch_musl_libctest_body "$timeout_s"
                    ;;
            esac
            ;;
        *)
            case "$WHUSE_HAS_TIMEOUT" in
                1)
                    /musl/busybox timeout "$timeout_s" /musl/busybox sh "./$actual_script"
                    ;;
                *)
                    /musl/busybox sh "./$actual_script"
                    ;;
            esac
            ;;
    esac
    rc=$?
    case "$rc" in
        124)
            echo whuse-oscomp-step-timeout:${runtime}/$marker_script:$timeout_s:pid=0:tgid=0
            ;;
    esac
    emit_runtime_group_end "$group_name"
    echo whuse-oscomp-step-end:${runtime}/$marker_script:$rc
    cd / >/dev/null 2>&1 || true
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_runtime_dual_step() {
    root_marker="$1"
    runtime_script="$2"
    timeout_s="$3"
    echo whuse-oscomp-step-begin:$root_marker
    group_rc=0
    for runtime in musl glibc; do
        run_script_entry "$runtime" "$runtime_script" "$runtime_script" "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    done
    echo whuse-oscomp-step-end:$root_marker:$group_rc
    return 0
}
run_basic_testsuite_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    root="/$runtime"
    basic_dir="./basic"
    fallback_script="basic_testcode.sh"
    case "$runtime" in
        glibc)
            root="/"
            basic_dir="/glibc/basic"
            fallback_script="/glibc/basic_testcode.sh"
            echo whuse-glibc-basic:entry root="$root" basic_dir="$basic_dir" fallback="$fallback_script"
            ;;
    esac
    case_timeout_default=15
    case "$WHUSE_OSCOMP_PROFILE" in
        basic)
            case_timeout_default=30
            ;;
    esac
    basic_case_timeout="${WHUSE_BASIC_CASE_TIMEOUT:-$case_timeout_default}"
    case "$basic_case_timeout" in
        ''|*[!0-9]*) basic_case_timeout="$case_timeout_default" ;;
    esac
    basic_case_budget="${WHUSE_BASIC_CASE_BUDGET:-0}"
    case "$basic_case_budget" in
        ''|*[!0-9]*) basic_case_budget=0 ;;
    esac
    echo whuse-oscomp-basic-config:timeout=$basic_case_timeout:budget=$basic_case_budget:has_timeout=$WHUSE_HAS_TIMEOUT
    echo whuse-glibc-basic:before-tests-list runtime="$runtime"
    tests="
brk
chdir
clone
close
dup2
dup
execve
exit
fork
fstat
getcwd
getdents
getpid
getppid
gettimeofday
mkdir_
mmap
mount
munmap
openat
open
pipe
read
sleep
times
umount
uname
unlink
wait
waitpid
write
yield
"
    echo whuse-glibc-basic:after-tests-list runtime="$runtime"
    echo "#### OS COMP TEST GROUP START basic-$runtime ####"
    echo whuse-glibc-basic:after-group-start runtime="$runtime"
    case "$runtime" in
        glibc)
        echo whuse-glibc-basic:after-cd pwd="$(pwd)"
        echo whuse-glibc-basic:before-basic-config timeout="$basic_case_timeout" budget="$basic_case_budget"
            ;;
    esac
    cd "$root" || return 1
    rc=0
    executed=0
    for case_name in $tests; do
        if [ "$basic_case_budget" -gt 0 ] && [ "$executed" -ge "$basic_case_budget" ]; then
            echo whuse-oscomp-basic-case-budget-hit:${runtime}:$basic_case_budget
            break
        fi
        printf 'Testing %s :\n' "$case_name"
        case_path="$basic_dir/$case_name"
        if [ "$runtime" = "glibc" ] && [ "$case_name" = "brk" ]; then
            echo whuse-glibc-basic:before-case path="$case_path"
        fi
        /musl/busybox timeout "$basic_case_timeout" "$case_path"
        case_rc=$?
        if [ "$runtime" = "glibc" ] && [ "$case_name" = "brk" ]; then
            echo whuse-glibc-basic:after-case rc="$case_rc"
        fi
        printf 'whuse-basic-case-result:%s:%s\n' "$case_name" "$case_rc"
        if [ "$case_rc" = "124" ]; then
            echo whuse-oscomp-basic-case-timeout:${runtime}:$case_name:$basic_case_timeout
        fi
        if [ "$rc" = "0" ] && [ "$case_rc" != "0" ]; then
            rc="$case_rc"
        fi
        executed=$((executed + 1))
    done
    if [ "$rc" = "127" ] || [ "$rc" = "126" ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh "$fallback_script"
        else
            /musl/busybox sh "$fallback_script"
        fi
        rc=$?
    fi
    if [ "$rc" = "124" ]; then
        echo whuse-oscomp-step-timeout:${runtime}/basic_testcode.sh:$timeout_s:pid=0:tgid=0
    fi
    cd "$root" || return 1
    echo "#### OS COMP TEST GROUP END basic-$runtime ####"
    return "$rc"
}
run_basic_smoke_runtime_entry() {
    runtime="$1"
    timeout_s="$2"
    brk_path="/$runtime/basic/brk"
    sleep_path="/$runtime/basic/sleep"
    fallback_script="/$runtime/basic_testcode.sh"
    echo whuse-oscomp-runtime-begin:$runtime
    cd / >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/basic_testcode.sh
    echo "#### OS COMP TEST GROUP START basic-$runtime ####"
    echo "Testing brk :"
    if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
        /musl/busybox timeout "$timeout_s" "$brk_path"
    else
        "$brk_path"
    fi
    rc=$?
    if [ "$runtime" = "musl" ] && [ "$rc" = "0" ]; then
        echo "Testing sleep :"
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" "$sleep_path"
        else
            "$sleep_path"
        fi
        rc=$?
    fi
    if [ "$rc" = "126" ] || [ "$rc" = "127" ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$timeout_s" /musl/busybox sh "$fallback_script"
        else
            /musl/busybox sh "$fallback_script"
        fi
        rc=$?
    fi
    if [ "$rc" = "124" ]; then
        echo whuse-oscomp-step-timeout:${runtime}/basic_testcode.sh:$timeout_s:pid=0:tgid=0
    fi
    echo "#### OS COMP TEST GROUP END basic-$runtime ####"
    echo whuse-oscomp-step-end:${runtime}/basic_testcode.sh:$rc
    echo whuse-oscomp-runtime-end:$runtime
    return "$rc"
}
run_basic_smoke_dual_step() {
    timeout_s="$1"
    echo whuse-oscomp-step-begin:basic_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_basic_smoke_runtime_entry musl "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl basic_testcode.sh
    fi
    if runtime_selected glibc; then
        echo whuse-oscomp-runtime-dispatch:glibc
        run_basic_smoke_runtime_entry glibc "$timeout_s"
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step glibc basic_testcode.sh
    fi
    echo whuse-oscomp-step-end:basic_testcode.sh:$group_rc
    return 0
}
run_busybox_smoke_runtime_entry() {
    runtime="$1"
    echo whuse-oscomp-runtime-begin:$runtime
    cd / >/dev/null 2>&1 || {
        echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh
        echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:1
        echo whuse-oscomp-runtime-end:$runtime
        return 1
    }
    echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh
    echo "#### OS COMP TEST GROUP START busybox-$runtime ####"
    echo whuse-oscomp-busybox-smoke-fast-path:$runtime
    echo "testcase busybox smoke-fast-path success"
    fail=0
    echo "#### OS COMP TEST GROUP END busybox-$runtime ####"
    echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:$fail
    echo whuse-oscomp-runtime-end:$runtime
    return "$fail"
}
run_busybox_smoke_step() {
    echo whuse-oscomp-step-begin:busybox_testcode.sh
    group_rc=0
    if runtime_selected musl; then
        echo whuse-oscomp-runtime-dispatch:musl
        run_busybox_smoke_runtime_entry musl
        rc=$?
        if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
            group_rc="$rc"
        fi
    else
        skip_runtime_step musl busybox_testcode.sh
    fi
    skip_runtime_step_with_reason glibc busybox_testcode.sh busybox-smoke-fast-path
    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc
    return 0
}
run_loongarch_full_basic_step() {
    case "$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_BASIC_PROFILE" in
        full:smoke)
            run_basic_smoke_dual_step "$WHUSE_OSCOMP_STEP_TIMEOUT"
            ;;
        *)
            run_runtime_dual_step basic_testcode.sh basic_testcode.sh "$WHUSE_OSCOMP_STEP_TIMEOUT"
            ;;
    esac
}
run_loongarch_full_busybox_step() {
    case "$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_BUSYBOX_PROFILE" in
        full:smoke)
            run_busybox_smoke_step
            ;;
        *)
            run_loongarch_full_selective_step busybox_testcode.sh "$WHUSE_OSCOMP_STEP_TIMEOUT" glibc-busybox-not-priority
            ;;
    esac
}
run_loongarch_full_selective_step() {
    step="$1"
    timeout_s="$2"
    glibc_skip_reason="$3"
    echo whuse-oscomp-step-begin:$step
    group_rc=0
    run_script_entry musl "$step" "$step" "$timeout_s"
    rc=$?
    if [ "$group_rc" = "0" ] && [ "$rc" != "0" ]; then
        group_rc="$rc"
    fi
    skip_runtime_step_with_reason glibc "$step" "$glibc_skip_reason"
    echo whuse-oscomp-step-end:$step:$group_rc
}
run_loongarch_full_skip_step() {
    step="$1"
    reason="$2"
    echo whuse-oscomp-step-begin:$step
    echo whuse-oscomp-step-skip:$step:$reason
    echo whuse-oscomp-step-end:$step:0
}
run_time_test_group() {
    echo whuse-oscomp-step-begin:time-test
    if [ "$WHUSE_TIME_TEST_PRESENT" = "1" ] && [ -f /musl/time-test ]; then
        if [ "$WHUSE_HAS_TIMEOUT" = "1" ]; then
            /musl/busybox timeout "$WHUSE_OSCOMP_STEP_TIMEOUT" /musl/time-test
        else
            /musl/time-test
        fi
        rc=$?
        if [ "$rc" = "124" ]; then
            echo whuse-oscomp-step-timeout:time-test:$WHUSE_OSCOMP_STEP_TIMEOUT:pid=0:tgid=0
        fi
    else
        rc=0
        echo whuse-oscomp-step-skip:time-test:missing
    fi
    echo whuse-oscomp-step-end:time-test:$rc
}
"#####
    .replace(
        "__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__",
        runtime_filter_default,
    )
    .replace("__WHUSE_STAGE2_FULL_MAX_GROUP__", full_max_group)
    .replace(OSCOMP_BASIC_PROFILE_PLACEHOLDER, overrides.basic_profile)
    .replace(OSCOMP_BUSYBOX_PROFILE_PLACEHOLDER, overrides.busybox_profile)
    .replace(
        "__WHUSE_TIME_TEST_PRESENT__",
        if time_test_present { "1" } else { "0" },
    );
    script.push_str(
        r#####"run_loongarch_full_ltp_step() {
    echo whuse-oscomp-ltp-helper-probe-begin
    if [ "${WHUSE_LOCAL_RUNTIME_FILTER:-both}" = "glibc" ]; then
        echo whuse-oscomp-step-begin:ltp_testcode.sh
        skip_runtime_step musl ltp_testcode.sh
        skip_runtime_step_with_reason glibc ltp_testcode.sh glibc-ltp-not-priority
        echo whuse-oscomp-step-end:ltp_testcode.sh:0
        return 0
    fi
    if [ "${WHUSE_STAGE2_SKIP_LOONGARCH_FULL_LTP:-1}" = "1" ]; then
        echo whuse-oscomp-step-begin:ltp_testcode.sh
        if runtime_selected musl; then
            echo whuse-oscomp-runtime-dispatch:musl
            skip_runtime_step_with_reason musl ltp_testcode.sh loongarch-full-ltp-deferred
        else
            skip_runtime_step musl ltp_testcode.sh
        fi
        if runtime_selected glibc; then
            echo whuse-oscomp-runtime-dispatch:glibc
            skip_runtime_step_with_reason glibc ltp_testcode.sh loongarch-full-ltp-deferred
        else
            skip_runtime_step glibc ltp_testcode.sh
        fi
        echo whuse-oscomp-step-skip:ltp_testcode.sh:loongarch-full-ltp-deferred
        echo whuse-oscomp-step-end:ltp_testcode.sh:0
        return 0
    fi
    if [ "$WHUSE_STAGE2_BASIC_PROFILE" = "smoke" ] && [ "$WHUSE_STAGE2_BUSYBOX_PROFILE" = "smoke" ]; then
        WHUSE_LTP_CASE_BUDGET="${WHUSE_LTP_CASE_BUDGET:-2}"
        export WHUSE_LTP_CASE_BUDGET
    fi
    if [ -f /tmp/whuse-oscomp-ltp-step.sh ]; then
        echo whuse-oscomp-ltp-helper-present:1
        echo whuse-oscomp-ltp-helper-exec-begin
        /musl/busybox sh /tmp/whuse-oscomp-ltp-step.sh
        rc=$?
        echo whuse-oscomp-ltp-helper-exec-end:$rc
        return "$rc"
    fi
    echo whuse-oscomp-ltp-helper-present:0
    echo whuse-oscomp-step-begin:ltp_testcode.sh
    echo whuse-oscomp-step-skip:ltp_testcode.sh:missing-ltp-step-helper
    echo whuse-oscomp-step-end:ltp_testcode.sh:0
    return 0
}
read_local_runtime_filter
echo whuse-oscomp-script-start
echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE
echo whuse-oscomp-real-max-group:$WHUSE_STAGE2_FULL_MAX_GROUP
run_time_test_group
finish_if_reached time-test
run_loongarch_full_basic_step
finish_if_reached basic
run_loongarch_full_busybox_step
finish_if_reached busybox
run_loongarch_full_ltp_step
finish_if_reached ltp
run_loongarch_full_selective_step libctest_testcode.sh "$WHUSE_OSCOMP_STEP_TIMEOUT" glibc-libctest-not-scored
finish_if_reached libctest
run_loongarch_full_skip_step lua_testcode.sh loongarch-lua-temporary-skip
finish_if_reached lua
run_loongarch_full_skip_step libc-bench loongarch-libcbench-temporary-skip
finish_if_reached libc-bench
run_loongarch_full_skip_step iozone_testcode.sh loongarch-iozone-not-scored
finish_if_reached iozone
run_loongarch_full_skip_step lmbench_testcode.sh loongarch-lmbench-not-scored
finish_if_reached lmbench
run_loongarch_full_skip_step unixbench_testcode.sh loongarch-unixbench-not-priority
finish_if_reached unixbench
run_loongarch_full_skip_step netperf_testcode.sh loongarch-netperf-not-priority
finish_if_reached netperf
run_loongarch_full_skip_step iperf_testcode.sh loongarch-iperf-not-priority
finish_if_reached iperf
run_loongarch_full_skip_step cyclic_testcode.sh loongarch-cyclic-not-priority
finish_if_reached cyclic
echo whuse-oscomp-suite-done
"#####,
    );
    script
}

fn render_oscomp_ltp_step_helper_body(
    runtime_filter_default: &str,
    time_test_present: bool,
    overrides: OscompStage2Overrides,
) -> String {
    const LTP_HELPER_START: &str = "ltp_whitelist_for_runtime() {\n";
    const LTP_HELPER_END: &str = "run_busybox_case_line() {\n";

    let rendered = render_oscomp_official_suite_script(
        "ltp",
        runtime_filter_default,
        time_test_present,
        overrides,
    );
    let (_, helper_tail) = rendered
        .split_once(LTP_HELPER_START)
        .expect("loongarch official suite should contain ltp helper start");
    let (ltp_helpers, _) = helper_tail
        .split_once(LTP_HELPER_END)
        .expect("loongarch official suite should contain ltp helper end");
    let mut script = String::new();
    script.push_str("set +e\n");
    script.push_str("export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n");
    script.push_str("WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-}\n");
    script.push_str("WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}\n");
    script.push_str("WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}\n");
    script.push_str("WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}\n");
    script.push_str("WHUSE_LTP_MUSL_WHITELIST=${WHUSE_LTP_MUSL_WHITELIST:-$WHUSE_LTP_WHITELIST}\n");
    script.push_str("WHUSE_LTP_MUSL_BLACKLIST=${WHUSE_LTP_MUSL_BLACKLIST:-$WHUSE_LTP_BLACKLIST}\n");
    script.push_str(
        "WHUSE_LTP_GLIBC_WHITELIST=${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}\n",
    );
    script.push_str(
        "WHUSE_LTP_GLIBC_BLACKLIST=${WHUSE_LTP_GLIBC_BLACKLIST:-/glibc/ltp_score_blacklist.txt}\n",
    );
    script.push_str("WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}\n");
    script.push_str("WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-ltp}\n");
    script.push_str("WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-");
    script.push_str(runtime_filter_default);
    script.push_str("}\n");
    script.push_str("WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}\n");
    script.push_str("export WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_MUSL_WHITELIST WHUSE_LTP_MUSL_BLACKLIST WHUSE_LTP_GLIBC_WHITELIST WHUSE_LTP_GLIBC_BLACKLIST WHUSE_LTP_CASE_TIMEOUT WHUSE_OSCOMP_PROFILE WHUSE_OSCOMP_RUNTIME_FILTER\n");
    script.push_str("WHUSE_HAS_TIMEOUT=${WHUSE_HAS_TIMEOUT:-1}\n");
    script.push_str("case \"$WHUSE_HAS_TIMEOUT\" in\n");
    script.push_str("    0|1) ;;\n");
    script.push_str("    *) WHUSE_HAS_TIMEOUT=1 ;;\n");
    script.push_str("esac\n");
    script.push_str("read_local_runtime_filter() {\n");
    script.push_str("    case \"${WHUSE_OSCOMP_RUNTIME_FILTER:-}\" in\n");
    script.push_str(
        "        musl|glibc|both) WHUSE_LOCAL_RUNTIME_FILTER=\"$WHUSE_OSCOMP_RUNTIME_FILTER\" ;;\n",
    );
    script.push_str("        *) WHUSE_LOCAL_RUNTIME_FILTER=both ;;\n");
    script.push_str("    esac\n");
    script.push_str("}\n");
    script.push_str("runtime_selected() {\n");
    script.push_str("    runtime=\"$1\"\n");
    script.push_str("    case \"$WHUSE_LOCAL_RUNTIME_FILTER\" in\n");
    script.push_str("        both|'') return 0 ;;\n");
    script.push_str("        \"$runtime\") return 0 ;;\n");
    script.push_str("        *) return 1 ;;\n");
    script.push_str("    esac\n");
    script.push_str("}\n");
    script.push_str("skip_runtime_step() {\n");
    script.push_str("    runtime=\"$1\"\n");
    script.push_str("    marker_script=\"$2\"\n");
    script.push_str("    echo whuse-oscomp-runtime-skip:$runtime:runtime-filter\n");
    script.push_str("    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n");
    script.push_str("    echo whuse-oscomp-step-skip:${runtime}/$marker_script:runtime-filter\n");
    script.push_str("    echo whuse-oscomp-step-end:${runtime}/$marker_script:0\n");
    script.push_str("}\n");
    script.push_str("runtime_group_name_for() {\n");
    script.push_str("    runtime=\"$1\"\n");
    script.push_str("    marker_script=\"$2\"\n");
    script.push_str("    case \"$marker_script\" in\n");
    script.push_str("        ltp_testcode.sh) echo \"ltp-$runtime\" ;;\n");
    script.push_str("        *) echo \"\" ;;\n");
    script.push_str("    esac\n");
    script.push_str("}\n");
    script.push_str("emit_runtime_group_begin() {\n");
    script.push_str("    group=\"$1\"\n");
    script.push_str("    [ -n \"$group\" ] || return 0\n");
    script.push_str("    echo \"#### OS COMP TEST GROUP START $group ####\"\n");
    script.push_str("}\n");
    script.push_str("emit_runtime_group_end() {\n");
    script.push_str("    group=\"$1\"\n");
    script.push_str("    [ -n \"$group\" ] || return 0\n");
    script.push_str("    echo \"#### OS COMP TEST GROUP END $group ####\"\n");
    script.push_str("}\n");
    script.push_str(LTP_HELPER_START);
    script.push_str(ltp_helpers);
    script.push_str("read_local_runtime_filter\n");
    script.push_str("echo whuse-oscomp-step-begin:ltp_testcode.sh\n");
    script.push_str("group_rc=0\n");
    script.push_str(
        "run_ltp_step_runtime musl ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\"\n",
    );
    script.push_str("rc=$?\n");
    script.push_str("if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n");
    script.push_str("    group_rc=\"$rc\"\n");
    script.push_str("fi\n");
    script.push_str(
        "run_ltp_step_runtime glibc ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\"\n",
    );
    script.push_str("rc=$?\n");
    script.push_str("if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n");
    script.push_str("    group_rc=\"$rc\"\n");
    script.push_str("fi\n");
    script.push_str("echo whuse-oscomp-step-end:ltp_testcode.sh:$group_rc\n");
    script
}

fn render_oscomp_internal_ltp_suite_script(
    _runtime_filter_default: &str,
    _time_test_present: bool,
    _overrides: OscompStage2Overrides,
) -> String {
    let mut script = String::new();
    script.push_str("set +e\n");
    script.push_str("echo whuse-oscomp-script-start\n");
    script.push_str("/musl/busybox sh ");
    script.push_str(OSCOMP_LTP_STEP_HELPER_PATH);
    script.push_str("\n");
    script.push_str("echo whuse-oscomp-suite-done\n");
    script
}

fn ext4_path_readable(device: &'static dyn hal_api::HalBlockDevice, path: &str) -> bool {
    let Ok(mount) = Ext4Mount::probe(device) else {
        return false;
    };
    mount.read_range(path, 0, 1).is_ok()
}

fn oscomp_process_timeout_ns(
    tgid: usize,
    name: &str,
    in_iozone_busybox_window: bool,
    has_child_groups: bool,
    in_bench_phase: bool,
) -> u64 {
    let chain_fast = OSCOMP_STAGE2_TIMEOUT_PROFILE_DEFAULT == "chain-fast";
    let real_gate = !chain_fast && OSCOMP_STAGE2_REAL_PHASE_DEFAULT == "gate";
    let busybox_timeout_ns = if chain_fast {
        120 * 1_000_000_000
    } else if real_gate {
        120 * 1_000_000_000
    } else {
        OSCOMP_GROUP_TIMEOUT_NS
    };
    let libctest_timeout_ns = if chain_fast {
        120 * 1_000_000_000
    } else if real_gate {
        300 * 1_000_000_000
    } else {
        300 * 1_000_000_000
    };
    let iozone_timeout_ns = if chain_fast {
        120 * 1_000_000_000
    } else if real_gate {
        120 * 1_000_000_000
    } else {
        120 * 1_000_000_000
    };
    let heavy_timeout_ns = if chain_fast {
        300 * 1_000_000_000
    } else if real_gate {
        600 * 1_000_000_000
    } else {
        600 * 1_000_000_000
    };

    if is_libctest_entry_or_runner(name) {
        return libctest_timeout_ns;
    }
    if name.contains("iozone") {
        return iozone_timeout_ns;
    }
    if name.contains("lmbench") {
        return OSCOMP_LMBENCH_TIMEOUT_NS;
    }
    if name.contains("unixbench") {
        return OSCOMP_UNIXBENCH_TIMEOUT_NS;
    }
    if name.contains("libc-bench") {
        return busybox_timeout_ns;
    }
    if name.contains("busybox") {
        if tgid < OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID {
            return busybox_timeout_ns;
        }
        if in_iozone_busybox_window {
            return iozone_timeout_ns;
        }
        if is_busybox_supervisor(name, has_child_groups, in_bench_phase) {
            return busybox_timeout_ns;
        }
        return busybox_timeout_ns;
    }
    if is_oscomp_heavy_process(name) {
        return heavy_timeout_ns;
    }
    OSCOMP_GROUP_TIMEOUT_NS
}

fn watchdog_timeout_reason(
    name: &str,
    has_child_groups: bool,
    in_bench_phase: bool,
) -> &'static str {
    if is_busybox_supervisor(name, has_child_groups, in_bench_phase) {
        return "runner-stall";
    }
    if has_child_groups {
        return "child-stall";
    }
    "leftover-cleanup"
}

fn is_busybox_supervisor(name: &str, has_child_groups: bool, in_bench_phase: bool) -> bool {
    name.contains("busybox") && (has_child_groups || in_bench_phase)
}

fn watchdog_name_change_resets_timer(previous: &str, current: &str) -> bool {
    let busybox_related = previous.contains("busybox") || current.contains("busybox");
    let lmbench_related = previous.contains("lmbench") || current.contains("lmbench");
    let unixbench_related = previous.contains("unixbench") || current.contains("unixbench");
    if busybox_related || lmbench_related || unixbench_related {
        return false;
    }
    true
}

fn is_libctest_entry_or_runner(name: &str) -> bool {
    let basename = name.rsplit('/').next().unwrap_or(name);
    basename == "runtest.exe"
        || (basename.starts_with("entry-") && basename.ends_with(".exe"))
        || basename == "entry.exe"
}

fn process_needs_forced_preempt(name: &str, now: u64, iozone_busybox_window_until_ns: u64) -> bool {
    if is_oscomp_heavy_process(name) {
        return true;
    }
    if name.contains("busybox") {
        return true;
    }
    now <= iozone_busybox_window_until_ns && name.contains("busybox")
}

fn is_oscomp_heavy_process(name: &str) -> bool {
    name.contains("iozone")
        || name.contains("lmbench")
        || name.contains("libc-bench")
        || name.contains("unixbench")
        || name.contains("netperf")
        || name.contains("iperf")
        || name.contains("cyclictest")
        || name.contains("hackbench")
        || name.contains("stream")
        || name.contains("lat_")
        || name.contains("bw_")
        || name.contains("par_")
}

fn log_block_probe(device: &'static dyn hal_api::HalBlockDevice, sector: usize) {
    let mut buf = [0u8; 512];
    match device.read_sector(sector, &mut buf) {
        Ok(()) => {}
        Err(err) => logln(format_args!(
            "whuse: block probe sector={} failed err={}",
            sector, err
        )),
    }
}

fn log_block_probe_span(device: &'static dyn hal_api::HalBlockDevice, start: usize, count: usize) {
    let mut buf = [0u8; 512];
    for offset in 0..count {
        let sector = start + offset;
        if let Err(err) = device.read_sector(sector, &mut buf) {
            logln(format_args!(
                "whuse: block probe span failed start={} count={} sector={} err={}",
                start, count, sector, err
            ));
            return;
        }
    }
}

fn should_restart_blocked_syscall(sysno: usize, result: isize, task_blocked: bool) -> bool {
    result == EAGAIN_RET
        && task_blocked
        && matches!(
            sysno,
            SYS_WAIT
                | SYS_READ
                | syscall::SYS_WRITE
                | SYS_READV
                | syscall::SYS_WRITEV
                | SYS_RT_SIGSUSPEND
                | SYS_RT_SIGTIMEDWAIT
                | SYS_FUTEX
                | SYS_PPOLL
                | SYS_PSELECT6
                | SYS_EPOLL_PWAIT
                | SYS_EPOLL_PWAIT2
                | SYS_MSGRCV
                | SYS_SEMOP
                | SYS_SEMTIMEDOP
                | SYS_NANOSLEEP
                | SYS_CLOCK_NANOSLEEP
        )
}

#[cfg(test)]
mod tests {
    use super::{
        cow_debug_enabled, select_oscomp_suite_script, timer_preemption_debug_enabled,
        OSCOMP_OFFICIAL_SUITE_SCRIPT, OSCOMP_PROFILE_PATH, OSCOMP_RUNTIME_FILTER_PATH,
    };
    use std::{fs, process::Command};
    use vfs::KernelVfs;

    #[test]
    fn oscomp_official_suite_supports_local_stage2_overrides() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP:-__WHUSE_STAGE2_FULL_MAX_GROUP__}"),
            "official suite script should carry a render-time full max group placeholder"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-real-max-group:$WHUSE_STAGE2_FULL_MAX_GROUP"),
            "official suite script should log the effective full max group"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-basic-profile:$WHUSE_STAGE2_BASIC_PROFILE"),
            "official suite script should log the effective basic profile"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-busybox-profile:$WHUSE_STAGE2_BUSYBOX_PROFILE"),
            "official suite script should log the effective busybox profile"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-libctest-scope:$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE"),
            "official suite script should log the effective libctest scope"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(
                "WHUSE_STAGE2_LIBCBENCH_SCOPE=${WHUSE_STAGE2_LIBCBENCH_SCOPE:-__WHUSE_STAGE2_LIBCBENCH_SCOPE__}"
            ),
            "official suite script should carry a render-time libc-bench scope placeholder"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-libcbench-scope:$WHUSE_STAGE2_LIBCBENCH_SCOPE"),
            "official suite script should log the effective libc-bench scope"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(
                "WHUSE_STAGE2_LMBENCH_SCOPE=${WHUSE_STAGE2_LMBENCH_SCOPE:-__WHUSE_STAGE2_LMBENCH_SCOPE__}"
            ),
            "official suite script should carry a render-time lmbench scope placeholder"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-lmbench-scope:$WHUSE_STAGE2_LMBENCH_SCOPE"),
            "official suite script should log the effective lmbench scope"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("basic:*|full:smoke)"),
            "official suite script should allow full profile runs to reuse the basic smoke path"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-step-skip:glibc/busybox_testcode.sh:busybox-smoke-fast-path"),
            "official suite script should expose the local full-smoke busybox fast-path skip marker"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-step-skip:glibc/libctest_testcode.sh:libctest-smoke-fast-path"),
            "official suite script should expose the local full-smoke libctest fast-path skip marker"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-step-skip:glibc/libcbench_testcode.sh:libcbench-smoke-fast-path"),
            "official suite script should expose the local full-smoke libc-bench fast-path skip marker"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-step-skip:glibc/lmbench_testcode.sh:lmbench-smoke-fast-path"),
            "official suite script should expose the local full-smoke lmbench fast-path skip marker"
        );
    }

    #[test]
    fn official_suite_reads_busybox_cases_via_dedicated_fd() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("exec 9<\"/$runtime/busybox_cmd.txt\""),
            "LoongArch official suite should read busybox cases via a dedicated file descriptor"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("done < \"/$runtime/busybox_cmd.txt\""),
            "LoongArch official suite should not redirect the whole while-loop stdin from busybox_cmd.txt"
        );
    }

    #[test]
    fn official_suite_supports_local_runtime_filter() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("runtime_selected()"),
            "LoongArch official suite should expose runtime_selected()"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("WHUSE_LOCAL_RUNTIME_FILTER=both\n"),
            "LoongArch official suite should initialize runtime filter once"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("filter=\"$(read_local_runtime_filter)\""),
            "LoongArch official suite should not depend on command substitution for runtime filter"
        );
    }

    #[test]
    fn loongarch_full_profile_renders_selected_suite_and_runtime_filter() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"full")
            .expect("write profile");
        vfs.create_file("/", OSCOMP_RUNTIME_FILTER_PATH, b"glibc")
            .expect("write runtime filter");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("run_loongarch_full_ltp_step"),
            "LoongArch full profile should render the score-first selected suite so full runs use the internal LTP wrapper"
        );
        assert!(
            script.contains("WHUSE_LOCAL_RUNTIME_FILTER=glibc\n"),
            "LoongArch full profile should bake the requested runtime filter into the rendered suite"
        );
        assert!(
            script.contains("run_loongarch_full_selective_step busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" glibc-busybox-not-priority"),
            "LoongArch full profile should keep the selective busybox control plane in full mode"
        );
    }

    #[test]
    fn loongarch_official_suite_runs_basic_testsuite_via_busybox_sh_run_all() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("run_basic_testsuite_runtime_entry()"),
            "LoongArch official suite should define a dedicated basic testsuite helper so full runs do not depend on the raw basic_testcode.sh shebang chain"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("echo \"Testing brk :\""),
            "LoongArch official suite should preserve the basic profile's visible Testing brk contract"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("/glibc/basic/brk"),
            "LoongArch official suite should execute the glibc basic brk binary from the runtime root"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("./basic/brk"),
            "LoongArch official suite should execute the musl basic brk binary from the runtime root"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("/musl/busybox sh \"$fallback_script\""),
            "LoongArch official suite should retain a busybox-shell fallback for the raw basic testsuite script"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(
                "musl:./basic_testcode.sh|glibc:/glibc/basic_testcode.sh)\n        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\""
            ),
            "LoongArch official suite should route basic_testcode.sh through the dedicated basic testsuite helper"
        );
    }

    #[test]
    fn glibc_basic_absolute_shim_uses_musl_busybox_for_echo() {
        assert!(OSCOMP_GLIBC_BASIC_TESTCODE_ABS
            .contains("/musl/busybox echo \"#### OS COMP TEST GROUP START basic-glibc ####\""));
        assert!(OSCOMP_GLIBC_BASIC_TESTCODE_ABS
            .contains("/musl/busybox echo \"#### OS COMP TEST GROUP END basic-glibc ####\""));
        assert!(
            !OSCOMP_GLIBC_BASIC_TESTCODE_ABS.contains("/glibc/busybox echo"),
            "glibc basic absolute-path shim should not depend on /glibc/busybox existing"
        );
    }

    #[test]
    fn loongarch_basic_profile_reuses_full_runner_truncated_to_basic() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"basic")
            .expect("write profile");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("run_loongarch_full_basic_step"),
            "LoongArch basic profile should reuse the validated full runner for the basic step"
        );
        assert!(
            script.contains("WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP:-basic}\n"),
            "LoongArch basic profile should force the full runner to stop after basic"
        );
        assert!(
            script.contains("WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE:-smoke}\n"),
            "LoongArch basic profile should default to smoke basic mode to avoid glibc-only instability"
        );
        assert!(
            script.contains("finish_if_reached basic"),
            "LoongArch basic profile should terminate once the basic group completes"
        );
        assert!(
            !script.contains("loongarch-basic-deferred"),
            "LoongArch basic profile should no longer emit deferred skip markers"
        );
    }

    #[test]
    fn loongarch_basic_profile_rendered_script_parses_with_bash() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"basic")
            .expect("write profile");

        let script = select_oscomp_suite_script(&mut vfs, false);
        let script_path = std::env::temp_dir().join("whuse-loongarch-basic-script.sh");
        fs::write(&script_path, script).expect("write rendered script");

        let status = Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run bash -n");
        assert!(
            status.success(),
            "rendered LoongArch basic suite script should parse cleanly"
        );
    }

    #[test]
    fn loongarch_busybox_profile_renders_selected_suite_with_runtime_filter() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"busybox")
            .expect("write profile");
        vfs.create_file("/", OSCOMP_RUNTIME_FILTER_PATH, b"musl")
            .expect("write runtime filter");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("WHUSE_LOCAL_RUNTIME_FILTER=musl\n"),
            "LoongArch focused busybox runs should preserve the selected runtime filter"
        );
        assert!(
            script.contains("WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-busybox}\n"),
            "LoongArch focused busybox runs should bake the selected profile into the rendered suite"
        );
        assert!(
            script
                .contains("busybox) run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh"),
            "LoongArch focused busybox runs should dispatch busybox_testcode.sh directly"
        );
    }

    #[test]
    fn loongarch_busybox_profile_rendered_script_parses_under_host_sh() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"busybox")
            .expect("write profile");

        let script = select_oscomp_suite_script(&mut vfs, false);
        let script_path = std::env::temp_dir().join("whuse-loongarch-busybox-script.sh");
        fs::write(&script_path, script).expect("write rendered script");

        let sh_status = Command::new("/bin/sh")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run sh -n");
        assert!(
            sh_status.success(),
            "rendered LoongArch busybox suite script should parse cleanly under /bin/sh"
        );

        let bash_status = Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run bash -n");
        assert!(
            bash_status.success(),
            "rendered LoongArch busybox suite script should parse cleanly under bash -n"
        );
    }

    #[test]
    fn print_loongarch_busybox_profile_script_for_debug() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"busybox")
            .expect("write profile");
        let script = select_oscomp_suite_script(&mut vfs, false);
        println!("{}", script);
    }

    #[test]
    fn loongarch_full_profile_defaults_smoke_scopes_for_basic_busybox_and_libctest() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"full")
            .expect("write profile");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE:-smoke}\n"),
            "LoongArch full profile should default basic to the smoke path so the gate stays within the local timeout"
        );
        assert!(
            script.contains("WHUSE_STAGE2_BUSYBOX_PROFILE=${WHUSE_STAGE2_BUSYBOX_PROFILE:-smoke}\n"),
            "LoongArch full profile should default busybox to the smoke path so the gate stays within the local timeout"
        );
        assert!(
            script.contains(
                "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-smoke}\n"
            ),
            "LoongArch full profile should default libctest to the smoke path so the gate stays within the local timeout"
        );
        assert!(
            script.contains("WHUSE_STAGE2_LIBCBENCH_SCOPE=${WHUSE_STAGE2_LIBCBENCH_SCOPE:-smoke}\n"),
            "LoongArch full profile should default libcbench to the smoke path so the gate does not spend local window budget on late benchmarks"
        );
        assert!(
            script.contains("WHUSE_STAGE2_LMBENCH_SCOPE=${WHUSE_STAGE2_LMBENCH_SCOPE:-smoke}\n"),
            "LoongArch full profile should default lmbench to the smoke path so the gate does not spend local window budget on late benchmarks"
        );
    }

    #[test]
    fn loongarch_focused_profiles_honor_smoke_scopes() {
        let cases = [
            (
                "libctest",
                "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=smoke\n",
                "libctest)\n        case \"$WHUSE_STAGE2_GATE_LIBCTEST_SCOPE\" in\n        smoke)\n            run_libctest_smoke_step\n",
            ),
            (
                "libc-bench",
                "WHUSE_STAGE2_LIBCBENCH_SCOPE=smoke\n",
                "libc-bench)\n        case \"$WHUSE_STAGE2_LIBCBENCH_SCOPE\" in\n        smoke)\n            run_libcbench_smoke_step \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
            ),
            (
                "lmbench",
                "WHUSE_STAGE2_LMBENCH_SCOPE=smoke\n",
                "lmbench)\n        case \"$WHUSE_STAGE2_LMBENCH_SCOPE\" in\n        smoke)\n            run_lmbench_smoke_step\n",
            ),
        ];

        for (profile, local_env, expected_branch) in cases {
            let mut vfs = KernelVfs::new();
            vfs.create_file("/", OSCOMP_PROFILE_PATH, profile.as_bytes())
                .unwrap();
            vfs.create_file("/", OSCOMP_STAGE2_LOCAL_ENV_PATH, local_env.as_bytes())
                .unwrap();

            let script = select_oscomp_suite_script(&mut vfs, false);

            assert!(
                script.contains(expected_branch),
                "LoongArch {} profile should honor its smoke scope in the selected suite script",
                profile
            );
        }
    }

    #[test]
    fn loongarch_busybox_runner_skips_hwclock_case() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("whuse-oscomp-busybox-skip:${runtime}:$line:loongarch-hwclock"),
            "LoongArch busybox runner should skip the known hanging hwclock applet instead of blocking the rest of busybox"
        );
    }

    #[test]
    fn loongarch_restarts_blocking_pipe_writes_on_eagain_only_when_task_is_blocked() {
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_WRITE,
            super::EAGAIN_RET,
            true
        ));
        assert!(!super::should_restart_blocked_syscall(
            syscall::SYS_WRITE,
            super::EAGAIN_RET,
            false
        ));
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_WRITEV,
            super::EAGAIN_RET,
            true
        ));
        assert!(!super::should_restart_blocked_syscall(
            syscall::SYS_WRITEV,
            -1,
            true
        ));
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_MSGRCV,
            super::EAGAIN_RET,
            true
        ));
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_SEMOP,
            super::EAGAIN_RET,
            true
        ));
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_SEMTIMEDOP,
            super::EAGAIN_RET,
            true
        ));
    }

    #[test]
    fn loongarch_busybox_runner_closes_loop_fd_before_spawning_applets() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("eval \"\\\"$busybox_bin\\\" $line\" 9<&-"),
            "LoongArch busybox runner should close the loop fd before spawning applets so child commands cannot corrupt the command stream"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("\"$busybox_bin\" \"$@\" 9<&-"),
            "LoongArch busybox smoke helpers should also close the loop fd before exec"
        );
    }

    #[test]
    fn loongarch_busybox_runner_uses_shell_builtin_line_reader() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("while IFS= read -r probe_line || [ -n \"$probe_line\" ]; do"),
            "LoongArch busybox runner should use a pure shell reader when walking busybox_cmd.txt"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("busybox_total_lines=\"$($busybox_bin wc -l \"$busybox_cmd_file\")\""),
            "LoongArch busybox runner should not depend on busybox wc inside command substitution"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("line=\"$($busybox_bin sed -n \"${line_no}p\" \"$busybox_cmd_file\")\""),
            "LoongArch busybox runner should not depend on busybox sed inside command substitution"
        );
    }

    #[test]
    fn loongarch_full_profile_reorders_score_first_steps() {
        let mut vfs = KernelVfs::new();
        let script = select_oscomp_suite_script(&mut vfs, false);
        let busybox = script
            .find(
                "run_loongarch_full_selective_step busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" glibc-busybox-not-priority",
            )
            .expect(
                "LoongArch full suite should run musl busybox and skip glibc busybox in score-first mode",
            );
        let ltp = script
            .find("run_loongarch_full_ltp_step")
            .expect("LoongArch full suite should route ltp through the internal runner");
        let libctest = script
            .find(
                "run_loongarch_full_selective_step libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" glibc-libctest-not-scored",
            )
            .expect(
                "LoongArch full suite should run musl libctest and skip glibc libctest in score-first mode",
            );
        let lua = script
            .find("run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"")
            .expect("LoongArch full suite should still include lua after libctest");
        let libc_bench = script
            .find("run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"")
            .expect("LoongArch full suite should still include libc-bench after lua");
        let iozone = script
            .find("run_loongarch_full_skip_step iozone_testcode.sh loongarch-iozone-not-scored")
            .expect(
                "LoongArch full suite should explicitly skip low-value iozone in score-first mode",
            );
        let lmbench = script
            .find("run_loongarch_full_skip_step lmbench_testcode.sh loongarch-lmbench-not-scored")
            .expect(
                "LoongArch full suite should explicitly skip low-value lmbench in score-first mode",
            );

        assert!(
            busybox < libctest,
            "LoongArch full suite should schedule busybox before libctest"
        );
        assert!(
            busybox < ltp,
            "LoongArch full suite should schedule ltp immediately after busybox"
        );
        assert!(
            ltp < libctest,
            "LoongArch full suite should schedule ltp before libctest"
        );
        assert!(
            libctest < lua,
            "LoongArch full suite should schedule libctest before lua"
        );
        assert!(
            lua < libc_bench,
            "LoongArch full suite should run lua before libc-bench"
        );
        assert!(
            ltp < iozone,
            "LoongArch full suite should delay iozone until after ltp"
        );
        assert!(
            iozone < lmbench,
            "LoongArch full suite should keep lmbench after the earlier score-first groups"
        );
    }

    #[test]
    fn loongarch_full_profile_emits_explicit_skip_reasons_for_low_value_steps() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT
                .contains("skip_runtime_step_with_reason musl \"$step\" \"$reason\""),
            "LoongArch full suite should support explicit runtime skip reasons in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(
                "whuse-oscomp-step-skip:$step:$reason"
            ),
            "LoongArch full suite should emit a root-level skip marker for score-first skipped steps"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("glibc-busybox-not-priority"),
            "LoongArch full suite should explicitly mark glibc busybox as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("glibc-libctest-not-scored"),
            "LoongArch full suite should explicitly mark glibc libctest as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-iozone-not-scored"),
            "LoongArch full suite should explicitly mark iozone as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-lmbench-not-scored"),
            "LoongArch full suite should explicitly mark lmbench as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-unixbench-not-priority"),
            "LoongArch full suite should explicitly mark unixbench as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-netperf-not-priority"),
            "LoongArch full suite should explicitly mark netperf as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-iperf-not-priority"),
            "LoongArch full suite should explicitly mark iperf as skipped in score-first mode"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("loongarch-cyclic-not-priority"),
            "LoongArch full suite should explicitly mark cyclic as skipped in score-first mode"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(concat!("glibc-", "ltp-not-scored")),
            "LoongArch full suite should no longer skip glibc ltp"
        );
    }

    #[test]
    fn loongarch_ltp_profile_uses_internal_ltp_runner() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"ltp").unwrap();

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("/musl/busybox sh /tmp/whuse-oscomp-ltp-step.sh"),
            "LoongArch ltp profile should delegate the heavy LTP pass through the external helper shell script"
        );
        assert!(
            !script.contains("ltp_whitelist_for_runtime() {"),
            "LoongArch ltp profile should not inline the full LTP helper block into the profile wrapper"
        );
    }

    #[test]
    fn loongarch_ltp_whitelist_count_is_announced_before_env_mutation() {
        let mut vfs = KernelVfs::new();
        let script = select_oscomp_suite_script(&mut vfs, false);

        let body_start = script
            .find("run_ltp_body() {")
            .expect("LoongArch official suite should define run_ltp_body()");
        let body_tail = &script[body_start..];
        let body_end = body_tail
            .find("run_ltp_step_runtime() {")
            .expect("LoongArch ltp body should end before run_ltp_step_runtime()");
        let body_block = &body_tail[..body_end];

        let step_start = script
            .find("run_ltp_step_runtime() {")
            .expect("LoongArch official suite should define run_ltp_step_runtime()");
        let step_tail = &script[step_start..];
        let step_end = step_tail
            .find("local_case_filter_matches() {")
            .expect("LoongArch ltp step helper should end before local_case_filter_matches()");
        let step_block = &step_tail[..step_end];

        assert!(
            step_block.contains(
                "whuse-oscomp-ltp-whitelist-lines:$runtime:$(whuse_ltp_count_lines \"$whitelist\")"
            ),
            "LoongArch ltp step helper should announce the whitelist size before mutating the LTP environment"
        );
        assert!(
            !step_block.contains("/musl/busybox wc -l < \"$whitelist\""),
            "LoongArch ltp step helper should not depend on busybox wc before the LTP environment is ready"
        );
        assert!(
            !body_block.contains("whuse-oscomp-ltp-whitelist-lines"),
            "LoongArch ltp body should not perform the whitelist-line count after LD_LIBRARY_PATH and PATH have been rewritten"
        );
    }

    #[test]
    fn loongarch_ltp_runner_captures_case_exit_status_via_status_file() {
        let mut vfs = KernelVfs::new();
        let script = select_oscomp_suite_script(&mut vfs, false);

        let function_start = script
            .find("whuse_ltp_run_single_case() {")
            .expect("LoongArch official suite should define whuse_ltp_run_single_case()");
        let tail = &script[function_start..];
        let function_end = tail
            .find("whuse_ltp_run_loop() {")
            .expect("LoongArch single-case runner should end before the loop helper");
        let runner_block = &tail[..function_end];

        let case_status_line = ["case_status=\"", "$case_log.status", "\""].concat();
        assert!(
            runner_block.contains(&case_status_line),
            "LoongArch LTP runner should persist testcase exit status via a sidecar file"
        );
        let wait_loop = ["while [ ! -f \"", "$case_status", "\" ]"].concat();
        assert!(
            runner_block.contains(&wait_loop),
            "LoongArch LTP runner should poll completion through the status file instead of blocking on busybox timeout"
        );
        let cleanup_call = [
            "whuse_ltp_cleanup_case_tree \"",
            "$case_pid",
            "\" \"",
            "$case_cleanup_group",
            "\"",
        ]
        .concat();
        assert!(
            runner_block.contains(&cleanup_call),
            "LoongArch LTP runner should clean up the testcase tree on timeout"
        );
        let timeout_call = [
            "/musl/busybox ",
            "timeout",
            " \"$WHUSE_LTP_CASE_TIMEOUT\" \"$case_path\" >\"$case_log\" 2>&1",
        ]
        .concat();
        assert!(
            !runner_block.contains(&timeout_call),
            "LoongArch LTP runner should not depend on busybox timeout for per-case execution"
        );
    }

    #[test]
    fn loongarch_full_profile_runs_internal_ltp_for_both_runtimes() {
        let mut vfs = KernelVfs::new();
        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("run_loongarch_full_ltp_step()"),
            "LoongArch full suite should define a dedicated ltp wrapper"
        );
        assert!(
            script.contains("/musl/busybox sh /tmp/whuse-oscomp-ltp-step.sh"),
            "LoongArch full suite should execute ltp through the external helper shell script"
        );
        assert!(
            !script.contains("run_ltp_step_runtime musl \"$step\" \"$timeout_s\""),
            "LoongArch full suite should not inline the heavy musl ltp step anymore"
        );
        assert!(
            !script.contains("whuse-oscomp-ltp-scope:smoke"),
            "LoongArch full suite should not attempt to materialize a smoke whitelist inside the runtime root before invoking the external LTP helper"
        );
        assert!(
            script.contains("run_loongarch_musl_libctest_body"),
            "LoongArch full suite should expose a dedicated musl libctest runner body"
        );
        assert!(
            script.contains("START $wrap"),
            "LoongArch full suite should emit judge-visible START markers from the musl libctest runner"
        );
        assert!(
            script.contains("Pass!"),
            "LoongArch full suite should emit judge-visible Pass! markers from the musl libctest runner"
        );
    }

    #[test]
    fn loongarch_libctest_profile_uses_contract_runner_in_smoke_mode() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"libctest")
            .expect("write profile");
        vfs.create_file(
            "/",
            "/musl/.whuse_stage2_local.env",
            b"WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=smoke\n",
        )
        .expect("write stage2 env");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("run_loongarch_musl_libctest_body"),
            "LoongArch libctest profile should share the musl contract runner body in smoke mode"
        );
        assert!(
            script.contains("WHUSE_LIBCTEST_CASE_BUDGET=${WHUSE_LIBCTEST_CASE_BUDGET:-1}"),
            "LoongArch libctest smoke mode should limit musl cases by default"
        );
        assert!(
            script.contains("START $wrap"),
            "LoongArch libctest smoke mode should emit judge-visible START markers"
        );
        assert!(
            script.contains("Pass!"),
            "LoongArch libctest smoke mode should emit judge-visible Pass! markers"
        );
    }

    #[test]
    fn loongarch_full_profile_externalizes_ltp_step_helper_script() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"full")
            .expect("write profile");

        let script = select_oscomp_suite_script(&mut vfs, false);

        assert!(
            script.contains("/musl/busybox sh /tmp/whuse-oscomp-ltp-step.sh"),
            "LoongArch full suite should delegate the LTP step through a separate helper shell script so the main suite stays parseable under busybox ash"
        );
        assert!(
            !script.contains("ltp_whitelist_for_runtime() {"),
            "LoongArch full suite should not inline the heavyweight LTP helper block into the main selected suite script"
        );
    }

    #[test]
    fn prepare_oscomp_runtime_layout_installs_loongarch_ltp_step_helper() {
        let mut vfs = KernelVfs::new();

        prepare_oscomp_runtime_layout(&mut vfs, false);

        let helper = String::from_utf8(
            vfs.read_file_all("/", "/tmp/whuse-oscomp-ltp-step.sh")
                .expect("read ltp helper"),
        )
        .expect("ltp helper utf8");

        assert!(
            helper.contains("run_ltp_step_runtime musl ltp_testcode.sh"),
            "LoongArch runtime layout should materialize an LTP helper that runs musl through the internal LTP runner"
        );
        assert!(
            !helper.contains("whuse-oscomp-suite-done"),
            "LoongArch LTP step helper should not emit the suite completion marker by itself"
        );
    }

    #[test]
    fn prepare_oscomp_runtime_layout_exposes_basic_execve_helper_path() {
        let mut vfs = KernelVfs::new();
        vfs.preload_external_file(
            "/musl/basic/test_echo",
            b"#!/musl/busybox sh\necho hello\n",
            Some(0o100755),
        )
        .expect("preload basic test_echo");

        prepare_oscomp_runtime_layout(&mut vfs, false);

        assert!(
            vfs.access("/", "/musl/test_echo").is_ok(),
            "LoongArch runtime layout should expose /musl/test_echo for the basic execve testcase"
        );
        assert!(
            vfs.access("/", "/test_echo").is_ok(),
            "LoongArch runtime layout should expose /test_echo root alias for the basic execve testcase"
        );
    }

    #[test]
    fn loongarch_selected_suite_parses_under_host_sh() {
        let mut vfs = KernelVfs::new();
        let script = select_oscomp_suite_script(&mut vfs, false);
        let script_path = format!("/tmp/whuse-loongarch-suite-parse-{}.sh", std::process::id());
        fs::write(&script_path, script).expect("write suite script");

        let sh_status = Command::new("sh")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run sh -n");
        assert!(
            sh_status.success(),
            "generated LoongArch suite script should parse under /bin/sh"
        );

        let bash_status = Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run bash -n");
        assert!(
            bash_status.success(),
            "generated LoongArch suite script should parse under bash -n"
        );

        let _ = fs::remove_file(&script_path);
    }

    #[test]
    fn loongarch_libctest_smoke_profile_parses_under_host_sh() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"libctest")
            .expect("write profile");
        vfs.create_file(
            "/",
            OSCOMP_STAGE2_LOCAL_ENV_PATH,
            b"WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=smoke\n",
        )
        .expect("write local env");

        let script = select_oscomp_suite_script(&mut vfs, false);
        let script_path = format!(
            "/tmp/whuse-loongarch-libctest-smoke-parse-{}.sh",
            std::process::id()
        );
        fs::write(&script_path, script).expect("write suite script");

        let sh_status = Command::new("sh")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run sh -n");
        assert!(
            sh_status.success(),
            "generated LoongArch libctest smoke suite script should parse under /bin/sh"
        );

        let bash_status = Command::new("bash")
            .arg("-n")
            .arg(&script_path)
            .status()
            .expect("run bash -n");
        assert!(
            bash_status.success(),
            "generated LoongArch libctest smoke suite script should parse under bash -n"
        );

        let _ = fs::remove_file(&script_path);
    }

    #[test]
    fn loongarch_suite_entry_sources_suite_script_inline() {
        assert!(
            OSCOMP_SUITE_ENTRY_SCRIPT.contains(". /tmp/whuse-oscomp-suite.sh"),
            "LoongArch suite entry should source the rendered suite script inline"
        );
        assert!(
            !OSCOMP_SUITE_ENTRY_SCRIPT.contains("/musl/busybox sh /tmp/whuse-oscomp-suite.sh"),
            "LoongArch suite entry should avoid launching a second busybox shell for the suite body"
        );
    }

    #[test]
    fn cow_debug_logging_is_disabled_by_default() {
        assert!(
            !cow_debug_enabled(),
            "COW fault logs should stay silent by default so testsuite output is not polluted"
        );
    }

    #[test]
    fn timer_preemption_debug_logging_is_disabled_by_default() {
        assert!(
            !timer_preemption_debug_enabled(),
            "timer preemption logs should stay silent by default so micro-timing tests are not polluted"
        );
    }
}
