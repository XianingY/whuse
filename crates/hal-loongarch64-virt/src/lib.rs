#![cfg_attr(not(test), no_std)]

#[cfg(target_arch = "loongarch64")]
use core::arch::global_asm;
#[cfg(target_arch = "loongarch64")]
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalInterrupt, HalMemory,
    HalNetDevice, HalPlatform, HalPlatformLifecycle, HalTimer, MemoryRegion, PlatformArch,
    Timespec, TrapFrame, VmSpaceToken,
};

#[cfg(target_arch = "loongarch64")]
#[no_mangle]
static mut __whuse_current_frame: usize = 0;

#[cfg(target_arch = "loongarch64")]
#[no_mangle]
static mut __whuse_kernel_ra: usize = 0;

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

    ld.d $t0, $a0, 80
    csrwr $t0, 0x30
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
    csrwr $t0, 0x1
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

    la.local $t0, __whuse_kernel_ra
    ld.d $ra, $t0, 0
    jirl $zero, $ra, 0
"#
);

#[cfg(target_arch = "loongarch64")]
unsafe extern "C" {
    fn __whuse_run_user(frame: *mut TrapFrame);
}

pub const DMWIN_UNCACHED_BASE: usize = 0x8000_0000_0000_0000;
pub const DMWIN_CACHED_BASE: usize = 0x9000_0000_0000_0000;
pub const UART0_PHYS_BASE: usize = 0x1fe0_01e0;
pub const MMIO_PHYS_BASE: usize = 0x1000_0000;
pub const UART0_BASE: usize = UART0_PHYS_BASE;
pub const PHYS_MEM_BASE: usize = 0x9000_0000;
pub const PHYS_MEM_SIZE: usize = 128 * 1024 * 1024;

static CPU: VirtCpu = VirtCpu::new();
static INTERRUPT: VirtInterruptController = VirtInterruptController;
static PLATFORM: VirtPlatform = VirtPlatform;
static LIFECYCLE: VirtLifecycle = VirtLifecycle;
static MEMORY: VirtMemory = VirtMemory;
static TIMER: VirtTimer = VirtTimer::new();
static UART: Ns16550 = Ns16550::new(UART0_BASE);
static VIRTIO_BLK: VirtioBlockStub = VirtioBlockStub;
static VIRTIO_NET: VirtioNetStub = VirtioNetStub;
static BLOCK_DEVS: [&'static dyn HalBlockDevice; 1] = [&VIRTIO_BLK];
static NET_DEVS: [&'static dyn HalNetDevice; 1] = [&VIRTIO_NET];

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

pub fn bootstrap() {
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

struct VirtInterruptController;
struct VirtPlatform;
struct VirtLifecycle;

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

impl HalInterrupt for VirtInterruptController {
    fn name(&self) -> &'static str {
        "platic-stub"
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
        }
        #[cfg(not(target_arch = "loongarch64"))]
        {
            frame.scause = usize::MAX;
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
        MMIO_PHYS_BASE
    }
}

struct VirtTimer {
    ticks: AtomicU64,
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
        self.ticks.fetch_add(1_000_000, Ordering::Relaxed)
    }

    fn program_oneshot(&self, _deadline_nanos: u64) {}
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

struct VirtioNetStub;

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
