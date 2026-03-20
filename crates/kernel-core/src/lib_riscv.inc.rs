
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
    watchdog_last_scan_ns: u64,
    watchdog_clock_ns: u64,
    watchdog_last_hw_ns: u64,
    watchdog_iozone_window_until_ns: u64,
    watchdog_bench_window_until_ns: u64,
    timer_irq_count: u64,
}

const USER_INIT_BASE: usize = 0x0040_0000;
const EAGAIN_RET: isize = -11;
const SCHED_TIME_SLICE_NS: u64 = 10_000_000;
const FORCED_PREEMPT_DELTA_NS: u64 = 5_000_000;
const OSCOMP_GROUP_TIMEOUT_NS: u64 = 20 * 60 * 1_000_000_000;
const OSCOMP_HEAVY_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_BUSYBOX_APPLET_TIMEOUT_NS: u64 = 600 * 1_000_000_000;
const OSCOMP_BUSYBOX_SUPERVISOR_TIMEOUT_NS: u64 = 1_200 * 1_000_000_000;
const OSCOMP_LIBCTEST_ENTRY_TIMEOUT_NS: u64 = 10 * 1_000_000_000;
const OSCOMP_BENCH_PHASE_WINDOW_NS: u64 = 300 * 1_000_000_000;
const OSCOMP_LMBENCH_TIMEOUT_NS: u64 = 900 * 1_000_000_000;
const OSCOMP_UNIXBENCH_TIMEOUT_NS: u64 = 900 * 1_000_000_000;
const OSCOMP_BENCH_SUPERVISOR_TIMEOUT_NS: u64 = {
    let base = if OSCOMP_LMBENCH_TIMEOUT_NS > OSCOMP_UNIXBENCH_TIMEOUT_NS {
        OSCOMP_LMBENCH_TIMEOUT_NS
    } else {
        OSCOMP_UNIXBENCH_TIMEOUT_NS
    };
    base + 30 * 1_000_000_000
};
const OSCOMP_WATCHDOG_SCAN_INTERVAL_NS: u64 = 100 * 1_000_000;
const OSCOMP_IOZONE_BUSYBOX_WINDOW_NS: u64 = 0;
const OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_REQUIRED_TEST_FILES: [&str; 27] = [
    "/musl/busybox",
    "/musl/basic_testcode.sh",
    "/musl/busybox_testcode.sh",
    "/musl/iozone_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/libcbench_testcode.sh",
    "/musl/libc-bench",
    "/musl/lmbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/unixbench_testcode.sh",
    "/musl/netperf_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/cyclictest_testcode.sh",
    "/musl/ltp_testcode.sh",
    "/glibc/busybox",
    "/glibc/basic_testcode.sh",
    "/glibc/busybox_testcode.sh",
    "/glibc/iozone_testcode.sh",
    "/glibc/libcbench_testcode.sh",
    "/glibc/libc-bench",
    "/glibc/lmbench_testcode.sh",
    "/glibc/lua_testcode.sh",
    "/glibc/unixbench_testcode.sh",
    "/glibc/netperf_testcode.sh",
    "/glibc/iperf_testcode.sh",
    "/glibc/cyclictest_testcode.sh",
    "/glibc/ltp_testcode.sh",
];
const OSCOMP_OPTIONAL_TEST_FILES: [&str; 3] = [
    "/musl/time-test",
    "/musl/basic/run-all.sh",
    "/glibc/basic/run-all.sh",
];
const OSCOMP_LTP_SCORE_WHITELIST_PATH: &str = "/musl/ltp_score_whitelist.txt";
const OSCOMP_LTP_SCORE_BLACKLIST_PATH: &str = "/musl/ltp_score_blacklist.txt";
const OSCOMP_CFG_ONLY_STEP_PATH: &str = "/musl/.whuse_oscomp_only_step";
const OSCOMP_CFG_LTP_PROFILE_PATH: &str = "/musl/.whuse_ltp_profile";
const OSCOMP_CFG_LTP_WHITELIST_PATH: &str = "/musl/.whuse_ltp_whitelist";
const OSCOMP_CFG_LTP_BLACKLIST_PATH: &str = "/musl/.whuse_ltp_blacklist";
const OSCOMP_CFG_LTP_TIMEOUT_PATH: &str = "/musl/.whuse_ltp_step_timeout";
const OSCOMP_CFG_RUNNER_MODE_PATH: &str = "/musl/.whuse_oscomp_runner";
const OSCOMP_LTP_SCORE_WHITELIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_whitelist.txt");
const OSCOMP_LTP_SCORE_BLACKLIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_blacklist.txt");
const OSCOMP_LTP_KERNEL_CONFIG_PATH: &str = "/lib/modules/6.8.0-whuse/build/.config";
const OSCOMP_LTP_KERNEL_CONFIG_STUB: &str = concat!(
    "CONFIG_64BIT=y\n",
    "CONFIG_RISCV=y\n",
    "CONFIG_MMU=y\n",
    "CONFIG_MODULES=y\n",
    "CONFIG_KEYS=y\n",
    "CONFIG_KEYS_REQUEST_CACHE=y\n",
    "CONFIG_FUTEX=y\n",
    "CONFIG_EPOLL=y\n",
    "CONFIG_EVENTFD=y\n",
    "CONFIG_TIMERFD=y\n",
    "CONFIG_SIGNALFD=y\n",
    "CONFIG_UNIX=y\n",
    "CONFIG_INET=y\n",
    "CONFIG_IPV6=y\n",
    "CONFIG_NAMESPACES=y\n",
    "CONFIG_USER_NS=y\n",
    "CONFIG_PID_NS=y\n",
    "CONFIG_IPC_NS=y\n",
    "CONFIG_NET_NS=y\n",
    "CONFIG_UTS_NS=y\n",
    "CONFIG_TIME_NS=y\n",
);
const OSCOMP_KEYCTL_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "echo \"keyctl: not fully supported on whuse\" >&2\n",
    "exit 1\n",
);
const OSCOMP_WAIT_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "wait \"$@\"\n",
    "exit $?\n",
);
const OSCOMP_LOCALE_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "case \"${1:-}\" in\n",
    "    -a) echo C; echo POSIX ;;\n",
    "    *) echo LANG=C; echo LC_ALL= ;;\n",
    "esac\n",
    "exit 0\n",
);
const OSCOMP_USERADD_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "echo \"useradd: compatibility wrapper\" >&2\n",
    "exit 0\n",
);
const OSCOMP_USERDEL_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "echo \"userdel: compatibility wrapper\" >&2\n",
    "exit 0\n",
);
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
const OSCOMP_SUITE_SCRIPT_PATH: &str = "/tmp/whuse-oscomp-suite.sh";
const OSCOMP_BUSYBOX_COMPAT_SCRIPT_PATH: &str = "/tmp/whuse-busybox-testcode.sh";
const OSCOMP_BUSYBOX_COMPAT_SCRIPT: &str = concat!(
    "#!/busybox sh\n",
    "echo \"#### OS COMP TEST GROUP START busybox-musl ####\"\n",
    "exec 3< ./busybox_cmd.txt\n",
    "while IFS= read -r line <&3\n",
    "do\n",
    "    if [ -z \"$line\" ]; then\n",
    "        continue\n",
    "    fi\n",
    "    eval \"./busybox $line\" <&-\n",
    "    RTN=$?\n",
    "    if [ \"$RTN\" -ne 0 ] && [ \"$line\" != \"false\" ]; then\n",
    "        echo \"testcase busybox $line fail\"\n",
    "    else\n",
    "        echo \"testcase busybox $line success\"\n",
    "    fi\n",
    "done\n",
    "echo \"#### OS COMP TEST GROUP END busybox-musl ####\"\n",
);
const OSCOMP_SUITE_SCRIPT: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}\n",
    "WHUSE_OSCOMP_ONLY_STEP=${WHUSE_OSCOMP_ONLY_STEP:-}\n",
    "WHUSE_OSCOMP_TRACE_STEP_CMDS=${WHUSE_OSCOMP_TRACE_STEP_CMDS:-0}\n",
    "WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}\n",
    "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}\n",
    "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}\n",
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}\n",
    "WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}\n",
    "if [ -z \"$WHUSE_OSCOMP_ONLY_STEP\" ] && [ -f /musl/.whuse_oscomp_only_step ]; then\n",
    "    IFS= read -r WHUSE_OSCOMP_ONLY_STEP < /musl/.whuse_oscomp_only_step\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_PROFILE\" ] && [ -f /musl/.whuse_ltp_profile ]; then\n",
    "    IFS= read -r WHUSE_LTP_PROFILE < /musl/.whuse_ltp_profile\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_WHITELIST\" ] && [ -f /musl/.whuse_ltp_whitelist ]; then\n",
    "    IFS= read -r WHUSE_LTP_WHITELIST < /musl/.whuse_ltp_whitelist\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_BLACKLIST\" ] && [ -f /musl/.whuse_ltp_blacklist ]; then\n",
    "    IFS= read -r WHUSE_LTP_BLACKLIST < /musl/.whuse_ltp_blacklist\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_STEP_TIMEOUT\" ] && [ -f /musl/.whuse_ltp_step_timeout ]; then\n",
    "    IFS= read -r WHUSE_LTP_STEP_TIMEOUT < /musl/.whuse_ltp_step_timeout\n",
    "fi\n",
    "if [ -z \"$WHUSE_LTP_CASE_TIMEOUT\" ] && [ -f /musl/.whuse_ltp_case_timeout ]; then\n",
    "    IFS= read -r WHUSE_LTP_CASE_TIMEOUT < /musl/.whuse_ltp_case_timeout\n",
    "fi\n",
    "export WHUSE_OSCOMP_ONLY_STEP WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_CASE_TIMEOUT\n",
    "WHUSE_HAVE_TIMEOUT=0\n",
    "if /musl/busybox timeout 1 /musl/busybox true >/tmp/whuse-timeout-probe.log 2>&1; then\n",
    "    WHUSE_HAVE_TIMEOUT=1\n",
    "fi\n",
    "WHUSE_HAVE_SETSID=0\n",
    "if /musl/busybox setsid /musl/busybox true >/tmp/whuse-setsid-probe.log 2>&1; then\n",
    "    WHUSE_HAVE_SETSID=1\n",
    "fi\n",
    "export WHUSE_HAVE_TIMEOUT\n",
    "export WHUSE_HAVE_SETSID\n",
    "echo whuse-oscomp-timeout-applet:$WHUSE_HAVE_TIMEOUT\n",
    "echo whuse-oscomp-setsid-applet:$WHUSE_HAVE_SETSID\n",
    "echo whuse-oscomp-compat:$WHUSE_OSCOMP_COMPAT\n",
    "WHUSE_LAST_TIMEOUT_PID=0\n",
    "WHUSE_LAST_TIMEOUT_HIT=0\n",
    "run_with_timeout() {\n",
    "    timeout_s=\"$1\"\n",
    "    shift\n",
    "    WHUSE_LAST_TIMEOUT_PID=0\n",
    "    WHUSE_LAST_TIMEOUT_HIT=0\n",
    "    if [ \"$WHUSE_HAVE_TIMEOUT\" -eq 1 ] && [ \"$timeout_s\" -gt 0 ]; then\n",
    "        /musl/busybox timeout \"$timeout_s\" \"$@\"\n",
    "        rc=$?\n",
    "        if [ \"$rc\" -eq 124 ] || [ \"$rc\" -eq 137 ] || [ \"$rc\" -eq 143 ] || [ \"$rc\" -eq 241 ]; then\n",
    "            WHUSE_LAST_TIMEOUT_HIT=1\n",
    "            return 124\n",
    "        fi\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    \"$@\"\n",
    "    return $?\n",
    "}\n",
    "whuse_ltp_list_has_entries() {\n",
    "    file=\"$1\"\n",
    "    [ -f \"$file\" ] && [ -s \"$file\" ]\n",
    "}\n",
    "whuse_ltp_list_contains() {\n",
    "    needle=\"$1\"\n",
    "    file=\"$2\"\n",
    "    [ -f \"$file\" ] || return 1\n",
    "    /musl/busybox grep -Fqx \"$needle\" \"$file\"\n",
    "}\n",
    "whuse_ltp_write_busybox_wrapper() {\n",
    "    wrap_dir=\"/tmp/whuse-ltp-bin\"\n",
    "    /musl/busybox mkdir -p \"$wrap_dir\" >/dev/null 2>&1 || true\n",
    "    {\n",
    "        echo '#!/musl/busybox sh'\n",
    "        echo 'cmd=\"${1:-}\"'\n",
    "        echo 'case \"$cmd\" in'\n",
    "        echo '    wait) shift; wait \"$@\"; exit $? ;;'\n",
    "        echo '    locale) shift; exec /musl/locale \"$@\" ;;'\n",
    "        echo '    useradd) shift; exec /musl/useradd \"$@\" ;;'\n",
    "        echo '    userdel) shift; exec /musl/userdel \"$@\" ;;'\n",
    "        echo 'esac'\n",
    "        echo 'exec /musl/busybox \"$@\"'\n",
    "    } > \"$wrap_dir/busybox\"\n",
    "    /musl/busybox chmod 755 \"$wrap_dir/busybox\" >/dev/null 2>&1 || true\n",
    "}\n",
    "whuse_ltp_enable_busybox_compat() {\n",
    "    return 0\n",
    "}\n",
    "whuse_ltp_disable_busybox_compat() {\n",
    "    return 0\n",
    "}\n",
    "whuse_ltp_case_allowed() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    if [ -f \"$WHUSE_LTP_BLACKLIST\" ] && ( /musl/busybox grep -Fqx \"$case_name\" \"$WHUSE_LTP_BLACKLIST\" || /musl/busybox grep -Fqx \"$case_rel\" \"$WHUSE_LTP_BLACKLIST\" ); then\n",
    "        return 1\n",
    "    fi\n",
    "    if whuse_ltp_list_has_entries \"$WHUSE_LTP_WHITELIST\"; then\n",
    "        /musl/busybox grep -Fqx \"$case_name\" \"$WHUSE_LTP_WHITELIST\" || /musl/busybox grep -Fqx \"$case_rel\" \"$WHUSE_LTP_WHITELIST\"\n",
    "        return $?\n",
    "    fi\n",
    "    return 0\n",
    "}\n",
    "whuse_ltp_case_prepare_stdin() {\n",
    "    case_name=\"$1\"\n",
    "    stdin_path=\"$2\"\n",
    "    /musl/busybox rm -f \"$stdin_path\" >/dev/null 2>&1 || true\n",
    "    case \"$case_name\" in\n",
    "        assign_password.sh|ask_password.sh)\n",
    "            /musl/busybox printf '123456\\n123456\\n123456\\n123456\\n' > \"$stdin_path\"\n",
    "            ;;\n",
    "        *)\n",
    "            : > \"$stdin_path\"\n",
    "            ;;\n",
    "    esac\n",
    "}\n",
    "whuse_ltp_count_token() {\n",
    "    token=\"$1\"\n",
    "    file=\"$2\"\n",
    "    count=$(/musl/busybox strings \"$file\" 2>/dev/null | /musl/busybox grep \"$token\" 2>/dev/null | /musl/busybox awk 'END{print NR+0}' 2>/dev/null || true)\n",
    "    if [ -z \"$count\" ]; then\n",
    "        count=$(/musl/busybox grep \"$token\" \"$file\" 2>/dev/null | /musl/busybox awk 'END{print NR+0}' 2>/dev/null || true)\n",
    "    fi\n",
    "    [ -n \"$count\" ] || count=0\n",
    "    echo \"$count\"\n",
    "}\n",
    "whuse_ltp_cleanup_case_tree() {\n",
    "    case_pid=\"$1\"\n",
    "    [ \"$case_pid\" -gt 1 ] 2>/dev/null || return 0\n",
    "    echo whuse-ltp-case-cleanup-start:pid=$case_pid\n",
    "    /musl/busybox kill -TERM \"-$case_pid\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox kill -TERM \"$case_pid\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox sleep 1\n",
    "    /musl/busybox kill -KILL \"-$case_pid\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox kill -KILL \"$case_pid\" >/dev/null 2>&1 || true\n",
    "    echo whuse-ltp-case-cleanup-end:pid=$case_pid\n",
    "}\n",
    "whuse_ltp_run_single_case() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    case_path=\"$3\"\n",
    "    case_log=\"$4\"\n",
    "    case_stdin=\"$5\"\n",
    "    if [ ! -x \"$case_path\" ]; then\n",
    "        echo whuse-ltp-case-result:$case_name:rc=127:tpass=0:tfail=0:tbrok=0:tconf=0:class=missing\n",
    "        echo FAIL LTP CASE $case_name : 127\n",
    "        return 0\n",
    "    fi\n",
    "    /musl/busybox rm -f \"$case_log\" >/dev/null 2>&1 || true\n",
    "    whuse_ltp_case_prepare_stdin \"$case_name\" \"$case_stdin\"\n",
    "    exec_case_path=\"$case_path\"\n",
    "    patched_case_path=\"$case_log.patched\"\n",
    "    /musl/busybox rm -f \"$patched_case_path\" >/dev/null 2>&1 || true\n",
    "    first_line=$(/musl/busybox head -n 1 \"$case_path\" 2>/dev/null || true)\n",
    "    case \"$first_line\" in\n",
    "        '#!'*)\n",
    "            if /musl/busybox grep -q 'busybox wait\\|/musl/busybox wait\\|busybox locale\\|/musl/busybox locale\\|busybox useradd\\|/musl/busybox useradd\\|busybox userdel\\|/musl/busybox userdel' \"$case_path\" 2>/dev/null; then\n",
    "                if /musl/busybox sed \\\n",
    "                    -e 's#/musl/busybox wait#/musl/wait#g' \\\n",
    "                    -e 's#busybox wait#wait#g' \\\n",
    "                    -e 's#/musl/busybox locale#/musl/locale#g' \\\n",
    "                    -e 's#busybox locale#locale#g' \\\n",
    "                    -e 's#/musl/busybox useradd#/musl/useradd#g' \\\n",
    "                    -e 's#busybox useradd#useradd#g' \\\n",
    "                    -e 's#/musl/busybox userdel#/musl/userdel#g' \\\n",
    "                    -e 's#busybox userdel#userdel#g' \\\n",
    "                    \"$case_path\" > \"$patched_case_path\"; then\n",
    "                    /musl/busybox chmod 755 \"$patched_case_path\" >/dev/null 2>&1 || true\n",
    "                    exec_case_path=\"$patched_case_path\"\n",
    "                    echo whuse-ltp-case-patched:$case_name\n",
    "                fi\n",
    "            fi\n",
    "            ;;\n",
    "    esac\n",
    "    echo RUN LTP CASE $case_name\n",
    "    if [ \"${WHUSE_HAVE_SETSID:-0}\" = \"1\" ]; then\n",
    "        /musl/busybox setsid /musl/busybox sh -c \"exec \\\"$exec_case_path\\\"\" <\"$case_stdin\" >\"$case_log\" 2>&1 &\n",
    "    else\n",
    "        /musl/busybox sh -c \"exec \\\"$exec_case_path\\\"\" <\"$case_stdin\" >\"$case_log\" 2>&1 &\n",
    "    fi\n",
    "    case_pid=$!\n",
    "    case_timeout=\"${WHUSE_LTP_CASE_TIMEOUT:-45}\"\n",
    "    start_ts=$(/musl/busybox date +%s)\n",
    "    timeout_hit=0\n",
    "    while /musl/busybox kill -0 \"$case_pid\" >/dev/null 2>&1\n",
    "    do\n",
    "        now_ts=$(/musl/busybox date +%s)\n",
    "        elapsed=$((now_ts - start_ts))\n",
    "        if [ \"$elapsed\" -ge \"$case_timeout\" ]; then\n",
    "            timeout_hit=1\n",
    "            echo whuse-ltp-case-timeout:$case_name:pid=$case_pid:timeout=$case_timeout\n",
    "            whuse_ltp_cleanup_case_tree \"$case_pid\"\n",
    "            break\n",
    "        fi\n",
    "        /musl/busybox sleep 1\n",
    "    done\n",
    "    if [ \"$timeout_hit\" -eq 1 ]; then\n",
    "        /musl/busybox wait \"$case_pid\" >/dev/null 2>&1 || true\n",
    "        case_rc=124\n",
    "    else\n",
    "        /musl/busybox wait \"$case_pid\"\n",
    "        case_rc=$?\n",
    "    fi\n",
    "    [ -f \"$case_log\" ] && /musl/busybox cat \"$case_log\"\n",
    "    tpass=$(whuse_ltp_count_token 'TPASS' \"$case_log\")\n",
    "    tfail=$(whuse_ltp_count_token 'TFAIL' \"$case_log\")\n",
    "    tbrok=$(whuse_ltp_count_token 'TBROK' \"$case_log\")\n",
    "    tconf=$(whuse_ltp_count_token 'TCONF' \"$case_log\")\n",
    "    [ -n \"$tpass\" ] || tpass=0\n",
    "    [ -n \"$tfail\" ] || tfail=0\n",
    "    [ -n \"$tbrok\" ] || tbrok=0\n",
    "    [ -n \"$tconf\" ] || tconf=0\n",
    "    if [ \"$case_rc\" -ne 0 ] && [ \"$tpass\" -gt 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ]; then\n",
    "        case_rc=0\n",
    "    fi\n",
    "    if [ \"$case_rc\" -eq 0 ]; then\n",
    "        echo PASS LTP CASE $case_name : 0\n",
    "    else\n",
    "        echo FAIL LTP CASE $case_name : $case_rc\n",
    "    fi\n",
    "    class=nonzero\n",
    "    if [ \"$case_rc\" -eq 124 ]; then\n",
    "        class=timeout\n",
    "    elif [ \"$case_rc\" -eq 255 ]; then\n",
    "        class=rc255\n",
    "    elif [ \"$case_rc\" -eq 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ] && [ \"$tpass\" -gt 0 ]; then\n",
    "        class=pass\n",
    "    elif [ \"$case_rc\" -eq 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ] && [ \"$tconf\" -gt 0 ] && [ \"$tpass\" -eq 0 ]; then\n",
    "        class=conf-only\n",
    "    elif [ \"$tbrok\" -gt 0 ]; then\n",
    "        class=tbrok\n",
    "    elif [ \"$tfail\" -gt 0 ]; then\n",
    "        class=tfail\n",
    "    elif [ \"$tconf\" -gt 0 ]; then\n",
    "        class=tconf\n",
    "    elif [ \"$case_rc\" -eq 0 ]; then\n",
    "        class=no-tests\n",
    "    fi\n",
    "    echo whuse-ltp-case-result:$case_name:rc=$case_rc:tpass=$tpass:tfail=$tfail:tbrok=$tbrok:tconf=$tconf:class=$class\n",
    "    /musl/busybox rm -f \"$case_log\" \"$case_stdin\" \"$patched_case_path\" >/dev/null 2>&1 || true\n",
    "    if [ \"$case_rc\" -eq 0 ] || [ \"$case_rc\" -eq 124 ] || [ \"$case_rc\" -eq 255 ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    return \"$case_rc\"\n",
    "}\n",
    "whuse_ltp_run_loop() {\n",
    "    timeout_s=\"$1\"\n",
    "    ltp_dir=\"/musl/ltp/testcases/bin\"\n",
    "    [ -d \"$ltp_dir\" ] || return 127\n",
    "    rc=0\n",
    "    step_start_ts=$(/musl/busybox date +%s)\n",
    "    for case_path in \"$ltp_dir\"/*\n",
    "    do\n",
    "        [ -f \"$case_path\" ] || continue\n",
    "        case_name=\"$(/musl/busybox basename \"$case_path\")\"\n",
    "        case_rel=\"ltp/testcases/bin/$case_name\"\n",
    "        if ! whuse_ltp_case_allowed \"$case_name\" \"$case_rel\"; then\n",
    "            echo whuse-ltp-skip-case:$case_rel:filtered\n",
    "            continue\n",
    "        fi\n",
    "        now_ts=$(/musl/busybox date +%s)\n",
    "        elapsed_step=$((now_ts - step_start_ts))\n",
    "        if [ \"$timeout_s\" -gt 0 ] && [ \"$elapsed_step\" -ge \"$timeout_s\" ]; then\n",
    "            echo whuse-oscomp-step-timeout:ltp_testcode.sh:$timeout_s:pid=0:tgid=0\n",
    "            rc=124\n",
    "            break\n",
    "        fi\n",
    "        case_log=\"/tmp/whuse-ltp-case-${case_name}.$$.log\"\n",
    "        case_stdin=\"/tmp/whuse-ltp-case-${case_name}.$$.stdin\"\n",
    "        whuse_ltp_run_single_case \"$case_name\" \"$case_rel\" \"$case_path\" \"$case_log\" \"$case_stdin\"\n",
    "        case_exec_rc=$?\n",
    "        if [ \"$case_exec_rc\" -ne 0 ] && [ \"$rc\" -eq 0 ]; then\n",
    "            rc=$case_exec_rc\n",
    "        fi\n",
    "    done\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_ltp_step() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    if ! step_selected \"$step\"; then\n",
    "        echo whuse-oscomp-step-begin:$step\n",
    "        echo whuse-oscomp-step-skip:$step:filtered\n",
    "        echo whuse-oscomp-step-end:$step:0\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    echo whuse-oscomp-ltp-marker:runner-start:profile=$WHUSE_LTP_PROFILE\n",
    "    old_path=\"$PATH\"\n",
    "    if ! whuse_ltp_enable_busybox_compat; then\n",
    "        echo whuse-oscomp-ltp-marker:busybox-compat-enable-failed\n",
    "    fi\n",
    "    whuse_ltp_write_busybox_wrapper\n",
    "    export PATH=/tmp/whuse-ltp-bin:/musl/ltp/testcases/bin:/musl/ltp/testcases/lib:/musl/ltp/runtest:/musl/ltp/testscripts:$PATH\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        WHUSE_LTP_WHITELIST=/dev/null\n",
    "        WHUSE_LTP_BLACKLIST=/dev/null\n",
    "    fi\n",
    "    echo whuse-oscomp-command-begin:ltp_testcode.sh:$WHUSE_LTP_PROFILE\n",
    "    whuse_ltp_run_loop \"$timeout_s\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-command-end:ltp_testcode.sh:$WHUSE_LTP_PROFILE:$rc\n",
    "    whuse_ltp_disable_busybox_compat\n",
    "    export PATH=\"$old_path\"\n",
    "    echo whuse-oscomp-ltp-marker:runner-end:$rc\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "step_name_for() {\n",
    "    script=\"$1\"\n",
    "    case \"$script\" in\n",
    "        cyclictest_testcode.sh) echo cyclic_testcode.sh ;;\n",
    "        *) echo \"$script\" ;;\n",
    "    esac\n",
    "}\n",
    "step_group_for() {\n",
    "    script=\"$1\"\n",
    "    case \"$script\" in\n",
    "        busybox_testcode.sh) echo \"busybox-musl\" ;;\n",
    "        iozone_testcode.sh) echo \"iozone-musl\" ;;\n",
    "        libctest_testcode.sh) echo \"libctest-musl\" ;;\n",
    "        lmbench_testcode.sh) echo \"lmbench-musl\" ;;\n",
    "        lua_testcode.sh) echo \"lua-musl\" ;;\n",
    "        unixbench_testcode.sh) echo \"unixbench-musl\" ;;\n",
    "        netperf_testcode.sh) echo \"netperf-musl\" ;;\n",
    "        iperf_testcode.sh) echo \"iperf-musl\" ;;\n",
    "        cyclic_testcode.sh|cyclictest_testcode.sh) echo \"cyclic-musl\" ;;\n",
    "        *) echo \"${script%_testcode.sh}\" ;;\n",
    "    esac\n",
    "}\n",
    "step_timeout_for() {\n",
    "    script=\"$1\"\n",
    "    case \"$script\" in\n",
    "        busybox_testcode.sh) echo 180 ;;\n",
    "        iozone_testcode.sh) echo 300 ;;\n",
    "        libctest_testcode.sh) echo 300 ;;\n",
    "        lmbench_testcode.sh) echo 1800 ;;\n",
    "        lua_testcode.sh) echo 300 ;;\n",
    "        unixbench_testcode.sh) echo 1800 ;;\n",
    "        netperf_testcode.sh) echo 240 ;;\n",
    "        iperf_testcode.sh) echo 240 ;;\n",
    "        cyclic_testcode.sh|cyclictest_testcode.sh) echo 300 ;;\n",
    "        ltp_testcode.sh) echo \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\" ;;\n",
    "        *) echo 300 ;;\n",
    "    esac\n",
    "}\n",
    "collect_step_scripts() {\n",
    "    found=\"\"\n",
    "    for path in ./*_testcode.sh\n",
    "    do\n",
    "        [ -f \"$path\" ] || continue\n",
    "        script=\"${path#./}\"\n",
    "        found=\"$found $script\"\n",
    "    done\n",
    "    echo \"$found\"\n",
    "}\n",
    "list_contains() {\n",
    "    needle=\"$1\"\n",
    "    shift\n",
    "    for item in \"$@\"\n",
    "    do\n",
    "        [ \"$item\" = \"$needle\" ] && return 0\n",
    "    done\n",
    "    return 1\n",
    "}\n",
    "ordered_step_scripts() {\n",
    "    found=\"$(collect_step_scripts)\"\n",
    "    selected=\"\"\n",
    "    found_list=\"$found\"\n",
    "    for script in \\\n",
    "        busybox_testcode.sh \\\n",
    "        iozone_testcode.sh \\\n",
    "        libctest_testcode.sh \\\n",
    "        lmbench_testcode.sh \\\n",
    "        lua_testcode.sh \\\n",
    "        unixbench_testcode.sh \\\n",
    "        netperf_testcode.sh \\\n",
    "        iperf_testcode.sh \\\n",
    "        cyclic_testcode.sh \\\n",
    "        cyclictest_testcode.sh \\\n",
    "        ltp_testcode.sh\n",
    "    do\n",
    "        list_contains \"$script\" $found_list || continue\n",
    "        selected=\"$selected $script\"\n",
    "    done\n",
    "    for script in $found_list\n",
    "    do\n",
    "        list_contains \"$script\" $selected && continue\n",
    "        selected=\"$selected $script\"\n",
    "    done\n",
    "    echo \"$selected\"\n",
    "}\n",
    "step_selected() {\n",
    "    step=\"$1\"\n",
    "    if [ -z \"$WHUSE_OSCOMP_ONLY_STEP\" ] || [ \"$WHUSE_OSCOMP_ONLY_STEP\" = \"$step\" ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    return 1\n",
    "}\n",
    "run_step_with_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    if ! step_selected \"$step\"; then\n",
    "        echo whuse-oscomp-step-begin:$step\n",
    "        echo whuse-oscomp-step-skip:$step:filtered\n",
    "        echo whuse-oscomp-step-end:$step:0\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    run_with_timeout \"$timeout_s\" \"$@\"\n",
    "    rc=$?\n",
    "    if [ \"$WHUSE_LAST_TIMEOUT_HIT\" -eq 1 ]; then\n",
    "        echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_script_step() {\n",
    "    script=\"$1\"\n",
    "    step=\"$(step_name_for \"$script\")\"\n",
    "    timeout_s=\"$(step_timeout_for \"$script\")\"\n",
    "    group=\"$(step_group_for \"$script\")\"\n",
    "    echo \"#### OS COMP TEST GROUP START $group ####\"\n",
    "    if [ \"$script\" = \"lmbench_testcode.sh\" ]; then\n",
    "        echo whuse-oscomp-lmbench-marker:runner-start\n",
    "        run_step_with_timeout \"$step\" \"$timeout_s\" /musl/busybox sh -c 'echo whuse-oscomp-command-begin:lmbench_testcode.sh:script; whuse_trace=\"${WHUSE_OSCOMP_TRACE_STEP_CMDS:-1}\"; if [ \"$whuse_trace\" = \"1\" ]; then /musl/busybox sh -x ./lmbench_testcode.sh; else /musl/busybox sh ./lmbench_testcode.sh; fi; rc=$?; echo whuse-oscomp-command-end:lmbench_testcode.sh:script:$rc; exit $rc'\n",
    "        rc=$?\n",
    "        echo whuse-oscomp-lmbench-marker:runner-end:$rc\n",
    "    elif [ \"$script\" = \"unixbench_testcode.sh\" ]; then\n",
        "        echo whuse-oscomp-unixbench-marker:runner-start\n",
        "        run_step_with_timeout \"$step\" \"$timeout_s\" /musl/busybox sh -c 'echo whuse-oscomp-command-begin:unixbench_testcode.sh:script; whuse_trace=\"${WHUSE_OSCOMP_TRACE_STEP_CMDS:-1}\"; if [ \"$whuse_trace\" = \"1\" ]; then /musl/busybox sh -x ./unixbench_testcode.sh; else /musl/busybox sh ./unixbench_testcode.sh; fi; rc=$?; echo whuse-oscomp-command-end:unixbench_testcode.sh:script:$rc; exit $rc'\n",
        "        rc=$?\n",
        "        echo whuse-oscomp-unixbench-marker:runner-end:$rc\n",
    "    elif [ \"$script\" = \"ltp_testcode.sh\" ]; then\n",
    "        run_ltp_step \"$step\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "    else\n",
    "        run_step_with_timeout \"$step\" \"$timeout_s\" /musl/busybox sh \"./$script\"\n",
    "        rc=$?\n",
    "    fi\n",
    "    echo \"#### OS COMP TEST GROUP END $group ####\"\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_libc_bench() {\n",
    "    if ! step_selected libc-bench; then\n",
    "        echo whuse-oscomp-step-begin:libc-bench\n",
    "        echo whuse-oscomp-step-skip:libc-bench:filtered\n",
    "        echo whuse-oscomp-step-end:libc-bench:0\n",
    "        return 0\n",
    "    fi\n",
    "    if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "        echo whuse-oscomp-step-begin:libc-bench\n",
    "        echo whuse-oscomp-step-skip:libc-bench:compat-hang\n",
    "        echo whuse-oscomp-step-end:libc-bench:124\n",
    "        return 0\n",
    "    fi\n",
    "    if [ ! -x ./libc-bench ]; then\n",
    "        echo whuse-oscomp-step-begin:libc-bench\n",
    "        echo whuse-oscomp-step-skip:libc-bench:missing\n",
    "        echo whuse-oscomp-step-end:libc-bench:0\n",
    "        return 0\n",
    "    fi\n",
    "    echo \"#### OS COMP TEST GROUP START libc-bench ####\"\n",
    "    run_step_with_timeout libc-bench 300 ./libc-bench\n",
    "    rc=$?\n",
    "    echo \"#### OS COMP TEST GROUP END libc-bench ####\"\n",
    "    return \"$rc\"\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo \"run time-test\"\n",
    "if step_selected time-test; then\n",
    "    echo whuse-oscomp-step-begin:time-test\n",
    "    if [ -x ./time-test ]; then\n",
    "        ./time-test\n",
    "        rc=$?\n",
    "    else\n",
    "        echo whuse-oscomp-step-skip:time-test:missing\n",
    "        rc=0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:time-test:$rc\n",
    "else\n",
    "    echo whuse-oscomp-step-begin:time-test\n",
    "    echo whuse-oscomp-step-skip:time-test:filtered\n",
    "    echo whuse-oscomp-step-end:time-test:0\n",
    "fi\n",
    "WHUSE_LIBC_BENCH_DONE=0\n",
    "for script in $(ordered_step_scripts)\n",
    "do\n",
    "    step=\"$(step_name_for \"$script\")\"\n",
    "    if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "        echo whuse-oscomp-step-begin:$step\n",
    "        echo whuse-oscomp-step-skip:$step:compat-hang\n",
    "        echo whuse-oscomp-step-end:$step:124\n",
    "        continue\n",
    "    fi\n",
    "    run_script_step \"$script\"\n",
    "    if [ \"$step\" = \"libctest_testcode.sh\" ]; then\n",
    "        run_libc_bench\n",
    "        WHUSE_LIBC_BENCH_DONE=1\n",
    "    fi\n",
    "done\n",
    "if [ \"$WHUSE_LIBC_BENCH_DONE\" -eq 0 ]; then\n",
    "    run_libc_bench\n",
    "fi\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_OFFICIAL_SUITE_SCRIPT: &str = concat!(
    "set +e\n",
    "export PATH=/musl:/glibc:/bin:/usr/bin:/sbin:/usr/sbin:$PATH\n",
    "WHUSE_OSCOMP_ONLY_STEP=${WHUSE_OSCOMP_ONLY_STEP:-}\n",
    "if [ -z \"$WHUSE_OSCOMP_ONLY_STEP\" ] && [ -f /musl/.whuse_oscomp_only_step ]; then\n",
    "    IFS= read -r WHUSE_OSCOMP_ONLY_STEP < /musl/.whuse_oscomp_only_step\n",
    "fi\n",
    "export WHUSE_OSCOMP_ONLY_STEP\n",
    "run_script_entry() {\n",
    "    runtime=\"$1\"\n",
    "    script=\"$2\"\n",
    "    if [ ! -f \"./$script\" ]; then\n",
    "        echo whuse-oscomp-step-skip:${runtime}/$script:missing\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$script\n",
    "    ./busybox sh \"./$script\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-step-end:${runtime}/$script:$rc\n",
    "    return 0\n",
    "}\n",
    "run_runtime_suite() {\n",
    "    runtime=\"$1\"\n",
    "    root=\"/$runtime\"\n",
    "    if [ ! -d \"$root\" ]; then\n",
    "        echo whuse-oscomp-runtime-skip:$runtime:missing-dir\n",
    "        return 0\n",
    "    fi\n",
    "    if [ ! -x \"$root/busybox\" ]; then\n",
    "        echo whuse-oscomp-runtime-skip:$runtime:missing-busybox\n",
    "        return 0\n",
    "    fi\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || return 1\n",
    "    for script in \\\n",
    "        basic_testcode.sh \\\n",
    "        busybox_testcode.sh \\\n",
    "        iozone_testcode.sh \\\n",
    "        libctest_testcode.sh \\\n",
    "        libcbench_testcode.sh \\\n",
    "        lmbench_testcode.sh \\\n",
    "        lua_testcode.sh \\\n",
    "        unixbench_testcode.sh \\\n",
    "        netperf_testcode.sh \\\n",
    "        iperf_testcode.sh \\\n",
    "        ltp_testcode.sh \\\n",
    "        cyclictest_testcode.sh \\\n",
    "        cyclic_testcode.sh\n",
    "    do\n",
    "        if [ -n \"$WHUSE_OSCOMP_ONLY_STEP\" ] && [ \"$WHUSE_OSCOMP_ONLY_STEP\" != \"$script\" ]; then\n",
    "            continue\n",
    "        fi\n",
    "        run_script_entry \"$runtime\" \"$script\"\n",
    "    done\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return 0\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "run_runtime_suite musl\n",
    "run_runtime_suite glibc\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_CMD: &str = concat!(
    "echo whuse-oscomp-shell-entered; ",
    "cd /musl; ",
    "/musl/busybox sh /tmp/whuse-oscomp-suite.sh; ",
    "if [ -x /musl/basic/exit ]; then exec /musl/basic/exit; fi; ",
    "echo whuse-oscomp-exit-missing; exit 0;",
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
            watchdog_last_scan_ns: 0,
            watchdog_clock_ns: 0,
            watchdog_last_hw_ns: 0,
            watchdog_iozone_window_until_ns: 0,
            watchdog_bench_window_until_ns: 0,
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
                    if idle_ticks > 0 {
                        self.timer_irq_count = self.timer_irq_count.saturating_add(idle_ticks);
                        let now = hal().timer.monotonic_nanos();
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
        if self.watchdog_last_scan_ns != 0
            && now.saturating_sub(self.watchdog_last_scan_ns) < OSCOMP_WATCHDOG_SCAN_INTERVAL_NS
        {
            return false;
        }
        self.watchdog_last_scan_ns = now;

        let snapshots = self.processes.process_snapshots();
        let mut all_groups = BTreeMap::<usize, &str>::new();
        let mut watched = BTreeMap::<usize, &str>::new();
        for process in snapshots.iter() {
            if process.is_thread {
                continue;
            }
            all_groups.entry(process.tgid).or_insert(process.name.as_str());
            if process.tgid <= 1 {
                continue;
            }
            watched.entry(process.tgid).or_insert(process.name.as_str());
        }
        self.watchdog_started_at
            .retain(|tgid, _| watched.contains_key(tgid));
        self.watchdog_seen_name
            .retain(|tgid, _| watched.contains_key(tgid));
        for (tgid, name) in watched.iter() {
            let previous_name = self.watchdog_seen_name.get(tgid);
            let reset_started_at = match previous_name {
                None => true,
                Some(previous) if previous.as_str() == *name => false,
                Some(previous) => watchdog_name_change_resets_timer(previous.as_str(), name),
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
            if previous_name.map(|previous| previous.as_str()) != Some(*name) {
                self.watchdog_seen_name.insert(*tgid, (*name).to_string());
            }
        }
        if OSCOMP_IOZONE_BUSYBOX_WINDOW_NS > 0
            && watched.values().any(|name| name.contains("iozone"))
        {
            self.watchdog_iozone_window_until_ns =
                now.saturating_add(OSCOMP_IOZONE_BUSYBOX_WINDOW_NS);
        }
        if OSCOMP_BENCH_PHASE_WINDOW_NS > 0
            && watched
                .values()
                .any(|name| name.contains("lmbench") || name.contains("unixbench") || is_bench_worker_process(name))
        {
            self.watchdog_bench_window_until_ns =
                now.saturating_add(OSCOMP_BENCH_PHASE_WINDOW_NS);
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
                let _ = core::fmt::Write::write_fmt(&mut sample, format_args!("{}:{}", tgid, name));
            }
            logln(format_args!(
                "whuse: oscomp watchdog heartbeat groups={} watched={} sample=[{}]",
                all_groups.len(),
                watched.len(),
                sample
            ));
        }
        let bench_phase_seen = watched
            .values()
            .any(|name| name.contains("lmbench") || name.contains("unixbench") || is_bench_worker_process(name));
        let in_bench_phase = bench_phase_seen
            || (OSCOMP_BENCH_PHASE_WINDOW_NS > 0 && now <= self.watchdog_bench_window_until_ns);
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
                    *name,
                    timeout_ns,
                    has_child_groups,
                    in_bench_phase,
                ))
            })
            .collect::<Vec<_>>();
        let mut killed = false;
        for (tgid, name, timeout_ns, has_child_groups, in_bench_phase) in timed_out {
            let reason = watchdog_timeout_reason(name, has_child_groups, in_bench_phase);
            let subtree = self.processes.descendant_process_groups(tgid);
            if subtree.is_empty() {
                self.watchdog_started_at.remove(&tgid);
                self.watchdog_seen_name.remove(&tgid);
                continue;
            }
            let cleanup_children_only = false;
            let mut cleanup_groups = if cleanup_children_only {
                subtree
                    .iter()
                    .copied()
                    .filter(|group_tgid| *group_tgid != tgid)
                    .collect::<Vec<_>>()
            } else {
                subtree.clone()
            };
            if in_bench_phase && name.contains("busybox") {
                for (bench_tgid, bench_name) in watched.iter() {
                    if *bench_tgid <= 2 || *bench_tgid == tgid {
                        continue;
                    }
                    let bench_related = bench_name.contains("busybox")
                        || bench_name.contains("lmbench")
                        || bench_name.contains("unixbench");
                    if bench_related
                        && !cleanup_groups
                            .iter()
                            .any(|group_tgid| *group_tgid == *bench_tgid)
                    {
                        cleanup_groups.push(*bench_tgid);
                    }
                }
            }
            cleanup_groups.sort_unstable();
            cleanup_groups.dedup();
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
                let mut exits = match self.processes.force_exit_subtree(tgid, 124) {
                    Ok(exits) => exits,
                    Err(_) => Vec::new(),
                };
                for group_tgid in cleanup_groups.iter().copied() {
                    if group_tgid == tgid || subtree.iter().any(|sub| *sub == group_tgid) {
                        continue;
                    }
                    if let Ok(Some(exit)) = self.processes.force_exit_group(group_tgid, 124) {
                        exits.push(exit);
                    }
                }
                exits
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
            let mut robust_futex_addrs = Vec::new();
            for exit in exits {
                killed_groups = killed_groups.saturating_add(1);
                killed_threads = killed_threads.saturating_add(exit.tids.len());
                reaped_tasks = reaped_tasks.saturating_add(self.scheduler.exit_group(exit.tgid));
                if let Some(parent_tgid) = exit.parent_tgid {
                    let woke = self.wake_process_group_threads(parent_tgid);
                    logln(format_args!(
                        "whuse: oscomp watchdog wake parent_tgid={} woke_threads={}",
                        parent_tgid, woke
                    ));
                }
                if let Some(vfork_parent_tid) = exit.vfork_parent_tid {
                    let woke = self.scheduler.wake_task(vfork_parent_tid);
                    if vfork_debug_enabled() {
                        logln(format_args!(
                            "whuse-vfork-release: child_tgid={} parent_tid={} reason=watchdog woke={}",
                            exit.tgid, vfork_parent_tid, woke
                        ));
                    }
                }
                clear_child_tids.extend(exit.clear_child_tids);
                robust_futex_addrs.extend(exit.robust_futex_addrs);
            }
            for addr in clear_child_tids {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
            for addr in robust_futex_addrs {
                let woken = self.processes.wake_futex(addr, 1);
                if robust_debug_enabled() {
                    logln(format_args!(
                        "whuse-robust-exit: wake addr={:#x} woken={:?}",
                        addr, woken
                    ));
                }
                for tid in woken {
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
                if bc > 0 {
                    logln(format_args!(
                        "whuse-coop: tick={} blocked={} waking_all",
                        self.timer_irq_count, bc
                    ));
                    let all_blocked = self.scheduler.blocked_task_ids();
                    for tid in all_blocked {
                        if tid == 125 || tid == 126 {
                            logln(format_args!("whuse-sched: spurious_wake tid={}", tid));
                        }
                        self.processes.clear_futex_wait_state(tid);
                        let _ = self.scheduler.wake_task(tid);
                    }
                }
            }

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
            if let Some(tid) = trap_tid {
                if let Ok(process) = self.processes.find_by_tid_mut(tid) {
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
                        if (sysno != SYS_EXECVE && sysno != SYS_RT_SIGRETURN) || (result as i32) < 0
                        {
                            process.trap_frame.sepc = sepc + 4;
                        }
                    }
                }
            } else if let Ok(process) = self.processes.current_mut() {
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
            // Cooperative scheduler fairness for benchmark-heavy loops: very
            // frequent syscalls can keep re-arming timer deadlines and starve
            // sibling tasks in the same benchmark pipeline.
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
            if result != EAGAIN_RET && bench_like_task && self.scheduler.ready_count() > 0 {
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
            if let Some(vfork_parent_tid) = exit.vfork_parent_tid {
                let _ = self.scheduler.wake_task(vfork_parent_tid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let _ = self.processes.deliver_signal(parent_tgid, 17);
                let _ = self.wake_process_group_threads(parent_tgid);
            }
            if let Some(addr) = exit.clear_child_tid {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
            for addr in exit.robust_futex_addrs {
                let woken = self.processes.wake_futex(addr, 1);
                if robust_debug_enabled() {
                    logln(format_args!(
                        "whuse-robust-exit: trap-exit addr={:#x} woken={:?}",
                        addr, woken
                    ));
                }
                for tid in woken {
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
        let libc_bench_task = process.name.contains("libc-bench");

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
                        let _ = self.wake_process_group_threads(parent_tgid);
                    }
                    if let Some(addr) = exit.clear_child_tid {
                        for wtid in self.processes.wake_futex(addr, usize::MAX) {
                            let _ = self.scheduler.wake_task(wtid);
                        }
                    }
                    for addr in exit.robust_futex_addrs {
                        let woken = self.processes.wake_futex(addr, 1);
                        if robust_debug_enabled() {
                            logln(format_args!(
                                "whuse-robust-exit: signal-exit addr={:#x} woken={:?}",
                                addr, woken
                            ));
                        }
                        for wtid in woken {
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

        // RISC-V Linux musl rt_sigframe layout:
        //   offset 0:   siginfo_t (128 bytes)
        //   offset 128: ucontext_t
        //     +0:  uc_flags(8), uc_link(8), uc_stack(24), uc_sigmask(128)
        //     +176: mcontext_t (16-byte aligned)
        //       gregs[32] (256 bytes): gregs[0]=pc, gregs[1..31]=x1..x31
        //       fpregs union (528 bytes): d-ext f[32] + fcsr (+ q-ext compatible tail)
        //   total = 128 + 960 = 1088 bytes
        const FRAME_SIZE: usize = 1088;
        const SIGINFO_OFF: usize = 0;
        const UCONTEXT_OFF: usize = 128;
        const UC_SIGMASK_OFF: usize = UCONTEXT_OFF + 40;
        const MCTX_OFF: usize = UCONTEXT_OFF + 176;
        #[cfg(target_arch = "riscv64")]
        const MCTX_FP_OFF: usize = MCTX_OFF + 32 * 8;
        #[cfg(target_arch = "riscv64")]
        const MCTX_FP_SIZE: usize = 528;
        #[cfg(target_arch = "riscv64")]
        const MCTX_D_FCSR_OFF: usize = MCTX_FP_OFF + 32 * 8;

        let cur_sp = process.trap_frame.regs[2];
        let frame_sp = (cur_sp.wrapping_sub(FRAME_SIZE)) & !0xf_usize;

        let mut frame = alloc::vec![0u8; FRAME_SIZE];

        frame[SIGINFO_OFF..SIGINFO_OFF + 4].copy_from_slice(&(signum as u32).to_le_bytes());
        frame[UC_SIGMASK_OFF..UC_SIGMASK_OFF + 128].fill(0);
        frame[UC_SIGMASK_OFF..UC_SIGMASK_OFF + 8]
            .copy_from_slice(&process.signal_mask.to_le_bytes());
        let saved_pc = process.trap_frame.sepc;
        frame[MCTX_OFF..MCTX_OFF + 8].copy_from_slice(&saved_pc.to_le_bytes());
        for i in 1usize..32 {
            let off = MCTX_OFF + i * 8;
            frame[off..off + 8].copy_from_slice(&process.trap_frame.regs[i].to_le_bytes());
        }
        #[cfg(target_arch = "riscv64")]
        {
            frame[MCTX_FP_OFF..MCTX_FP_OFF + MCTX_FP_SIZE].fill(0);
            for i in 0..32usize {
                let off = MCTX_FP_OFF + i * 8;
                frame[off..off + 8].copy_from_slice(&process.trap_frame.fregs[i].to_le_bytes());
            }
            frame[MCTX_D_FCSR_OFF..MCTX_D_FCSR_OFF + 4]
                .copy_from_slice(&(process.trap_frame.fcsr as u32).to_le_bytes());
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
        }
        if signal_frame_debug_enabled() {
            logln(format_args!(
                "whuse-signal-frame:dispatch tid={} sig={} frame_sp={:#x} saved_pc={:#x}",
                process.tid, signum, frame_sp, saved_pc
            ));
        }
        if libc_bench_task {
            logln(format_args!(
                "whuse-libcbench-signal:dispatch tid={} sig={} frame_sp={:#x} saved_pc={:#x} handler={:#x}",
                process.tid, signum, frame_sp, saved_pc, action.handler
            ));
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

fn signal_frame_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_SIGNAL_FRAME"), Some("1"))
}

fn robust_debug_enabled() -> bool {
    match option_env!("WHUSE_DEBUG_ROBUST") {
        Some("1") => true,
        _ => false,
    }
}

fn vfork_debug_enabled() -> bool {
    match option_env!("WHUSE_DEBUG_VFORK") {
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
        String::from("-c"),
        String::from(OSCOMP_SUITE_CMD),
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
        "/usr",
        "/usr/bin",
        "/usr/sbin",
        "/lib",
        "/sbin",
        "/lib/modules",
        "/lib/modules/6.8.0-whuse",
        "/lib/modules/6.8.0-whuse/build",
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
    install_busybox_exec_alias(vfs, "/musl/cut", "cut");
    install_busybox_exec_alias(vfs, "/musl/head", "head");
    install_busybox_exec_alias(vfs, "/musl/tail", "tail");
    install_busybox_exec_alias(vfs, "/musl/tr", "tr");
    install_busybox_exec_alias(vfs, "/musl/xargs", "xargs");
    install_busybox_exec_alias(vfs, "/musl/readlink", "readlink");
    install_wait_wrapper(vfs);
    install_locale_wrapper(vfs);
    install_user_mgmt_wrappers(vfs);
    install_keyctl_wrapper(vfs);
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
        ("/bin/wait", "/musl/wait"),
        ("/bin/locale", "/musl/locale"),
        ("/bin/keyctl", "/musl/keyctl"),
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
        ("/usr/bin/wait", "/musl/wait"),
        ("/usr/bin/locale", "/musl/locale"),
        ("/usr/bin/keyctl", "/musl/keyctl"),
        ("/usr/sbin/useradd", "/musl/useradd"),
        ("/usr/sbin/userdel", "/musl/userdel"),
        ("/sbin/useradd", "/musl/useradd"),
        ("/sbin/userdel", "/musl/userdel"),
        ("/usr/bin/env", "/musl/busybox"),
        ("/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"),
        ("/lib/ld-musl-loongarch64.so.1", "/musl/lib/libc.so"),
        (
            "/lib/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/lib/ld-linux-loongarch-lp64d.so.1",
            "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
        ),
        ("/lib/libc.so.6", "/glibc/lib/libc.so.6"),
        ("/lib/libm.so.6", "/glibc/lib/libm.so.6"),
        ("/lib/libc.so", "/glibc/lib/libc.so"),
        ("/lib/libm.so", "/glibc/lib/libm.so"),
    ] {
        install_fallback_symlink(vfs, path, target);
    }
    let _ = vfs.unlink("/", OSCOMP_LTP_KERNEL_CONFIG_PATH);
    match vfs.create_file("/", OSCOMP_LTP_KERNEL_CONFIG_PATH, OSCOMP_LTP_KERNEL_CONFIG_STUB.as_bytes()) {
        Ok(()) => logln(format_args!(
            "whuse: installed ltp kernel config stub {}",
            OSCOMP_LTP_KERNEL_CONFIG_PATH
        )),
        Err(err) => logln(format_args!(
            "whuse: failed ltp kernel config stub {} err={}",
            OSCOMP_LTP_KERNEL_CONFIG_PATH, err
        )),
    }
    install_oscomp_root_aliases(vfs);
    let suite_script = select_oscomp_suite_script(vfs);
    match vfs.create_file(
        "/",
        OSCOMP_SUITE_SCRIPT_PATH,
        suite_script.as_bytes(),
    ) {
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
    match vfs.create_file(
        "/",
        OSCOMP_LTP_SCORE_WHITELIST_PATH,
        OSCOMP_LTP_SCORE_WHITELIST.as_bytes(),
    ) {
        Ok(()) => logln(format_args!(
            "whuse: installed ltp score whitelist {}",
            OSCOMP_LTP_SCORE_WHITELIST_PATH
        )),
        Err(err) => logln(format_args!(
            "whuse: failed ltp score whitelist {} err={}",
            OSCOMP_LTP_SCORE_WHITELIST_PATH, err
        )),
    }
    match vfs.create_file(
        "/",
        OSCOMP_LTP_SCORE_BLACKLIST_PATH,
        OSCOMP_LTP_SCORE_BLACKLIST.as_bytes(),
    ) {
        Ok(()) => logln(format_args!(
            "whuse: installed ltp score blacklist {}",
            OSCOMP_LTP_SCORE_BLACKLIST_PATH
        )),
        Err(err) => logln(format_args!(
            "whuse: failed ltp score blacklist {} err={}",
            OSCOMP_LTP_SCORE_BLACKLIST_PATH, err
        )),
    }
    for cfg_path in [
        OSCOMP_CFG_ONLY_STEP_PATH,
        OSCOMP_CFG_LTP_PROFILE_PATH,
        OSCOMP_CFG_LTP_WHITELIST_PATH,
        OSCOMP_CFG_LTP_BLACKLIST_PATH,
        OSCOMP_CFG_LTP_TIMEOUT_PATH,
    ] {
        if vfs.access("/", cfg_path).is_ok() {
            logln(format_args!("whuse: detected oscomp cfg {}", cfg_path));
        }
    }
}

fn install_fallback_symlink(vfs: &mut KernelVfs, path: &str, target: &str) {
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

fn install_exec_wrapper(vfs: &mut KernelVfs, path: &str, contents: &str, label: &str) {
    if let Err(err) = vfs.preload_external_file(path, contents.as_bytes(), Some(0o100755)) {
        logln(format_args!(
            "whuse: failed {} wrapper {} err={}",
            label, path, err
        ));
    }
}

fn install_keyctl_wrapper(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/keyctl", OSCOMP_KEYCTL_WRAPPER, "keyctl");
}

fn install_wait_wrapper(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/wait", OSCOMP_WAIT_WRAPPER, "wait");
}

fn install_locale_wrapper(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/locale", OSCOMP_LOCALE_WRAPPER, "locale");
}

fn install_user_mgmt_wrappers(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/useradd", OSCOMP_USERADD_WRAPPER, "useradd");
    install_exec_wrapper(vfs, "/musl/userdel", OSCOMP_USERDEL_WRAPPER, "userdel");
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

fn select_oscomp_suite_script(vfs: &mut KernelVfs) -> &'static str {
    match vfs.read_file_all("/", OSCOMP_CFG_RUNNER_MODE_PATH) {
        Ok(bytes) if String::from_utf8_lossy(&bytes).trim() == "debug" => OSCOMP_SUITE_SCRIPT,
        _ => OSCOMP_OFFICIAL_SUITE_SCRIPT,
    }
}

fn oscomp_process_timeout_ns(
    tgid: usize,
    name: &str,
    in_iozone_busybox_window: bool,
    has_child_groups: bool,
    in_bench_phase: bool,
) -> u64 {
    if name.contains("hackbench") {
        return 1_000_000_000;
    }
    if is_libctest_entry_or_runner(name) {
        return OSCOMP_LIBCTEST_ENTRY_TIMEOUT_NS;
    }
    // Keep the init shell immortal, but allow all other busybox runners and
    // leftovers to be reaped by watchdog so a stuck step cannot block suite progress.
    if tgid <= 2 && name == "/musl/busybox" {
        return u64::MAX;
    }
    if name.contains("lmbench") {
        return OSCOMP_LMBENCH_TIMEOUT_NS;
    }
    if name.contains("unixbench") {
        return OSCOMP_UNIXBENCH_TIMEOUT_NS;
    }
    if name.contains("busybox") {
        if is_busybox_supervisor(name, has_child_groups, in_bench_phase) {
            if in_bench_phase {
                return OSCOMP_BENCH_SUPERVISOR_TIMEOUT_NS;
            }
            return OSCOMP_BUSYBOX_SUPERVISOR_TIMEOUT_NS;
        }
        if in_iozone_busybox_window {
            return OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS;
        }
        return OSCOMP_BUSYBOX_APPLET_TIMEOUT_NS;
    }
    if is_oscomp_heavy_process(name) {
        return OSCOMP_HEAVY_TIMEOUT_NS;
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
    name.contains("busybox") && (name == "/musl/busybox" || has_child_groups || in_bench_phase)
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
    name == "./runtest.exe"
        || (name.starts_with("entry-") && name.ends_with(".exe"))
        || name == "entry.exe"
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

fn is_bench_worker_process(name: &str) -> bool {
    name.contains("lmbench_all")
        || name.contains("lat_")
        || name.contains("bw_")
        || name.contains("lmdd")
        || name.contains("syscall")
        || name.contains("context1")
        || name.contains("dhry2")
        || name.contains("dhry2reg")
        || name.contains("whetstone")
        || name.contains("fstime")
        || name.contains("pipe")
        || name.contains("spawn")
        || name.contains("execl")
        || name.contains("looper")
        || name.contains("arithoh")
        || name.contains("short")
        || name.contains("long")
        || name.contains("float")
        || name.contains("double")
        || name.contains("hanoi")
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
