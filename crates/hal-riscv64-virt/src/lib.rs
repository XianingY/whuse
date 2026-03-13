#![cfg_attr(not(test), no_std)]

#[cfg(target_arch = "riscv64")]
use core::arch::global_asm;
#[cfg(target_arch = "riscv64")]
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalMemory, HalPlatform,
    HalTimer, MemoryRegion, PlatformArch, Timespec, TrapFrame, VmSpaceToken,
};

pub const UART0_BASE: usize = 0x1000_0000;
pub const VIRTIO0_BASE: usize = 0x1000_1000;
pub const MMIO_BASE: usize = 0x1000_0000;
pub const PHYS_MEM_BASE: usize = 0x8000_0000;
pub const PHYS_MEM_SIZE: usize = 128 * 1024 * 1024;

static CPU: VirtCpu = VirtCpu::new();
static PLATFORM: VirtPlatform = VirtPlatform;
static MEMORY: VirtMemory = VirtMemory;
static TIMER: VirtTimer = VirtTimer::new();
static UART: Ns16550 = Ns16550::new(UART0_BASE);
static VIRTIO_BLK: VirtioBlockStub = VirtioBlockStub;
static BLOCK_DEVS: [&'static dyn HalBlockDevice; 1] = [&VIRTIO_BLK];

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
    ld t1,  48(t6)
    ld t2,  56(t6)
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
    li t2, -257
    and t0, t0, t2
    ori t0, t0, 32
    csrw sstatus, t0

    ld s0,  64(t6)
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

    addi sp, sp, 16
    la t0, __whuse_kernel_ra
    ld ra, 0(t0)
    ret
"#
);

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    fn __whuse_run_user(frame: *mut TrapFrame);
}

static MEMORY_MAP: [MemoryRegion; 2] = [
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

pub fn bootstrap() {
    register_hal(HalBundle {
        platform: &PLATFORM,
        cpu: &CPU,
        memory: &MEMORY,
        timer: &TIMER,
        console: &UART,
        block_devices: &BLOCK_DEVS,
    });
}

struct VirtCpu {
    interrupts_enabled: AtomicBool,
}

struct VirtPlatform;

impl HalPlatform for VirtPlatform {
    fn platform_name(&self) -> &'static str {
        "riscv64-virt"
    }

    fn architecture(&self) -> PlatformArch {
        PlatformArch::Riscv64
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
        self.interrupts_enabled.store(true, Ordering::Relaxed);
    }

    fn disable_interrupts(&self) {
        self.interrupts_enabled.store(false, Ordering::Relaxed);
    }

    fn interrupts_enabled(&self) -> bool {
        self.interrupts_enabled.load(Ordering::Relaxed)
    }

    fn switch_address_space(&self, _token: VmSpaceToken) {}

    fn wait_for_interrupt(&self) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("wfi");
        }
    }

    fn run_user(&self, frame: &mut TrapFrame) {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            __whuse_run_user(frame as *mut TrapFrame);
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            let _ = frame;
        }
    }
}

struct VirtMemory;

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
        MMIO_BASE
    }
}

struct VirtTimer {
    ticks: AtomicU64,
    deadline: AtomicU64,
}

impl VirtTimer {
    const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
        }
    }
}

impl HalTimer for VirtTimer {
    fn monotonic_time(&self) -> Timespec {
        Timespec::from_nanos(self.monotonic_nanos())
    }

    fn monotonic_nanos(&self) -> u64 {
        self.ticks.fetch_add(1_000_000, Ordering::Relaxed)
    }

    fn program_oneshot(&self, deadline_nanos: u64) {
        self.deadline.store(deadline_nanos, Ordering::Relaxed);
    }
}

struct Ns16550 {
    #[allow(dead_code)]
    base: usize,
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

struct VirtioBlockStub;

impl HalBlockDevice for VirtioBlockStub {
    fn name(&self) -> &'static str {
        "virtio-blk0"
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn sector_count(&self) -> usize {
        0
    }

    fn read_sector(&self, _sector: usize, _buf: &mut [u8]) -> Result<(), i32> {
        Err(95)
    }

    fn write_sector(&self, _sector: usize, _buf: &[u8]) -> Result<(), i32> {
        Err(95)
    }
}
