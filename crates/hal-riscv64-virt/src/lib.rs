#![cfg_attr(not(test), no_std)]

#[cfg(target_arch = "riscv64")]
use core::arch::global_asm;
#[cfg(target_arch = "riscv64")]
use core::ptr::{read_volatile, write_volatile};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use hal_api::{
    register_hal, HalBlockDevice, HalBundle, HalCharDevice, HalCpu, HalInterrupt, HalMemory,
    HalNetDevice, HalPlatform, HalPlatformLifecycle, HalTimer, MemoryRegion, PlatformArch,
    Timespec, TrapFrame, VmSpaceToken,
};
use hal_virtio::{
    RiscvVirtioDiscovery, VirtioBlockConfig, VirtioDmaArena, parse_riscv_virtio_discovery,
    virtio_error_to_errno,
};
use spin::{Mutex, Once};
use virtio_drivers::BufferDirection;
use virtio_drivers::Hal as VirtioHal;
use virtio_drivers::device::blk::{BlkReq, BlkResp, SECTOR_SIZE, VirtIOBlk};
use virtio_drivers::transport::InterruptStatus;
use virtio_drivers::transport::SomeTransport;
use virtio_drivers::transport::Transport;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};

pub const UART0_BASE: usize = 0x1000_0000;
pub const VIRTIO0_BASE: usize = 0x1000_1000;
pub const MMIO_BASE: usize = 0x1000_0000;
pub const PHYS_MEM_BASE: usize = 0x8000_0000;
pub const PHYS_MEM_SIZE: usize = 128 * 1024 * 1024;
const DMA_ARENA_BYTES: usize = 2 * 1024 * 1024;
const DMA_ARENA_WORDS: usize = DMA_ARENA_BYTES / SECTOR_SIZE / 64;
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

pub fn bootstrap(dtb_pa: usize) {
    if dtb_pa != 0 {
        if let Some(discovery) = parse_riscv_virtio_discovery(dtb_pa) {
            INTERRUPT.configure(discovery.plic);
            VIRTIO_BLK.bootstrap(&discovery);
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
        #[cfg(not(target_arch = "riscv64"))]
        core::hint::spin_loop();
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
        let transport =
            unsafe { MmioTransport::new(header, config.size) }.map_err(|_| ENODEV)?;
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
        let _ = self.state.call_once(|| Mutex::new(VirtioBlockState { driver }));
        INTERRUPT.enable_irq(config.irq);
        Ok(())
    }

    fn with_state(&self) -> Result<&Mutex<VirtioBlockState>, i32> {
        self.state
            .get()
            .ok_or_else(|| self.init_error.load(Ordering::Relaxed))
    }

    fn wait_for_completion(
        &self,
        state: &Mutex<VirtioBlockState>,
        token: u16,
    ) -> Result<(), i32> {
        let mut saw_interrupt = false;
        for _ in 0..1_000_000usize {
            if let Some(irq) = self.irq_line() {
                if let Some(pending) = INTERRUPT.next_pending() {
                    let mut guard = state.lock();
                    if pending == irq
                        && guard
                            .driver
                            .ack_interrupt()
                            .contains(InterruptStatus::QUEUE_INTERRUPT)
                    {
                        saw_interrupt = true;
                    }
                    let ready = pending == irq && saw_interrupt && guard.driver.peek_used() == Some(token);
                    INTERRUPT.ack_irq(pending);
                    if ready {
                        return Ok(());
                    }
                }
            }
            let mut guard = state.lock();
            if guard
                .driver
                .ack_interrupt()
                .contains(InterruptStatus::QUEUE_INTERRUPT)
            {
                saw_interrupt = true;
            }
            if saw_interrupt && guard.driver.peek_used() == Some(token) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(5)
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
        let state = self.with_state()?;
        let mut request = BlkReq::default();
        let mut response = BlkResp::default();
        let token = {
            let mut guard = state.lock();
            unsafe { guard.driver.read_blocks_nb(sector, &mut request, buf, &mut response) }
                .map_err(virtio_error_to_errno)?
        };
        self.wait_for_completion(state, token)?;
        let mut guard = state.lock();
        unsafe {
            guard
                .driver
                .complete_read_blocks(token, &request, buf, &mut response)
        }
        .map_err(virtio_error_to_errno)
    }

    fn write_sector(&self, sector: usize, buf: &[u8]) -> Result<(), i32> {
        if self.readonly.load(Ordering::Relaxed) {
            return Err(EROFS);
        }
        if buf.len() != SECTOR_SIZE {
            return Err(EINVAL);
        }
        let state = self.with_state()?;
        let mut request = BlkReq::default();
        let mut response = BlkResp::default();
        let token = {
            let mut guard = state.lock();
            unsafe { guard.driver.write_blocks_nb(sector, &mut request, buf, &mut response) }
                .map_err(virtio_error_to_errno)?
        };
        self.wait_for_completion(state, token)?;
        let mut guard = state.lock();
        unsafe {
            guard
                .driver
                .complete_write_blocks(token, &request, buf, &mut response)
        }
        .map_err(virtio_error_to_errno)
    }

    fn flush(&self) -> Result<(), i32> {
        let state = self.with_state()?;
        state
            .lock()
            .driver
            .flush()
            .map_err(virtio_error_to_errno)
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
        DMA_ARENA
            .alloc(pages)
            .unwrap_or((0, NonNull::dangling()))
    }

    unsafe fn dma_dealloc(
        paddr: virtio_drivers::PhysAddr,
        _vaddr: NonNull<u8>,
        pages: usize,
    ) -> i32 {
        DMA_ARENA.dealloc(paddr, pages)
    }

    unsafe fn mmio_phys_to_virt(
        paddr: virtio_drivers::PhysAddr,
        _size: usize,
    ) -> NonNull<u8> {
        NonNull::new(paddr as usize as *mut u8).unwrap()
    }

    unsafe fn share(
        buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) -> virtio_drivers::PhysAddr {
        buffer.as_ptr() as *mut u8 as usize as u64
    }

    unsafe fn unshare(
        _paddr: virtio_drivers::PhysAddr,
        _buffer: NonNull<[u8]>,
        _direction: BufferDirection,
    ) {
    }
}
