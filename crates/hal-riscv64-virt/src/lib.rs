#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(target_arch = "riscv64")]
use core::arch::global_asm;
#[cfg(target_arch = "riscv64")]
use core::fmt::Write;
use core::ptr::NonNull;
#[cfg(target_arch = "riscv64")]
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalInterrupt, HalMemory,
    HalNetDevice, HalPlatform, HalPlatformLifecycle, HalTimer, MemoryRegion, PlatformArch,
    Timespec, TrapFrame, VmSpaceToken,
};
use hal_virtio::{
    parse_riscv_virtio_discovery, virtio_error_to_errno, RiscvVirtioDiscovery, VirtioBlockConfig,
    VirtioDmaArena,
};
use spin::{Mutex, Once};
use virtio_drivers::device::blk::{VirtIOBlk, SECTOR_SIZE};
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::InterruptStatus;
use virtio_drivers::transport::SomeTransport;
use virtio_drivers::transport::Transport;
use virtio_drivers::BufferDirection;
use virtio_drivers::Hal as VirtioHal;

pub const UART0_BASE: usize = 0x1000_0000;
pub const VIRTIO0_BASE: usize = 0x1000_1000;
pub const MMIO_BASE: usize = 0x1000_0000;
pub const PHYS_MEM_BASE: usize = 0x8000_0000;
pub const PHYS_MEM_SIZE: usize = 256 * 1024 * 1024;
const DMA_ARENA_BYTES: usize = 2 * 1024 * 1024;
const DMA_ARENA_WORDS: usize = DMA_ARENA_BYTES / SECTOR_SIZE / 64;
const EIO: i32 = 5;
const ENODEV: i32 = 19;
const EINVAL: i32 = 22;
const EROFS: i32 = 30;
const RISCV_TIMEBASE_HZ: u64 = 10_000_000;
const SBI_EXT_TIME: usize = 0x5449_4d45;
const SBI_FID_SET_TIMER: usize = 0;
const SBI_EXT_LEGACY_SET_TIMER: usize = 0x00;

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

#[cfg(target_arch = "riscv64")]
#[no_mangle]
static mut __whuse_current_frame: usize = 0;

#[cfg(target_arch = "riscv64")]
#[no_mangle]
static mut __whuse_kernel_ra: usize = 0;

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text
    .globl __whuse_run_user
__whuse_run_user:
    la t0, __whuse_current_frame
    sd a0, 0(t0)
    la t0, __whuse_kernel_ra
    sd ra, 0(t0)
    la t0, __whuse_user_trap_entry
    csrw stvec, t0
    csrw sscratch, sp

    mv t6, a0

    ld ra,   8(t6)
    ld sp,  16(t6)
    ld gp,  24(t6)
    ld tp,  32(t6)
    ld s1,  72(t6)
    ld a0,  80(t6)
    ld a1,  88(t6)
    ld a2,  96(t6)
    ld a3, 104(t6)
    ld a4, 112(t6)
    ld a5, 120(t6)
    ld a6, 128(t6)
    ld a7, 136(t6)
    ld s2, 144(t6)
    ld s3, 152(t6)
    ld s4, 160(t6)
    ld s5, 168(t6)
    ld s6, 176(t6)
    ld s7, 184(t6)
    ld s8, 192(t6)
    ld s9, 200(t6)
    ld s10, 208(t6)
    ld s11, 216(t6)
    ld t3, 224(t6)
    ld t4, 232(t6)
    ld t5, 240(t6)

    ld t0, 256(t6)
    csrw sepc, t0
    ld t0, 264(t6)
    ori t0, t0, 32
    li t2, 0x6000
    or t0, t0, t2
    csrw sstatus, t0

    fld f0,  288(t6)
    fld f1,  296(t6)
    fld f2,  304(t6)
    fld f3,  312(t6)
    fld f4,  320(t6)
    fld f5,  328(t6)
    fld f6,  336(t6)
    fld f7,  344(t6)
    fld f8,  352(t6)
    fld f9,  360(t6)
    fld f10, 368(t6)
    fld f11, 376(t6)
    fld f12, 384(t6)
    fld f13, 392(t6)
    fld f14, 400(t6)
    fld f15, 408(t6)
    fld f16, 416(t6)
    fld f17, 424(t6)
    fld f18, 432(t6)
    fld f19, 440(t6)
    fld f20, 448(t6)
    fld f21, 456(t6)
    fld f22, 464(t6)
    fld f23, 472(t6)
    fld f24, 480(t6)
    fld f25, 488(t6)
    fld f26, 496(t6)
    fld f27, 504(t6)
    fld f28, 512(t6)
    fld f29, 520(t6)
    fld f30, 528(t6)
    fld f31, 536(t6)
    ld t1, 544(t6)
    csrw fcsr, t1

    ld s0,  64(t6)
    ld t1,  48(t6)
    ld t2,  56(t6)
    ld t0,  40(t6)
    ld t6, 248(t6)
    sret

    .align 4
    .globl __whuse_user_trap_entry
__whuse_user_trap_entry:
    csrrw sp, sscratch, sp
    addi sp, sp, -16
    sd t0, 0(sp)
    sd t1, 8(sp)

    la t0, __whuse_current_frame
    ld t0, 0(t0)

    sd zero, 0(t0)
    sd ra,   8(t0)
    csrr t1, sscratch
    sd t1,  16(t0)
    sd gp,  24(t0)
    sd tp,  32(t0)
    ld t1,   0(sp)
    sd t1,  40(t0)
    ld t1,   8(sp)
    sd t1,  48(t0)
    sd t2,  56(t0)
    sd s0,  64(t0)
    sd s1,  72(t0)
    sd a0,  80(t0)
    sd a1,  88(t0)
    sd a2,  96(t0)
    sd a3, 104(t0)
    sd a4, 112(t0)
    sd a5, 120(t0)
    sd a6, 128(t0)
    sd a7, 136(t0)
    sd s2, 144(t0)
    sd s3, 152(t0)
    sd s4, 160(t0)
    sd s5, 168(t0)
    sd s6, 176(t0)
    sd s7, 184(t0)
    sd s8, 192(t0)
    sd s9, 200(t0)
    sd s10, 208(t0)
    sd s11, 216(t0)
    sd t3, 224(t0)
    sd t4, 232(t0)
    sd t5, 240(t0)
    sd t6, 248(t0)

    csrr t1, sepc
    sd t1, 256(t0)
    csrr t1, sstatus
    sd t1, 264(t0)
    csrr t1, scause
    sd t1, 272(t0)
    csrr t1, stval
    sd t1, 280(t0)
    fsd f0,  288(t0)
    fsd f1,  296(t0)
    fsd f2,  304(t0)
    fsd f3,  312(t0)
    fsd f4,  320(t0)
    fsd f5,  328(t0)
    fsd f6,  336(t0)
    fsd f7,  344(t0)
    fsd f8,  352(t0)
    fsd f9,  360(t0)
    fsd f10, 368(t0)
    fsd f11, 376(t0)
    fsd f12, 384(t0)
    fsd f13, 392(t0)
    fsd f14, 400(t0)
    fsd f15, 408(t0)
    fsd f16, 416(t0)
    fsd f17, 424(t0)
    fsd f18, 432(t0)
    fsd f19, 440(t0)
    fsd f20, 448(t0)
    fsd f21, 456(t0)
    fsd f22, 464(t0)
    fsd f23, 472(t0)
    fsd f24, 480(t0)
    fsd f25, 488(t0)
    fsd f26, 496(t0)
    fsd f27, 504(t0)
    fsd f28, 512(t0)
    fsd f29, 520(t0)
    fsd f30, 528(t0)
    fsd f31, 536(t0)
    csrr t1, fcsr
    sd t1, 544(t0)

    addi sp, sp, 16
    la t0, __whuse_kernel_ra
    ld ra, 0(t0)
    ret
"#
);

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text
    .globl __whuse_kernel_trap_entry
    .align 4
__whuse_kernel_trap_entry:
    addi sp, sp, -256
    sd ra,    0(sp)
    sd t0,    8(sp)
    sd t1,   16(sp)
    sd t2,   24(sp)
    sd t3,   32(sp)
    sd t4,   40(sp)
    sd t5,   48(sp)
    sd t6,   56(sp)
    sd a0,   64(sp)
    sd a1,   72(sp)
    sd a2,   80(sp)
    sd a3,   88(sp)
    sd a4,   96(sp)
    sd a5,  104(sp)
    sd a6,  112(sp)
    sd a7,  120(sp)
    sd s0,  128(sp)
    sd s1,  136(sp)
    sd s2,  144(sp)
    sd s3,  152(sp)
    sd s4,  160(sp)
    sd s5,  168(sp)
    sd s6,  176(sp)
    sd s7,  184(sp)
    sd s8,  192(sp)
    sd s9,  200(sp)
    sd s10, 208(sp)
    sd s11, 216(sp)
    csrr a0, scause
    csrr a1, sepc
    call __whuse_kernel_trap_handler
    ld ra,    0(sp)
    ld t0,    8(sp)
    ld t1,   16(sp)
    ld t2,   24(sp)
    ld t3,   32(sp)
    ld t4,   40(sp)
    ld t5,   48(sp)
    ld t6,   56(sp)
    ld a0,   64(sp)
    ld a1,   72(sp)
    ld a2,   80(sp)
    ld a3,   88(sp)
    ld a4,   96(sp)
    ld a5,  104(sp)
    ld a6,  112(sp)
    ld a7,  120(sp)
    ld s0,  128(sp)
    ld s1,  136(sp)
    ld s2,  144(sp)
    ld s3,  152(sp)
    ld s4,  160(sp)
    ld s5,  168(sp)
    ld s6,  176(sp)
    ld s7,  184(sp)
    ld s8,  192(sp)
    ld s9,  200(sp)
    ld s10, 208(sp)
    ld s11, 216(sp)
    addi sp, sp, 256
    sret
"#
);

extern "C" {
    fn end();
    fn __whuse_kernel_trap_entry();
    fn __whuse_run_user(frame: *mut TrapFrame);
}

pub static KERNEL_TRAP_HANDLER: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

#[no_mangle]
unsafe extern "C" fn __whuse_kernel_trap_handler(scause: usize, _sepc: usize) {
    let interrupt_bit = 1usize << (usize::BITS as usize - 1);
    let is_timer = (scause & interrupt_bit) != 0 && (scause & !interrupt_bit) == 5;
    if !is_timer {
        return;
    }
    let cb_ptr = KERNEL_TRAP_HANDLER.load(core::sync::atomic::Ordering::Relaxed);
    if cb_ptr != 0 {
        let cb: fn() = core::mem::transmute(cb_ptr);
        cb();
    }
}

static mut MEMORY_MAP: [MemoryRegion; 2] = [
    MemoryRegion {
        start: 0x0,
        size: MMIO_BASE,
        usable: false,
    },
    MemoryRegion {
        start: PHYS_MEM_BASE,
        size: PHYS_MEM_SIZE,
        usable: true,
    },
];

pub fn bootstrap(dtb_pa: usize) {
    if dtb_pa != 0 {
        if let Some(discovery) = parse_riscv_virtio_discovery(dtb_pa) {
            INTERRUPT.configure(discovery.plic);
            VIRTIO_BLK.bootstrap(&discovery);
        }
    }
    unsafe {
        let kernel_end = (end as *const () as usize + 4095) & !4095;
        MEMORY_MAP[1].start = kernel_end;
        MEMORY_MAP[1].size = (PHYS_MEM_BASE + PHYS_MEM_SIZE).saturating_sub(kernel_end);
        // Use UART directly before HAL is registered to avoid panic.
        for byte in "whuse: memory map adjusted\n".bytes() {
            UART.put_byte(byte);
        }
    }
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
    context: AtomicUsize,
    sources: AtomicUsize,
}

struct VirtPlatform;
struct VirtLifecycle;

struct VirtioBlockState {
    driver: VirtIOBlk<RiscvVirtioHal, SomeTransport<'static>>,
}

struct VirtioBlockDevice {
    state: Once<Mutex<VirtioBlockState>>,
    init_error: AtomicI32,
    capacity_sectors: AtomicUsize,
    readonly: AtomicBool,
    irq: AtomicUsize,
}

struct VirtioNetStub;
struct VirtMemory;

struct VirtTimer {
    ticks: AtomicU64,
    deadline: AtomicU64,
}

struct Ns16550 {
    #[allow(dead_code)]
    base: usize,
}

struct RiscvVirtioHal;

impl HalPlatform for VirtPlatform {
    fn platform_name(&self) -> &'static str {
        "riscv64-virt"
    }

    fn architecture(&self) -> PlatformArch {
        PlatformArch::Riscv64
    }
}

impl HalPlatformLifecycle for VirtLifecycle {
    fn supports_userspace(&self) -> bool {
        true
    }

    fn idle(&self) -> ! {
        loop {
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("wfi");
            }
            #[cfg(not(target_arch = "riscv64"))]
            core::hint::spin_loop();
        }
    }
}

#[allow(dead_code)]
impl VirtInterruptController {
    const fn new() -> Self {
        Self {
            base: AtomicUsize::new(0),
            context: AtomicUsize::new(0),
            sources: AtomicUsize::new(0),
        }
    }

    fn configure(&self, config: Option<hal_virtio::VirtioPlicConfig>) {
        let Some(config) = config else {
            return;
        };
        self.base.store(config.base, Ordering::Relaxed);
        self.context
            .store(config.supervisor_context, Ordering::Relaxed);
        self.sources.store(config.sources, Ordering::Relaxed);
        self.write_threshold(0);
    }

    fn is_ready(&self) -> bool {
        self.base.load(Ordering::Relaxed) != 0
    }

    fn enable_ptr(&self, irq: usize) -> Option<*mut u32> {
        if !self.is_ready() || irq == 0 || irq > self.sources.load(Ordering::Relaxed) {
            return None;
        }
        let base = self.base.load(Ordering::Relaxed);
        let context = self.context.load(Ordering::Relaxed);
        let word = irq / 32;
        Some((base + 0x2000 + context * 0x80 + word * 4) as *mut u32)
    }

    fn priority_ptr(&self, irq: usize) -> Option<*mut u32> {
        if !self.is_ready() || irq == 0 || irq > self.sources.load(Ordering::Relaxed) {
            return None;
        }
        Some((self.base.load(Ordering::Relaxed) + irq * 4) as *mut u32)
    }

    fn claim_complete_ptr(&self) -> Option<*mut u32> {
        if !self.is_ready() {
            return None;
        }
        let base = self.base.load(Ordering::Relaxed);
        let context = self.context.load(Ordering::Relaxed);
        Some((base + 0x20_0000 + context * 0x1000 + 4) as *mut u32)
    }

    fn threshold_ptr(&self) -> Option<*mut u32> {
        if !self.is_ready() {
            return None;
        }
        let base = self.base.load(Ordering::Relaxed);
        let context = self.context.load(Ordering::Relaxed);
        Some((base + 0x20_0000 + context * 0x1000) as *mut u32)
    }

    fn write_threshold(&self, value: u32) {
        #[cfg(target_arch = "riscv64")]
        if let Some(ptr) = self.threshold_ptr() {
            unsafe {
                write_volatile(ptr, value);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        let _ = value;
    }
}

impl HalInterrupt for VirtInterruptController {
    fn name(&self) -> &'static str {
        if self.is_ready() {
            "plic"
        } else {
            "plic-unavailable"
        }
    }

    fn enable_irq(&self, irq: usize) {
        #[cfg(target_arch = "riscv64")]
        {
            if let Some(priority) = self.priority_ptr(irq) {
                unsafe {
                    write_volatile(priority, 1);
                }
            }
            if let Some(enable) = self.enable_ptr(irq) {
                let bit = 1u32 << (irq % 32);
                unsafe {
                    let current = read_volatile(enable);
                    write_volatile(enable, current | bit);
                }
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        let _ = irq;
    }

    fn disable_irq(&self, irq: usize) {
        #[cfg(target_arch = "riscv64")]
        if let Some(enable) = self.enable_ptr(irq) {
            let bit = 1u32 << (irq % 32);
            unsafe {
                let current = read_volatile(enable);
                write_volatile(enable, current & !bit);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        let _ = irq;
    }

    fn ack_irq(&self, irq: usize) {
        #[cfg(target_arch = "riscv64")]
        if let Some(claim) = self.claim_complete_ptr() {
            unsafe {
                write_volatile(claim, irq as u32);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        let _ = irq;
    }

    fn next_pending(&self) -> Option<usize> {
        #[cfg(target_arch = "riscv64")]
        if let Some(claim) = self.claim_complete_ptr() {
            let irq = unsafe { read_volatile(claim) as usize };
            return (irq != 0).then_some(irq);
        }
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
        #[cfg(target_arch = "riscv64")]
        unsafe {
            let mut sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            core::arch::asm!("csrw sstatus, {}", in(reg) sstatus | (1 << 1));
        }
        self.interrupts_enabled.store(true, Ordering::Relaxed);
    }

    fn disable_interrupts(&self) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            let mut sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            core::arch::asm!("csrw sstatus, {}", in(reg) sstatus & !(1 << 1));
        }
        self.interrupts_enabled.store(false, Ordering::Relaxed);
    }

    fn interrupts_enabled(&self) -> bool {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            let sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            (sstatus & (1 << 1)) != 0
        }
        #[cfg(not(target_arch = "riscv64"))]
        self.interrupts_enabled.load(Ordering::Relaxed)
    }

    fn switch_address_space(&self, token: VmSpaceToken) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            const SATP_MODE_SV39: usize = 8usize << 60;
            let satp = if token.0 == 0 {
                0
            } else {
                SATP_MODE_SV39 | (token.0 >> 12)
            };
            core::arch::asm!("csrw satp, {}", in(reg) satp);
            core::arch::asm!("sfence.vma zero, zero");
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            let _ = token;
        }
    }

    fn wait_for_interrupt(&self) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("csrsi sstatus, 2", "wfi",);
        }
        #[cfg(not(target_arch = "riscv64"))]
        core::hint::spin_loop();
    }

    fn run_user(&self, frame: &mut TrapFrame) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            __whuse_run_user(frame as *mut TrapFrame);
            if KERNEL_TRAP_HANDLER.load(core::sync::atomic::Ordering::Relaxed) != 0 {
                core::arch::asm!(
                    "la t0, __whuse_kernel_trap_entry",
                    "csrw stvec, t0",
                    out("t0") _,
                );
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            let _ = frame;
        }
    }

    fn set_kernel_timer_callback(&self, cb: fn()) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            KERNEL_TRAP_HANDLER.store(cb as usize, core::sync::atomic::Ordering::Relaxed);
            core::arch::asm!(
                "la t0, __whuse_kernel_trap_entry",
                "csrw stvec, t0",
                out("t0") _,
            );
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            let _ = cb;
        }
    }
}

impl HalMemory for VirtMemory {
    fn memory_regions(&self) -> &'static [MemoryRegion] {
        unsafe { &MEMORY_MAP }
    }

    fn phys_to_virt(&self, phys: usize) -> usize {
        phys
    }

    fn virt_to_phys(&self, virt: usize) -> usize {
        virt
    }

    fn mmio_base(&self) -> usize {
        MMIO_BASE
    }
}

impl VirtTimer {
    const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
        }
    }
}

#[cfg(target_arch = "riscv64")]
fn read_time_ticks() -> u64 {
    let ticks: u64;
    unsafe {
        core::arch::asm!("rdtime {}", out(reg) ticks);
    }
    ticks
}

#[cfg(not(target_arch = "riscv64"))]
fn read_time_ticks() -> u64 {
    0
}

#[cfg(target_arch = "riscv64")]
fn nanos_to_time_ticks(nanos: u64) -> u64 {
    nanos.saturating_mul(RISCV_TIMEBASE_HZ) / 1_000_000_000
}

#[cfg(target_arch = "riscv64")]
fn sbi_set_timer_v02(timer_ticks: u64) -> isize {
    let error: isize;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") timer_ticks as usize => error,
            in("a6") SBI_FID_SET_TIMER,
            in("a7") SBI_EXT_TIME,
            lateout("a1") _,
        );
    }
    error
}

#[cfg(target_arch = "riscv64")]
fn sbi_set_timer_legacy(timer_ticks: u64) -> isize {
    let error: isize;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") timer_ticks as usize => error,
            in("a7") SBI_EXT_LEGACY_SET_TIMER,
            lateout("a1") _,
            lateout("a2") _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
        );
    }
    error
}

impl HalTimer for VirtTimer {
    fn monotonic_time(&self) -> Timespec {
        Timespec::from_nanos(self.monotonic_nanos())
    }

    fn monotonic_nanos(&self) -> u64 {
        let ticks = read_time_ticks();
        ticks.saturating_mul(1_000_000_000) / RISCV_TIMEBASE_HZ
    }

    fn program_oneshot(&self, deadline_nanos: u64) {
        self.deadline.store(deadline_nanos, Ordering::Relaxed);
        #[cfg(target_arch = "riscv64")]
        unsafe {
            // Enable supervisor timer interrupts before arming next deadline.
            const STIE: usize = 1 << 5;
            core::arch::asm!("csrs sie, {}", in(reg) STIE);
        }
        #[cfg(target_arch = "riscv64")]
        {
            let ticks = nanos_to_time_ticks(deadline_nanos);
            // QEMU virt exposes SSTC in OpenSBI; direct stimecmp programming
            // keeps periodic preemption reliable during long user loops.
            unsafe {
                core::arch::asm!("csrw 0x14d, {}", in(reg) ticks);
            }
            let err_v02 = sbi_set_timer_v02(ticks);
            if err_v02 != 0 {
                let err_legacy = sbi_set_timer_legacy(ticks);
                if err_legacy != 0 {
                    let mut console = hal_api::ConsoleWriter;
                    let _ = writeln!(
                        console,
                        "whuse: warn timer arm failed v0.2={} legacy={}",
                        err_v02, err_legacy
                    );
                }
            }
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
        #[cfg(target_arch = "riscv64")]
        unsafe {
            write_volatile(self.base as *mut u8, byte);
        }
        #[cfg(not(target_arch = "riscv64"))]
        let _ = byte;
    }

    fn get_byte(&self) -> Option<u8> {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            return Some(read_volatile(self.base as *const u8));
        }
        #[cfg(not(target_arch = "riscv64"))]
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
            irq: AtomicUsize::new(0),
        }
    }

    fn bootstrap(&self, discovery: &RiscvVirtioDiscovery) {
        if self.state.get().is_some() {
            return;
        }
        for config in discovery.mmio_devices.iter().flatten().copied() {
            if self.try_init_mmio(config).is_ok() {
                break;
            }
        }
    }

    fn try_init_mmio(&self, config: hal_virtio::VirtioMmioConfig) -> Result<(), i32> {
        let header = NonNull::new(config.base as *mut VirtIOHeader).ok_or(ENODEV)?;
        let transport = unsafe { MmioTransport::new(header, config.size) }.map_err(|_| ENODEV)?;
        if transport.device_type() != virtio_drivers::transport::DeviceType::Block {
            return Err(ENODEV);
        }
        let mut driver = VirtIOBlk::<RiscvVirtioHal, _>::new(SomeTransport::from(transport))
            .map_err(virtio_error_to_errno)?;
        driver.enable_interrupts();
        let info = VirtioBlockConfig {
            transport: hal_virtio::TransportKind::Mmio,
            irq: Some(config.irq),
            capacity_sectors: driver.capacity() as usize,
            readonly: driver.readonly(),
        };
        self.capacity_sectors
            .store(info.capacity_sectors, Ordering::Relaxed);
        self.readonly.store(info.readonly, Ordering::Relaxed);
        self.irq.store(config.irq, Ordering::Relaxed);
        self.init_error.store(0, Ordering::Relaxed);
        let _ = self
            .state
            .call_once(|| Mutex::new(VirtioBlockState { driver }));
        INTERRUPT.enable_irq(config.irq);
        Ok(())
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

    fn irq_line(&self) -> Option<usize> {
        let irq = self.irq.load(Ordering::Relaxed);
        (irq != 0).then_some(irq)
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
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]
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

unsafe impl VirtioHal for RiscvVirtioHal {
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
