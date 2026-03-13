#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::fmt::{self, Write};
use hal_api::{hal, ConsoleWriter};
use mm::MemoryManager;
use proc::ProcessTable;
use syscall::SyscallDispatcher;
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

        let mut vfs = KernelVfs::new();
        let _ = user_init::seed_filesystem(&mut vfs);

        let mut processes = ProcessTable::new();
        let init_pid = processes.spawn_init("init");
        processes.set_current(init_pid).expect("init pid must exist");
        user_init::seed_process(processes.current_mut().expect("init process must exist"));

        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init_pid);
        scheduler.start();

        let memory = MemoryManager::from_hal(hal().memory);
        logln(format_args!("whuse: memory initialized"));
        logln(format_args!("whuse: rootfs mounted with devfs/procfs-lite"));

        Self {
            info,
            memory,
            processes,
            scheduler,
            vfs,
            syscalls: SyscallDispatcher::new(),
        }
    }

    pub fn run_forever(&mut self) -> ! {
        logln(format_args!("whuse: entering scheduler loop"));
        loop {
            hal().cpu.wait_for_interrupt();
        }
    }
}

pub fn boot_forever(info: BootInfo) -> ! {
    let mut kernel = Kernel::bootstrap(info);
    kernel.run_forever();
}

pub fn logln(args: fmt::Arguments<'_>) {
    let mut writer = ConsoleWriter;
    let _ = writer.write_fmt(args);
    let _ = writer.write_str("\n");
}
