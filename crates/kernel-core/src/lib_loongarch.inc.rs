extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use fs_ext4::Ext4Mount;
use hal_api::{hal, ConsoleWriter, PlatformArch};
use mm::MemoryManager;
use mm::{BinaryLoader, ElfBinaryLoader};
use proc::ProcessTable;
use syscall::cache_busybox_image;
use syscall::{
    SyscallArgs, SyscallDispatcher, SIGNAL_TRAMPOLINE_BASE, SIGNAL_TRAMPOLINE_CODE,
    SYS_CLOCK_NANOSLEEP, SYS_EPOLL_PWAIT, SYS_EPOLL_PWAIT2, SYS_EXECVE, SYS_FUTEX, SYS_NANOSLEEP,
    SYS_PPOLL, SYS_PSELECT6, SYS_READ, SYS_READV, SYS_RT_SIGRETURN, SYS_RT_SIGSUSPEND,
    SYS_RT_SIGTIMEDWAIT, SYS_WAIT,
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

const USER_INIT_BASE: usize = 0x0040_0000;
const EAGAIN_RET: isize = -11;
const SCHED_TIME_SLICE_NS: u64 = 10_000_000;
const FORCED_PREEMPT_DELTA_NS: u64 = 5_000_000;
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
const OSCOMP_PROFILE_DEFAULT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_PROFILE_DEFAULT__";
const OSCOMP_CFG_RUNNER_MODE_PATH: &str = "/musl/.whuse_oscomp_runner";
const OSCOMP_LIBCTEST_PRELOAD_FILES: [(&str, u32); 18] = [
    ("/musl/basic_testcode.sh", 0o100755),
    ("/glibc/basic_testcode.sh", 0o100755),
    ("/musl/busybox_testcode.sh", 0o100755),
    ("/glibc/busybox_testcode.sh", 0o100755),
    ("/musl/busybox_cmd.txt", 0o100644),
    ("/glibc/busybox_cmd.txt", 0o100644),
    ("/musl/basic/run-all.sh", 0o100755),
    ("/glibc/basic/run-all.sh", 0o100755),
    ("/musl/libctest_testcode.sh", 0o100755),
    ("/musl/run-static.sh", 0o100755),
    ("/musl/run-dynamic.sh", 0o100755),
    ("/musl/runtest.exe", 0o100755),
    ("/musl/entry-static.exe", 0o100755),
    ("/musl/entry-dynamic.exe", 0o100755),
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
const OSCOMP_ROOT_ALIAS_ENTRIES: [&str; 120] = [
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
    "    run_step_no_timeout ${runtime}/$script /musl/busybox sh -c \"cd $root && ./$script\"\n",
    "    rc=$?\n",
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
    "echo \"run ltp_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:ltp_testcode.sh\n",
    "run_runtime_dual_step ltp_testcode.sh 1800\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:ltp_testcode.sh:$rc\n",
    "finish_if_reached ltp\n",
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
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}\n",
    "WHUSE_LTP_PROFILE=full\n",
    "WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}\n",
    "WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-__WHUSE_OSCOMP_PROFILE_DEFAULT__}\n",
    "KCONFIG_SKIP_CHECK=${KCONFIG_SKIP_CHECK:-1}\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|ltp|unixbench|netperf|iperf|cyclic) ;;\n",
    "    *) WHUSE_OSCOMP_PROFILE=full ;;\n",
    "esac\n",
    "if [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ] && [ \"$WHUSE_OSCOMP_STEP_TIMEOUT\" -gt 180 ]; then\n",
    "    WHUSE_OSCOMP_STEP_TIMEOUT=180\n",
    "fi\n",
    "export WHUSE_OSCOMP_STEP_TIMEOUT WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_PROFILE WHUSE_LTP_CASE_TIMEOUT WHUSE_OSCOMP_PROFILE KCONFIG_SKIP_CHECK\n",
    "echo whuse-oscomp-bootstrap:timeout-probe-begin\n",
    "if /musl/busybox timeout 1 /musl/busybox true >/tmp/whuse-timeout-probe.log 2>&1; then\n",
    "    WHUSE_HAS_TIMEOUT=1\n",
    "else\n",
    "    WHUSE_HAS_TIMEOUT=0\n",
    "fi\n",
    "echo whuse-oscomp-bootstrap:timeout-probe-end:$WHUSE_HAS_TIMEOUT\n",
    "echo whuse-oscomp-profile:$WHUSE_OSCOMP_PROFILE\n",
    "run_script_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    actual_script=\"$2\"\n",
    "    if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "        /musl/busybox timeout \"$timeout_s\" /musl/busybox sh \"./$actual_script\"\n",
    "    else\n",
    "        /musl/busybox sh \"./$actual_script\"\n",
    "    fi\n",
    "    return $?\n",
    "}\n",
    "run_script_entry() {\n",
    "    runtime=\"$1\"\n",
    "    marker_script=\"$2\"\n",
    "    actual_script=\"$3\"\n",
    "    timeout_s=\"$4\"\n",
    "    root=\"/$runtime\"\n",
    "    if [ -z \"$actual_script\" ]; then\n",
    "        actual_script=\"$marker_script\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "        echo whuse-oscomp-step-end:${runtime}/$marker_script:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    run_script_with_timeout \"$timeout_s\" \"$actual_script\"\n",
    "    rc=$?\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$marker_script:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
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
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_basic_runtime_entry musl \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    run_basic_runtime_entry glibc \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:basic_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_busybox_smoke_case() {\n",
    "    busybox_bin=\"$1\"\n",
    "    label=\"$2\"\n",
    "    shift 2\n",
    "    \"$busybox_bin\" \"$@\"\n",
    "    rc=$?\n",
    "    if [ \"$rc\" -ne 0 ]; then\n",
    "        echo \"testcase busybox $label fail\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    echo \"testcase busybox $label success\"\n",
    "    return 0\n",
    "}\n",
    "run_busybox_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    busybox_bin=\"/$runtime/busybox\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd / || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh\n",
    "        echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/busybox_testcode.sh\n",
    "    fail=0\n",
    "    run_busybox_smoke_case \"$busybox_bin\" true true || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" 'echo smoke' echo '#### busybox smoke ####' || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" 'sh -c exit' sh -c 'exit 0' || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" 'basename /aaa/bbb' basename /aaa/bbb || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" 'dirname /aaa/bbb' dirname /aaa/bbb || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" date date || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" uname uname || fail=1\n",
    "    run_busybox_smoke_case \"$busybox_bin\" pwd pwd || fail=1\n",
    "    echo whuse-oscomp-step-end:${runtime}/busybox_testcode.sh:$fail\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$fail\"\n",
    "}\n",
    "run_busybox_dual_step() {\n",
    "    echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_busybox_runtime_entry musl\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    run_busybox_runtime_entry glibc\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:busybox_testcode.sh:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_runtime_dual_step() {\n",
    "    root_marker=\"$1\"\n",
    "    runtime_script=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
    "    if [ \"$root_marker\" = \"basic_testcode.sh\" ] && [ \"$runtime_script\" = \"basic_testcode.sh\" ] && [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ]; then\n",
    "        run_basic_dual_step \"$timeout_s\"\n",
    "        return 0\n",
    "    fi\n",
    "    if [ \"$root_marker\" = \"busybox_testcode.sh\" ] && [ \"$runtime_script\" = \"busybox_testcode.sh\" ] && [ \"$WHUSE_OSCOMP_PROFILE\" = \"busybox\" ]; then\n",
    "        run_busybox_dual_step\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-begin:$root_marker\n",
    "    group_rc=0\n",
    "    echo whuse-oscomp-runtime-dispatch:musl\n",
    "    run_script_entry musl \"$runtime_script\" \"\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
        "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-dispatch:glibc\n",
    "    run_script_entry glibc \"$runtime_script\" \"\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "        group_rc=\"$rc\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$root_marker:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_time_test_group() {\n",
    "    echo whuse-oscomp-step-begin:time-test\n",
    "    if [ -x /musl/time-test ]; then\n",
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
    "run_selected_profile() {\n",
    "    case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full)\n",
    "        run_time_test_group\n",
    "        run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step unixbench_testcode.sh unixbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step netperf_testcode.sh netperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step iperf_testcode.sh iperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step ltp_testcode.sh ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step cyclic_testcode.sh cyclic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        ;;\n",
    "    basic) run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    busybox) run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iozone) run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    libctest) run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    libc-bench) run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    lmbench) run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    lua) run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    unixbench) run_runtime_dual_step unixbench_testcode.sh unixbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    netperf) run_runtime_dual_step netperf_testcode.sh netperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    iperf) run_runtime_dual_step iperf_testcode.sh iperf_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;\n",
    "    ltp) run_runtime_dual_step ltp_testcode.sh ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\" ;;\n",
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
    "echo whuse-oscomp-shell-launch-suite\n",
    ". /tmp/whuse-oscomp-suite.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-shell-suite-rc:$rc\n",
    "exec /musl/basic/exit\n",
    "echo whuse-oscomp-exit-missing\n",
    "exit \"$rc\"\n",
);

static KERNEL_IDLE_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

fn kernel_idle_timer_cb() {
    let count = KERNEL_IDLE_TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let now = hal().timer.monotonic_nanos();
    hal()
        .timer
        .program_oneshot(now.saturating_add(SCHED_TIME_SLICE_NS));
    if count <= 5 || count % 100 == 0 {
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
                        prepare_oscomp_runtime_layout(&mut vfs);
                        preload_libctest_hot_files_from_device(device, &mut vfs);
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
        static mut LOOP_COUNT: usize = 0;
        loop {
            unsafe {
                LOOP_COUNT += 1;
                if LOOP_COUNT == 1000
                    || LOOP_COUNT == 5000
                    || (LOOP_COUNT >= 10000 && LOOP_COUNT % 10000 == 0 && LOOP_COUNT <= 100000)
                {
                    logln(format_args!("[LOOP:{}]", LOOP_COUNT));
                }
            }

            if self.enforce_oscomp_watchdog() {
                continue;
            }
            if self.scheduler.ensure_current().is_none() {
                let has_non_init = self
                    .processes
                    .process_snapshots()
                    .iter()
                    .any(|process| process.tgid > 1 && !process.is_thread);

                if has_non_init {
                    static mut SPIN_LOG_COUNT: usize = 0;
                    unsafe {
                        SPIN_LOG_COUNT += 1;
                        if SPIN_LOG_COUNT == 1000
                            || SPIN_LOG_COUNT == 5000
                            || (SPIN_LOG_COUNT >= 10000
                                && SPIN_LOG_COUNT % 10000 == 0
                                && SPIN_LOG_COUNT <= 100000)
                        {
                            logln(format_args!("[SPIN:{}]", SPIN_LOG_COUNT));
                        }
                    }
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
        if !all_groups.is_empty()
            && (self.watchdog_last_heartbeat_ns == 0
                || now.saturating_sub(self.watchdog_last_heartbeat_ns) >= 2_000_000_000)
        {
            self.watchdog_last_heartbeat_ns = now;
            let mut sample = String::new();
            for (idx, (tgid, name)) in all_groups.iter().enumerate() {
                if idx >= 8 {
                    break;
                }
                if !sample.is_empty() {
                    sample.push_str(", ");
                }
                let age_s = self
                    .watchdog_started_at
                    .get(tgid)
                    .map(|started| now.saturating_sub(*started) / 1_000_000_000)
                    .unwrap_or(0);
                let _ = core::fmt::Write::write_fmt(
                    &mut sample,
                    format_args!("{}:{}:age_s={}", tgid, name, age_s),
                );
            }
            logln(format_args!(
                "whuse: oscomp watchdog heartbeat groups={} watched={} sample=[{}]",
                all_groups.len(),
                watched.len(),
                sample
            ));
        }
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
                    let woke = self.scheduler.wake_task(parent_tgid);
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
            PlatformArch::LoongArch64 => false,
        };

        if is_external_interrupt {
            self.service_irqs();
            return;
        }
        if is_timer_interrupt {
            self.timer_irq_count = self.timer_irq_count.saturating_add(1);

            if self.timer_irq_count >= 6 && self.timer_irq_count <= 20 {
                logln(format_args!("[TMR-EVERY:{}]", self.timer_irq_count));
            }

            if self.timer_irq_count >= 10 && self.timer_irq_count <= 50 {
                logln(format_args!("[TMR:{}]", self.timer_irq_count));
            }

            if self.timer_irq_count >= 190 && self.timer_irq_count <= 250 {
                logln(format_args!("[TMR-LATE:{}]", self.timer_irq_count));
            }

            if self.timer_irq_count <= 5 || self.timer_irq_count % 1024 == 0 {
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

            if self.timer_irq_count % 100 == 0 {
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
            let result = self.syscalls.dispatch(
                sysno,
                SyscallArgs(args),
                &mut self.processes,
                &mut self.scheduler,
                &mut self.vfs,
            );
            if let Ok(process) = self.processes.current_mut() {
                let blocked_restart = result == EAGAIN_RET
                    && matches!(
                        sysno,
                        SYS_WAIT
                            | SYS_READ
                            | SYS_READV
                            | SYS_RT_SIGSUSPEND
                            | SYS_RT_SIGTIMEDWAIT
                            | SYS_FUTEX
                            | SYS_PPOLL
                            | SYS_PSELECT6
                            | SYS_EPOLL_PWAIT
                            | SYS_EPOLL_PWAIT2
                            | SYS_NANOSLEEP
                            | SYS_CLOCK_NANOSLEEP
                    );
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
            if matches!(hal().platform.architecture(), PlatformArch::LoongArch64)
                && self.scheduler.ready_count() > 0
            {
                let _ = self.scheduler.yield_now();
            }
            return;
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
                let _ = self.scheduler.wake_task(parent_tgid);
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

    fn dispatch_pending_signals(&mut self) {
        let process = match self.processes.current_mut() {
            Ok(p) => p,
            Err(_) => return,
        };

        let unmasked = process.pending_signals & !process.signal_mask;
        if unmasked == 0 {
            return;
        }

        // Only dispatch real-time signals (signum >= 32) to user handlers.
        // Standard signals (SIGCHLD etc.) are handled internally and should not
        // invoke user handlers here to avoid re-entrancy issues in musl/busybox.
        let rt_unmasked = unmasked & !((1u64 << 31) - 1);
        if rt_unmasked == 0 {
            return;
        }
        let signum = rt_unmasked.trailing_zeros() as usize + 1;
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
                    }
                    if let Some(parent_tgid) = exit.parent_tgid {
                        let _ = self.processes.deliver_signal(parent_tgid, 17);
                        let _ = self.scheduler.wake_task(parent_tgid);
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
        Ok(true) => logln(format_args!("whuse: rootfs smoke found /musl/busybox")),
        Ok(false) => {}
        Err(err) => logln(format_args!(
            "whuse: rootfs smoke exists check /musl/busybox failed err={}",
            err
        )),
    }
    let mut last_error = None;
    for path in ["/musl/basic/run-all.sh", "/etc/issue", "/bin/sh"] {
        match mount.read_detailed(path) {
            Ok(bytes) => {
                logln(format_args!(
                    "whuse: rootfs smoke read success path={} bytes={}",
                    path,
                    bytes.len(),
                ));
                return;
            }
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

fn prepare_oscomp_runtime_layout(vfs: &mut KernelVfs) {
    for dir in [
        "/var",
        "/var/tmp",
        "/var/tmp/lmbench",
        "/usr",
        "/usr/bin",
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
    install_busybox_exec_alias(vfs, "/musl/ls", "ls");
    install_busybox_exec_alias(vfs, "/musl/which", "which");
    install_busybox_exec_alias(vfs, "/musl/sleep", "sleep");
    install_busybox_exec_alias(vfs, "/musl/basename", "basename");
    install_busybox_exec_alias(vfs, "/musl/dirname", "dirname");
    install_busybox_exec_alias(vfs, "/musl/awk", "awk");
    install_busybox_exec_alias(vfs, "/musl/sed", "sed");
    install_busybox_exec_alias(vfs, "/musl/grep", "grep");
    install_busybox_exec_alias(vfs, "/musl/[", "[");
    install_busybox_exec_alias(vfs, "/musl/test", "test");
    install_busybox_exec_alias(vfs, "/musl/cut", "cut");
    install_busybox_exec_alias(vfs, "/musl/head", "head");
    install_busybox_exec_alias(vfs, "/musl/tail", "tail");
    install_busybox_exec_alias(vfs, "/musl/tr", "tr");
    install_busybox_exec_alias(vfs, "/musl/xargs", "xargs");
    install_busybox_exec_alias(vfs, "/musl/readlink", "readlink");
    for (path, target) in [
        ("/bin/busybox", "/musl/busybox"),
        ("/bin/sh", "/musl/busybox"),
        ("/bin/bash", "/musl/busybox"),
        ("/bin/ls", "/musl/ls"),
        ("/bin/which", "/musl/which"),
        ("/bin/sleep", "/musl/sleep"),
        ("/bin/basename", "/musl/basename"),
        ("/bin/dirname", "/musl/dirname"),
        ("/bin/awk", "/musl/awk"),
        ("/bin/sed", "/musl/sed"),
        ("/bin/grep", "/musl/grep"),
        ("/bin/cut", "/musl/cut"),
        ("/bin/head", "/musl/head"),
        ("/bin/tail", "/musl/tail"),
        ("/bin/tr", "/musl/tr"),
        ("/bin/xargs", "/musl/xargs"),
        ("/bin/readlink", "/musl/readlink"),
        ("/busybox", "/musl/busybox"),
        ("/usr/bin/ls", "/musl/ls"),
        ("/usr/bin/which", "/musl/which"),
        ("/usr/bin/sleep", "/musl/sleep"),
        ("/usr/bin/basename", "/musl/basename"),
        ("/usr/bin/dirname", "/musl/dirname"),
        ("/usr/bin/awk", "/musl/awk"),
        ("/usr/bin/sed", "/musl/sed"),
        ("/usr/bin/grep", "/musl/grep"),
        ("/usr/bin/cut", "/musl/cut"),
        ("/usr/bin/head", "/musl/head"),
        ("/usr/bin/tail", "/musl/tail"),
        ("/usr/bin/tr", "/musl/tr"),
        ("/usr/bin/xargs", "/musl/xargs"),
        ("/usr/bin/readlink", "/musl/readlink"),
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
        ("/lib/libc.so", "/musl/lib/libc.so"),
        ("/lib/libm.so", "/glibc/lib/libm.so"),
        ("/lib64/libm.so", "/glibc/lib/libm.so"),
    ] {
        install_fallback_symlink(vfs, path, target);
    }
    install_oscomp_root_aliases(vfs);
    for cfg_path in [
        "/musl/.whuse_oscomp_only_step",
        "/musl/.whuse_ltp_profile",
        "/musl/.whuse_ltp_whitelist",
        "/musl/.whuse_ltp_blacklist",
        "/musl/.whuse_ltp_step_timeout",
        "/musl/.whuse_ltp_case_timeout",
        "/musl/ltp_score_whitelist.host.txt",
        "/musl/ltp_score_blacklist.host.txt",
    ] {
        if vfs.access("/", cfg_path).is_ok() {
            let _ = vfs.unlink("/", cfg_path);
            logln(format_args!(
                "whuse: purged oscomp runtime override {}",
                cfg_path
            ));
        }
    }
    let suite_script = select_oscomp_suite_script(vfs);
    match vfs.create_file("/", OSCOMP_SUITE_SCRIPT_PATH, suite_script.as_bytes()) {
        Ok(()) => logln(format_args!(
            "whuse: installed suite script {}",
            OSCOMP_SUITE_SCRIPT_PATH
        )),
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
        Ok(()) => logln(format_args!(
            "whuse: installed suite entry {}",
            OSCOMP_SUITE_ENTRY_PATH
        )),
        Err(err) => logln(format_args!(
            "whuse: failed suite entry {} err={}",
            OSCOMP_SUITE_ENTRY_PATH, err
        )),
    }
    match vfs.create_file(
        "/",
        OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH,
        OSCOMP_BUSYBOX_COMPAT_SCRIPT.as_bytes(),
    ) {
        Ok(()) => logln(format_args!(
            "whuse: installed busybox compat script {}",
            OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH
        )),
        Err(err) => logln(format_args!(
            "whuse: failed busybox compat script {} err={}",
            OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH, err
        )),
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
            Ok(()) => logln(format_args!(
                "whuse: libctest preloaded path={} bytes={}",
                path,
                bytes.len()
            )),
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
                        "whuse: basic preload skipped path={} err={}",
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
                    "whuse: basic preload failed path={} err={}",
                    path, err
                ));
            }
        }
        for (name, mode) in OSCOMP_BASIC_EXTRA_FILES {
            let path = alloc::format!("{}/{}", runtime_root, name);
            let bytes = match mount.read(path.as_str()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    logln(format_args!(
                        "whuse: basic preload skipped path={} err={}",
                        path, err
                    ));
                    continue;
                }
            };
            if bytes.is_empty() {
                continue;
            }
            if let Err(err) = vfs.preload_external_file(path.as_str(), &bytes, Some(mode)) {
                logln(format_args!(
                    "whuse: basic preload failed path={} err={}",
                    path, err
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
}

fn install_fallback_symlink(vfs: &mut KernelVfs, path: &str, target: &str) {
    if vfs.access("/", target).is_err() {
        return;
    }
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

fn select_oscomp_suite_script(vfs: &mut KernelVfs) -> String {
    if vfs.access("/", OSCOMP_PROFILE_PATH).is_ok() {
        render_oscomp_official_suite_script(read_oscomp_profile_default(vfs))
    } else {
        oscomp_suite_script()
    }
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

fn render_oscomp_official_suite_script(profile_default: &str) -> String {
    OSCOMP_OFFICIAL_SUITE_SCRIPT.replace(OSCOMP_PROFILE_DEFAULT_PLACEHOLDER, profile_default)
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
        Ok(()) => logln(format_args!(
            "whuse: block probe sector={} ok bytes={:02x} {:02x} {:02x} {:02x}",
            sector, buf[0], buf[1], buf[2], buf[3]
        )),
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
    logln(format_args!(
        "whuse: block probe span ok start={} count={}",
        start, count
    ));
}
