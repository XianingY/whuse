#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(target_arch = "loongarch64")]
use core::arch::global_asm;
use core::ptr::NonNull;
#[cfg(target_arch = "loongarch64")]
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalInterrupt, HalMemory,
    HalNetDevice, HalPlatform, HalPlatformLifecycle, HalTimer, MemoryRegion, PlatformArch,
    Timespec, TrapFrame, VmSpaceToken,
};
use hal_virtio::{
    parse_loongarch_virtio_discovery, virtio_error_to_errno, LoongArchInterruptConfig,
    LoongArchVirtioDiscovery, PciHostConfig, PciHostWindow, VirtioBlockConfig, VirtioDmaArena,
};
use spin::{Mutex, Once};
use virtio_drivers::device::blk::{VirtIOBlk, SECTOR_SIZE};
use virtio_drivers::transport::pci::bus::{
    BarInfo, Cam, Command, ConfigurationAccess, DeviceFunction, MemoryBarType, MmioCam, PciRoot,
};
use virtio_drivers::transport::pci::{virtio_device_type, PciTransport};
use virtio_drivers::transport::InterruptStatus;
use virtio_drivers::transport::SomeTransport;
use virtio_drivers::BufferDirection;
use virtio_drivers::Hal as VirtioHal;

#[cfg(target_arch = "loongarch64")]
#[no_mangle]
static mut __whuse_current_frame: usize = 0;

#[cfg(target_arch = "loongarch64")]
#[no_mangle]
static mut __whuse_kernel_ra: usize = 0;

/// Timer callback pointer for kernel-mode timer interrupts.
pub static KERNEL_TRAP_HANDLER: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

// LoongArch CSR numbers for timer handling
const CSR_CRMD: u32 = 0x0;    // Current mode
const CSR_PRMD: u32 = 0x1;    // Previous mode
const CSR_ECFG: u32 = 0x4;    // Exception config (interrupt enables)
const CSR_ESTAT: u32 = 0x5;   // Exception status
const CSR_ERA: u32 = 0x6;     // Exception return address
const CSR_EENTRY: u32 = 0xc;  // Exception entry
const CSR_TCFG: u32 = 0x41;   // Timer config
const CSR_TVAL: u32 = 0x42;   // Timer value (countdown)
const CSR_TICLR: u32 = 0x44;  // Timer interrupt clear
const CSR_SAVE0: u32 = 0x30;  // Scratch register 0
const CSR_SAVE1: u32 = 0x31;  // Scratch register 1

// Timer interrupt bit in ECFG/ESTAT (bit 11)
const ECFG_TI: usize = 1 << 11;

// LoongArch timer frequency (100 MHz on QEMU virt)
const LA_TIMER_FREQ_HZ: u64 = 100_000_000;

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text
    .globl __whuse_kernel_trap_entry
    .align 4
__whuse_kernel_trap_entry:
    // Save caller-saved registers we'll use
    addi.d $sp, $sp, -144
    st.d $ra,   $sp, 0
    st.d $t0,   $sp, 8
    st.d $t1,   $sp, 16
    st.d $t2,   $sp, 24
    st.d $a0,   $sp, 32
    st.d $a1,   $sp, 40
    st.d $a2,   $sp, 48
    st.d $a3,   $sp, 56
    st.d $a4,   $sp, 64
    st.d $a5,   $sp, 72
    st.d $a6,   $sp, 80
    st.d $a7,   $sp, 88

    // Save CRMD and set kernel-mode CRMD (DA=0, PG=1 for DMW)
    csrrd $t0, 0x0
    st.d $t0, $sp, 96
    // Set CRMD for kernel: PLV=0, IE=0, DA=0, PG=1, DATF=01, DATM=01
    // Value: 0x53 = DA=0, PG=1, DATF=01, DATM=01 (keeps DMW active)
    li.d $t0, 0x53
    csrwr $t0, 0x0

    // Read ESTAT (exception status) and ERA (return address) for handler
    csrrd $a0, 0x5
    csrrd $a1, 0x6
    bl __whuse_kernel_trap_handler

    // Restore CRMD
    ld.d $t0, $sp, 96
    csrwr $t0, 0x0

    // Restore registers
    ld.d $ra,   $sp, 0
    ld.d $t0,   $sp, 8
    ld.d $t1,   $sp, 16
    ld.d $t2,   $sp, 24
    ld.d $a0,   $sp, 32
    ld.d $a1,   $sp, 40
    ld.d $a2,   $sp, 48
    ld.d $a3,   $sp, 56
    ld.d $a4,   $sp, 64
    ld.d $a5,   $sp, 72
    ld.d $a6,   $sp, 80
    ld.d $a7,   $sp, 88
    addi.d $sp, $sp, 144
    ertn
"#
);

#[cfg(target_arch = "loongarch64")]
extern "C" {
    fn __whuse_kernel_trap_entry();
}

/// Kernel-mode trap handler for LoongArch.
/// Called from __whuse_kernel_trap_entry with:
///   a0 = ESTAT (exception status)
///   a1 = ERA (exception return address)
#[cfg(target_arch = "loongarch64")]
#[no_mangle]
unsafe extern "C" fn __whuse_kernel_trap_handler(estat: usize, _era: usize) {
    // Check if this is a timer interrupt (bit 11 of ESTAT)
    let is_timer = (estat & ECFG_TI) != 0;
    if is_timer {
        // Clear the timer interrupt by writing 1 to TICLR bit 0
        core::arch::asm!("li.d $t0, 1", "csrwr $t0, 0x44", out("$t0") _);

        let cb_ptr = KERNEL_TRAP_HANDLER.load(core::sync::atomic::Ordering::Relaxed);
        if cb_ptr != 0 {
            let cb: fn() = core::mem::transmute(cb_ptr);
            cb();
        }
        return;
    }

    // Non-timer trap in kernel mode is fatal
    let mut console = hal_api::ConsoleWriter;
    let _ = core::fmt::Write::write_fmt(
        &mut console,
        format_args!(
            "\nwhuse: FATAL KERNEL TRAP estat={:#x} era={:#x}\n",
            estat, _era
        ),
    );
    panic!("unhandled kernel trap");
}

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text
    .globl __whuse_run_user
__whuse_run_user:
    la.local $t0, __whuse_current_frame
    st.d $a0, $t0, 0
    la.local $t0, __whuse_kernel_ra
    st.d $ra, $t0, 0
    la.local $t0, __whuse_user_trap_entry
    csrwr $t0, 0xc

    move $t0, $sp
    csrwr $t0, 0x31

    ld.d $ra, $a0, 8
    ld.d $sp, $a0, 16
    ld.d $tp, $a0, 32
    ld.d $t0, $a0, 40
    ld.d $t1, $a0, 48
    ld.d $t2, $a0, 56
    ld.d $fp, $a0, 64
    ld.d $s0, $a0, 72
    ld.d $a1, $a0, 88
    ld.d $a2, $a0, 96
    ld.d $a3, $a0, 104
    ld.d $a4, $a0, 112
    ld.d $a5, $a0, 120
    ld.d $a6, $a0, 128
    ld.d $a7, $a0, 136
    ld.d $s1, $a0, 144
    ld.d $s2, $a0, 152
    ld.d $s3, $a0, 160
    ld.d $s4, $a0, 168
    ld.d $s5, $a0, 176
    ld.d $s6, $a0, 184
    ld.d $s7, $a0, 192
    ld.d $s8, $a0, 200
    ld.d $r31, $a0, 208
    ld.d $r22, $a0, 216
    ld.d $r12, $a0, 224
    ld.d $r13, $a0, 232
    ld.d $r14, $a0, 240
    ld.d $r15, $a0, 248

    ld.d $t0, $a0, 256
    csrwr $t0, 0x6
    ld.d $t0, $a0, 264
    bnez $t0, 1f
    li.d $t0, 3
1:
    li.d $t1, -4
    and $t0, $t0, $t1
    ori $t0, $t0, 0x3
    csrwr $t0, 0x1
    ld.d $t0, $a0, 80
    csrwr $t0, 0x30
    csrwr $a0, 0x30
    ertn

    .align 4
    .globl __whuse_user_trap_entry
__whuse_user_trap_entry:
    csrwr $a0, 0x30

    st.d $ra, $a0, 8
    st.d $sp, $a0, 16
    st.d $tp, $a0, 32
    st.d $t0, $a0, 40
    st.d $t1, $a0, 48
    st.d $t2, $a0, 56
    st.d $fp, $a0, 64
    st.d $s0, $a0, 72
    st.d $a1, $a0, 88
    st.d $a2, $a0, 96
    st.d $a3, $a0, 104
    st.d $a4, $a0, 112
    st.d $a5, $a0, 120
    st.d $a6, $a0, 128
    st.d $a7, $a0, 136
    st.d $s1, $a0, 144
    st.d $s2, $a0, 152
    st.d $s3, $a0, 160
    st.d $s4, $a0, 168
    st.d $s5, $a0, 176
    st.d $s6, $a0, 184
    st.d $s7, $a0, 192
    st.d $s8, $a0, 200
    st.d $r31, $a0, 208
    st.d $r22, $a0, 216
    st.d $r12, $a0, 224
    st.d $r13, $a0, 232
    st.d $r14, $a0, 240
    st.d $r15, $a0, 248

    csrrd $t0, 0x30
    st.d $t0, $a0, 80
    csrrd $sp, 0x31
    csrrd $t0, 0x6
    st.d $t0, $a0, 256
    csrrd $t0, 0x1
    st.d $t0, $a0, 264
    csrrd $t0, 0x5
    srli.d $t1, $t0, 16
    andi $t1, $t1, 0x3f
    st.d $t1, $a0, 272
    csrrd $t0, 0x7
    st.d $t0, $a0, 280

    csrwr $a0, 0x30
    csrrd $t0, 0x1
    li.d $t1, -5
    and $t0, $t0, $t1
    csrwr $t0, 0x1
    la.local $t0, __whuse_kernel_ra
    ld.d $ra, $t0, 0
    jirl $zero, $ra, 0

    .balign 4096
    .globl __whuse_tlb_refill_entry
__whuse_tlb_refill_entry:
    csrwr   $t0, 0x8b
    csrrd   $t0, 0x1b

    lddir   $t0, $t0, 3
    beqz    $t0, 2f

    lddir   $t0, $t0, 2
    beqz    $t0, 2f

    lddir   $t0, $t0, 1
    beqz    $t0, 2f

    ldpte   $t0, 0
    ldpte   $t0, 1
    b       3f

2:
    csrrd   $t0, 0x8e
    ori     $t0, $t0, 0xC
    csrwr   $t0, 0x8e

    rotri.d $t0, $t0, 61
    ori     $t0, $t0, 3
    rotri.d $t0, $t0, 3

    csrwr   $t0, 0x8c
    csrrd   $t0, 0x8c
    csrwr   $t0, 0x8d

3:
    tlbfill
    csrrd   $t0, 0x8b
    ertn
"#
);

#[cfg(target_arch = "loongarch64")]
unsafe extern "C" {
    fn __whuse_run_user(frame: *mut TrapFrame);
    fn __whuse_tlb_refill_entry();
}

#[cfg(target_arch = "loongarch64")]
const LA_PWCL_VALUE: usize = 12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25);
#[cfg(target_arch = "loongarch64")]
const LA_PWCH_VALUE: usize = (39 | (9 << 6)) | (1 << 24);

pub const DMWIN_UNCACHED_BASE: usize = 0x8000_0000_0000_0000;
pub const DMWIN_CACHED_BASE: usize = 0x9000_0000_0000_0000;
pub const UART0_PHYS_BASE: usize = 0x1fe0_01e0;
pub const MMIO_PHYS_BASE: usize = 0x1000_0000;
pub const UART0_BASE: usize = UART0_PHYS_BASE;
pub const PHYS_MEM_BASE: usize = 0x9000_0000;
pub const PHYS_MEM_SIZE: usize = 512 * 1024 * 1024;
const DMA_ARENA_BYTES: usize = 2 * 1024 * 1024;
const DMA_ARENA_WORDS: usize = DMA_ARENA_BYTES / SECTOR_SIZE / 64;
const EIO: i32 = 5;
const ENODEV: i32 = 19;
const EINVAL: i32 = 22;
const EROFS: i32 = 30;

static CPU: VirtCpu = VirtCpu::new();
static INTERRUPT: VirtInterruptController = VirtInterruptController::new();
static PLATFORM: VirtPlatform = VirtPlatform;
static LIFECYCLE: VirtLifecycle = VirtLifecycle;
static MEMORY: VirtMemory = VirtMemory;
static TIMER: VirtTimer = VirtTimer::new();
static UART: Ns16550 = Ns16550::new(UART0_BASE);
static VIRTIO_BLK: VirtioBlockDevice = VirtioBlockDevice::new();
static VIRTIO_NET: VirtioNetStub = VirtioNetStub;
static BLOCK_DEVS: [&'static dyn HalBlockDevice; 1] = [&VIRTIO_BLK];
static NET_DEVS: [&'static dyn HalNetDevice; 1] = [&VIRTIO_NET];
static DMA_ARENA: VirtioDmaArena<DMA_ARENA_BYTES, DMA_ARENA_WORDS> = VirtioDmaArena::new();

static MEMORY_MAP: [MemoryRegion; 2] = [
    MemoryRegion {
        start: 0,
        size: MMIO_PHYS_BASE,
        usable: false,
    },
    MemoryRegion {
        start: PHYS_MEM_BASE,
        size: PHYS_MEM_SIZE,
        usable: true,
    },
];

#[cfg(target_arch = "loongarch64")]
fn init_loongarch_mmu() {
    unsafe {
        let tlbrentry = __whuse_tlb_refill_entry as *const () as usize;
        core::arch::asm!(
            "csrwr {pwcl}, 0x1c",
            "csrwr {pwch}, 0x1d",
            "csrwr {tlbrentry}, 0x88",
            "dbar 0",
            "invtlb 0x00, $r0, $r0",
            pwcl = in(reg) LA_PWCL_VALUE,
            pwch = in(reg) LA_PWCH_VALUE,
            tlbrentry = in(reg) tlbrentry,
        );
    }
}

#[cfg(not(target_arch = "loongarch64"))]
fn init_loongarch_mmu() {}

pub fn bootstrap(dtb_pa: usize) {
    extern "C" {
        fn end();
    }
    let kernel_end = ((end as *const () as usize) + 4095) & !4095;
    unsafe {
        let map_ptr = MEMORY_MAP.as_ptr() as *mut MemoryRegion;
        (*map_ptr.add(1)).start = kernel_end;
        (*map_ptr.add(1)).size = (PHYS_MEM_BASE + PHYS_MEM_SIZE).saturating_sub(kernel_end);
    }

    let discovery = if dtb_pa != 0 {
        parse_loongarch_virtio_discovery(dtb_pa)
    } else {
        Some(qemu_virt_fallback())
    };
    if let Some(discovery) = discovery {
        INTERRUPT.configure(&discovery);
        VIRTIO_BLK.bootstrap(&discovery);
    }
    init_loongarch_mmu();
    register_hal(HalBundle {
        platform: &PLATFORM,
        lifecycle: &LIFECYCLE,
        interrupt: &INTERRUPT,
        cpu: &CPU,
        memory: &MEMORY,
        timer: &TIMER,
        console: &UART,
        block_devices: &BLOCK_DEVS,
        net_devices: &NET_DEVS,
    });
}

struct VirtCpu {
    interrupts_enabled: AtomicBool,
}

struct VirtInterruptController {
    base: AtomicUsize,
}

struct VirtPlatform;
struct VirtLifecycle;
struct VirtMemory;

struct VirtioBlockState {
    driver: VirtIOBlk<LoongArchVirtioHal, SomeTransport<'static>>,
}

struct VirtioBlockDevice {
    state: Once<Mutex<VirtioBlockState>>,
    init_error: AtomicI32,
    capacity_sectors: AtomicUsize,
    readonly: AtomicBool,
}

struct VirtioNetStub;

struct VirtTimer {
    ticks: AtomicU64,
}

struct Ns16550 {
    #[allow(dead_code)]
    base: usize,
}

struct PciBarAllocator {
    mmio_next: u64,
    mmio_limit: u64,
    io_next: u32,
    io_limit: u32,
    used_mem: bool,
    used_io: bool,
}

struct LoongArchVirtioHal;

impl HalPlatform for VirtPlatform {
    fn platform_name(&self) -> &'static str {
        "loongarch64-virt"
    }

    fn architecture(&self) -> PlatformArch {
        PlatformArch::LoongArch64
    }
}

impl HalPlatformLifecycle for VirtLifecycle {
    fn supports_userspace(&self) -> bool {
        cfg!(target_arch = "loongarch64")
    }

    fn idle(&self) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
}

impl VirtInterruptController {
    const fn new() -> Self {
        Self {
            base: AtomicUsize::new(0),
        }
    }

    fn configure(&self, discovery: &LoongArchVirtioDiscovery) {
        if let Some(config) = discovery.interrupt {
            self.base.store(config.pch_pic_base, Ordering::Relaxed);
        }
    }

    fn is_ready(&self) -> bool {
        self.base.load(Ordering::Relaxed) != 0
    }
}

impl HalInterrupt for VirtInterruptController {
    fn name(&self) -> &'static str {
        if self.is_ready() {
            "ls7a-pch-pic"
        } else {
            "ls7a-pch-pic-unavailable"
        }
    }

    fn enable_irq(&self, _irq: usize) {}

    fn disable_irq(&self, _irq: usize) {}

    fn ack_irq(&self, _irq: usize) {}

    fn next_pending(&self) -> Option<usize> {
        None
    }
}

impl VirtCpu {
    const fn new() -> Self {
        Self {
            interrupts_enabled: AtomicBool::new(false),
        }
    }
}

impl HalCpu for VirtCpu {
    fn cpu_id(&self) -> usize {
        0
    }

    fn enable_interrupts(&self) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            let mut crmd: usize;
            core::arch::asm!("csrrd {}, 0x0", out(reg) crmd);
            crmd |= 1 << 2; // Set IE bit
            core::arch::asm!("csrwr {}, 0x0", in(reg) crmd);
            
            use hal_api::ConsoleWriter;
            use core::fmt::Write;
            let mut console = ConsoleWriter;
            let _ = write!(console, "whuse-debug: enable_interrupts CRMD={:#x}\n", crmd);
        }
        self.interrupts_enabled.store(true, Ordering::Relaxed);
    }

    fn disable_interrupts(&self) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            let mut crmd: usize;
            core::arch::asm!("csrrd {}, 0x0", out(reg) crmd);
            crmd &= !(1 << 2); // Clear IE bit
            core::arch::asm!("csrwr {}, 0x0", in(reg) crmd);
        }
        self.interrupts_enabled.store(false, Ordering::Relaxed);
    }

    fn interrupts_enabled(&self) -> bool {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            let crmd: usize;
            core::arch::asm!("csrrd {}, 0x0", out(reg) crmd);
            return (crmd & (1 << 2)) != 0;
        }
        #[cfg(not(target_arch = "loongarch64"))]
        self.interrupts_enabled.load(Ordering::Relaxed)
    }

    fn switch_address_space(&self, token: VmSpaceToken) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            let root = token.0;
            let mut crmd: usize;
            core::arch::asm!("csrrd {}, 0x0", out(reg) crmd);
            crmd &= !0x1e8; // clear DA/DATF/DATM
            crmd |= 0xb0; // PG=1, DATF=01, DATM=01
            core::arch::asm!(
                "csrwr {root}, 0x19",
                "csrwr {root}, 0x1a",
                "csrwr {crmd}, 0x0",
                "dbar 0",
                "invtlb 0x00, $r0, $r0",
                root = in(reg) root,
                crmd = in(reg) crmd,
            );
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            let _ = token;
        }
    }

    fn wait_for_interrupt(&self) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("idle 0");
        }
        #[cfg(not(target_arch = "loongarch64"))]
        core::hint::spin_loop();
    }

    fn run_user(&self, frame: &mut TrapFrame) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            __whuse_run_user(frame as *mut TrapFrame);
            if KERNEL_TRAP_HANDLER.load(core::sync::atomic::Ordering::Relaxed) != 0 {
                let eentry = __whuse_kernel_trap_entry as *const () as usize;
                core::arch::asm!("csrwr {}, 0xc", in(reg) eentry);
            }
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            frame.scause = usize::MAX;
        }
    }

    fn set_kernel_timer_callback(&self, cb: fn()) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            use hal_api::ConsoleWriter;
            use core::fmt::Write;
            let mut console = ConsoleWriter;
            let _ = write!(console, "whuse-debug: set_kernel_timer_callback called\n");
            
            KERNEL_TRAP_HANDLER.store(cb as usize, core::sync::atomic::Ordering::Relaxed);
            let eentry = __whuse_kernel_trap_entry as *const () as usize;
            core::arch::asm!("csrwr {}, 0xc", in(reg) eentry);
            
            let _ = write!(console, "whuse-debug: EENTRY set to {:#x}\n", eentry);
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            let _ = cb;
        }
    }
}

impl HalMemory for VirtMemory {
    fn memory_regions(&self) -> &'static [MemoryRegion] {
        &MEMORY_MAP
    }

    fn phys_to_virt(&self, phys: usize) -> usize {
        phys
    }

    fn virt_to_phys(&self, virt: usize) -> usize {
        virt
    }

    fn mmio_base(&self) -> usize {
        MMIO_PHYS_BASE
    }
}

impl VirtTimer {
    const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
        }
    }
}

impl HalTimer for VirtTimer {
    fn monotonic_time(&self) -> Timespec {
        Timespec::from_nanos(self.monotonic_nanos())
    }

    fn monotonic_nanos(&self) -> u64 {
        #[cfg(target_arch = "loongarch64")]
        {
            let count: u64;
            unsafe {
                core::arch::asm!("rdtime.d {}, $r0", out(reg) count);
            }
            count * 10
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            self.ticks.fetch_add(1_000_000, Ordering::Relaxed)
        }
    }

    fn program_oneshot(&self, deadline_nanos: u64) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            let now_ticks: u64;
            core::arch::asm!("rdtime.d {}, $r0", out(reg) now_ticks);
            let now_nanos = now_ticks * 10;

            let delta_nanos = deadline_nanos.saturating_sub(now_nanos);
            let delta_ticks = delta_nanos / 10;
            let init_val = delta_ticks.max(1000);

            use hal_api::ConsoleWriter;
            use core::fmt::Write;
            let mut console = ConsoleWriter;
            let _ = write!(console, "whuse-debug: program_oneshot delta_ticks={} init_val={}\n", delta_ticks, init_val);

            let mut ecfg: usize;
            core::arch::asm!("csrrd {}, 0x4", out(reg) ecfg);
            ecfg |= ECFG_TI;
            core::arch::asm!("csrwr {}, 0x4", in(reg) ecfg);

            let tcfg: usize = (init_val as usize) << 2 | 0x1;
            core::arch::asm!("csrwr {}, 0x41", in(reg) tcfg);
            
            let _ = write!(console, "whuse-debug: TCFG={:#x} ECFG={:#x}\n", tcfg, ecfg);
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            let _ = deadline_nanos;
        }
    }
}

impl Ns16550 {
    const fn new(base: usize) -> Self {
        Self { base }
    }
}

impl HalCharDevice for Ns16550 {
    fn name(&self) -> &'static str {
        "uart0"
    }

    fn put_byte(&self, byte: u8) {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            while read_volatile((self.base + 5) as *const u8) & (1 << 5) == 0 {}
            write_volatile(self.base as *mut u8, byte);
        }
        #[cfg(not(target_arch = "loongarch64"))]
        let _ = byte;
    }

    fn get_byte(&self) -> Option<u8> {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            if read_volatile((self.base + 5) as *const u8) & 1 == 0 {
                return None;
            }
            return Some(read_volatile(self.base as *const u8));
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            None
        }
    }
}

impl VirtioBlockDevice {
    const fn new() -> Self {
        Self {
            state: Once::new(),
            init_error: AtomicI32::new(ENODEV),
            capacity_sectors: AtomicUsize::new(0),
            readonly: AtomicBool::new(false),
        }
    }

    fn bootstrap(&self, discovery: &LoongArchVirtioDiscovery) {
        if self.state.get().is_some() {
            return;
        }
        let Some(host) = discovery.pci_host else {
            return;
        };
        let _ = self.try_init_pci(host);
    }

    fn try_init_pci(&self, host: PciHostConfig) -> Result<(), i32> {
        let enum_cam = unsafe { MmioCam::new(host.ecam_base as *mut u8, Cam::Ecam) };
        let mut raw_cam = unsafe { MmioCam::new(host.ecam_base as *mut u8, Cam::Ecam) };
        let enum_root = PciRoot::new(enum_cam);
        let mut cfg_root =
            PciRoot::new(unsafe { MmioCam::new(host.ecam_base as *mut u8, Cam::Ecam) });
        let mut allocator = PciBarAllocator::new(&host).ok_or(ENODEV)?;
        for bus in host.bus_start..=host.bus_end {
            for (device_function, info) in enum_root.enumerate_bus(bus) {
                if virtio_device_type(&info) != Some(virtio_drivers::transport::DeviceType::Block) {
                    continue;
                }
                let command = allocate_virtio_bars(
                    &mut cfg_root,
                    &mut raw_cam,
                    device_function,
                    &mut allocator,
                )?;
                cfg_root.set_command(device_function, command);
                let transport =
                    PciTransport::new::<LoongArchVirtioHal, _>(&mut cfg_root, device_function)
                        .map_err(|_| ENODEV)?;
                let mut driver =
                    VirtIOBlk::<LoongArchVirtioHal, _>::new(SomeTransport::from(transport))
                        .map_err(virtio_error_to_errno)?;
                // LoongArch virt platform currently does not wire a functional IRQ controller
                // in this HAL; keep virtio-blk in polling mode to avoid blocking forever on
                // completion paths that wait for queue interrupts.
                let info = VirtioBlockConfig {
                    transport: hal_virtio::TransportKind::Pci,
                    irq: None,
                    capacity_sectors: driver.capacity() as usize,
                    readonly: driver.readonly(),
                };
                self.capacity_sectors
                    .store(info.capacity_sectors, Ordering::Relaxed);
                self.readonly.store(info.readonly, Ordering::Relaxed);
                self.init_error.store(0, Ordering::Relaxed);
                let _ = self
                    .state
                    .call_once(|| Mutex::new(VirtioBlockState { driver }));
                return Ok(());
            }
        }
        Err(ENODEV)
    }

    fn with_state(&self) -> Result<&Mutex<VirtioBlockState>, i32> {
        self.state
            .get()
            .ok_or_else(|| self.init_error.load(Ordering::Relaxed))
    }
}

impl HalBlockDevice for VirtioBlockDevice {
    fn name(&self) -> &'static str {
        "virtio-blk0"
    }

    fn init(&self) -> Result<(), i32> {
        if self.state.get().is_some() {
            return Ok(());
        }
        Err(self.init_error.load(Ordering::Relaxed))
    }

    fn is_ready(&self) -> bool {
        self.state.get().is_some() && self.capacity_sectors.load(Ordering::Relaxed) != 0
    }

    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn sector_count(&self) -> usize {
        self.capacity_sectors.load(Ordering::Relaxed)
    }

    fn ack_interrupt(&self) -> bool {
        let Ok(state) = self.with_state() else {
            return false;
        };
        state
            .lock()
            .driver
            .ack_interrupt()
            .contains(InterruptStatus::QUEUE_INTERRUPT)
    }

    fn read_sector(&self, sector: usize, buf: &mut [u8]) -> Result<(), i32> {
        if buf.len() != SECTOR_SIZE {
            return Err(EINVAL);
        }
        if sector >= self.capacity_sectors.load(Ordering::Relaxed) {
            return Err(EINVAL);
        }
        let state = self.with_state()?;
        for _ in 0..3 {
            if state.lock().driver.read_blocks(sector, buf).is_ok() {
                return Ok(());
            }
        }
        Err(EIO)
    }

    fn read_sectors(&self, start_sector: usize, buf: &mut [u8]) -> Result<(), i32> {
        if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
            return Err(EINVAL);
        }
        for (index, chunk) in buf.chunks_exact_mut(SECTOR_SIZE).enumerate() {
            self.read_sector(start_sector + index, chunk)?;
        }
        Ok(())
    }

    fn write_sector(&self, sector: usize, buf: &[u8]) -> Result<(), i32> {
        if self.readonly.load(Ordering::Relaxed) {
            return Err(EROFS);
        }
        if buf.len() != SECTOR_SIZE {
            return Err(EINVAL);
        }
        if sector >= self.capacity_sectors.load(Ordering::Relaxed) {
            return Err(EINVAL);
        }
        let state = self.with_state()?;
        for _ in 0..3 {
            if state.lock().driver.write_blocks(sector, buf).is_ok() {
                return Ok(());
            }
        }
        Err(EIO)
    }

    fn flush(&self) -> Result<(), i32> {
        let state = self.with_state()?;
        state.lock().driver.flush().map_err(virtio_error_to_errno)
    }
}

impl HalNetDevice for VirtioNetStub {
    fn name(&self) -> &'static str {
        "virtio-net0"
    }

    fn mac_address(&self) -> [u8; 6] {
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x64]
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

impl PciBarAllocator {
    fn new(host: &PciHostConfig) -> Option<Self> {
        let mmio = host.mmio?;
        let io = host
            .io
            .unwrap_or(hal_virtio::PciHostWindow { base: 0, size: 0 });
        Some(Self {
            mmio_next: mmio.base as u64,
            mmio_limit: mmio.base as u64 + mmio.size as u64,
            io_next: io.base as u32,
            io_limit: io.base as u32 + io.size as u32,
            used_mem: false,
            used_io: false,
        })
    }

    fn alloc_memory(&mut self, size: u64, address_type: MemoryBarType) -> Option<u64> {
        let align = size.max(0x1000);
        let start = align_up_u64(self.mmio_next, align);
        if matches!(address_type, MemoryBarType::Below1MiB) && start + size > 0x10_0000 {
            return None;
        }
        if start.checked_add(size)? > self.mmio_limit {
            return None;
        }
        self.mmio_next = start + size;
        self.used_mem = true;
        Some(start)
    }

    fn alloc_io(&mut self, size: u32) -> Option<u32> {
        if self.io_limit == 0 {
            return None;
        }
        let start = align_up_u32(self.io_next, size.max(4));
        if start.checked_add(size)? > self.io_limit {
            return None;
        }
        self.io_next = start + size;
        self.used_io = true;
        Some(start)
    }

    fn command(&self) -> Command {
        let mut command = Command::BUS_MASTER;
        if self.used_mem {
            command |= Command::MEMORY_SPACE;
        }
        if self.used_io {
            command |= Command::IO_SPACE;
        }
        command
    }
}

fn allocate_virtio_bars(
    root: &mut PciRoot<MmioCam<'static>>,
    raw_cam: &mut MmioCam<'static>,
    device_function: DeviceFunction,
    allocator: &mut PciBarAllocator,
) -> Result<Command, i32> {
    let bars = root.bars(device_function).map_err(|_| ENODEV)?;
    let mut bar_index = 0usize;
    while bar_index < bars.len() {
        let Some(bar) = bars[bar_index].as_ref() else {
            bar_index += 1;
            continue;
        };
        match bar {
            BarInfo::Memory {
                address_type,
                prefetchable,
                size,
                ..
            } => {
                let address = allocator.alloc_memory(*size, *address_type).ok_or(ENODEV)?;
                let flags =
                    ((u8::from(*address_type) as u32) << 1) | if *prefetchable { 0x8 } else { 0 };
                write_memory_bar(raw_cam, device_function, bar_index as u8, address, flags);
                bar_index += if bar.takes_two_entries() { 2 } else { 1 };
            }
            BarInfo::IO { size, .. } => {
                let address = allocator.alloc_io(*size).ok_or(ENODEV)?;
                raw_cam.write_word(device_function, 0x10 + 4 * bar_index as u8, address | 0x1);
                bar_index += 1;
            }
        }
    }
    Ok(allocator.command())
}

fn write_memory_bar(
    raw_cam: &mut MmioCam<'static>,
    device_function: DeviceFunction,
    bar_index: u8,
    address: u64,
    flags: u32,
) {
    raw_cam.write_word(
        device_function,
        0x10 + 4 * bar_index,
        ((address as u32) & !0xf) | flags,
    );
    if flags & 0b100 != 0 {
        raw_cam.write_word(
            device_function,
            0x10 + 4 * (bar_index + 1),
            (address >> 32) as u32,
        );
    }
}

fn align_up_u64(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

fn align_up_u32(value: u32, align: u32) -> u32 {
    (value + align - 1) & !(align - 1)
}

unsafe impl VirtioHal for LoongArchVirtioHal {
    fn dma_alloc(
        pages: usize,
        _direction: BufferDirection,
    ) -> (virtio_drivers::PhysAddr, NonNull<u8>) {
        DMA_ARENA.alloc(pages).unwrap_or((0, NonNull::dangling()))
    }

    unsafe fn dma_dealloc(
        paddr: virtio_drivers::PhysAddr,
        _vaddr: NonNull<u8>,
        pages: usize,
    ) -> i32 {
        DMA_ARENA.dealloc(paddr, pages)
    }

    unsafe fn mmio_phys_to_virt(paddr: virtio_drivers::PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as usize as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> virtio_drivers::PhysAddr {
        let len = buffer.len();
        if len == 0 {
            return 0;
        }
        let pages = len.div_ceil(hal_virtio::DMA_PAGE_SIZE);
        let Some((paddr, vaddr)) = DMA_ARENA.alloc(pages) else {
            return 0;
        };
        if matches!(
            direction,
            BufferDirection::DriverToDevice | BufferDirection::Both
        ) {
            core::ptr::copy_nonoverlapping(buffer.as_ptr().cast::<u8>(), vaddr.as_ptr(), len);
        }
        paddr
    }

    unsafe fn unshare(
        paddr: virtio_drivers::PhysAddr,
        buffer: NonNull<[u8]>,
        direction: BufferDirection,
    ) {
        let len = buffer.len();
        if len == 0 || paddr == 0 {
            return;
        }
        let pages = len.div_ceil(hal_virtio::DMA_PAGE_SIZE);
        if matches!(
            direction,
            BufferDirection::DeviceToDriver | BufferDirection::Both
        ) {
            if let Some(vaddr) = DMA_ARENA.phys_to_virt(paddr) {
                core::ptr::copy_nonoverlapping(vaddr.as_ptr(), buffer.as_ptr().cast::<u8>(), len);
            }
        }
        let _ = DMA_ARENA.dealloc(paddr, pages);
    }
}

fn qemu_virt_fallback() -> LoongArchVirtioDiscovery {
    LoongArchVirtioDiscovery {
        pci_host: Some(PciHostConfig {
            ecam_base: 0x2000_0000,
            ecam_size: 0x0800_0000,
            bus_start: 0,
            bus_end: 0x7f,
            io: Some(PciHostWindow {
                base: 0x1800_4000,
                size: 0x0000_c000,
            }),
            mmio: Some(PciHostWindow {
                base: 0x4000_0000,
                size: 0x4000_0000,
            }),
        }),
        interrupt: Some(LoongArchInterruptConfig {
            pch_pic_base: 0x1000_0000,
            pch_pic_size: 0x1000,
        }),
    }
}
