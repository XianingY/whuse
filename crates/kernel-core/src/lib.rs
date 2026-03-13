#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::fmt::{self, Write};
use hal_api::{hal, ConsoleWriter, PlatformArch};
use mm::MemoryManager;
use proc::ProcessTable;
use syscall::{
    SyscallArgs, SyscallDispatcher, SYS_EXECVE,
};
use task::Scheduler;
use vfs::KernelVfs;

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
}

impl Kernel {
    pub fn bootstrap(info: BootInfo) -> Self {
        logln(format_args!("whuse: booting on {}", info.platform));
        logln(format_args!("whuse: hart={} dtb={:#x}", info.hart_id, info.dtb_pa));
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

        let mut processes = ProcessTable::new();
        let init_entry = user_init::builtin_program("/sbin/init")
            .or_else(|| user_init::builtin_program("/bin/init"))
            .map(|program| program.image.as_ptr() as usize + program.entry)
            .unwrap_or(0);
        let init_pid = processes.spawn_init("init", init_entry);
        processes.set_current(init_pid).expect("init pid must exist");
        if init_entry == 0 {
            user_init::seed_process(processes.current_mut().expect("init process must exist"));
        }

        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init_pid);
        scheduler.start();

        let memory = MemoryManager::from_hal(hal().memory);
        logln(format_args!("whuse: memory initialized"));
        logln(format_args!("whuse: rootfs mounted with devfs/procfs-lite"));

        let kernel = Self {
            info,
            memory,
            processes,
            scheduler,
            vfs,
            syscalls: SyscallDispatcher::new(),
        };
        logln(format_args!("whuse: init process bootstrapped"));
        kernel
    }

    pub fn run_forever(&mut self) -> ! {
        logln(format_args!("whuse: entering scheduler loop"));
        loop {
            if self.scheduler.ensure_current().is_none() {
                hal().cpu.wait_for_interrupt();
                continue;
            }
            let pid = match self.scheduler.current_process_id() {
                Some(pid) => pid,
                None => continue,
            };
            if self.processes.set_current(pid).is_err() {
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
    fn run_current_process(&mut self) {
        {
            let process = match self.processes.current() {
                Ok(process) => process,
                Err(_) => return,
            };
            hal().cpu.switch_address_space(process.address_space.token());
        }

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
                process.pid,
            ),
            Err(_) => return,
        };

        let is_syscall = match hal().platform.architecture() {
            PlatformArch::Riscv64 => scause == 8,
            PlatformArch::LoongArch64 => scause == 11,
        };

        if is_syscall {
                let result = self.syscalls.dispatch(
                    sysno,
                    SyscallArgs(args),
                    &mut self.processes,
                    &mut self.scheduler,
                    &mut self.vfs,
                );
                if let Ok(process) = self.processes.current_mut() {
                    process.trap_frame.set_retval(result as usize);
                    if sysno != SYS_EXECVE {
                        process.trap_frame.sepc = sepc + 4;
                    }
                }
                return;
        }

        logln(format_args!(
            "whuse: pid {} trapped with scause={} stval={:#x}",
            pid,
            scause,
            self.processes
                .current()
                .map(|process| process.trap_frame.stval)
                .unwrap_or(0),
        ));
        let _ = self.processes.exit_current(-1);
        self.scheduler.exit_current();
    }
}

pub fn logln(args: fmt::Arguments<'_>) {
    let mut writer = ConsoleWriter;
    let _ = writer.write_fmt(args);
    let _ = writer.write_str("\n");
}
