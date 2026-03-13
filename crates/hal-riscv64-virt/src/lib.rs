#![cfg_attr(not(test), no_std)]

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalMemory, HalTimer,
    MemoryRegion, Timespec, VmSpaceToken,
};

pub const UART0_BASE: usize = 0x1000_0000;
pub const VIRTIO0_BASE: usize = 0x1000_1000;
pub const MMIO_BASE: usize = 0x1000_0000;
pub const PHYS_MEM_BASE: usize = 0x8000_0000;
pub const PHYS_MEM_SIZE: usize = 128 * 1024 * 1024;

static CPU: VirtCpu = VirtCpu::new();
static MEMORY: VirtMemory = VirtMemory;
static TIMER: VirtTimer = VirtTimer::new();
static UART: Ns16550 = Ns16550::new(UART0_BASE);
static VIRTIO_BLK: VirtioBlockStub = VirtioBlockStub;
static BLOCK_DEVS: [&'static dyn HalBlockDevice; 1] = [&VIRTIO_BLK];

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

