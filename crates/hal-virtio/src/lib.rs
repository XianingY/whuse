#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;
use core::ptr::NonNull;
use fdt::Fdt;
use fdt::properties::interrupts::pci::{PciAddress, PciAddressSpace};
use spin::Mutex;
use virtio_drivers::Error as VirtioError;

pub const DMA_PAGE_SIZE: usize = virtio_drivers::PAGE_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportKind {
    Mmio,
    Pci,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtioMmioConfig {
    pub base: usize,
    pub size: usize,
    pub irq: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtioPlicConfig {
    pub base: usize,
    pub size: usize,
    pub supervisor_context: usize,
    pub sources: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciHostWindow {
    pub base: usize,
    pub size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciHostConfig {
    pub ecam_base: usize,
    pub ecam_size: usize,
    pub bus_start: u8,
    pub bus_end: u8,
    pub io: Option<PciHostWindow>,
    pub mmio: Option<PciHostWindow>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoongArchInterruptConfig {
    pub pch_pic_base: usize,
    pub pch_pic_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiscvVirtioDiscovery {
    pub plic: Option<VirtioPlicConfig>,
    pub mmio_devices: [Option<VirtioMmioConfig>; 8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoongArchVirtioDiscovery {
    pub pci_host: Option<PciHostConfig>,
    pub interrupt: Option<LoongArchInterruptConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtioBlockConfig {
    pub transport: TransportKind,
    pub irq: Option<usize>,
    pub capacity_sectors: usize,
    pub readonly: bool,
}

#[repr(align(4096))]
struct DmaStorage<const BYTES: usize>([u8; BYTES]);

pub struct VirtioDmaArena<const BYTES: usize, const WORDS: usize> {
    storage: UnsafeCell<DmaStorage<BYTES>>,
    bitmap: Mutex<[u64; WORDS]>,
}

unsafe impl<const BYTES: usize, const WORDS: usize> Sync for VirtioDmaArena<BYTES, WORDS> {}

impl<const BYTES: usize, const WORDS: usize> VirtioDmaArena<BYTES, WORDS> {
    pub const fn new() -> Self {
        Self {
            storage: UnsafeCell::new(DmaStorage([0; BYTES])),
            bitmap: Mutex::new([0; WORDS]),
        }
    }

    pub fn alloc(&self, pages: usize) -> Option<(u64, NonNull<u8>)> {
        if pages == 0 {
            return None;
        }
        let total_pages = BYTES / DMA_PAGE_SIZE;
        if pages > total_pages || WORDS * 64 < total_pages {
            return None;
        }
        let mut bitmap = self.bitmap.lock();
        let mut run_start = 0usize;
        let mut run_len = 0usize;
        for page in 0..total_pages {
            if bit_is_set(&bitmap[..], page) {
                run_start = page + 1;
                run_len = 0;
                continue;
            }
            run_len += 1;
            if run_len == pages {
                set_range(&mut bitmap[..], run_start, pages, true);
                let offset = run_start * DMA_PAGE_SIZE;
                let ptr = unsafe { self.base_ptr().add(offset) };
                unsafe {
                    core::ptr::write_bytes(ptr, 0, pages * DMA_PAGE_SIZE);
                }
                return Some((ptr as usize as u64, NonNull::new(ptr)?));
            }
        }
        None
    }

    pub fn dealloc(&self, paddr: u64, pages: usize) -> i32 {
        if pages == 0 {
            return 0;
        }
        let base = self.base_ptr() as usize;
        let end = base + BYTES;
        let ptr = paddr as usize;
        if ptr < base || ptr >= end || ptr % DMA_PAGE_SIZE != 0 {
            return -1;
        }
        let offset = ptr - base;
        if offset + pages * DMA_PAGE_SIZE > BYTES {
            return -1;
        }
        let mut bitmap = self.bitmap.lock();
        set_range(
            &mut bitmap[..],
            offset / DMA_PAGE_SIZE,
            pages,
            false,
        );
        0
    }

    pub fn contains(&self, paddr: u64) -> bool {
        let base = self.base_ptr() as usize;
        let end = base + BYTES;
        let ptr = paddr as usize;
        ptr >= base && ptr < end
    }

    pub fn virt_to_phys(&self, ptr: NonNull<u8>) -> u64 {
        ptr.as_ptr() as usize as u64
    }

    fn base_ptr(&self) -> *mut u8 {
        unsafe { (*self.storage.get()).0.as_ptr() as *mut u8 }
    }
}

pub fn parse_riscv_virtio_discovery(dtb_pa: usize) -> Option<RiscvVirtioDiscovery> {
    let fdt = unsafe { Fdt::from_ptr_unaligned(dtb_pa as *const u8).ok()? };
    let mut discovery = RiscvVirtioDiscovery {
        plic: None,
        mmio_devices: [None; 8],
    };
    let mut mmio_index = 0usize;
    for (_depth, node) in fdt.all_nodes() {
        let Some(compatible) = node.raw_property("compatible") else {
            continue;
        };
        if compatible_contains(compatible.value, b"virtio,mmio") {
            if mmio_index >= discovery.mmio_devices.len() {
                continue;
            }
            let Some(reg) = node.raw_property("reg") else {
                continue;
            };
            let Some(interrupts) = node.raw_property("interrupts") else {
                continue;
            };
            let base = first_u64(reg.value)?;
            let size = second_u64(reg.value)?;
            let irq = first_u32(interrupts.value)? as usize;
            discovery.mmio_devices[mmio_index] = Some(VirtioMmioConfig {
                base,
                size,
                irq,
            });
            mmio_index += 1;
            continue;
        }
        if compatible_contains(compatible.value, b"riscv,plic0")
            || compatible_contains(compatible.value, b"sifive,plic-1.0.0")
        {
            let Some(reg) = node.raw_property("reg") else {
                continue;
            };
            let base = first_u64(reg.value)?;
            let size = second_u64(reg.value)?;
            let Some(sources_prop) = node.raw_property("riscv,ndev") else {
                continue;
            };
            let Some(interrupts_extended) = node.raw_property("interrupts-extended") else {
                continue;
            };
            let sources = sources_prop.value.get(..4).map(read_be_u32)? as usize;
            let context = interrupts_extended
                .value
                .chunks_exact(8)
                .enumerate()
                .find_map(|(index, entry)| (read_be_u32(&entry[4..8]) == 9).then_some(index))
                .unwrap_or(1);
            discovery.plic = Some(VirtioPlicConfig {
                base,
                size,
                supervisor_context: context,
                sources,
            });
        }
    }
    Some(discovery)
}

pub fn parse_loongarch_virtio_discovery(dtb_pa: usize) -> Option<LoongArchVirtioDiscovery> {
    let fdt = unsafe { Fdt::from_ptr_unaligned(dtb_pa as *const u8).ok()? };
    let mut discovery = LoongArchVirtioDiscovery {
        pci_host: None,
        interrupt: None,
    };
    for (_depth, node) in fdt.all_nodes() {
        if discovery.pci_host.is_none() {
            let compatible = node.raw_property("compatible");
            let device_type = node.raw_property("device_type");
            if compatible.is_some_and(|prop| compatible_contains(prop.value, b"pci-host-ecam-generic"))
                && device_type.is_some_and(|prop| compatible_contains(prop.value, b"pci"))
            {
                let Some(reg) = node.raw_property("reg") else {
                    continue;
                };
                let ecam_base = first_u64(reg.value)?;
                let ecam_size = second_u64(reg.value)?;
                let Some(bus_range) = node.raw_property("bus-range") else {
                    continue;
                };
                let bus_start = first_u32(bus_range.value)? as u8;
                let bus_end = second_u32(bus_range.value)? as u8;
                let mut io = None;
                let mut mmio = None;
                if let Some(ranges) = node.ranges() {
                    for range in ranges.iter::<PciAddress, u64, u64>() {
                        let Ok(range) = range else {
                            continue;
                        };
                        match range.child_bus_address.hi.address_space() {
                            PciAddressSpace::Io => {
                                io = Some(PciHostWindow {
                                    base: range.parent_bus_address as usize,
                                    size: range.len as usize,
                                });
                            }
                            PciAddressSpace::Memory32 | PciAddressSpace::Memory64 => {
                                if mmio.is_none() {
                                    mmio = Some(PciHostWindow {
                                        base: range.parent_bus_address as usize,
                                        size: range.len as usize,
                                    });
                                }
                            }
                            PciAddressSpace::Configuration => {}
                        }
                    }
                }
                discovery.pci_host = Some(PciHostConfig {
                    ecam_base,
                    ecam_size,
                    bus_start,
                    bus_end,
                    io,
                    mmio,
                });
                continue;
            }
        }
        if discovery.interrupt.is_none() {
            let compatible = node.raw_property("compatible");
            if compatible.is_some_and(|prop| compatible_contains(prop.value, b"loongarch,ls7a")) {
                let Some(reg) = node.raw_property("reg") else {
                    continue;
                };
                discovery.interrupt = Some(LoongArchInterruptConfig {
                    pch_pic_base: first_u64(reg.value)?,
                    pch_pic_size: second_u64(reg.value)?,
                });
            }
        }
    }
    Some(discovery)
}

pub fn virtio_error_to_errno(err: VirtioError) -> i32 {
    match err {
        VirtioError::QueueFull | VirtioError::NotReady => 11,
        VirtioError::AlreadyUsed => 16,
        VirtioError::InvalidParam | VirtioError::ConfigSpaceTooSmall | VirtioError::ConfigSpaceMissing => 22,
        VirtioError::DmaError => 12,
        VirtioError::IoError | VirtioError::WrongToken | VirtioError::SocketDeviceError(_) => 5,
        VirtioError::Unsupported => 95,
    }
}

fn compatible_contains(value: &[u8], needle: &[u8]) -> bool {
    value.split(|byte| *byte == 0).any(|part| part == needle)
}

fn first_u32(bytes: &[u8]) -> Option<u32> {
    bytes.get(..4).map(read_be_u32)
}

fn second_u32(bytes: &[u8]) -> Option<u32> {
    bytes.get(4..8).map(read_be_u32)
}

fn first_u64(bytes: &[u8]) -> Option<usize> {
    bytes.get(..8).map(read_be_u64)
}

fn second_u64(bytes: &[u8]) -> Option<usize> {
    bytes.get(8..16).map(read_be_u64)
}

fn read_be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().unwrap())
}

fn read_be_u64(bytes: &[u8]) -> usize {
    u64::from_be_bytes(bytes.try_into().unwrap()) as usize
}

fn bit_is_set(bitmap: &[u64], index: usize) -> bool {
    let word = index / 64;
    let bit = index % 64;
    bitmap
        .get(word)
        .is_some_and(|entry| (*entry & (1u64 << bit)) != 0)
}

fn set_range(bitmap: &mut [u64], start: usize, len: usize, value: bool) {
    for page in start..start + len {
        let word = page / 64;
        let bit = page % 64;
        if value {
            bitmap[word] |= 1u64 << bit;
        } else {
            bitmap[word] &= !(1u64 << bit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VirtioDmaArena;

    #[test]
    fn dma_arena_allocates_and_reuses_pages() {
        static ARENA: VirtioDmaArena<{ 8 * 4096 }, 1> = VirtioDmaArena::new();
        let (first_paddr, first_ptr) = ARENA.alloc(2).expect("first alloc");
        let (second_paddr, _) = ARENA.alloc(2).expect("second alloc");
        assert_ne!(first_paddr, second_paddr);
        assert_eq!(ARENA.virt_to_phys(first_ptr), first_paddr);
        assert_eq!(ARENA.dealloc(first_paddr, 2), 0);
        let (reused_paddr, _) = ARENA.alloc(2).expect("reused alloc");
        assert_eq!(reused_paddr, first_paddr);
    }

    #[test]
    fn dma_arena_rejects_out_of_range_free() {
        static ARENA: VirtioDmaArena<{ 4 * 4096 }, 1> = VirtioDmaArena::new();
        assert_ne!(ARENA.dealloc(0xdead_beef, 1), 0);
    }
}
