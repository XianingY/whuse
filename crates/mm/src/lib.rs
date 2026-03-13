#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use hal_api::{HalMemory, MemoryRegion, VmSpaceToken};

pub type KernelResult<T> = Result<T, i32>;

const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;

#[derive(Clone, Debug)]
pub struct MappingArea {
    pub start: usize,
    pub len: usize,
    pub prot: usize,
}

#[derive(Clone, Debug)]
pub struct FrameAllocator {
    start: usize,
    end: usize,
    next: usize,
}

impl FrameAllocator {
    pub fn from_regions(regions: &[MemoryRegion]) -> Self {
        let usable = regions.iter().find(|region| region.usable).copied().unwrap_or(MemoryRegion {
            start: 0,
            size: 0,
            usable: false,
        });
        Self {
            start: usable.start,
            end: usable.start + usable.size,
            next: usable.start,
        }
    }

    pub fn alloc_page(&mut self) -> Option<usize> {
        let page = self.next;
        let next = page.checked_add(4096)?;
        if next > self.end {
            return None;
        }
        self.next = next;
        Some(page)
    }

    pub fn used_bytes(&self) -> usize {
        self.next.saturating_sub(self.start)
    }
}

#[derive(Clone, Debug)]
pub struct AddressSpace {
    token: VmSpaceToken,
    mappings: Vec<MappingArea>,
    buffers: BTreeMap<usize, Vec<u8>>,
    program_break: usize,
    next_mapping_base: usize,
}

impl AddressSpace {
    pub fn new_user() -> Self {
        Self {
            token: VmSpaceToken(0),
            mappings: Vec::new(),
            buffers: BTreeMap::new(),
            program_break: 0x4000_0000,
            next_mapping_base: 0x5000_0000,
        }
    }

    pub fn token(&self) -> VmSpaceToken {
        self.token
    }

    pub fn map_anonymous(&mut self, len: usize, prot: usize) -> KernelResult<usize> {
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, 4096);
        let start = self.next_mapping_base;
        self.next_mapping_base = self.next_mapping_base.checked_add(aligned).ok_or(ENOMEM)?;
        self.mappings.push(MappingArea { start, len: aligned, prot });
        self.buffers.insert(start, vec![0; aligned]);
        Ok(start)
    }

    pub fn unmap(&mut self, addr: usize, len: usize) -> KernelResult<()> {
        let index = self
            .mappings
            .iter()
            .position(|mapping| mapping.start == addr && mapping.len == align_up(len, 4096))
            .ok_or(EINVAL)?;
        self.mappings.remove(index);
        self.buffers.remove(&addr);
        Ok(())
    }

    pub fn mprotect(&mut self, addr: usize, len: usize, prot: usize) -> KernelResult<()> {
        let mapping = self
            .mappings
            .iter_mut()
            .find(|mapping| mapping.start == addr && mapping.len == align_up(len, 4096))
            .ok_or(EINVAL)?;
        mapping.prot = prot;
        Ok(())
    }

    pub fn brk(&mut self, new_break: Option<usize>) -> KernelResult<usize> {
        if let Some(new_break) = new_break {
            if new_break < 0x4000_0000 {
                return Err(EINVAL);
            }
            self.program_break = align_up(new_break, 16);
        }
        Ok(self.program_break)
    }

    pub fn install_bytes(&mut self, addr: usize, bytes: &[u8]) {
        self.buffers.insert(addr, bytes.to_vec());
    }

    pub fn read_bytes(&self, addr: usize, len: usize) -> KernelResult<Vec<u8>> {
        let (base, segment) = self.find_segment(addr, len)?;
        let offset = addr - base;
        Ok(segment[offset..offset + len].to_vec())
    }

    pub fn write_bytes(&mut self, addr: usize, bytes: &[u8]) -> KernelResult<()> {
        match self.find_segment_mut(addr, bytes.len()) {
            Ok((base, segment)) => {
                let offset = addr - base;
                segment[offset..offset + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
            Err(_) => {
                self.buffers.insert(addr, bytes.to_vec());
                Ok(())
            }
        }
    }

    pub fn read_cstr(&self, addr: usize) -> KernelResult<String> {
        let (base, segment) = self.find_segment(addr, 1)?;
        let mut offset = addr - base;
        let mut out = Vec::new();
        while offset < segment.len() {
            let byte = segment[offset];
            if byte == 0 {
                return String::from_utf8(out).map_err(|_| EFAULT);
            }
            out.push(byte);
            offset += 1;
        }
        Err(EFAULT)
    }

    fn find_segment(&self, addr: usize, len: usize) -> KernelResult<(usize, &Vec<u8>)> {
        self.buffers
            .iter()
            .find(|(base, segment)| **base <= addr && addr + len <= **base + segment.len())
            .map(|(base, segment)| (*base, segment))
            .ok_or(EFAULT)
    }

    fn find_segment_mut(&mut self, addr: usize, len: usize) -> KernelResult<(usize, &mut Vec<u8>)> {
        self.buffers
            .iter_mut()
            .find(|(base, segment)| **base <= addr && addr + len <= **base + segment.len())
            .map(|(base, segment)| (*base, segment))
            .ok_or(EFAULT)
    }
}

#[derive(Clone, Debug)]
pub struct MemoryManager {
    frame_allocator: FrameAllocator,
}

impl MemoryManager {
    pub fn from_hal(memory: &dyn HalMemory) -> Self {
        Self {
            frame_allocator: FrameAllocator::from_regions(memory.memory_regions()),
        }
    }

    pub fn alloc_page(&mut self) -> KernelResult<usize> {
        self.frame_allocator.alloc_page().ok_or(ENOMEM)
    }

    pub fn used_bytes(&self) -> usize {
        self.frame_allocator.used_bytes()
    }
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::AddressSpace;

    #[test]
    fn address_space_round_trip() {
        let mut aspace = AddressSpace::new_user();
        aspace.install_bytes(0x1000, b"hello\0");
        assert_eq!(aspace.read_cstr(0x1000).unwrap(), "hello");

        let addr = aspace.map_anonymous(8192, 0).unwrap();
        aspace.write_bytes(addr, b"abc").unwrap();
        assert_eq!(aspace.read_bytes(addr, 3).unwrap(), b"abc");
        aspace.unmap(addr, 8192).unwrap();
    }
}

