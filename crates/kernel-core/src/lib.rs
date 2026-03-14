#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
#[cfg(target_arch = "loongarch64")]
use alloc::vec;
use core::fmt::{self, Write};
use fs_ext4::Ext4Mount;
#[cfg(target_arch = "loongarch64")]
use fs_ext4::Ext4NodeKind;
use hal_api::{hal, ConsoleWriter, PlatformArch};
use mm::MemoryManager;
#[cfg(target_arch = "loongarch64")]
use mm::{BinaryLoader, ElfBinaryLoader};
use proc::ProcessTable;
#[cfg(target_arch = "loongarch64")]
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
}

const USER_INIT_BASE: usize = 0x0040_0000;
const EAGAIN_RET: isize = -11;

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
                    #[cfg(target_arch = "loongarch64")]
                    preload_loong_basic_files(&mut vfs, device);
                    if vfs.access("/", "/musl/busybox").is_ok() {
                        for (path, target) in [
                            ("/bin/busybox", "/musl/busybox"),
                            ("/bin/sh", "/musl/busybox"),
                        ] {
                            let _ = vfs.unlink("/", path);
                            match vfs.create_symlink("/", path, target) {
                                Ok(()) => logln(format_args!(
                                    "whuse: installed fallback symlink {} -> {}",
                                    path, target
                                )),
                                Err(17) => {}
                                Err(err) => logln(format_args!(
                                    "whuse: failed fallback symlink {} -> {} err={}",
                                    path, target, err
                                )),
                            }
                        }
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
            #[cfg(target_arch = "loongarch64")]
            let mut loaded_from_rootfs = false;
            #[cfg(not(target_arch = "loongarch64"))]
            let loaded_from_rootfs = false;
            #[cfg(target_arch = "loongarch64")]
            {
                if vfs.access("/", "/musl/busybox").is_ok() {
                    match vfs.read_file_all("/", "/musl/busybox") {
                        Ok(image) => {
                            cache_busybox_image(&image);
                            let args = vec![
                                String::from("/musl/busybox"),
                                String::from("sh"),
                                String::from("-c"),
                                String::from(
                                    "echo whuse-oscomp-shell-entered; cd /musl/basic; for i in brk chdir clone close dup2 dup execve exit fork fstat getcwd getdents getpid getppid gettimeofday mkdir_ mmap mount munmap openat open pipe read sleep times umount uname unlink wait waitpid write yield; do echo \"Testing $i :\"; /musl/basic/$i; done; echo whuse-after-basic",
                                ),
                            ];
                            let envs = vec![
                                String::from("PATH=/musl:/bin:/sbin:/usr/bin:/usr/sbin"),
                                String::from("TERM=vt100"),
                            ];
                            let loader = ElfBinaryLoader::new();
                            let process = processes.current_mut().expect("init process must exist");
                            match loader.load(&process.address_space, &image, &args, &envs) {
                                Ok(loaded) => {
                                    process.trap_frame.sepc = loaded.entry;
                                    process.trap_frame.regs[2] = loaded.stack_pointer;
                                    loaded_from_rootfs = true;
                                    logln(format_args!(
                                        "whuse: loong init switched to /musl/busybox entry={:#x} sp={:#x}",
                                        loaded.entry, loaded.stack_pointer
                                    ));
                                }
                                Err(err) => logln(format_args!(
                                    "whuse: loong init load /musl/busybox failed err={}",
                                    err
                                )),
                            }
                        }
                        Err(err) => logln(format_args!(
                            "whuse: loong init read /musl/busybox failed err={}",
                            err
                        )),
                    }
                }
            }
            if !loaded_from_rootfs {
                #[cfg(target_arch = "loongarch64")]
                logln(format_args!(
                    "whuse: loong builtin init image_len={} entry_off={:#x}",
                    program.image.len(),
                    program.entry
                ));
                let process = processes.current_mut().expect("init process must exist");
                let _ = process
                    .address_space
                    .map_fixed_bytes(USER_INIT_BASE, program.image, program.image.len(), 0b101);
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
    fn run_current_process(&mut self) {
        {
            let process = match self.processes.current() {
                Ok(process) => process,
                Err(_) => return,
            };
            hal()
                .cpu
                .switch_address_space(process.address_space.token());
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

        if is_external_interrupt {
            self.service_irqs();
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

        let (name, stval) = self
            .processes
            .current()
            .map(|process| (process.name.as_str(), process.trap_frame.stval))
            .unwrap_or(("?", 0));
        logln(format_args!(
            "whuse: pid {} ({}) trapped with scause={} stval={:#x}",
            pid, name, scause, stval,
        ));
        if let Ok(exit) = self.processes.exit_current_thread(-1) {
            self.scheduler.remove_task(exit.tid);
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
            logln(format_args!("whuse: init stdio open stdin failed err={}", err));
            return;
        }
    };
    let stdout = match vfs.open("/", "/dev/console", O_RDWR, 0) {
        Ok(handle) => handle,
        Err(err) => {
            logln(format_args!("whuse: init stdio open stdout failed err={}", err));
            return;
        }
    };
    let stderr = match vfs.open("/", "/dev/console", O_RDWR, 0) {
        Ok(handle) => handle,
        Err(err) => {
            logln(format_args!("whuse: init stdio open stderr failed err={}", err));
            return;
        }
    };
    if let Ok(process) = processes.current_mut() {
        process.fds.insert(0, stdin);
        process.fds.insert(1, stdout);
        process.fds.insert(2, stderr);
    }
}

#[cfg(target_arch = "loongarch64")]
fn preload_loong_basic_files(vfs: &mut KernelVfs, device: &'static dyn hal_api::HalBlockDevice) {
    let Ok(mount) = Ext4Mount::probe(device) else {
        logln(format_args!(
            "whuse: loong preload skipped (ext4 probe failed)"
        ));
        return;
    };
    let mut preloaded = 0usize;

    for path in ["/musl/basic_testcode.sh", "/musl/basic/run-all.sh"] {
        if let Ok(bytes) = mount.read(path) {
            let _ = vfs.preload_external_file(path, &bytes, Some(0o100755));
            preloaded += 1;
        }
    }

    match mount.read_dir("/musl/basic") {
        Ok(entries) => {
            for entry in entries {
                if entry.kind != Ext4NodeKind::Regular {
                    continue;
                }
                let path = format!("/musl/basic/{}", entry.name);
                if let Ok(bytes) = mount.read(&path) {
                    let _ = vfs.preload_external_file(&path, &bytes, Some(entry.stat.mode));
                    preloaded += 1;
                }
            }
        }
        Err(err) => logln(format_args!(
            "whuse: loong preload read_dir /musl/basic failed err={}",
            err
        )),
    }

    logln(format_args!(
        "whuse: loong preloaded musl files count={}",
        preloaded
    ));
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
