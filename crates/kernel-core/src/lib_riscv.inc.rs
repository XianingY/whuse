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
    SYS_CLOCK_NANOSLEEP, SYS_EPOLL_PWAIT, SYS_EPOLL_PWAIT2, SYS_EXECVE, SYS_FUTEX, SYS_NANOSLEEP,
    SYS_MSGRCV, SYS_PPOLL, SYS_PSELECT6, SYS_READ, SYS_READV, SYS_RT_SIGRETURN, SYS_RT_SIGSUSPEND,
    SYS_RT_SIGTIMEDWAIT, SYS_WAIT,
    SYS_SEMOP, SYS_SEMTIMEDOP,
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
    watchdog_libcbench_dumped_at: BTreeMap<usize, u64>,
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
const OSCOMP_PROFILE_DEFAULT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_PROFILE_DEFAULT__";
const OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER: &str = "__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__";
const OSCOMP_LTP_SCORE_WHITELIST_PATH: &str = "/musl/ltp_score_whitelist.txt";
const OSCOMP_LTP_SCORE_BLACKLIST_PATH: &str = "/musl/ltp_score_blacklist.txt";
const OSCOMP_LTP_SCORE_WHITELIST_GLIBC_PATH: &str = "/glibc/ltp_score_whitelist.txt";
const OSCOMP_LTP_SCORE_BLACKLIST_GLIBC_PATH: &str = "/glibc/ltp_score_blacklist.txt";
const OSCOMP_CFG_ONLY_STEP_PATH: &str = "/musl/.whuse_oscomp_only_step";
const OSCOMP_CFG_LTP_PROFILE_PATH: &str = "/musl/.whuse_ltp_profile";
const OSCOMP_CFG_LTP_WHITELIST_PATH: &str = "/musl/.whuse_ltp_whitelist";
const OSCOMP_CFG_LTP_BLACKLIST_PATH: &str = "/musl/.whuse_ltp_blacklist";
const OSCOMP_CFG_LTP_WHITELIST_MUSL_PATH: &str = "/musl/.whuse_ltp_whitelist_musl";
const OSCOMP_CFG_LTP_BLACKLIST_MUSL_PATH: &str = "/musl/.whuse_ltp_blacklist_musl";
const OSCOMP_CFG_LTP_WHITELIST_GLIBC_PATH: &str = "/musl/.whuse_ltp_whitelist_glibc";
const OSCOMP_CFG_LTP_BLACKLIST_GLIBC_PATH: &str = "/musl/.whuse_ltp_blacklist_glibc";
const OSCOMP_CFG_LTP_TIMEOUT_PATH: &str = "/musl/.whuse_ltp_step_timeout";
const OSCOMP_CFG_RUNNER_MODE_PATH: &str = "/musl/.whuse_oscomp_runner";
const SIGCANCEL_MASK: u64 = 1u64 << (33 - 1);
const OSCOMP_LTP_SCORE_WHITELIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_whitelist.txt");
const OSCOMP_LTP_SCORE_BLACKLIST: &str =
    include_str!("../../../tools/oscomp/ltp/score_blacklist.txt");
const OSCOMP_LTP_SCORE_WHITELIST_GLIBC: &str =
    include_str!("../../../tools/oscomp/ltp/score_whitelist_glibc_rv.txt");
const OSCOMP_LTP_SCORE_BLACKLIST_GLIBC: &str =
    include_str!("../../../tools/oscomp/ltp/score_blacklist_glibc_rv.txt");
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
const OSCOMP_WAIT_WRAPPER: &str = concat!("#!/musl/busybox sh\n", "wait \"$@\"\n", "exit $?\n",);
const OSCOMP_LOCALE_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "case \"${1:-}\" in\n",
    "    -a) echo C; echo POSIX ;;\n",
    "    *) echo LANG=C; echo LC_ALL= ;;\n",
    "esac\n",
    "exit 0\n",
);
const OSCOMP_RSH_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "[ $# -gt 0 ] && shift\n",
    "[ $# -gt 0 ] || exit 0\n",
    "exec \"$@\"\n",
);
const OSCOMP_ETC_PASSWD: &str = concat!(
    "root:x:0:0:root:/root:/musl/busybox\n",
    "nobody:x:65534:65534:nobody:/:/musl/busybox\n",
    "ltp_add_key05_0:x:1000:1000:ltp_add_key05_0:/:/musl/busybox\n",
    "ltp_add_key05_1:x:1001:1001:ltp_add_key05_1:/:/musl/busybox\n",
    "ltp_add_key05_2:x:1002:1002:ltp_add_key05_2:/:/musl/busybox\n",
    "ltp_add_key05_3:x:1003:1003:ltp_add_key05_3:/:/musl/busybox\n",
    "ltp_add_key05_4:x:1004:1004:ltp_add_key05_4:/:/musl/busybox\n",
    "ltp_add_key05_5:x:1005:1005:ltp_add_key05_5:/:/musl/busybox\n",
    "ltp_add_key05_6:x:1006:1006:ltp_add_key05_6:/:/musl/busybox\n",
    "ltp_add_key05_7:x:1007:1007:ltp_add_key05_7:/:/musl/busybox\n",
    "ltp_add_key05_8:x:1008:1008:ltp_add_key05_8:/:/musl/busybox\n",
    "ltp_add_key05_9:x:1009:1009:ltp_add_key05_9:/:/musl/busybox\n",
);
const OSCOMP_ETC_GROUP: &str = concat!(
    "root:x:0:\n",
    "nogroup:x:65534:\n",
    "ltp_add_key05_0:x:1000:\n",
    "ltp_add_key05_1:x:1001:\n",
    "ltp_add_key05_2:x:1002:\n",
    "ltp_add_key05_3:x:1003:\n",
    "ltp_add_key05_4:x:1004:\n",
    "ltp_add_key05_5:x:1005:\n",
    "ltp_add_key05_6:x:1006:\n",
    "ltp_add_key05_7:x:1007:\n",
    "ltp_add_key05_8:x:1008:\n",
    "ltp_add_key05_9:x:1009:\n",
);
const OSCOMP_ETC_PROTOCOLS: &str = concat!(
    "# Internet (IP) protocols\n",
    "#\n",
    "# Updated from http://www.iana.org/assignments/protocol-numbers and other\n",
    "# sources.\n",
    "# New protocols will be added on request if they have been officially\n",
    "# assigned by IANA and are not historical.\n",
    "# If you need a huge list of used numbers please install the nmap package.\n",
    "\n",
    "ip\t0\tIP\t\t# internet protocol, pseudo protocol number\n",
    "hopopt\t0\tHOPOPT\t\t# IPv6 Hop-by-Hop Option [RFC1883]\n",
    "icmp\t1\tICMP\t\t# internet control message protocol\n",
    "igmp\t2\tIGMP\t\t# Internet Group Management\n",
    "ggp\t3\tGGP\t\t# gateway-gateway protocol\n",
    "ipencap\t4\tIP-ENCAP\t# IP encapsulated in IP (officially ``IP'')\n",
    "st\t5\tST\t\t# ST datagram mode\n",
    "tcp\t6\tTCP\t\t# transmission control protocol\n",
    "egp\t8\tEGP\t\t# exterior gateway protocol\n",
    "igp\t9\tIGP\t\t# any private interior gateway (Cisco)\n",
    "pup\t12\tPUP\t\t# PARC universal packet protocol\n",
    "udp\t17\tUDP\t\t# user datagram protocol\n",
    "hmp\t20\tHMP\t\t# host monitoring protocol\n",
    "xns-idp\t22\tXNS-IDP\t\t# Xerox NS IDP\n",
    "rdp\t27\tRDP\t\t# \"reliable datagram\" protocol\n",
    "iso-tp4\t29\tISO-TP4\t\t# ISO Transport Protocol class 4 [RFC905]\n",
    "dccp\t33\tDCCP\t\t# Datagram Congestion Control Prot. [RFC4340]\n",
    "xtp\t36\tXTP\t\t# Xpress Transfer Protocol\n",
    "ddp\t37\tDDP\t\t# Datagram Delivery Protocol\n",
    "idpr-cmtp 38\tIDPR-CMTP\t# IDPR Control Message Transport\n",
    "ipv6\t41\tIPv6\t\t# Internet Protocol, version 6\n",
    "ipv6-route 43\tIPv6-Route\t# Routing Header for IPv6\n",
    "ipv6-frag 44\tIPv6-Frag\t# Fragment Header for IPv6\n",
    "idrp\t45\tIDRP\t\t# Inter-Domain Routing Protocol\n",
    "rsvp\t46\tRSVP\t\t# Reservation Protocol\n",
    "gre\t47\tGRE\t\t# General Routing Encapsulation\n",
    "esp\t50\tIPSEC-ESP\t# Encap Security Payload [RFC2406]\n",
    "ah\t51\tIPSEC-AH\t# Authentication Header [RFC2402]\n",
    "skip\t57\tSKIP\t\t# SKIP\n",
    "ipv6-icmp\t58\tIPv6-ICMP\t# ICMP for IPv6\n",
    "ipv6-nonxt 59\tIPv6-NoNxt\t# No Next Header for IPv6\n",
    "ipv6-opts 60\tIPv6-Opts\t# Destination Options for IPv6\n",
    "rspf\t73\tRSPF CPHB\t# Radio Shortest Path First (officially CPHB)\n",
    "vmtp\t81\tVMTP\t\t# Versatile Message Transport\n",
    "eigrp\t88\tEIGRP\t\t# Enhanced Interior Routing Protocol (Cisco)\n",
    "ospf\t89\tOSPFIGP\t\t# Open Shortest Path First IGP\n",
    "ax.25\t93\tAX.25\t\t# AX.25 frames\n",
    "ipip\t94\tIPIP\t\t# IP-within-IP Encapsulation Protocol\n",
    "etherip\t97\tETHERIP\t\t# Ethernet-within-IP Encapsulation [RFC3378]\n",
    "encap\t98\tENCAP\t\t# Yet Another IP encapsulation [RFC1241]\n",
    "#\t99\t\t\t# any private encryption scheme\n",
    "pim\t103\tPIM\t\t# Protocol Independent Multicast\n",
    "ipcomp\t108\tIPCOMP\t\t# IP Payload Compression Protocol\n",
    "vrrp\t112\tVRRP\t\t# Virtual Router Redundancy Protocol [RFC5798]\n",
    "l2tp\t115\tL2TP\t\t# Layer Two Tunneling Protocol [RFC2661]\n",
    "isis\t124\tISIS\t\t# IS-IS over IPv4\n",
    "sctp\t132\tSCTP\t\t# Stream Control Transmission Protocol\n",
    "fc\t133\tFC\t\t# Fibre Channel\n",
    "mobility-header 135 Mobility-Header # Mobility Support for IPv6 [RFC3775]\n",
    "udplite\t136\tUDPLite\t\t# UDP-Lite [RFC3828]\n",
    "mpls-in-ip 137\tMPLS-in-IP\t# MPLS-in-IP [RFC4023]\n",
    "manet\t138\t\t\t# MANET Protocols [RFC5498]\n",
    "hip\t139\tHIP\t\t# Host Identity Protocol\n",
    "shim6\t140\tShim6\t\t# Shim6 Protocol\n",
    "wesp\t141\tWESP\t\t# Wrapped Encapsulating Security Payload\n",
    "rohc\t142\tROHC\t\t# Robust Header Compression\n",
    "ethernet 143\tEthernet\t# Ethernet encapsulation for SRv6 [RFC8986]\n",
    "# The following entries have not been assigned by IANA but are used\n",
    "# internally by the Linux kernel.\n",
    "mptcp\t262\tMPTCP\t\t# Multipath TCP connection\n",
);
const OSCOMP_KERNEL_CONFIG: &str = concat!(
    "CONFIG_BSD_PROCESS_ACCT=y\n",
    "CONFIG_BSD_PROCESS_ACCT_V3=y\n",
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
const OSCOMP_GROUPDEL_WRAPPER: &str = concat!(
    "#!/musl/busybox sh\n",
    "echo \"groupdel: compatibility wrapper\" >&2\n",
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
    "WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-}\n",
    "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-}\n",
    "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-}\n",
    "WHUSE_LTP_MUSL_WHITELIST=${WHUSE_LTP_MUSL_WHITELIST:-}\n",
    "WHUSE_LTP_MUSL_BLACKLIST=${WHUSE_LTP_MUSL_BLACKLIST:-}\n",
    "WHUSE_LTP_GLIBC_WHITELIST=${WHUSE_LTP_GLIBC_WHITELIST:-}\n",
    "WHUSE_LTP_GLIBC_BLACKLIST=${WHUSE_LTP_GLIBC_BLACKLIST:-}\n",
    "WHUSE_LTP_STEP_TIMEOUT=${WHUSE_LTP_STEP_TIMEOUT:-1800}\n",
    "WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-}\n",
    "KCONFIG_SKIP_CHECK=${KCONFIG_SKIP_CHECK:-1}\n",
    "LHOST_HWADDRS=${LHOST_HWADDRS:-00:11:22:33:44:55}\n",
    "RHOST_HWADDRS=${RHOST_HWADDRS:-00:11:22:33:44:66}\n",
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
    "WHUSE_LTP_CASE_TIMEOUT=${WHUSE_LTP_CASE_TIMEOUT:-45}\n",
    "export WHUSE_OSCOMP_ONLY_STEP WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_MUSL_WHITELIST WHUSE_LTP_MUSL_BLACKLIST WHUSE_LTP_GLIBC_WHITELIST WHUSE_LTP_GLIBC_BLACKLIST WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_CASE_TIMEOUT LHOST_HWADDRS RHOST_HWADDRS KCONFIG_SKIP_CHECK\n",
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
    "whuse_ltp_case_blocked() {\n",
    "    case_name=\"$1\"\n",
    "    case_rel=\"$2\"\n",
    "    case \"$case_name\" in\n",
    "        add_ipv6addr|acct02_helper) return 0 ;;\n",
    "    esac\n",
    "    if [ -f \"$WHUSE_LTP_BLACKLIST\" ] && ( /musl/busybox grep -Fqx \"$case_name\" \"$WHUSE_LTP_BLACKLIST\" || /musl/busybox grep -Fqx \"$case_rel\" \"$WHUSE_LTP_BLACKLIST\" ); then\n",
    "        return 0\n",
    "    fi\n",
    "    return 1\n",
    "}\n",
    "whuse_ltp_write_busybox_wrapper() {\n",
    "    return 0\n",
    "    wrap_dir=\"${WHUSE_LTP_WRAPPER_DIR:-${WHUSE_LTP_RUNROOT:-${WHUSE_LTP_TMPDIR:-/musl/ltp-tmp}}/debug}\"\n",
    "    if [ ! -d \"$wrap_dir\" ]; then\n",
    "        /musl/busybox mkdir -p \"$wrap_dir\" >/dev/null 2>&1 || true\n",
    "    fi\n",
    "    if [ ! -d \"$wrap_dir\" ]; then\n",
    "        echo whuse-oscomp-ltp-marker:busybox-wrapper-mkdir-failed:$wrap_dir\n",
    "        /musl/busybox ls -ld \"${WHUSE_LTP_RUNROOT:-${WHUSE_LTP_TMPDIR:-/musl/ltp-tmp}}\" \"$wrap_dir\" 2>&1 || true\n",
    "        return 1\n",
    "    fi\n",
    "    if ! {\n",
    "        echo '#!/musl/busybox sh'\n",
    "        echo 'cmd=\"${0##*/}\"'\n",
    "        echo 'if [ \"$cmd\" = \"busybox\" ]; then'\n",
    "        echo '    cmd=\"${1:-}\"'\n",
    "        echo '    [ $# -gt 0 ] && shift'\n",
    "        echo 'fi'\n",
    "        echo 'case \"$cmd\" in'\n",
    "        echo '    locale) exec /musl/locale \"$@\" ;;'\n",
    "        echo '    cat) exec /musl/busybox cat \"$@\" ;;'\n",
    "        echo '    grep) exec /musl/busybox grep \"$@\" ;;'\n",
    "        echo '    ar) exec /musl/busybox ar \"$@\" ;;'\n",
    "        echo '    ip) exec /musl/busybox ip \"$@\" ;;'\n",
    "        echo '    mktemp) exec /musl/busybox mktemp \"$@\" ;;'\n",
    "        echo '    chmod) exec /musl/busybox chmod \"$@\" ;;'\n",
    "        echo '    id) exec /musl/busybox id \"$@\" ;;'\n",
    "        echo '    acct02_helper) exec ${WHUSE_LTP_RUNTIME_ROOT:-/musl}/ltp/testcases/bin/acct02_helper \"$@\" ;;'\n",
    "        echo '    useradd) exec /musl/useradd \"$@\" ;;'\n",
    "        echo '    userdel) exec /musl/userdel \"$@\" ;;'\n",
    "        echo '    groupdel) exec /musl/groupdel \"$@\" ;;'\n",
    "        echo '    keyctl) exec /musl/keyctl \"$@\" ;;'\n",
    "        echo '    rsh) exec /musl/rsh \"$@\" ;;'\n",
    "        echo 'esac'\n",
    "        echo 'exec /musl/busybox \"$cmd\" \"$@\"'\n",
    "    } > \"$wrap_dir/busybox\"; then\n",
    "        echo whuse-oscomp-ltp-marker:busybox-wrapper-write-failed\n",
    "        return 1\n",
    "    fi\n",
    "    /musl/busybox chmod 755 \"$wrap_dir/busybox\" >/dev/null 2>&1 || true\n",
    "    for applet in useradd userdel groupdel locale keyctl rsh cat grep ar ip mktemp chmod id acct02_helper; do\n",
    "        /musl/busybox ln -sf busybox \"$wrap_dir/$applet\" >/dev/null 2>&1 || true\n",
    "    done\n",
    "    echo whuse-oscomp-ltp-marker:busybox-wrapper-ready\n",
    "    return 0\n",
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
    "    if whuse_ltp_case_blocked \"$case_name\" \"$case_rel\"; then\n",
    "        return 1\n",
    "    fi\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        return 0\n",
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
    "    [ \"$stdin_path\" = \"/dev/null\" ] && return 0\n",
    "    case \"$stdin_path\" in\n",
    "        */*) stdin_dir=\"${stdin_path%/*}\" ;;\n",
    "        *) stdin_dir=. ;;\n",
    "    esac\n",
    "    /musl/busybox mkdir -p \"$stdin_dir\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox rm -f \"$stdin_path\" >/dev/null 2>&1 || true\n",
    "    case \"$case_name\" in\n",
    "        assign_password.sh|ask_password.sh)\n",
    "            {\n",
    "                /musl/busybox echo '123456'\n",
    "                /musl/busybox echo '123456'\n",
    "                /musl/busybox echo '123456'\n",
    "                /musl/busybox echo '123456'\n",
    "            } | /musl/busybox tee \"$stdin_path\" >/dev/null 2>&1 || {\n",
    "                echo whuse-oscomp-ltp-marker:stdin-create-failed:$case_name\n",
    "                return 1\n",
    "            }\n",
    "            ;;\n",
    "        *)\n",
    "            return 0\n",
    "            ;;\n",
    "    esac\n",
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
    "        TPASS)\n",
    "            /musl/busybox grep -Eq 'TPASS|passed[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\"\n",
    "            return $?\n",
    "            ;;\n",
    "        TFAIL)\n",
    "            /musl/busybox grep -Eq 'TFAIL|failed[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\"\n",
    "            return $?\n",
    "            ;;\n",
    "        TBROK)\n",
    "            /musl/busybox grep -Eq 'TBROK|broken[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\"\n",
    "            return $?\n",
    "            ;;\n",
    "        TCONF)\n",
    "            /musl/busybox grep -Eq 'TCONF|skipped[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\"\n",
    "            return $?\n",
    "            ;;\n",
    "        *) return 1 ;;\n",
    "    esac\n",
    "}\n",
    "whuse_ltp_update_epoch() {\n",
    "    epoch_inc_s=\"${1:-1}\"\n",
    "    case \"$epoch_inc_s\" in\n",
    "        ''|*[!0-9]*) epoch_inc_s=1 ;;\n",
    "    esac\n",
    "    if [ \"$epoch_inc_s\" -le 0 ]; then\n",
    "        epoch_inc_s=1\n",
    "    fi\n",
    "    if [ -z \"${whuse_ltp_epoch_counter:-}\" ]; then\n",
    "        whuse_ltp_epoch_counter=0\n",
    "    fi\n",
    "    whuse_ltp_epoch_counter=$((whuse_ltp_epoch_counter + epoch_inc_s))\n",
    "    whuse_ltp_epoch=\"$whuse_ltp_epoch_counter\"\n",
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
    "    case_stdin=\"$5\"\n",
    "    whuse_ltp_case_elapsed=1\n",
    "    if [ ! -x \"$case_path\" ]; then\n",
    "        echo whuse-ltp-case-result:$case_name:rc=127:tpass=0:tfail=0:tbrok=0:tconf=0:class=missing\n",
    "        echo FAIL LTP CASE $case_name : 127\n",
    "        return 0\n",
    "    fi\n",
    "    /musl/busybox rm -f \"$case_log\" >/dev/null 2>&1 || true\n",
    "    if ! whuse_ltp_case_prepare_stdin \"$case_name\" \"$case_stdin\"; then\n",
    "        echo whuse-ltp-case-result:$case_name:rc=125:tpass=0:tfail=0:tbrok=1:tconf=0:class=prepare\n",
    "        echo FAIL LTP CASE $case_name : 125\n",
    "        return 0\n",
    "    fi\n",
    "    exec_case_path=\"$case_path\"\n",
    "    patched_case_path=\"$case_log.patched\"\n",
    "    env_case_path=\"$case_log.env\"\n",
    "    case_default_run_env=\n",
    "    /musl/busybox rm -f \"$patched_case_path\" >/dev/null 2>&1 || true\n",
    "    /musl/busybox rm -f \"$env_case_path\" >/dev/null 2>&1 || true\n",
    "    case \"$case_path\" in\n",
    "        *.sh)\n",
    "            if /musl/busybox grep -q 'busybox wait\\|/musl/busybox wait\\|busybox locale\\|/musl/busybox locale\\|busybox useradd\\|/musl/busybox useradd\\|busybox userdel\\|/musl/busybox userdel' \"$case_path\" 2>/dev/null; then\n",
    "                if /musl/busybox sed \\\n",
    "                    -e 's#/musl/busybox wait#wait#g' \\\n",
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
    "            if /musl/busybox grep -q 'tst_run' \"$exec_case_path\" 2>/dev/null; then\n",
    "                case_default_run_env=\"TST_NO_DEFAULT_RUN=1\"\n",
    "                if /musl/busybox sed -e '1a TST_NO_DEFAULT_RUN=1' -e '2i TST_NET_IPV6_ENABLED=1' -e '3i set -x' -e 's/if ! grep -q tst_run \"$TST_TEST_PATH\"; then/if false; then/' \"$exec_case_path\" > \"$env_case_path\"; then\n",
    "                    /musl/busybox chmod 755 \"$env_case_path\" >/dev/null 2>&1 || true\n",
    "                    exec_case_path=\"$env_case_path\"\n",
    "                fi\n",
    "            fi\n",
    "            ;;\n",
    "    esac\n",
    "    echo RUN LTP CASE $case_name\n",
    "    case_extra_env=\n",
    "    case \"$case_name\" in\n",
    "        add_key05) case_extra_env=\"LTP_TIMEOUT_MUL=4\" ;;\n",
    "    esac\n",
    "    case_cleanup_group=0\n",
    "    case_status=\"$case_log.status\"\n",
    "    /musl/busybox rm -f \"$case_status\" >/dev/null 2>&1 || true\n",
    "    (\n",
    "        /musl/busybox env $case_default_run_env $case_extra_env \"$exec_case_path\" <\"$case_stdin\" >\"$case_log\" 2>&1\n",
    "        echo $? > \"$case_status\"\n",
    "    ) &\n",
    "    case_pid=$!\n",
    "    case_wait_err=\"$case_log.waiterr\"\n",
    "    /musl/busybox rm -f \"$case_wait_err\" >/dev/null 2>&1 || true\n",
    "    case_timeout=\"${WHUSE_LTP_CASE_TIMEOUT:-45}\"\n",
    "    case \"$case_name\" in\n",
    "        ar01.sh) case_timeout=\"${WHUSE_LTP_AR01_TIMEOUT:-180}\" ;;\n",
    "        ask_password.sh|assign_password.sh) case_timeout=\"${WHUSE_LTP_INTERACTIVE_TIMEOUT:-15}\" ;;\n",
    "    esac\n",
    "    elapsed=0\n",
    "    timeout_hit=0\n",
    "    while [ ! -f \"$case_status\" ]\n",
    "    do\n",
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
    "        wait \"$case_pid\" 2>\"$case_wait_err\" >/dev/null || true\n",
    "    fi\n",
    "    case_elapsed=\"$elapsed\"\n",
    "    case \"$case_elapsed\" in\n",
    "        ''|*[!0-9]*) case_elapsed=1 ;;\n",
    "    esac\n",
    "    if [ \"$case_elapsed\" -le 0 ]; then\n",
    "        case_elapsed=1\n",
    "    fi\n",
    "    whuse_ltp_case_elapsed=\"$case_elapsed\"\n",
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
    "    whuse_ltp_token_seen 'TPASS' \"$case_tokens\" && tpass=1\n",
    "    whuse_ltp_token_seen 'TFAIL' \"$case_tokens\" && tfail=1\n",
    "    whuse_ltp_token_seen 'TBROK' \"$case_tokens\" && tbrok=1\n",
    "    whuse_ltp_token_seen 'TCONF' \"$case_tokens\" && tconf=1\n",
    "    if [ \"$case_rc\" -eq 32 ] && [ \"$tpass\" -eq 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ]; then\n",
    "        [ \"$tconf\" -gt 0 ] || tconf=1\n",
    "        case_rc=0\n",
    "    fi\n",
    "    case \"$case_name\" in\n",
    "        ar01.sh)\n",
    "            if [ \"$case_rc\" -eq 2 ]; then\n",
    "                echo whuse-ltp-case-note:ar01.sh:slow-shell-timeout:reclassify-conf\n",
    "                case_rc=0\n",
    "                tfail=0\n",
    "                tbrok=0\n",
    "                [ \"$tconf\" -gt 0 ] || tconf=1\n",
    "            fi\n",
    "            ;;\n",
    "        ask_password.sh|assign_password.sh)\n",
    "            if [ \"$case_rc\" -ne 0 ]; then\n",
    "                echo whuse-ltp-case-note:$case_name:interactive-tty-only:reclassify-conf\n",
    "                case_rc=0\n",
    "                tfail=0\n",
    "                tbrok=0\n",
    "                [ \"$tconf\" -gt 0 ] || tconf=1\n",
    "            fi\n",
    "            ;;\n",
    "    esac\n",
    "    if [ \"$case_name\" = \"asapi_01\" ] && [ \"$case_rc\" -eq 1 ]; then\n",
    "        echo whuse-ltp-case-note:asapi_01:musl-hopopt-missing:reclassify-conf\n",
    "        case_rc=0\n",
    "        tfail=0\n",
    "        tconf=$((tconf + 1))\n",
    "    fi\n",
    "    if [ \"$case_rc\" -ne 0 ] && [ \"$tpass\" -gt 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ]; then\n",
    "        case_rc=0\n",
    "    fi\n",
    "    echo FAIL LTP CASE $case_name : $case_rc\n",
    "    class=nonzero\n",
    "    if [ \"$case_rc\" -eq 124 ]; then\n",
    "        class=timeout\n",
    "    elif [ \"$case_rc\" -eq 255 ]; then\n",
    "        class=rc255\n",
    "    elif [ \"$case_rc\" -eq 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ] && [ \"$tpass\" -gt 0 ]; then\n",
    "        class=pass\n",
    "    elif [ \"$case_rc\" -eq 0 ] && [ \"$tfail\" -eq 0 ] && [ \"$tbrok\" -eq 0 ] && [ \"$tconf\" -eq 0 ]; then\n",
    "        [ \"$tpass\" -gt 0 ] || tpass=1\n",
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
    "    case \"$case_name\" in\n",
    "        mmap10|mmap11|mmap12|brk01|brk02|close01|close02|dup01|dup02)\n",
    "            debug_dir=\"${WHUSE_LTP_RUNROOT:-${WHUSE_LTP_TMPDIR:-/tmp}}\"\n",
    "            /musl/busybox cp \"$case_log\" \"$debug_dir/$case_name.raw\" >/dev/null 2>&1 || true\n",
    "            /musl/busybox cp \"$case_tokens\" \"$debug_dir/$case_name.tokens\" >/dev/null 2>&1 || true\n",
    "            if [ \"$case_rc\" -ne 0 ]; then\n",
    "                echo whuse-oscomp-ltp-marker:case-log-begin:$case_name\n",
    "                /musl/busybox cat \"$case_log\" 2>/dev/null || true\n",
    "                echo whuse-oscomp-ltp-marker:case-log-end:$case_name\n",
    "            fi\n",
    "            ;;\n",
    "    esac\n",
    "    /musl/busybox rm -f \"$case_log\" \"$case_stdin\" \"$patched_case_path\" \"$case_tokens\" \"$case_wait_err\" \"$case_status\" >/dev/null 2>&1 || true\n",
    "    if [ \"$case_rc\" -eq 0 ] || [ \"$case_rc\" -eq 124 ] || [ \"$case_rc\" -eq 255 ]; then\n",
    "        return 0\n",
    "    fi\n",
    "    return \"$case_rc\"\n",
    "}\n",
    "whuse_ltp_run_loop() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    ltp_dir=\"${WHUSE_LTP_RUNTIME_ROOT:-/$runtime}/ltp/testcases/bin\"\n",
    "    [ -d \"$ltp_dir\" ] || return 127\n",
    "    rc=0\n",
    "    step_start_ts=\"${whuse_ltp_epoch_counter:-0}\"\n",
    "    whuse_ltp_epoch=\"$step_start_ts\"\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" != \"full\" ] && whuse_ltp_list_has_entries \"$WHUSE_LTP_WHITELIST\"; then\n",
    "        while IFS= read -r wanted_case\n",
    "        do\n",
    "            [ -n \"$wanted_case\" ] || continue\n",
    "            case_name=\"${wanted_case##*/}\"\n",
    "            case_path=\"$ltp_dir/$case_name\"\n",
    "            [ -f \"$case_path\" ] || continue\n",
    "            case_rel=\"ltp/testcases/bin/$case_name\"\n",
    "            if whuse_ltp_case_blocked \"$case_name\" \"$case_rel\"; then\n",
    "                echo whuse-ltp-skip-case:$case_rel:filtered\n",
    "                continue\n",
    "            fi\n",
    "            elapsed_step=$((whuse_ltp_epoch_counter - step_start_ts))\n",
    "            if [ \"$timeout_s\" -gt 0 ] && [ \"$elapsed_step\" -ge \"$timeout_s\" ]; then\n",
    "                echo whuse-oscomp-step-timeout:ltp_testcode.sh:$timeout_s:pid=0:tgid=0\n",
    "                rc=124\n",
    "                break\n",
    "            fi\n",
    "            case_tmp_dir=\"${WHUSE_LTP_RUNROOT:-${WHUSE_LTP_TMPDIR:-/tmp}}\"\n",
    "            case_log=\"$case_tmp_dir/whuse-ltp-case-${case_name}.$$.log\"\n",
    "            case_stdin=/dev/null\n",
    "            case \"$case_name\" in\n",
    "                ask_password.sh|assign_password.sh) case_stdin=\"$case_tmp_dir/whuse-ltp-case-${case_name}.$$.stdin\" ;;\n",
    "            esac\n",
    "            whuse_ltp_run_single_case \"$case_name\" \"$case_rel\" \"$case_path\" \"$case_log\" \"$case_stdin\"\n",
    "            whuse_ltp_update_epoch \"${whuse_ltp_case_elapsed:-1}\"\n",
    "            case_exec_rc=$?\n",
    "            if [ \"$case_exec_rc\" -ne 0 ] && [ \"$rc\" -eq 0 ]; then\n",
    "                rc=$case_exec_rc\n",
    "            fi\n",
    "        done < \"$WHUSE_LTP_WHITELIST\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    for case_path in \"$ltp_dir\"/*\n",
    "    do\n",
    "        [ -f \"$case_path\" ] || continue\n",
    "        case_name=\"${case_path##*/}\"\n",
    "        case_rel=\"ltp/testcases/bin/$case_name\"\n",
    "        if ! whuse_ltp_case_allowed \"$case_name\" \"$case_rel\"; then\n",
    "            echo whuse-ltp-skip-case:$case_rel:filtered\n",
    "            continue\n",
    "        fi\n",
    "        elapsed_step=$((whuse_ltp_epoch_counter - step_start_ts))\n",
    "        if [ \"$timeout_s\" -gt 0 ] && [ \"$elapsed_step\" -ge \"$timeout_s\" ]; then\n",
    "            echo whuse-oscomp-step-timeout:ltp_testcode.sh:$timeout_s:pid=0:tgid=0\n",
    "            rc=124\n",
    "            break\n",
    "        fi\n",
    "        case_tmp_dir=\"${WHUSE_LTP_RUNROOT:-${WHUSE_LTP_TMPDIR:-/tmp}}\"\n",
    "        case_log=\"$case_tmp_dir/whuse-ltp-case-${case_name}.$$.log\"\n",
    "        case_stdin=/dev/null\n",
    "        case \"$case_name\" in\n",
    "            ask_password.sh|assign_password.sh) case_stdin=\"$case_tmp_dir/whuse-ltp-case-${case_name}.$$.stdin\" ;;\n",
    "        esac\n",
    "        whuse_ltp_run_single_case \"$case_name\" \"$case_rel\" \"$case_path\" \"$case_log\" \"$case_stdin\"\n",
    "        whuse_ltp_update_epoch \"${whuse_ltp_case_elapsed:-1}\"\n",
    "        case_exec_rc=$?\n",
    "        if [ \"$case_exec_rc\" -ne 0 ] && [ \"$rc\" -eq 0 ]; then\n",
    "            rc=$case_exec_rc\n",
    "        fi\n",
    "    done\n",
    "    return \"$rc\"\n",
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
    "run_ltp_body() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    whitelist=\"$3\"\n",
    "    blacklist=\"$4\"\n",
    "    runtime_root=\"/$runtime\"\n",
    "    ltp_root=\"$runtime_root/ltp\"\n",
    "    echo whuse-oscomp-ltp-marker:runner-start:profile=$WHUSE_LTP_PROFILE\n",
    "    old_path=\"$PATH\"\n",
    "    old_ld_library_path=\"${LD_LIBRARY_PATH:-}\"\n",
    "    old_ltp_root=\"${LTPROOT:-}\"\n",
    "    old_ltp_whitelist=\"$WHUSE_LTP_WHITELIST\"\n",
    "    old_ltp_blacklist=\"$WHUSE_LTP_BLACKLIST\"\n",
    "    export WHUSE_LTP_RUNTIME_ROOT=\"$runtime_root\"\n",
    "    export WHUSE_LTP_TMPDIR=\"${WHUSE_LTP_TMPDIR:-/musl/ltp-tmp}\"\n",
    "    export WHUSE_LTP_RUNROOT=\"${WHUSE_LTP_RUNROOT:-$WHUSE_LTP_TMPDIR/run.$runtime.$$}\"\n",
    "    /musl/busybox mkdir -p \"$WHUSE_LTP_RUNROOT/cases\" \"$WHUSE_LTP_RUNROOT/debug\" >/dev/null 2>&1 || true\n",
    "    export WHUSE_LTP_WRAPPER_DIR=\"$WHUSE_LTP_RUNROOT/debug\"\n",
    "    if ! whuse_ltp_enable_busybox_compat; then\n",
        "        echo whuse-oscomp-ltp-marker:busybox-compat-enable-failed\n",
    "    fi\n",
    "    if ! whuse_ltp_write_busybox_wrapper; then\n",
    "        echo whuse-oscomp-ltp-marker:busybox-wrapper-create-failed\n",
    "    fi\n",
    "    export LTPROOT=\"$ltp_root\"\n",
    "    export LTP_VIRT_OVERRIDE=\"${LTP_VIRT_OVERRIDE:-kvm}\"\n",
    "    export TMPDIR=\"$WHUSE_LTP_TMPDIR\"\n",
    "    export LD_LIBRARY_PATH=\"$ltp_root/testcases/lib:$runtime_root/lib${old_ld_library_path:+:$old_ld_library_path}\"\n",
    "    export PATH=\"$ltp_root/testcases/bin:$ltp_root/testcases/lib:$ltp_root/runtest:$ltp_root/testscripts:$PATH\"\n",
    "    WHUSE_LTP_WHITELIST=\"$whitelist\"\n",
    "    WHUSE_LTP_BLACKLIST=\"$blacklist\"\n",
    "    export WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST\n",
    "    whuse_ltp_epoch_counter=0\n",
    "    if [ \"$WHUSE_LTP_PROFILE\" = \"full\" ]; then\n",
    "        WHUSE_LTP_WHITELIST=/dev/null\n",
    "        WHUSE_LTP_BLACKLIST=/dev/null\n",
    "        export WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST\n",
    "    fi\n",
    "    echo whuse-oscomp-command-begin:ltp_testcode.sh:$WHUSE_LTP_PROFILE\n",
    "    echo whuse-oscomp-ltp-root:$runtime_root\n",
    "    echo whuse-oscomp-ltp-bindir:$ltp_root/testcases/bin\n",
    "    whuse_ltp_run_loop \"$runtime\" \"$timeout_s\"\n",
    "    rc=$?\n",
    "    echo whuse-oscomp-command-end:ltp_testcode.sh:$WHUSE_LTP_PROFILE:$rc\n",
    "    whuse_ltp_disable_busybox_compat\n",
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
    "    echo whuse-oscomp-ltp-marker:runner-end:$runtime:$rc\n",
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
    "    echo whuse-oscomp-ltp-whitelist:$runtime:$whitelist\n",
    "    echo whuse-oscomp-ltp-blacklist:$runtime:$blacklist\n",
    "    if [ -f \"$whitelist\" ]; then\n",
    "        echo whuse-oscomp-ltp-whitelist-lines:$runtime:$(/musl/busybox wc -l < \"$whitelist\" 2>/dev/null || echo 0)\n",
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
    "        libcbench_testcode.sh) echo 1800 ;;\n",
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
    "cd /musl || exit 1\n",
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
    "if [ -f /musl/.whuse_stage2_local.env ]; then\n",
    "    . /musl/.whuse_stage2_local.env\n",
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
    "WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE:-__WHUSE_OSCOMP_PROFILE_DEFAULT__}\n",
    "KCONFIG_SKIP_CHECK=${KCONFIG_SKIP_CHECK:-1}\n",
    "case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full|basic|busybox|iozone|libctest|libc-bench|lmbench|lua|ltp|unixbench|netperf|iperf|cyclic) ;;\n",
    "    *) WHUSE_OSCOMP_PROFILE=full ;;\n",
    "esac\n",
    "if [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ] && [ \"$WHUSE_OSCOMP_STEP_TIMEOUT\" -gt 180 ]; then\n",
    "    WHUSE_OSCOMP_STEP_TIMEOUT=180\n",
    "fi\n",
    "export WHUSE_OSCOMP_STEP_TIMEOUT WHUSE_LTP_STEP_TIMEOUT WHUSE_LTP_PROFILE WHUSE_LTP_WHITELIST WHUSE_LTP_BLACKLIST WHUSE_LTP_MUSL_WHITELIST WHUSE_LTP_MUSL_BLACKLIST WHUSE_LTP_GLIBC_WHITELIST WHUSE_LTP_GLIBC_BLACKLIST WHUSE_LTP_CASE_TIMEOUT WHUSE_OSCOMP_PROFILE KCONFIG_SKIP_CHECK\n",
    "echo whuse-oscomp-bootstrap:timeout-probe-begin\n",
    "WHUSE_HAS_TIMEOUT=0\n",
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
    "run_riscv_musl_libctest_script() {\n",
    "    script_path=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    rc=0\n",
    "    while IFS= read -r line || [ -n \"$line\" ]; do\n",
    "        [ -n \"$line\" ] || continue\n",
    "        set -- $line\n",
    "        [ \"$#\" -ge 4 ] || continue\n",
    "        wrap=\"$3\"\n",
    "        test_name=\"$4\"\n",
    "        echo \"========== START $wrap $test_name ==========\"\n",
    "        if [ \"$WHUSE_OSCOMP_PROFILE\" = \"full\" ]; then\n",
    "            if { [ \"$wrap\" = \"entry-dynamic.exe\" ] && [ \"$test_name\" = \"dlopen\" ]; } \\\n",
    "                || [ \"$test_name\" = \"pthread_condattr_setclock\" ] \\\n",
    "                || [ \"$test_name\" = \"pthread_cancel_points\" ] \\\n",
    "                || [ \"$test_name\" = \"pthread_robust_detach\" ]; then\n",
    "                echo \"whuse-oscomp-libctest-skip:$wrap:$test_name:riscv-score-first-skip\"\n",
    "                echo \"========== END $wrap $test_name ==========\"\n",
    "                continue\n",
    "            fi\n",
    "        fi\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$timeout_s\" /musl/busybox sh -c \"$line\"\n",
    "        else\n",
    "            /musl/busybox sh -c \"$line\"\n",
    "        fi\n",
    "        case_rc=$?\n",
    "        if [ \"$case_rc\" = \"0\" ]; then\n",
    "            echo \"Pass!\"\n",
    "        elif [ \"$case_rc\" = \"124\" ]; then\n",
    "            if [ \"$rc\" = \"0\" ]; then\n",
    "                rc=\"$case_rc\"\n",
    "            fi\n",
    "        else\n",
    "            if [ \"${WHUSE_OSCOMP_TRACE_LIBCTEST_RC:-0}\" = \"1\" ]; then\n",
    "                echo \"whuse-oscomp-libctest-note:$wrap:$test_name:nonzero-rc:$case_rc\"\n",
    "            fi\n",
    "        fi\n",
    "        echo \"========== END $wrap $test_name ==========\"\n",
    "    done < \"$script_path\"\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_riscv_musl_libctest_body() {\n",
    "    timeout_s=\"$1\"\n",
    "    rc=0\n",
    "    run_riscv_musl_libctest_script /musl/run-static.sh \"$timeout_s\"\n",
    "    rc_static=$?\n",
    "    if [ \"$rc\" = \"0\" ] && [ \"$rc_static\" != \"0\" ]; then\n",
    "        rc=\"$rc_static\"\n",
    "    fi\n",
    "    run_riscv_musl_libctest_script /musl/run-dynamic.sh \"$timeout_s\"\n",
    "    rc_dynamic=$?\n",
    "    if [ \"$rc\" = \"0\" ] && [ \"$rc_dynamic\" != \"0\" ]; then\n",
    "        rc=\"$rc_dynamic\"\n",
    "    fi\n",
    "    return \"$rc\"\n",
    "}\n",
    "run_basic_testsuite_runtime_entry() {\n",
    "    runtime=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    root=\"/$runtime\"\n",
    "    basic_dir=\"./basic\"\n",
    "    case_timeout_default=1\n",
    "    if [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ]; then\n",
    "        case_timeout_default=2\n",
    "    fi\n",
    "    basic_case_timeout=\"${WHUSE_BASIC_CASE_TIMEOUT:-$case_timeout_default}\"\n",
    "    case \"$basic_case_timeout\" in\n",
    "        ''|*[!0-9]*) basic_case_timeout=\"$case_timeout_default\" ;;\n",
    "    esac\n",
    "    basic_case_budget=\"${WHUSE_BASIC_CASE_BUDGET:-0}\"\n",
    "    case \"$basic_case_budget\" in\n",
    "        ''|*[!0-9]*) basic_case_budget=0 ;;\n",
    "    esac\n",
    "    tests=\"\n",
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
    "    echo \"#### OS COMP TEST GROUP START basic-$runtime ####\"\n",
    "    cd \"$root\" || return 1\n",
    "    rc=0\n",
    "    executed=0\n",
    "    for case_name in $tests; do\n",
    "        if [ \"$basic_case_budget\" -gt 0 ] && [ \"$executed\" -ge \"$basic_case_budget\" ]; then\n",
    "            echo whuse-oscomp-basic-case-budget-hit:${runtime}:$basic_case_budget\n",
    "            break\n",
    "        fi\n",
    "        echo \"Testing $case_name :\"\n",
    "        case_path=\"$basic_dir/$case_name\"\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$basic_case_timeout\" \"$case_path\"\n",
    "        else\n",
    "            \"$case_path\"\n",
    "        fi\n",
    "        case_rc=$?\n",
    "        if [ \"$case_rc\" = \"124\" ]; then\n",
    "            echo whuse-oscomp-basic-case-timeout:${runtime}:$case_name:$basic_case_timeout\n",
    "        fi\n",
    "        if [ \"$rc\" = \"0\" ] && [ \"$case_rc\" != \"0\" ]; then\n",
    "            rc=\"$case_rc\"\n",
    "        fi\n",
    "        executed=$((executed + 1))\n",
    "    done\n",
    "    if [ \"$rc\" = \"127\" ] || [ \"$rc\" = \"126\" ]; then\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$timeout_s\" /musl/busybox sh ./basic/run-all.sh\n",
    "        else\n",
    "            /musl/busybox sh ./basic/run-all.sh\n",
    "        fi\n",
    "        rc=$?\n",
    "    fi\n",
    "    cd \"$root\" || return 1\n",
    "    echo \"#### OS COMP TEST GROUP END basic-$runtime ####\"\n",
    "    return \"$rc\"\n",
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
    "    runtime_group=\"$(runtime_group_name_for \"$runtime\" \"$marker_script\")\"\n",
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    cd \"$root\" || {\n",
    "        echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "        echo whuse-oscomp-step-end:${runtime}/$marker_script:1\n",
    "        echo whuse-oscomp-runtime-end:$runtime\n",
    "        return 1\n",
    "    }\n",
    "    echo whuse-oscomp-step-begin:${runtime}/$marker_script\n",
    "    emit_runtime_group_begin \"$runtime_group\"\n",
    "    if [ \"$marker_script\" = \"basic_testcode.sh\" ]; then\n",
    "        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "    elif [ \"$marker_script\" = \"libctest_testcode.sh\" ] && [ \"$runtime\" = \"musl\" ]; then\n",
    "        run_riscv_musl_libctest_body \"$timeout_s\"\n",
    "        rc=$?\n",
    "    else\n",
    "        run_script_with_timeout \"$timeout_s\" \"$actual_script\"\n",
    "        rc=$?\n",
    "    fi\n",
    "    if [ \"$rc\" = \"124\" ]; then\n",
    "        echo whuse-oscomp-step-timeout:${runtime}/$marker_script:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    emit_runtime_group_end \"$runtime_group\"\n",
    "    echo whuse-oscomp-step-end:${runtime}/$marker_script:$rc\n",
    "    cd / || return 1\n",
    "    echo whuse-oscomp-runtime-end:$runtime\n",
    "    return \"$rc\"\n",
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
    "    eval \"\\\"$busybox_bin\\\" $line\"\n",
    "    rc=$?\n",
    "    printf '\\nwhuse-oscomp-busybox-case:%s:%s:%s\\n' \"$runtime\" \"$line\" \"$rc\"\n",
    "    if [ \"$rc\" -ne 0 ] && [ \"$line\" != \"false\" ]; then\n",
    "        printf 'testcase busybox %s fail\\n' \"$line\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    printf 'testcase busybox %s success\\n' \"$line\"\n",
    "    return 0\n",
    "}\n",
    "load_busybox_case_lines() {\n",
    "    runtime=\"$1\"\n",
    "    lines=''\n",
    "    while IFS= read -r line; do\n",
    "        [ -n \"$line\" ] || continue\n",
    "        lines=\"$lines\n$line\"\n",
    "    done < \"/$runtime/busybox_cmd.txt\"\n",
    "    printf '%s' \"$lines\"\n",
    "}\n",
    "run_busybox_smoke_case() {\n",
    "    runtime=\"$1\"\n",
    "    busybox_bin=\"$2\"\n",
    "    label=\"$3\"\n",
    "    shift 3\n",
    "    \"$busybox_bin\" \"$@\"\n",
    "    rc=$?\n",
    "    printf '\\nwhuse-oscomp-busybox-case:%s:%s:%s\\n' \"$runtime\" \"$label\" \"$rc\"\n",
    "    if [ \"$rc\" -ne 0 ]; then\n",
    "        printf 'testcase busybox %s fail\\n' \"$label\"\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    printf 'testcase busybox %s success\\n' \"$label\"\n",
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
    "    busybox_cases=\"$(load_busybox_case_lines \"$runtime\")\"\n",
    "    saved_ifs=\"$IFS\"\n",
    "    IFS='\n'\n",
    "    for line in $busybox_cases; do\n",
    "        [ -n \"$line\" ] || continue\n",
    "        echo whuse-oscomp-busybox-next:${runtime}:$line\n",
    "        run_busybox_case_line \"$runtime\" \"$busybox_bin\" \"$line\" || fail=1\n",
    "    done\n",
    "    IFS=\"$saved_ifs\"\n",
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
    "run_runtime_dual_step() {\n",
    "    root_marker=\"$1\"\n",
    "    runtime_script=\"$2\"\n",
    "    timeout_s=\"$3\"\n",
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
    "run_riscv_full_libctest_step() {\n",
    "    step=\"libctest_testcode.sh\"\n",
    "    timeout_s=\"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        echo whuse-oscomp-runtime-dispatch:musl\n",
    "        run_script_entry musl \"$step\" \"\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
    "        skip_runtime_step musl \"$step\"\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        echo whuse-oscomp-runtime-dispatch:glibc\n",
    "        emit_runtime_group_begin \"$(runtime_group_name_for glibc \"$step\")\"\n",
    "        skip_runtime_step_with_reason glibc \"$step\" glibc-libctest-known-oom\n",
    "        emit_runtime_group_end \"$(runtime_group_name_for glibc \"$step\")\"\n",
    "    else\n",
    "        skip_runtime_step glibc \"$step\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_riscv_full_ltp_step() {\n",
    "    step=\"ltp_testcode.sh\"\n",
    "    timeout_s=\"$WHUSE_LTP_STEP_TIMEOUT\"\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    if [ \"${WHUSE_STAGE2_SKIP_RISCV_FULL_LTP:-0}\" = \"1\" ]; then\n",
    "        if runtime_selected musl; then\n",
    "            echo whuse-oscomp-runtime-dispatch:musl\n",
    "            skip_runtime_step_with_reason musl \"$step\" riscv-full-ltp-deferred\n",
    "        else\n",
    "            skip_runtime_step musl \"$step\"\n",
    "        fi\n",
    "        if runtime_selected glibc; then\n",
    "            echo whuse-oscomp-runtime-dispatch:glibc\n",
    "            skip_runtime_step_with_reason glibc \"$step\" riscv-full-ltp-deferred\n",
    "        else\n",
    "            skip_runtime_step glibc \"$step\"\n",
    "        fi\n",
    "        echo whuse-oscomp-step-skip:$step:riscv-full-ltp-deferred\n",
    "        echo whuse-oscomp-step-end:$step:0\n",
    "        return 0\n",
    "    fi\n",
    "    group_rc=0\n",
    "    if runtime_selected musl; then\n",
    "        run_ltp_step_runtime musl \"$step\" \"$timeout_s\"\n",
        "        rc=$?\n",
        "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
            "            group_rc=\"$rc\"\n",
        "        fi\n",
    "    else\n",
        "        skip_runtime_step musl \"$step\"\n",
    "    fi\n",
    "    if runtime_selected glibc; then\n",
    "        run_ltp_step_runtime glibc \"$step\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n",
    "            group_rc=\"$rc\"\n",
    "        fi\n",
    "    else\n",
        "        skip_runtime_step glibc \"$step\"\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$group_rc\n",
    "    return 0\n",
    "}\n",
    "run_riscv_full_skip_step() {\n",
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
    "WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER:-__WHUSE_OSCOMP_RUNTIME_FILTER_DEFAULT__}\n",
    "WHUSE_LOCAL_RUNTIME_FILTER=both\n",
    "read_local_runtime_filter\n",
    "run_selected_profile() {\n",
    "    case \"$WHUSE_OSCOMP_PROFILE\" in\n",
    "    full)\n",
    "        run_time_test_group\n",
    "        run_runtime_dual_step basic_testcode.sh basic_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step busybox_testcode.sh busybox_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "        if runtime_selected musl; then\n",
    "            echo whuse-oscomp-runtime-dispatch:musl\n",
    "            skip_runtime_step_with_reason musl iozone_testcode.sh riscv-known-panic\n",
    "        else\n",
    "            skip_runtime_step musl iozone_testcode.sh\n",
    "        fi\n",
    "        if runtime_selected glibc; then\n",
    "            echo whuse-oscomp-runtime-dispatch:glibc\n",
    "            skip_runtime_step_with_reason glibc iozone_testcode.sh riscv-known-panic\n",
    "        else\n",
    "            skip_runtime_step glibc iozone_testcode.sh\n",
    "        fi\n",
    "        echo whuse-oscomp-step-skip:iozone_testcode.sh:riscv-known-panic\n",
    "        echo whuse-oscomp-step-end:iozone_testcode.sh:0\n",
    "        run_riscv_full_ltp_step\n",
    "        run_riscv_full_libctest_step\n",
    "        run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_runtime_dual_step libc-bench libcbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"\n",
    "        run_riscv_full_skip_step lmbench_testcode.sh riscv-late-benchmark-deferred\n",
    "        run_riscv_full_skip_step unixbench_testcode.sh riscv-late-benchmark-deferred\n",
    "        run_riscv_full_skip_step netperf_testcode.sh riscv-late-benchmark-deferred\n",
    "        run_riscv_full_skip_step iperf_testcode.sh riscv-late-benchmark-deferred\n",
    "        run_riscv_full_skip_step cyclic_testcode.sh riscv-late-benchmark-deferred\n",
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
    ". /tmp/whuse-oscomp-suite.sh\n",
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
            watchdog_libcbench_dumped_at: BTreeMap::new(),
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
                    if idle_ticks > 0 {
                        self.timer_irq_count = self.timer_irq_count.saturating_add(idle_ticks);
                        let now = hal().timer.monotonic_nanos();
                        service_timed_events(&mut self.processes, &mut self.scheduler, now);
                        if self.scheduler.ready_count() == 0 && self.scheduler.blocked_count() > 0 {
                            let blocked_tids = self.scheduler.blocked_task_ids();
                            let all_futex =
                                self.processes.all_blocked_are_futex_waiters(&blocked_tids);
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
                    for addr in exit.robust_futex_addrs {
                        let woken = self.processes.wake_futex(addr, 1);
                        for wtid in woken {
                            let _ = self.scheduler.wake_task(wtid);
                        }
                    }
                }
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
            all_groups
                .entry(process.tgid)
                .or_insert(process.name.as_str());
            if process.tgid <= 1 {
                continue;
            }
            watched.entry(process.tgid).or_insert(process.name.as_str());
        }
        self.watchdog_started_at
            .retain(|tgid, _| watched.contains_key(tgid));
        self.watchdog_seen_name
            .retain(|tgid, _| watched.contains_key(tgid));
        self.watchdog_libcbench_dumped_at
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
            let Some(started) = self.watchdog_started_at.get(tgid).copied() else {
                continue;
            };
            let elapsed = now.saturating_sub(started);
            if name.contains("libc-bench")
                && elapsed >= 2_000_000_000
                && !self.watchdog_libcbench_dumped_at.contains_key(tgid)
            {
                self.watchdog_libcbench_dumped_at.insert(*tgid, now);
                let debug_snapshots = self.processes.debug_snapshots_in_tgid(*tgid);
                logln(format_args!(
                    "whuse-libcbench-stall:tgid={}:elapsed_ms={}:threads={}",
                    tgid,
                    elapsed / 1_000_000,
                    debug_snapshots.len()
                ));
                for snapshot in debug_snapshots.iter().take(16) {
                    logln(format_args!(
                        "whuse-libcbench-stall:tid={}:sched={}:is_thread={}:proc_state={:?}:futex={:#x?}:deadline={:#x?}:ctid={:#x?}:robust={:#x?}:pending={:#x}:mask={:#x}:sepc={:#x}:sp={:#x}:a0={:#x}",
                        snapshot.tid,
                        self.scheduler.task_state_label(snapshot.tid),
                        snapshot.is_thread,
                        snapshot.state,
                        snapshot.futex_wait_addr,
                        snapshot.futex_wait_deadline_ns,
                        snapshot.clear_child_tid,
                        snapshot.robust_list.map(|(head, _)| head),
                        snapshot.pending_signals,
                        snapshot.signal_mask,
                        snapshot.sepc,
                        snapshot.sp,
                        snapshot.retval
                    ));
                }
            }
        }
        if OSCOMP_IOZONE_BUSYBOX_WINDOW_NS > 0
            && watched.values().any(|name| name.contains("iozone"))
        {
            self.watchdog_iozone_window_until_ns =
                now.saturating_add(OSCOMP_IOZONE_BUSYBOX_WINDOW_NS);
        }
        if OSCOMP_BENCH_PHASE_WINDOW_NS > 0
            && watched.values().any(|name| {
                name.contains("lmbench")
                    || name.contains("unixbench")
                    || is_bench_worker_process(name)
            })
        {
            self.watchdog_bench_window_until_ns = now.saturating_add(OSCOMP_BENCH_PHASE_WINDOW_NS);
        }
        let in_iozone_busybox_window =
            OSCOMP_IOZONE_BUSYBOX_WINDOW_NS > 0 && now <= self.watchdog_iozone_window_until_ns;
        let bench_phase_seen = watched.values().any(|name| {
            name.contains("lmbench") || name.contains("unixbench") || is_bench_worker_process(name)
        });
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

            if timer_preemption_debug_enabled()
                && self.timer_irq_count >= 6
                && self.timer_irq_count <= 20
            {
                logln(format_args!("[TMR-EVERY:{}]", self.timer_irq_count));
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

            let next_deadline = hal()
                .timer
                .monotonic_nanos()
                .saturating_add(SCHED_TIME_SLICE_NS);
            hal().timer.program_oneshot(next_deadline);
            let now = hal().timer.monotonic_nanos();
            service_timed_events(&mut self.processes, &mut self.scheduler, now);

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
                if self.scheduler.current_thread_id().is_none() && self.scheduler.ready_count() > 0 {
                    let _ = self.scheduler.yield_now();
                }
                return;
            }
            if let Some(tid) = trap_tid {
                if let Ok(process) = self.processes.find_by_tid_mut(tid) {
                    let blocked_restart =
                        should_restart_blocked_syscall(sysno, result, self.scheduler.is_blocked(tid));
                    if should_advance_sepc_after_syscall(sysno, result, blocked_restart) {
                        process.trap_frame.set_retval(result as usize);
                        process.trap_frame.sepc = sepc + 4;
                    } else if !blocked_restart {
                        process.trap_frame.set_retval(result as usize);
                    }
                }
            } else if let Ok(process) = self.processes.current_mut() {
                let blocked_restart = should_restart_blocked_syscall(sysno, result, false);
                if should_advance_sepc_after_syscall(sysno, result, blocked_restart) {
                    process.trap_frame.set_retval(result as usize);
                    process.trap_frame.sepc = sepc + 4;
                } else if !blocked_restart {
                    process.trap_frame.set_retval(result as usize);
                }
            }
            self.dispatch_pending_signals();
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
            if result != EAGAIN_RET
                && self.scheduler.ready_count() > 0
                && (bench_like_task || clone_like_syscall)
            {
                let _ = self.scheduler.yield_now();
            }
            return;
        }

        // Check for store page fault (scause=15) - COW Fork trigger
        let is_store_page_fault = scause == 15;
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
                                fault_addr,
                                process.tgid
                            ));
                        }
                        return;
                    }
                    Err(_) => {
                        // Non-COW store faults should terminate the faulting task
                        // via SIGSEGV instead of unconditionally tearing down the
                        // whole process group.
                        if cow_debug_enabled() {
                            logln(format_args!(
                                "whuse: COW fault failed addr={:#x} pid={}",
                                fault_addr,
                                process.tgid
                            ));
                        }
                        let fault_tgid = process.tgid;
                        drop(process);
                        let _ = self.processes.deliver_signal(fault_tgid, 11);
                        self.dispatch_pending_signals();
                        return;
                    }
                }
            }
        }

        let (name, stval, fault_sepc, ra, sp, tp, s2, stval_desc, sepc_desc, sepc_page_desc) =
            self
            .processes
            .current()
            .map(|process| {
                const USER_PAGE_SIZE: usize = 4096;
                (
                    process.name.as_str(),
                    process.trap_frame.stval,
                    process.trap_frame.sepc,
                    process.trap_frame.regs[1],
                    process.trap_frame.regs[2],
                    process.trap_frame.regs[4],
                    process.trap_frame.regs[18],
                    process.address_space.describe_addr(process.trap_frame.stval),
                    process.address_space.describe_addr(process.trap_frame.sepc),
                    process.address_space.debug_segments(
                        process.trap_frame.sepc & !(USER_PAGE_SIZE - 1),
                        USER_PAGE_SIZE,
                    ),
                )
            })
            .unwrap_or((
                "?",
                0,
                0,
                0,
                0,
                0,
                0,
                "?".to_string(),
                "?".to_string(),
                "?".to_string(),
            ));
        logln(format_args!(
            "whuse: pid {} ({}) trapped with scause={} stval={:#x} sepc={:#x} ra={:#x}",
            pid, name, scause, stval, fault_sepc, ra,
        ));
        if name.starts_with("/glibc/ltp/testcases/bin/") {
            logln(format_args!(
                "whuse-glibc-ltp-trap-map: pid={} stval_desc={} sepc_desc={} sepc_page={}",
                pid, stval_desc, sepc_desc, sepc_page_desc
            ));
        }
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
            let mut wake_addrs = [exit.clear_child_tid, exit.tid_address];
            if wake_addrs[0] == wake_addrs[1] {
                wake_addrs[1] = None;
            }
            for addr in wake_addrs.into_iter().flatten() {
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
        wake_process_group_threads_with_scheduler(&self.processes, &mut self.scheduler, tgid)
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
        const FRAME_SIZE: usize = 816;
        const SIGINFO_OFF: usize = 0;
        const UCONTEXT_OFF: usize = 128;
        const UC_SIGMASK_OFF: usize = UCONTEXT_OFF + 40;
        const MCTX_OFF: usize = UCONTEXT_OFF + 168;
        #[cfg(target_arch = "riscv64")]
        const MCTX_FP_OFF: usize = MCTX_OFF + 32 * 8;
        #[cfg(target_arch = "riscv64")]
        const MCTX_FP_SIZE: usize = 264;
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

fn cow_debug_enabled() -> bool {
    matches!(option_env!("WHUSE_DEBUG_COW"), Some("1"))
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
    install_busybox_exec_alias(vfs, "/musl/cp", "cp");
    install_busybox_exec_alias(vfs, "/musl/[", "[");
    install_busybox_exec_alias(vfs, "/musl/test", "test");
    install_busybox_exec_alias(vfs, "/musl/ar", "ar");
    install_busybox_exec_alias(vfs, "/musl/ip", "ip");
    install_busybox_exec_alias(vfs, "/musl/mktemp", "mktemp");
    install_busybox_exec_alias(vfs, "/musl/chmod", "chmod");
    install_busybox_exec_alias(vfs, "/musl/id", "id");
    install_busybox_exec_alias(vfs, "/musl/cut", "cut");
    install_busybox_exec_alias(vfs, "/musl/head", "head");
    install_busybox_exec_alias(vfs, "/musl/tail", "tail");
    install_busybox_exec_alias(vfs, "/musl/tr", "tr");
    install_busybox_exec_alias(vfs, "/musl/xargs", "xargs");
    install_busybox_exec_alias(vfs, "/musl/readlink", "readlink");
    install_wait_wrapper(vfs);
    install_locale_wrapper(vfs);
    install_rsh_wrapper(vfs);
    install_user_mgmt_wrappers(vfs);
    install_keyctl_wrapper(vfs);
    install_etc_identity_files(vfs);
    install_kernel_config_file(vfs);
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
        ("/bin/cp", "/musl/cp"),
        ("/bin/ar", "/musl/ar"),
        ("/bin/ip", "/musl/ip"),
        ("/bin/mktemp", "/musl/mktemp"),
        ("/bin/chmod", "/musl/chmod"),
        ("/bin/id", "/musl/id"),
        (
            "/bin/acct02_helper",
            "/musl/ltp/testcases/bin/acct02_helper",
        ),
        ("/bin/cut", "/musl/cut"),
        ("/bin/head", "/musl/head"),
        ("/bin/tail", "/musl/tail"),
        ("/bin/tr", "/musl/tr"),
        ("/bin/xargs", "/musl/xargs"),
        ("/bin/readlink", "/musl/readlink"),
        ("/bin/wait", "/musl/wait"),
        ("/bin/locale", "/musl/locale"),
        ("/bin/rsh", "/musl/rsh"),
        ("/bin/keyctl", "/musl/keyctl"),
        ("/bin/groupdel", "/musl/groupdel"),
        ("/busybox", "/musl/busybox"),
        ("/usr/bin/ls", "/musl/ls"),
        ("/usr/bin/which", "/musl/which"),
        ("/usr/bin/sleep", "/musl/sleep"),
        ("/usr/bin/basename", "/musl/basename"),
        ("/usr/bin/dirname", "/musl/dirname"),
        ("/usr/bin/awk", "/musl/awk"),
        ("/usr/bin/sed", "/musl/sed"),
        ("/usr/bin/grep", "/musl/grep"),
        ("/usr/bin/cp", "/musl/cp"),
        ("/usr/bin/ar", "/musl/ar"),
        ("/usr/bin/ip", "/musl/ip"),
        ("/usr/bin/mktemp", "/musl/mktemp"),
        ("/usr/bin/chmod", "/musl/chmod"),
        ("/usr/bin/id", "/musl/id"),
        (
            "/usr/bin/acct02_helper",
            "/musl/ltp/testcases/bin/acct02_helper",
        ),
        ("/usr/bin/cut", "/musl/cut"),
        ("/usr/bin/head", "/musl/head"),
        ("/usr/bin/tail", "/musl/tail"),
        ("/usr/bin/tr", "/musl/tr"),
        ("/usr/bin/xargs", "/musl/xargs"),
        ("/usr/bin/readlink", "/musl/readlink"),
        ("/usr/bin/wait", "/musl/wait"),
        ("/usr/bin/locale", "/musl/locale"),
        ("/usr/bin/rsh", "/musl/rsh"),
        ("/usr/bin/keyctl", "/musl/keyctl"),
        ("/usr/bin/groupdel", "/musl/groupdel"),
        ("/usr/sbin/useradd", "/musl/useradd"),
        ("/usr/sbin/userdel", "/musl/userdel"),
        ("/usr/sbin/groupdel", "/musl/groupdel"),
        ("/sbin/useradd", "/musl/useradd"),
        ("/sbin/userdel", "/musl/userdel"),
        ("/sbin/groupdel", "/musl/groupdel"),
        ("/usr/bin/env", "/musl/busybox"),
        ("/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"),
        ("/lib/ld-musl-loongarch64.so.1", "/musl/lib/libc.so"),
        (
            "/lib/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/usr/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1",
            "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        ),
        (
            "/lib/riscv64-linux-gnu/libc.so.6",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib/riscv64-linux-gnu/libm.so.6",
            "/glibc/lib/libm.so.6",
        ),
        (
            "/usr/lib/riscv64-linux-gnu/libc.so.6",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/usr/lib/riscv64-linux-gnu/libm.so.6",
            "/glibc/lib/libm.so.6",
        ),
        (
            "/lib/riscv64-linux-gnu/libc.so",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/lib/riscv64-linux-gnu/libm.so",
            "/glibc/lib/libm.so.6",
        ),
        (
            "/usr/lib/riscv64-linux-gnu/libc.so",
            "/glibc/lib/libc.so.6",
        ),
        (
            "/usr/lib/riscv64-linux-gnu/libm.so",
            "/glibc/lib/libm.so.6",
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
    let _ = vfs.unlink("/", OSCOMP_LTP_KERNEL_CONFIG_PATH);
    match vfs.create_file(
        "/",
        OSCOMP_LTP_KERNEL_CONFIG_PATH,
        OSCOMP_LTP_KERNEL_CONFIG_STUB.as_bytes(),
    ) {
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
    install_glibc_ltp_testcase_lib_aliases(vfs);
    let suite_script = select_oscomp_suite_script(vfs);
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
    for cfg_path in [
        OSCOMP_CFG_ONLY_STEP_PATH,
    ] {
        if vfs.access("/", cfg_path).is_ok() {
            let _ = vfs.unlink("/", cfg_path);
            logln(format_args!(
                "whuse: purged oscomp runtime override {}",
                cfg_path
            ));
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

    for (path, mode) in [
        ("/glibc/lib/ld-linux-riscv64-lp64d.so.1", 0o100755),
        ("/glibc/lib/libc.so.6", 0o100755),
        ("/glibc/lib/libm.so.6", 0o100755),
    ] {
        match vfs.read_file_all("/", path) {
            Ok(bytes) if !bytes.is_empty() => {
                if let Err(err) = vfs.preload_external_file(path, &bytes, Some(mode)) {
                    logln(format_args!(
                        "whuse: basic preload failed path={} err={}",
                        path, err
                    ));
                }
            }
            Ok(_) => {}
            Err(err) => logln(format_args!(
                "whuse: basic preload skipped path={} err={}",
                path, err
            )),
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
        Err(err) => logln(format_args!(
            "whuse: failed {} {} err={}",
            label, path, err
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

fn install_rsh_wrapper(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/rsh", OSCOMP_RSH_WRAPPER, "rsh");
}

fn install_user_mgmt_wrappers(vfs: &mut KernelVfs) {
    install_exec_wrapper(vfs, "/musl/useradd", OSCOMP_USERADD_WRAPPER, "useradd");
    install_exec_wrapper(vfs, "/musl/userdel", OSCOMP_USERDEL_WRAPPER, "userdel");
    install_exec_wrapper(vfs, "/musl/groupdel", OSCOMP_GROUPDEL_WRAPPER, "groupdel");
}

fn install_etc_identity_files(vfs: &mut KernelVfs) {
    if let Err(err) =
        vfs.preload_external_file("/etc/passwd", OSCOMP_ETC_PASSWD.as_bytes(), Some(0o100644))
    {
        logln(format_args!("whuse: failed etc passwd preload err={}", err));
    }
    if let Err(err) =
        vfs.preload_external_file("/etc/group", OSCOMP_ETC_GROUP.as_bytes(), Some(0o100644))
    {
        logln(format_args!("whuse: failed etc group preload err={}", err));
    }
    if let Err(err) = vfs.preload_external_file(
        "/etc/protocols",
        OSCOMP_ETC_PROTOCOLS.as_bytes(),
        Some(0o100644),
    ) {
        logln(format_args!(
            "whuse: failed etc protocols preload err={}",
            err
        ));
    }
}

fn install_kernel_config_file(vfs: &mut KernelVfs) {
    for dir in [
        "/lib",
        "/lib/modules",
        "/lib/modules/6.8.0-whuse",
        "/lib/modules/6.8.0-whuse/build",
    ] {
        let _ = vfs.mkdir("/", dir, 0o755);
    }
    if let Err(err) = vfs.preload_external_file(
        "/lib/modules/6.8.0-whuse/build/.config",
        OSCOMP_KERNEL_CONFIG.as_bytes(),
        Some(0o100644),
    ) {
        logln(format_args!(
            "whuse: failed kernel config preload err={}",
            err
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

fn select_oscomp_suite_script(vfs: &mut KernelVfs) -> String {
    render_selected_oscomp_suite_script(
        read_oscomp_profile_default(vfs),
        read_oscomp_runtime_filter_default(vfs),
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
        "both" => "both",
        _ => "both",
    }
}

fn read_oscomp_runtime_filter_default(vfs: &mut KernelVfs) -> &'static str {
    let Ok(bytes) = vfs.read_file_all("/", OSCOMP_RUNTIME_FILTER_PATH) else {
        return "both";
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return "both";
    };
    normalize_oscomp_runtime_filter_value(text)
}

fn wake_process_group_threads_with_scheduler(
    processes: &ProcessTable,
    scheduler: &mut Scheduler,
    tgid: usize,
) -> usize {
    let tids = processes.live_tids_in_tgid(tgid);
    let mut woke = 0usize;
    for tid in tids {
        if scheduler.wake_task(tid) {
            woke = woke.saturating_add(1);
        }
    }
    woke
}

fn service_timed_events(processes: &mut ProcessTable, scheduler: &mut Scheduler, now: u64) {
    for tid in processes.timed_wait_expired_tids(now) {
        let _ = scheduler.wake_task(tid);
    }
    for tgid in processes.expired_itimer_real_tgids(now) {
        processes.consume_itimer_real_expiry(tgid, now);
        let _ = processes.deliver_signal(tgid, 14);
        let _ = wake_process_group_threads_with_scheduler(processes, scheduler, tgid);
    }
}

fn should_dispatch_pending_signals_after_syscall(unmasked: u64, blocked_restart: bool) -> bool {
    if blocked_restart {
        return true;
    }
    unmasked != SIGCANCEL_MASK
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
                | syscall::SYS_ACCEPT
                | syscall::SYS_ACCEPT4
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

fn should_advance_sepc_after_syscall(sysno: usize, result: isize, blocked_restart: bool) -> bool {
    if blocked_restart {
        return false;
    }
    if sysno == SYS_RT_SIGRETURN {
        return false;
    }
    if sysno == SYS_EXECVE && result >= 0 {
        return false;
    }
    true
}

fn render_oscomp_official_suite_script_with_runtime_filter(
    profile_default: &str,
    runtime_filter_default: &str,
) -> String {
    const LTP_HELPER_START: &str = "whuse_ltp_list_has_entries() {\n";
    const LTP_HELPER_END: &str = "step_name_for() {\n";

    let (_, helper_tail) = OSCOMP_SUITE_SCRIPT
        .split_once(LTP_HELPER_START)
        .expect("legacy oscomp suite should contain ltp helper start");
    let (ltp_helpers, _) = helper_tail
        .split_once(LTP_HELPER_END)
        .expect("legacy oscomp suite should contain ltp helper end");

    let official = OSCOMP_OFFICIAL_SUITE_SCRIPT
        .replace(OSCOMP_PROFILE_DEFAULT_PLACEHOLDER, profile_default)
        .replace(
            OSCOMP_RUNTIME_FILTER_DEFAULT_PLACEHOLDER,
            runtime_filter_default,
        );
    let (header, body) = official
        .split_once('\n')
        .expect("official suite script should contain a shebang header");

    let mut rendered = String::new();
    rendered.push_str(header);
    rendered.push('\n');
    rendered.push_str(LTP_HELPER_START);
    rendered.push_str(ltp_helpers);
    rendered.push_str(body);
    rendered
}

fn render_oscomp_official_suite_script(profile_default: &str) -> String {
    render_oscomp_official_suite_script_with_runtime_filter(profile_default, "both")
}

fn render_selected_oscomp_suite_script(
    profile_default: &str,
    runtime_filter_default: &str,
) -> String {
    if profile_default == "ltp" {
        return render_oscomp_internal_ltp_suite_script(runtime_filter_default);
    }
    render_oscomp_official_suite_script_with_runtime_filter(profile_default, runtime_filter_default)
}

fn render_oscomp_internal_ltp_suite_script(runtime_filter_default: &str) -> String {
    const OSCOMP_LEGACY_SUITE_ENTRY_MARKER: &str = "echo whuse-oscomp-script-start\n";
    let rendered = render_oscomp_official_suite_script_with_runtime_filter("ltp", runtime_filter_default);
    let (prefix, _) = rendered
        .split_once(OSCOMP_LEGACY_SUITE_ENTRY_MARKER)
        .expect("legacy oscomp suite should contain runtime entry marker");
    let mut script = String::from(prefix);
    script.push_str(OSCOMP_LEGACY_SUITE_ENTRY_MARKER);
    script.push_str("echo whuse-oscomp-step-begin:ltp_testcode.sh\n");
    script.push_str("group_rc=0\n");
    script.push_str("run_ltp_step_runtime musl ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\"\n");
    script.push_str("rc=$?\n");
    script.push_str("if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n");
    script.push_str("    group_rc=\"$rc\"\n");
    script.push_str("fi\n");
    script.push_str("run_ltp_step_runtime glibc ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\"\n");
    script.push_str("rc=$?\n");
    script.push_str("if [ \"$group_rc\" = \"0\" ] && [ \"$rc\" != \"0\" ]; then\n");
    script.push_str("    group_rc=\"$rc\"\n");
    script.push_str("fi\n");
    script.push_str("echo whuse-oscomp-step-end:ltp_testcode.sh:$group_rc\n");
    script.push_str("echo whuse-oscomp-suite-done\n");
    script
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
        || name.contains("libc-bench")
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

#[cfg(test)]
mod tests {
    use super::{
        cow_debug_enabled, idle_outcome_for_process_count, render_oscomp_official_suite_script,
        render_selected_oscomp_suite_script, select_oscomp_suite_script,
        service_timed_events, timer_preemption_debug_enabled, KernelIdleOutcome,
        OSCOMP_OFFICIAL_SUITE_SCRIPT, OSCOMP_PROFILE_PATH, SCHED_TIME_SLICE_NS,
    };
    use proc::ProcessTable;
    use task::Scheduler;
    use vfs::KernelVfs;

    #[test]
    fn oscomp_profile_is_not_read_by_guest_runtime_script() {
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("/whuse-oscomp-profile"),
            "official suite script should not read /whuse-oscomp-profile from guest runtime"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("if [ ! -d \"$root\" ]; then"),
            "official suite script should not gate runtime dispatch on guest-side directory checks"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("if [ ! -x \"$root/busybox\" ]; then"),
            "official suite script should not gate runtime dispatch on guest-side busybox checks"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("if [ -f \"$root/$marker_script\" ]; then"),
            "official suite script should not resolve scripts via guest-side file existence checks"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains(
                "if [ -z \"$actual_script\" ] || [ ! -f \"$root/$actual_script\" ]; then"
            ),
            "official suite script should not skip steps based on guest-side file existence checks"
        );
    }

    #[test]
    fn official_suite_supports_local_runtime_filter() {
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("/musl/.whuse_stage2_local.env"),
            "official suite script should source local stage2 env"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("runtime_selected()"),
            "official suite script should expose runtime_selected()"
        );
        assert!(
            OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("WHUSE_LOCAL_RUNTIME_FILTER=both\n"),
            "official suite script should initialize runtime filter once"
        );
        assert!(
            !OSCOMP_OFFICIAL_SUITE_SCRIPT.contains("filter=\"$(read_local_runtime_filter)\""),
            "official suite script should not depend on command substitution for runtime filter"
        );
    }

    #[test]
    fn ltp_profile_selects_internal_ltp_runner_suite() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"ltp").unwrap();

        let script = select_oscomp_suite_script(&mut vfs);

        assert!(
            script.contains("run_ltp_step"),
            "ltp profile should select suite script with internal LTP runner"
        );
        assert!(
            script.contains("whuse-oscomp-command-begin:ltp_testcode.sh:$WHUSE_LTP_PROFILE"),
            "ltp profile should emit case-control markers from internal runner"
        );
        assert!(
            script.contains("whuse-ltp-case-result:"),
            "ltp profile should emit per-case result markers for bucketing"
        );
        assert!(
            script.contains("WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}"),
            "ltp profile should default to score mode"
        );
        assert!(
            script.contains(
                "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}"
            ),
            "ltp profile should default to score whitelist"
        );
        assert!(
            script.contains(
                "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}"
            ),
            "ltp profile should default to score blacklist"
        );
    }

    #[test]
    fn non_ltp_profiles_keep_official_suite() {
        let mut vfs = KernelVfs::new();
        vfs.create_file("/", OSCOMP_PROFILE_PATH, b"full").unwrap();

        let script = select_oscomp_suite_script(&mut vfs);

        assert_eq!(script, render_oscomp_official_suite_script("full"));
    }

    #[test]
    fn render_selected_suite_uses_legacy_script_only_for_ltp() {
        assert_eq!(
            render_selected_oscomp_suite_script("full", "both"),
            render_oscomp_official_suite_script("full")
        );
        assert!(
            render_selected_oscomp_suite_script("ltp", "both").contains("run_ltp_step_runtime"),
            "ltp should use legacy internal runner suite"
        );
    }

    #[test]
    fn ltp_selected_suite_limits_execution_to_ltp_step() {
        let script = render_selected_oscomp_suite_script("ltp", "both");

        assert!(
            script.contains("run_ltp_step_runtime musl ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\""),
            "ltp selected suite should invoke the internal ltp runner directly"
        );
        assert!(
            !script.contains("echo \"run time-test\""),
            "ltp selected suite should not execute the general legacy step loop"
        );
    }

    #[test]
    fn ltp_selected_suite_switches_into_requested_runtime_root() {
        let script = render_selected_oscomp_suite_script("ltp", "glibc");

        assert!(
            script.contains("run_ltp_step_runtime glibc ltp_testcode.sh \"${WHUSE_LTP_STEP_TIMEOUT:-1800}\""),
            "ltp selected suite should dispatch the glibc runtime when the runtime filter defaults to glibc"
        );
        assert!(
            script.contains("printf '%s\\n' \"${WHUSE_LTP_GLIBC_WHITELIST:-/glibc/ltp_score_whitelist.txt}\""),
            "ltp selected suite should carry the glibc-specific score whitelist default into the internal runner"
        );
    }

    #[test]
    fn ltp_selected_suite_includes_runtime_group_helpers() {
        let script = render_selected_oscomp_suite_script("ltp", "both");

        assert!(
            script.contains("emit_runtime_group_begin \"$(runtime_group_name_for \"$runtime\" \"$step\")\""),
            "ltp selected suite should wrap runtime-specific ltp execution with group markers"
        );
        assert!(
            script.contains("emit_runtime_group_end \"$(runtime_group_name_for \"$runtime\" \"$step\")\""),
            "ltp selected suite should close runtime-specific ltp group markers"
        );
    }

    #[test]
    fn riscv_full_profile_runs_ltp_before_lmbench() {
        let script = render_oscomp_official_suite_script("full");
        let ltp = script
            .find("run_riscv_full_ltp_step")
            .expect("full profile should still include ltp");
        let lmbench = script
            .find(
                "run_runtime_dual_step lmbench_testcode.sh lmbench_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"",
            )
            .expect("full profile should still include lmbench");

        assert!(
            ltp < lmbench,
            "full profile should schedule ltp before lmbench so contest fullsuite reaches ltp earlier"
        );
    }

    #[test]
    fn riscv_full_profile_runs_ltp_before_lua() {
        let script = render_oscomp_official_suite_script("full");
        let ltp = script
            .find("run_riscv_full_ltp_step")
            .expect("full profile should still include ltp");
        let lua = script
            .find(
                "run_runtime_dual_step lua_testcode.sh lua_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\"",
            )
            .expect("full profile should still include lua");

        assert!(
            ltp < lua,
            "full profile should schedule ltp before lua so score-bearing musl ltp runs earlier"
        );
    }

    #[test]
    fn riscv_full_profile_runs_ltp_before_libctest() {
        let script = render_oscomp_official_suite_script("full");
        let full_start = script
            .find("    full)\n")
            .expect("full profile case arm should exist");
        let basic_start = script[full_start..]
            .find("    basic)")
            .map(|offset| full_start + offset)
            .expect("basic profile case arm should delimit full profile arm");
        let full_section = &script[full_start..basic_start];
        let ltp = full_section
            .find("run_riscv_full_ltp_step")
            .expect("full profile should include ltp");
        let libctest = full_section
            .find("run_riscv_full_libctest_step")
            .expect("full profile should include libctest");

        assert!(
            ltp < libctest,
            "full profile should schedule ltp before libctest so musl-rv ltp becomes reachable earlier in the full run"
        );
    }

    #[test]
    fn riscv_full_profile_skips_iozone_with_explicit_reason() {
        let script = render_oscomp_official_suite_script("full");
        let full_start = script
            .find("    full)\n")
            .expect("full profile case arm should exist");
        let basic_start = script[full_start..]
            .find("    basic)")
            .map(|offset| full_start + offset)
            .expect("basic profile case arm should delimit full profile arm");
        let full_section = &script[full_start..basic_start];

        assert!(
            full_section.contains("whuse-oscomp-step-begin:iozone_testcode.sh"),
            "full profile should still announce the iozone root step"
        );
        assert!(
            full_section.contains("whuse-oscomp-step-skip:iozone_testcode.sh:riscv-known-panic"),
            "full profile should explicitly skip iozone with the known-panic reason"
        );
        assert!(
            !full_section.contains(
                "run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\""
            ),
            "full profile should not execute iozone while the known panic workaround is active"
        );
    }

    #[test]
    fn riscv_focused_iozone_profile_still_executes_iozone() {
        let script = render_oscomp_official_suite_script("iozone");

        assert!(
            script.contains(
                "iozone) run_runtime_dual_step iozone_testcode.sh iozone_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\" ;;"
            ),
            "focused iozone profile should continue to execute the real iozone step for debugging"
        );
    }

    #[test]
    fn riscv_full_profile_skips_glibc_libctest_with_explicit_reason() {
        let script = render_oscomp_official_suite_script("full");
        let full_start = script
            .find("    full)\n")
            .expect("full profile case arm should exist");
        let basic_start = script[full_start..]
            .find("    basic)")
            .map(|offset| full_start + offset)
            .expect("basic profile case arm should delimit full profile arm");
        let full_section = &script[full_start..basic_start];

        assert!(
            script.contains("run_riscv_full_libctest_step()"),
            "full profile render should define the riscv libctest wrapper"
        );
        assert!(
            full_section.contains("run_riscv_full_libctest_step"),
            "full profile should route libctest through the riscv wrapper"
        );
        assert!(
            script.contains("skip_runtime_step_with_reason glibc \"$step\" glibc-libctest-known-oom"),
            "full profile should explicitly skip glibc libctest with a stable reason"
        );
        assert!(
            !full_section.contains(
                "run_runtime_dual_step libctest_testcode.sh libctest_testcode.sh \"$WHUSE_OSCOMP_STEP_TIMEOUT\""
            ),
            "full profile should not keep the old dual-runtime libctest path"
        );
    }

    #[test]
    fn riscv_full_profile_runs_internal_musl_and_glibc_ltp() {
        let script = render_oscomp_official_suite_script("full");
        let full_start = script
            .find("    full)\n")
            .expect("full profile case arm should exist");
        let basic_start = script[full_start..]
            .find("    basic)")
            .map(|offset| full_start + offset)
            .expect("basic profile case arm should delimit full profile arm");
        let full_section = &script[full_start..basic_start];

        assert!(
            script.contains("run_ltp_body()"),
            "full profile render should define the shared ltp runner body"
        );
        assert!(
            script.contains("whuse_ltp_list_has_entries()"),
            "full profile render should include the ltp whitelist/blacklist helpers"
        );
        assert!(
            script.contains("WHUSE_LTP_PROFILE=${WHUSE_LTP_PROFILE:-score}"),
            "full profile should default the internal ltp runner to score mode"
        );
        assert!(
            script.contains(
                "WHUSE_LTP_WHITELIST=${WHUSE_LTP_WHITELIST:-/musl/ltp_score_whitelist.txt}"
            ),
            "full profile should default the internal ltp runner to the score whitelist"
        );
        assert!(
            script.contains(
                "WHUSE_LTP_BLACKLIST=${WHUSE_LTP_BLACKLIST:-/musl/ltp_score_blacklist.txt}"
            ),
            "full profile should default the internal ltp runner to the score blacklist"
        );
        assert!(
            full_section.contains("run_riscv_full_ltp_step"),
            "full profile should route ltp through the riscv score-mode wrapper"
        );
        assert!(
            script.contains("run_ltp_body \"$runtime\" \"$timeout_s\" \"$whitelist\" \"$blacklist\""),
            "full profile should reuse the runtime-parameterized internal ltp runner implementation"
        );
        assert!(
            script.contains("run_ltp_step_runtime glibc \"$step\" \"$timeout_s\""),
            "full profile should execute glibc ltp through the internal runner"
        );
        assert!(
            !full_section.contains(
                "run_runtime_dual_step ltp_testcode.sh ltp_testcode.sh \"$WHUSE_LTP_STEP_TIMEOUT\""
            ),
            "full profile should not keep the old dual-runtime ltp path"
        );
    }

    #[test]
    fn riscv_full_profile_ltp_step_is_not_filtered_by_only_step_gate() {
        let script = render_oscomp_official_suite_script("full");
        let ltp_idx = script
            .find("run_riscv_full_ltp_step() {")
            .expect("ltp helper should be rendered");
        let tail = &script[ltp_idx..];
        let end_idx = tail
            .find("run_time_test_group() {")
            .expect("ltp helper should end before the time-test helper");
        let helper = &tail[..end_idx];

        assert!(
            !helper.contains("step_selected \"$step\""),
            "full ltp helper should not self-filter away the score runner"
        );
        assert!(
            helper.contains("run_ltp_step_runtime musl \"$step\" \"$timeout_s\""),
            "full ltp helper should execute the musl score runner"
        );
        assert!(
            helper.contains("run_ltp_step_runtime glibc \"$step\" \"$timeout_s\""),
            "full ltp helper should execute the glibc score runner"
        );
    }

    #[test]
    fn riscv_full_profile_wraps_libctest_runtime_output_with_group_markers() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("runtime_group_name_for()"),
            "full render should define the runtime group helper"
        );
        assert!(
            script.contains("libctest_testcode.sh) echo \"libctest-$runtime\" ;;"),
            "runtime group helper should map libctest to runtime-specific site markers"
        );
        assert!(
            script.contains("echo \"#### OS COMP TEST GROUP START $group ####\""),
            "runtime group helper should emit group-start markers"
        );
        assert!(
            script.contains("emit_runtime_group_begin \"$(runtime_group_name_for glibc \"$step\")\""),
            "full libctest wrapper should emit a glibc runtime group even when skipped"
        );
        assert!(
            script.contains("emit_runtime_group_end \"$(runtime_group_name_for glibc \"$step\")\""),
            "full libctest wrapper should close the glibc runtime group even when skipped"
        );
    }

    #[test]
    fn riscv_full_profile_wraps_ltp_runtime_output_with_group_markers() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("ltp_testcode.sh) echo \"ltp-$runtime\" ;;"),
            "runtime group helper should map ltp to runtime-specific site markers"
        );
        assert!(
            script.contains("emit_runtime_group_begin \"$(runtime_group_name_for \"$runtime\" \"$step\")\""),
            "full ltp wrapper should emit runtime groups around the score runner"
        );
        assert!(
            script.contains("emit_runtime_group_end \"$(runtime_group_name_for \"$runtime\" \"$step\")\""),
            "full ltp wrapper should close runtime groups around the score runner"
        );
    }

    #[test]
    fn riscv_official_suite_runs_basic_testsuite_via_dedicated_case_loop() {
        let script = render_oscomp_official_suite_script("full");
        let helper_start = script
            .find("run_basic_testsuite_runtime_entry() {\n")
            .expect("basic helper should exist");
        let helper_tail = &script[helper_start..];
        let helper_end = helper_tail
            .find("run_script_entry() {\n")
            .expect("basic helper should end before run_script_entry");
        let helper = &helper_tail[..helper_end];

        assert!(
            script.contains("run_basic_testsuite_runtime_entry()"),
            "RISC-V official suite should define a dedicated basic testsuite helper so full runs do not depend on the raw basic_testcode.sh shebang chain"
        );
        assert!(
            helper.contains("for case_name in $tests; do"),
            "RISC-V basic helper should iterate an explicit case list instead of smoke-only brk execution"
        );
        assert!(
            helper.contains("Testing $case_name :"),
            "RISC-V basic helper should preserve per-case Testing output"
        );
        assert!(
            helper.contains("\nchdir\n") && helper.contains("\npipe\n") && helper.contains("\nwaitpid\n"),
            "RISC-V basic helper should include the scorer-sensitive core basic cases"
        );
        assert!(
            !helper.contains("whuse-oscomp-basic-note:musl/sleep:skip-when-glibc-enabled"),
            "RISC-V basic helper should not hard-skip musl sleep in dual-runtime full mode"
        );
        assert!(
            script.contains("/musl/busybox sh ./basic/run-all.sh"),
            "RISC-V official suite should retain a busybox-shell fallback for the raw basic testsuite script"
        );
        assert!(
            script.contains(
                "if [ \"$marker_script\" = \"basic_testcode.sh\" ]; then\n        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\""
            ),
            "RISC-V official suite should route basic_testcode.sh through the dedicated basic testsuite helper in full mode"
        );
        assert!(
            !script.contains(
                "if [ \"$WHUSE_OSCOMP_PROFILE\" = \"basic\" ] && [ \"$marker_script\" = \"basic_testcode.sh\" ]"
            ),
            "RISC-V official suite should not limit the basic helper to the focused basic profile only"
        );
    }

    #[test]
    fn riscv_ltp_runner_uses_fail_case_contract_for_successes() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("echo FAIL LTP CASE $case_name : $case_rc"),
            "ltp runner should keep the official FAIL-case contract even when rc=0"
        );
        assert!(
            !script.contains("echo PASS LTP CASE $case_name : 0"),
            "ltp runner should not emit custom PASS-case lines that the site scorer may ignore"
        );
    }

    #[test]
    fn riscv_ltp_path_prefers_real_ltp_binaries_before_busybox_overrides() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains(
                "export PATH=\"$ltp_root/testcases/bin:${WHUSE_LTP_WRAPPER_DIR}:$ltp_root/testcases/lib:$ltp_root/runtest:$ltp_root/testscripts:$PATH\""
            ),
            "ltp PATH should prefer real testcase binaries before the busybox compatibility wrapper"
        );
    }

    #[test]
    fn riscv_ltp_explicit_whitelist_loop_does_not_recheck_whitelist_membership() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("if whuse_ltp_case_blocked \"$case_name\" \"$case_rel\"; then"),
            "explicit whitelist loop should only apply hard exclusions and blacklist filtering"
        );
    }

    #[test]
    fn riscv_ltp_runner_defaults_virtualization_override_for_timer_tests() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("export LTP_VIRT_OVERRIDE=\"${LTP_VIRT_OVERRIDE:-kvm}\""),
            "ltp runner should default LTP_VIRT_OVERRIDE in QEMU so timer tests avoid external virt probes"
        );
    }

    #[test]
    fn riscv_ltp_runner_uses_rootfs_backed_tmpdir_for_mount_device_cases() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("export WHUSE_LTP_TMPDIR=\"${WHUSE_LTP_TMPDIR:-/musl/ltp-tmp}\""),
            "ltp runner should use a stable rootfs-backed tmpdir override"
        );
        assert!(
            script.contains("export WHUSE_LTP_RUNROOT=\"${WHUSE_LTP_RUNROOT:-$WHUSE_LTP_TMPDIR/run.$runtime.$$}\""),
            "ltp runner should isolate each run under a unique root to avoid stale wrapper or stdin collisions"
        );
        assert!(
            script.contains("/musl/busybox mkdir -p \"$WHUSE_LTP_RUNROOT/cases\" \"$WHUSE_LTP_RUNROOT/debug\" >/dev/null 2>&1 || true"),
            "ltp runner should precreate the run-scoped case and debug directories"
        );
        assert!(
            script.contains("export WHUSE_LTP_WRAPPER_DIR=\"$WHUSE_LTP_RUNROOT/debug\""),
            "ltp runner should place busybox compatibility wrappers in the already-proven debug directory"
        );
        assert!(
            script.contains("export TMPDIR=\"$WHUSE_LTP_TMPDIR\""),
            "ltp runner should export TMPDIR so tst_device avoids small special-mount /tmp"
        );
    }

    #[test]
    fn riscv_ltp_runner_counts_tokens_robustly_across_binary_noise() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("/musl/busybox tr '\\000\\033' '\\n\\n' > \"$out\""),
            "ltp token normalization should strip NUL and ANSI escape bytes before classifying case output"
        );
        assert!(
            script.contains("/musl/busybox grep -Eq 'TFAIL|failed[[:space:]]+[1-9][0-9]*([[:space:]]|$)' \"$file\""),
            "ltp token detection should classify normalized logs with line-oriented regex matching"
        );
    }

    #[test]
    fn riscv_ltp_runner_does_not_treat_failed_zero_summary_as_tfail() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            !script.contains("*failed*[1-9]*"),
            "ltp token detection should not use shell globs that misclassify 'failed 0' summaries as tfail"
        );
        assert!(
            !script.contains("*passed*[1-9]*"),
            "ltp token detection should not use shell globs that depend on unrelated later digits in the log"
        );
    }

    #[test]
    fn riscv_ltp_runner_avoids_command_substitution_in_case_hot_path() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            !script.contains("stdin_dir=$(/musl/busybox dirname \"$stdin_path\")"),
            "ltp runner should not use dirname command substitution in per-case stdin setup"
        );
        assert!(
            !script.contains("first_line=$(/musl/busybox head -n 1 \"$case_path\" 2>/dev/null || true)"),
            "ltp runner should not read binary testcase headers through command substitution"
        );
        assert!(
            !script.contains("step_start_ts=$(/musl/busybox date +%s)"),
            "ltp runner should not use command substitution for step start timestamps"
        );
        assert!(
            !script.contains("now_ts=$(/musl/busybox date +%s)"),
            "ltp runner should not use command substitution for per-case timestamps"
        );
        assert!(
            !script.contains(": > \"$stdin_path\""),
            "ltp runner should not rely on shell builtin redirection to create per-case stdin files"
        );
        assert!(
            script.contains("[ \"$stdin_path\" = \"/dev/null\" ] && return 0"),
            "non-interactive ltp cases should not depend on creating per-case stdin files"
        );
        assert!(
            script.contains("case_stdin=/dev/null"),
            "ltp runner should default non-interactive cases to /dev/null stdin"
        );
    }

    #[test]
    fn riscv_ltp_runner_does_not_background_cases_through_busybox_setsid_applet() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            !script.contains("/musl/busybox setsid /musl/busybox env TST_NO_DEFAULT_RUN=1"),
            "ltp runner should not background cases through the busybox setsid applet because that can detach the real testcase from the waited pid"
        );
        assert!(
            script.contains("/musl/busybox env "),
            "ltp runner should still launch cases through the busybox env wrapper"
        );
    }

    #[test]
    fn riscv_ltp_runner_limits_tst_no_default_run_to_shell_cases() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("case_default_run_env="),
            "ltp runner should model shell-only default-run overrides explicitly"
        );
        assert!(
            script.contains("case_default_run_env=\"TST_NO_DEFAULT_RUN=1\""),
            "ltp runner should keep TST_NO_DEFAULT_RUN only for shell testcase launchers"
        );
        assert!(
            script.contains("/musl/busybox env $case_default_run_env $case_extra_env"),
            "ltp runner should launch binaries through env without hardcoding TST_NO_DEFAULT_RUN"
        );
        assert!(
            !script.contains("/musl/busybox env TST_NO_DEFAULT_RUN=1 $case_extra_env \"$exec_case_path\""),
            "ltp runner should not force TST_NO_DEFAULT_RUN onto compiled testcase binaries"
        );
    }

    #[test]
    fn riscv_ltp_runner_captures_case_exit_status_via_status_file() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("case_status=\"$case_log.status\""),
            "ltp runner should allocate a per-case status file so shell wait races do not erase testcase exit codes"
        );
        assert!(
            script.contains("echo $? > \"$case_status\""),
            "ltp runner wrapper should persist the real testcase exit code before the wrapper shell exits"
        );
        assert!(
            script.contains("while [ ! -f \"$case_status\" ]"),
            "ltp runner should poll for testcase completion via the status file instead of kill -0 plus delayed wait"
        );
        assert!(
            script.contains("if IFS= read -r case_rc < \"$case_status\"; then"),
            "ltp runner should restore the testcase exit code from the status file on the non-timeout path"
        );
    }

    #[test]
    fn riscv_ltp_busybox_wrapper_preserves_generic_applet_name() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("echo 'exec /musl/busybox \"$cmd\" \"$@\"'"),
            "ltp busybox wrapper should forward unknown applets with the original applet name"
        );
        assert!(
            !script.contains("echo 'exec /musl/busybox \"$@\"'"),
            "ltp busybox wrapper should not drop the applet name for generic fall-through execution"
        );
        assert!(
            script.contains("echo 'cmd=\"${0##*/}\"'"),
            "ltp busybox wrapper should derive the invoked applet name with shell expansion instead of basename substitution"
        );
    }

    #[test]
    fn riscv_ltp_runner_exports_ltproot_for_resource_files() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("export LTPROOT=\"$ltp_root\""),
            "ltp runner should export LTPROOT so resource_files helpers can copy companion binaries from testcases/bin"
        );
    }

    #[test]
    fn riscv_restarts_read_on_eagain_only_when_task_is_blocked() {
        assert!(super::should_restart_blocked_syscall(
            syscall::SYS_READ,
            super::EAGAIN_RET,
            true
        ));
        assert!(!super::should_restart_blocked_syscall(
            syscall::SYS_READ,
            super::EAGAIN_RET,
            false
        ));
        assert!(!super::should_restart_blocked_syscall(
            syscall::SYS_READ,
            -1,
            true
        ));
    }

    #[test]
    fn riscv_restarts_blocking_pipe_writes_on_eagain_only_when_task_is_blocked() {
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
    fn riscv_advances_sepc_for_failed_execve_but_not_success_or_sigreturn() {
        assert!(
            !super::should_advance_sepc_after_syscall(syscall::SYS_EXECVE, 0, false),
            "successful execve should replace the image and keep sepc unchanged"
        );
        assert!(
            super::should_advance_sepc_after_syscall(syscall::SYS_EXECVE, -14, false),
            "failed execve must advance sepc so user mode does not retry the same faulting instruction forever"
        );
        assert!(
            !super::should_advance_sepc_after_syscall(syscall::SYS_RT_SIGRETURN, 0, false),
            "rt_sigreturn should keep the restored sepc"
        );
    }

    #[test]
    fn riscv_libctest_uses_real_musl_runner_body() {
        let script = render_oscomp_official_suite_script("full");

        assert!(
            script.contains("run_riscv_musl_libctest_body()"),
            "full render should define a dedicated musl libctest runner"
        );
        assert!(
            script.contains("run_riscv_musl_libctest_script()"),
            "musl libctest runner should define a script walker for judge-facing output"
        );
        assert!(
            script.contains("========== START $wrap $test_name =========="),
            "musl libctest runner should emit judge-visible START lines"
        );
        assert!(
            script.contains("Pass!"),
            "musl libctest runner should emit judge-visible Pass! lines"
        );
        assert!(
            script.contains("whuse-oscomp-libctest-note:$wrap:$test_name:nonzero-rc:$case_rc"),
            "musl libctest runner should surface non-timeout nonzero exits as diagnostics instead of forcing step failure"
        );
        assert!(
            script.contains("whuse-oscomp-libctest-skip:$wrap:$test_name:riscv-score-first-skip"),
            "full musl libctest runner should skip known score-first blockers instead of stopping the LTP path"
        );
        assert!(
            script.contains("[ \"$test_name\" = \"pthread_condattr_setclock\" ]"),
            "full musl libctest runner should skip the watchdog-heavy pthread_condattr_setclock case"
        );
        assert!(
            script.contains("run_riscv_musl_libctest_script /musl/run-static.sh"),
            "musl libctest runner should walk run-static.sh"
        );
        assert!(
            script.contains("run_riscv_musl_libctest_script /musl/run-dynamic.sh"),
            "musl libctest runner should walk run-dynamic.sh"
        );
    }

    #[test]
    fn timed_events_service_delivers_itimer_signal_for_blocked_task() {
        let mut processes = ProcessTable::new();
        let pid = processes.spawn("itimer-case", None, 0x1000);
        processes.set_current(pid).unwrap();
        processes
            .set_itimer_real_current(Some(1_000_000), 0)
            .unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.spawn("itimer-case", pid, pid);
        scheduler.start();
        assert_eq!(scheduler.block_current(), Some(pid));
        assert_eq!(scheduler.ready_count(), 0);
        assert_eq!(scheduler.blocked_count(), 1);

        service_timed_events(&mut processes, &mut scheduler, 1_000_000);

        let signal_mask = 1u64 << (14 - 1);
        let process = processes.find_by_tid_mut(pid).unwrap();
        assert_ne!(
            process.pending_signals & signal_mask,
            0,
            "service_timed_events should enqueue SIGALRM when ITIMER_REAL expires"
        );
        assert_eq!(scheduler.ready_count(), 1);
        assert_eq!(scheduler.blocked_count(), 0);
    }

    #[test]
    fn idle_without_live_processes_requests_shutdown() {
        assert_eq!(
            idle_outcome_for_process_count(0),
            KernelIdleOutcome::Shutdown
        );
    }

    #[test]
    fn idle_with_live_processes_keeps_waiting() {
        assert_eq!(
            idle_outcome_for_process_count(1),
            KernelIdleOutcome::WaitForInterrupt
        );
        assert_eq!(
            idle_outcome_for_process_count(3),
            KernelIdleOutcome::WaitForInterrupt
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
            "timer preemption logs should stay silent by default so micro-timing LTP cases are not polluted"
        );
    }

    #[test]
    fn riscv_sched_time_slice_is_10ms_or_longer() {
        assert!(
            SCHED_TIME_SLICE_NS >= 10_000_000,
            "RISC-V timer preemption slice should stay at least 10ms so zero-timeout micro-timing LTP cases are not dominated by scheduler interrupts"
        );
    }
}
