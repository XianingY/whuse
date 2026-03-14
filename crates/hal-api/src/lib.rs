#![cfg_attr(not(test), no_std)]

use core::fmt;
use spin::Once;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VmSpaceToken(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub start: usize,
    pub size: usize,
    pub usable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformArch {
    Riscv64,
    LoongArch64,
}

impl Timespec {
    pub const fn from_nanos(total_nanos: u64) -> Self {
        Self {
            tv_sec: (total_nanos / 1_000_000_000) as i64,
            tv_nsec: (total_nanos % 1_000_000_000) as i64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
    pub scause: usize,
    pub stval: usize,
}

impl TrapFrame {
    pub fn new_user(entry: usize, sp: usize) -> Self {
        let mut frame = Self::default();
        frame.sepc = entry;
        frame.regs[2] = sp;
        frame
    }

    pub fn a(&self, index: usize) -> usize {
        self.regs[10 + index]
    }

    pub fn set_a(&mut self, index: usize, value: usize) {
        self.regs[10 + index] = value;
    }

    pub fn syscall_number(&self) -> usize {
        self.regs[17]
    }

    pub fn syscall_args(&self) -> [usize; 6] {
        [
            self.regs[10],
            self.regs[11],
            self.regs[12],
            self.regs[13],
            self.regs[14],
            self.regs[15],
        ]
    }

    pub fn set_retval(&mut self, value: usize) {
        self.regs[10] = value;
    }
}

pub trait HalCpu: Send + Sync {
    fn cpu_id(&self) -> usize;
    fn enable_interrupts(&self);
    fn disable_interrupts(&self);
    fn interrupts_enabled(&self) -> bool;
    fn switch_address_space(&self, token: VmSpaceToken);
    fn wait_for_interrupt(&self);
    fn run_user(&self, frame: &mut TrapFrame);
}

pub trait HalMemory: Send + Sync {
    fn memory_regions(&self) -> &'static [MemoryRegion];
    fn phys_to_virt(&self, phys: usize) -> usize;
    fn virt_to_phys(&self, virt: usize) -> usize;
    fn mmio_base(&self) -> usize;
}

pub trait HalTimer: Send + Sync {
    fn monotonic_time(&self) -> Timespec;
    fn monotonic_nanos(&self) -> u64;
    fn program_oneshot(&self, deadline_nanos: u64);
}

pub trait HalInterrupt: Send + Sync {
    fn name(&self) -> &'static str;
    fn enable_irq(&self, irq: usize);
    fn disable_irq(&self, irq: usize);
    fn ack_irq(&self, irq: usize);
    fn next_pending(&self) -> Option<usize>;
}

pub trait HalBlockDevice: Send + Sync {
    fn name(&self) -> &'static str;
    fn init(&self) -> Result<(), i32> {
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.sector_count() != 0
    }
    fn sector_size(&self) -> usize;
    fn sector_count(&self) -> usize;
    fn irq_line(&self) -> Option<usize> {
        None
    }
    fn ack_interrupt(&self) -> bool {
        false
    }
    fn read_sector(&self, sector: usize, buf: &mut [u8]) -> Result<(), i32>;
    fn read_sectors(&self, start_sector: usize, buf: &mut [u8]) -> Result<(), i32> {
        let sector_size = self.sector_size();
        if sector_size == 0 || buf.len() % sector_size != 0 {
            return Err(22);
        }
        for (index, chunk) in buf.chunks_exact_mut(sector_size).enumerate() {
            self.read_sector(start_sector + index, chunk)?;
        }
        Ok(())
    }
    fn write_sector(&self, sector: usize, buf: &[u8]) -> Result<(), i32>;
    fn flush(&self) -> Result<(), i32> {
        Ok(())
    }
}

pub trait HalCharDevice: Send + Sync {
    fn name(&self) -> &'static str;
    fn put_byte(&self, byte: u8);
    fn get_byte(&self) -> Option<u8>;
}

pub trait HalNetDevice: Send + Sync {
    fn name(&self) -> &'static str;
    fn init(&self) -> Result<(), i32> {
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.can_send() || self.can_recv()
    }
    fn mac_address(&self) -> [u8; 6];
    fn mtu(&self) -> usize;
    fn can_send(&self) -> bool;
    fn can_recv(&self) -> bool;
    fn send_frame(&self, frame: &[u8]) -> Result<usize, i32>;
    fn recv_frame(&self, frame: &mut [u8]) -> Result<usize, i32>;
    fn poll(&self) -> Result<(), i32> {
        Ok(())
    }
}

pub trait HalPlatform: Send + Sync {
    fn platform_name(&self) -> &'static str;
    fn architecture(&self) -> PlatformArch;
}

pub trait HalPlatformLifecycle: Send + Sync {
    fn supports_userspace(&self) -> bool;
    fn idle(&self) -> !;
}

pub struct HalBundle {
    pub platform: &'static dyn HalPlatform,
    pub lifecycle: &'static dyn HalPlatformLifecycle,
    pub interrupt: &'static dyn HalInterrupt,
    pub cpu: &'static dyn HalCpu,
    pub memory: &'static dyn HalMemory,
    pub timer: &'static dyn HalTimer,
    pub console: &'static dyn HalCharDevice,
    pub block_devices: &'static [&'static dyn HalBlockDevice],
    pub net_devices: &'static [&'static dyn HalNetDevice],
}

static HAL_BUNDLE: Once<HalBundle> = Once::new();

pub fn register_hal(bundle: HalBundle) -> &'static HalBundle {
    HAL_BUNDLE.call_once(|| bundle)
}

pub fn hal() -> &'static HalBundle {
    HAL_BUNDLE
        .get()
        .expect("HAL bundle must be registered before kernel bootstrap")
}

pub struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            hal().console.put_byte(byte);
        }
        Ok(())
    }
}
