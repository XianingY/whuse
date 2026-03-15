#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use fs_ext4::Ext4Mount;
use hal_api::{hal, ConsoleWriter, PlatformArch};
use mm::MemoryManager;
use mm::{BinaryLoader, ElfBinaryLoader};
use proc::ProcessTable;
use syscall::cache_busybox_image;
use syscall::{SyscallArgs, SyscallDispatcher, SYS_EXECVE, SYS_WAIT};
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
const OSCOMP_GROUP_TIMEOUT_NS: u64 = 120 * 1_000_000_000;
const OSCOMP_HEAVY_TIMEOUT_NS: u64 = 2 * 1_000_000_000;
const OSCOMP_IOZONE_BUSYBOX_WINDOW_NS: u64 = 90 * 1_000_000_000;
const OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS: u64 = 8 * 1_000_000_000;
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
const OSCOMP_SUITE_SCRIPT: &str = concat!(
    "set +e\n",
    "./busybox echo whuse-oscomp-script-start\n",
    "./busybox echo \"run time-test\"\n",
    "echo whuse-oscomp-step-begin:time-test\n",
    "./time-test\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:time-test:$rc\n",
    "./busybox echo \"run busybox_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
    "./busybox_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:busybox_testcode.sh:$rc\n",
    "./busybox echo \"run iozone_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:iozone_testcode.sh\n",
    "./iozone_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:iozone_testcode.sh:$rc\n",
    "./busybox echo \"run libctest_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:libctest_testcode.sh\n",
    "./libctest_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:libctest_testcode.sh:$rc\n",
    "./busybox echo \"run libc-bench\"\n",
    "echo whuse-oscomp-step-begin:libc-bench\n",
    "./libc-bench\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:libc-bench:$rc\n",
    "./busybox echo \"run lmbench_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:lmbench_testcode.sh\n",
    "./lmbench_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:lmbench_testcode.sh:$rc\n",
    "./busybox echo \"run lua_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:lua_testcode.sh\n",
    "./lua_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:lua_testcode.sh:$rc\n",
    "./busybox echo \"run unixbench_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:unixbench_testcode.sh\n",
    "./unixbench_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:unixbench_testcode.sh:$rc\n",
    "./busybox echo \"run netperf_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:netperf_testcode.sh\n",
    "./netperf_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:netperf_testcode.sh:$rc\n",
    "./busybox echo \"run iperf_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:iperf_testcode.sh\n",
    "./iperf_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:iperf_testcode.sh:$rc\n",
    "./busybox echo \"run cyclictest_testcode.sh\"\n",
    "echo whuse-oscomp-step-begin:cyclictest_testcode.sh\n",
    "./cyclictest_testcode.sh\n",
    "rc=$?\n",
    "echo whuse-oscomp-step-end:cyclictest_testcode.sh:$rc\n",
    "echo whuse-oscomp-suite-done\n",
);
const OSCOMP_SUITE_CMD: &str = concat!(
    "echo whuse-oscomp-shell-entered; ",
    "/musl/busybox sh /tmp/whuse-oscomp-suite.sh; ",
    "if [ -x /musl/basic/exit ]; then exec /musl/basic/exit; fi; ",
    "echo whuse-oscomp-exit-missing; exit 0;",
);

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
        kernel
    }

    pub fn run_forever(&mut self) -> ! {
        logln(format_args!("whuse: entering scheduler loop"));
        loop {
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
                    // Keep watchdog progress alive even when runnable queue is empty.
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
            let reset_started_at = self
                .watchdog_seen_name
                .get(tgid)
                .map(|previous| previous != name)
                .unwrap_or(true);
            if reset_started_at {
                self.watchdog_started_at.insert(*tgid, now);
                self.watchdog_seen_name.insert(*tgid, name.clone());
            } else {
                self.watchdog_started_at.entry(*tgid).or_insert(now);
            }
        }
        if watched.values().any(|name| is_oscomp_heavy_process(name)) {
            self.watchdog_iozone_window_until_ns =
                now.saturating_add(OSCOMP_IOZONE_BUSYBOX_WINDOW_NS);
        }
        let in_iozone_busybox_window = now <= self.watchdog_iozone_window_until_ns;
        if !all_groups.is_empty()
            && (self.watchdog_last_heartbeat_ns == 0
                || now.saturating_sub(self.watchdog_last_heartbeat_ns) >= 5_000_000_000)
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
        let timed_out = watched
            .iter()
            .filter_map(|(tgid, name)| {
                let started = *self.watchdog_started_at.get(tgid)?;
                let timeout_ns = oscomp_process_timeout_ns(name, in_iozone_busybox_window);
                (now.saturating_sub(started) >= timeout_ns).then_some((
                    *tgid,
                    name.clone(),
                    timeout_ns,
                ))
            })
            .collect::<Vec<_>>();
        let mut killed = false;
        for (tgid, name, timeout_ns) in timed_out {
            self.watchdog_started_at.remove(&tgid);
            self.watchdog_seen_name.remove(&tgid);
            let Ok(Some(exit)) = self.processes.force_exit_group(tgid, 124) else {
                continue;
            };
            let _ = self.scheduler.exit_group(exit.tgid);
            for tid in &exit.tids {
                let _ = self.scheduler.remove_task(*tid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let woke = self.scheduler.wake_task(parent_tgid);
                logln(format_args!(
                    "whuse: oscomp watchdog wake parent_tgid={} woke={}",
                    parent_tgid, woke
                ));
            }
            for process in self.processes.process_snapshots() {
                if process.is_thread || process.state != proc::ProcessState::Blocked {
                    continue;
                }
                let _ = self.scheduler.wake_task(process.tid);
            }
            for addr in exit.clear_child_tids {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
            logln(format_args!(
                "whuse: oscomp watchdog timeout tgid={} name={} after {}s",
                tgid,
                name,
                timeout_ns / 1_000_000_000,
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
            if self.timer_irq_count <= 5 || self.timer_irq_count % 1024 == 0 {
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
                let blocked_wait = sysno == SYS_WAIT && result == EAGAIN_RET;
                if !blocked_wait {
                    process.trap_frame.set_retval(result as usize);
                    if sysno != SYS_EXECVE || (result as i32) < 0 {
                        process.trap_frame.sepc = sepc + 4;
                    }
                }
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
        if let Ok(exit) = self.processes.exit_current_thread(-1) {
            self.scheduler.remove_task(exit.tid);
            if exit.group_exited {
                self.scheduler.exit_group(exit.tgid);
            }
            if let Some(parent_tgid) = exit.parent_tgid {
                let _ = self.scheduler.wake_task(parent_tgid);
            }
            if let Some(addr) = exit.clear_child_tid {
                for tid in self.processes.wake_futex(addr, usize::MAX) {
                    let _ = self.scheduler.wake_task(tid);
                }
            }
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
    for (path, target) in [
        ("/bin/busybox", "/musl/busybox"),
        ("/bin/sh", "/musl/busybox"),
        ("/bin/bash", "/musl/busybox"),
        ("/busybox", "/musl/busybox"),
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

fn oscomp_process_timeout_ns(name: &str, in_iozone_busybox_window: bool) -> u64 {
    if is_oscomp_heavy_process(name) {
        return OSCOMP_HEAVY_TIMEOUT_NS;
    }
    if in_iozone_busybox_window && name.contains("busybox") {
        return OSCOMP_IOZONE_BUSYBOX_TIMEOUT_NS;
    }
    OSCOMP_GROUP_TIMEOUT_NS
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
