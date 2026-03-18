
extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
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
const OSCOMP_BUSYBOX_APPLET_TIMEOUT_NS: u64 = 600 * 1_000_000_000;
const OSCOMP_BUSYBOX_SUPERVISOR_TIMEOUT_NS: u64 = 300 * 1_000_000_000;
const OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID: usize = 128;
const OSCOMP_LIBCTEST_ENTRY_TIMEOUT_NS: u64 = 10 * 1_000_000_000;
const OSCOMP_LMBENCH_TIMEOUT_NS: u64 = 300 * 1_000_000_000;
const OSCOMP_UNIXBENCH_TIMEOUT_NS: u64 = 300 * 1_000_000_000;
const OSCOMP_IOZONE_BUSYBOX_WINDOW_NS: u64 = 0;
const OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS: u64 = OSCOMP_GROUP_TIMEOUT_NS;
const OSCOMP_REQUIRED_TEST_FILES: [&str; 12] = [
    "/musl/busybox",
    "/musl/basic/run-all.sh",
    "/musl/busybox_testcode.sh",
    "/musl/iozone_testcode.sh",
    "/musl/libctest_testcode.sh",
    "/musl/libc-bench",
    "/musl/lmbench_testcode.sh",
    "/musl/lua_testcode.sh",
    "/musl/unixbench_testcode.sh",
    "/musl/netperf_testcode.sh",
    "/musl/iperf_testcode.sh",
    "/musl/cyclictest_testcode.sh",
];
const OSCOMP_OPTIONAL_TEST_FILES: [&str; 1] = ["/musl/time-test"];
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
    "WHUSE_HAVE_TIMEOUT=0\n",
    "if /musl/busybox timeout 1 /musl/busybox true >/tmp/whuse-timeout-probe.log 2>&1; then\n",
    "    WHUSE_HAVE_TIMEOUT=1\n",
    "fi\n",
    "echo whuse-oscomp-timeout-applet:$WHUSE_HAVE_TIMEOUT\n",
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
    "        if [ \"$rc\" -eq 124 ] || [ \"$rc\" -eq 137 ] || [ \"$rc\" -eq 143 ]; then\n",
    "            WHUSE_LAST_TIMEOUT_HIT=1\n",
    "            return 124\n",
    "        fi\n",
    "        return \"$rc\"\n",
    "    fi\n",
    "    \"$@\"\n",
    "    return $?\n",
    "}\n",
    "run_step_with_timeout() {\n",
    "    step=\"$1\"\n",
    "    timeout_s=\"$2\"\n",
    "    shift 2\n",
    "    echo whuse-oscomp-step-begin:$step\n",
    "    run_with_timeout \"$timeout_s\" \"$@\"\n",
    "    rc=$?\n",
    "    if [ \"$WHUSE_LAST_TIMEOUT_HIT\" -eq 1 ]; then\n",
    "        echo whuse-oscomp-step-timeout:$step:$timeout_s:pid=0:tgid=0\n",
    "    fi\n",
    "    echo whuse-oscomp-step-end:$step:$rc\n",
    "    return \"$rc\"\n",
    "}\n",
    "echo whuse-oscomp-script-start\n",
    "echo \"run time-test\"\n",
    "echo whuse-oscomp-step-begin:time-test\n",
    "if [ -x ./time-test ]; then\n",
    "    ./time-test\n",
    "    rc=$?\n",
    "else\n",
    "    echo whuse-oscomp-step-skip:time-test:missing\n",
    "    rc=0\n",
    "fi\n",
    "echo whuse-oscomp-step-end:time-test:$rc\n",
    "echo \"run busybox_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    run_step_with_timeout busybox_testcode.sh 600 /musl/busybox sh /tmp/whuse-busybox-testcode.sh\n",
    "else\n",
    "    run_step_with_timeout busybox_testcode.sh 180 /musl/busybox sh ./busybox_testcode.sh\n",
    "fi\n",
    "echo \"run iozone_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:iozone_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:iozone_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout iozone_testcode.sh 300 /musl/busybox sh ./iozone_testcode.sh\n",
    "fi\n",
    "echo \"run libctest_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:libctest_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:libctest_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:libctest_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout libctest_testcode.sh 300 /musl/busybox sh ./libctest_testcode.sh\n",
    "fi\n",
    "echo \"run libc-bench\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:libc-bench\n",
    "    echo whuse-oscomp-step-skip:libc-bench:compat-hang\n",
    "    echo whuse-oscomp-step-end:libc-bench:124\n",
    "else\n",
    "    run_step_with_timeout libc-bench 300 ./libc-bench\n",
    "fi\n",
    "echo \"run lmbench_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:lmbench_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:lmbench_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:lmbench_testcode.sh:124\n",
    "else\n",
    "    echo whuse-oscomp-lmbench-marker:runner-start\n",
    "    run_step_with_timeout lmbench_testcode.sh 300 /musl/busybox sh ./lmbench_testcode.sh\n",
    "    lmbench_rc=$?\n",
    "    echo whuse-oscomp-lmbench-marker:runner-end:$lmbench_rc\n",
    "fi\n",
    "echo \"run lua_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:lua_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:lua_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:lua_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout lua_testcode.sh 300 /musl/busybox sh ./lua_testcode.sh\n",
    "fi\n",
    "echo \"run unixbench_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:unixbench_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:unixbench_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:unixbench_testcode.sh:124\n",
    "else\n",
    "    echo whuse-oscomp-unixbench-marker:runner-start\n",
    "    run_step_with_timeout unixbench_testcode.sh 300 /musl/busybox sh ./unixbench_testcode.sh\n",
    "    unixbench_rc=$?\n",
    "    echo whuse-oscomp-unixbench-marker:runner-end:$unixbench_rc\n",
    "fi\n",
    "echo \"run netperf_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:netperf_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:netperf_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:netperf_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout netperf_testcode.sh 240 /musl/busybox sh ./netperf_testcode.sh\n",
    "fi\n",
    "echo \"run iperf_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:iperf_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:iperf_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:iperf_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout iperf_testcode.sh 240 /musl/busybox sh ./iperf_testcode.sh\n",
    "fi\n",
    "echo \"run cyclic_testcode.sh\"\n",
    "if [ \"$WHUSE_OSCOMP_COMPAT\" = \"1\" ]; then\n",
    "    echo whuse-oscomp-step-begin:cyclic_testcode.sh\n",
    "    echo whuse-oscomp-step-skip:cyclic_testcode.sh:compat-hang\n",
    "    echo whuse-oscomp-step-end:cyclic_testcode.sh:124\n",
    "else\n",
    "    run_step_with_timeout cyclic_testcode.sh 240 /musl/busybox sh -c 'if [ -x ./cyclic_testcode.sh ]; then exec ./cyclic_testcode.sh; else exec ./cyclictest_testcode.sh; fi'\n",
    "fi\n",
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
                let _ = core::fmt::Write::write_fmt(&mut sample, format_args!("{}:{}", tgid, name));
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
            let mut robust_futex_addrs = Vec::new();
            for exit in exits {
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
                let _ = self.scheduler.wake_task(parent_tgid);
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
    for dir in ["/var", "/var/tmp", "/usr", "/usr/bin", "/lib"] {
        let _ = vfs.mkdir("/", dir, 0o755);
    }
    install_busybox_exec_alias(vfs, "/musl/ls", "ls");
    install_busybox_exec_alias(vfs, "/musl/which", "which");
    install_busybox_exec_alias(vfs, "/musl/sleep", "sleep");
    for (path, target) in [
        ("/bin/busybox", "/musl/busybox"),
        ("/bin/sh", "/musl/busybox"),
        ("/bin/bash", "/musl/busybox"),
        ("/bin/ls", "/musl/ls"),
        ("/bin/which", "/musl/which"),
        ("/bin/sleep", "/musl/sleep"),
        ("/busybox", "/musl/busybox"),
        ("/usr/bin/ls", "/musl/ls"),
        ("/usr/bin/which", "/musl/which"),
        ("/usr/bin/sleep", "/musl/sleep"),
        ("/usr/bin/env", "/musl/busybox"),
        ("/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"),
        ("/lib/ld-musl-loongarch64.so.1", "/musl/lib/libc.so"),
    ] {
        install_fallback_symlink(vfs, path, target);
    }
    install_oscomp_root_aliases(vfs);
    match vfs.create_file(
        "/",
        OSCOMP_SUITE_SCRIPT_PATH,
        OSCOMP_SUITE_SCRIPT.as_bytes(),
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

fn oscomp_process_timeout_ns(
    tgid: usize,
    name: &str,
    in_iozone_busybox_window: bool,
    has_child_groups: bool,
    in_bench_phase: bool,
) -> u64 {
    if is_libctest_entry_or_runner(name) {
        return OSCOMP_LIBCTEST_ENTRY_TIMEOUT_NS;
    }
    if tgid < OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID && name == "/musl/busybox" {
        return u64::MAX;
    }
    if name.contains("lmbench") {
        return OSCOMP_LMBENCH_TIMEOUT_NS;
    }
    if name.contains("unixbench") {
        return OSCOMP_UNIXBENCH_TIMEOUT_NS;
    }
    if name.contains("busybox") {
        if tgid < OSCOMP_BUSYBOX_SHORT_TIMEOUT_MIN_TGID {
            return OSCOMP_GROUP_TIMEOUT_NS;
        }
        if is_busybox_supervisor(name, has_child_groups, in_bench_phase) {
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
