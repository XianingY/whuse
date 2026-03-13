#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr;
use hal_api::{HalMemory, MemoryRegion, VmSpaceToken};
use spin::Mutex;

pub type KernelResult<T> = Result<T, i32>;

const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENOEXEC: i32 = 8;
const ENOMEM: i32 = 12;

const USER_HEAP_BASE: usize = 0x4000_0000;
const USER_STACK_TOP: usize = 0x8000_0000;
const USER_STACK_SIZE: usize = 0x20_000;
const USER_MMAP_BASE: usize = 0x5000_0000;
const DEFAULT_PROT: usize = 0b11;
const PAGE_SIZE: usize = 4096;

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
        let usable = regions
            .iter()
            .find(|region| region.usable)
            .copied()
            .unwrap_or(MemoryRegion {
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
        let next = page.checked_add(PAGE_SIZE)?;
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
enum SegmentStorage {
    Owned(Vec<u8>),
    Host { ptr: usize, len: usize },
}

#[derive(Clone, Debug)]
struct Segment {
    area: MappingArea,
    storage: SegmentStorage,
}

#[derive(Clone, Debug)]
struct AddressSpaceInner {
    token: VmSpaceToken,
    mappings: BTreeMap<usize, Segment>,
    program_break: usize,
    next_mapping_base: usize,
}

#[derive(Clone, Debug)]
pub struct AddressSpace {
    inner: alloc::sync::Arc<Mutex<AddressSpaceInner>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoadedImage {
    pub entry: usize,
    pub stack_pointer: usize,
}

pub trait BinaryLoader {
    fn load(
        &self,
        address_space: &AddressSpace,
        image: &[u8],
        args: &[String],
        envs: &[String],
    ) -> KernelResult<LoadedImage>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ElfBinaryLoader;

impl ElfBinaryLoader {
    pub const fn new() -> Self {
        Self
    }
}

impl BinaryLoader for ElfBinaryLoader {
    fn load(
        &self,
        address_space: &AddressSpace,
        image: &[u8],
        args: &[String],
        envs: &[String],
    ) -> KernelResult<LoadedImage> {
        address_space.load_elf_image(image, args, envs)
    }
}

impl AddressSpace {
    pub fn new_user() -> Self {
        Self {
            inner: alloc::sync::Arc::new(Mutex::new(AddressSpaceInner {
                token: VmSpaceToken(0),
                mappings: BTreeMap::new(),
                program_break: USER_HEAP_BASE,
                next_mapping_base: USER_MMAP_BASE,
            })),
        }
    }

    pub fn token(&self) -> VmSpaceToken {
        self.inner.lock().token
    }

    pub fn map_anonymous(&self, len: usize, prot: usize) -> KernelResult<usize> {
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let start = {
            let mut inner = self.inner.lock();
            let start = inner.next_mapping_base;
            inner.next_mapping_base = inner.next_mapping_base.checked_add(aligned).ok_or(ENOMEM)?;
            start
        };
        self.map_owned(start, vec![0; aligned], prot)?;
        Ok(start)
    }

    pub fn map_fixed_bytes(
        &self,
        addr: usize,
        bytes: &[u8],
        mem_len: usize,
        prot: usize,
    ) -> KernelResult<()> {
        if mem_len == 0 || bytes.len() > mem_len {
            return Err(EINVAL);
        }
        let mut buffer = vec![0; mem_len];
        buffer[..bytes.len()].copy_from_slice(bytes);
        self.map_owned(addr, buffer, prot)
    }

    pub fn install_bytes(&self, addr: usize, bytes: &[u8]) {
        if self.write_bytes(addr, bytes).is_err() {
            let _ = self.map_fixed_bytes(addr, bytes, bytes.len().max(1), DEFAULT_PROT);
        }
    }

    pub fn install_host_range(&self, addr: usize, len: usize, prot: usize) -> KernelResult<()> {
        if len == 0 {
            return Err(EINVAL);
        }
        self.insert_segment(Segment {
            area: MappingArea {
                start: addr,
                len,
                prot,
            },
            storage: SegmentStorage::Host { ptr: addr, len },
        })
    }

    pub fn unmap(&self, addr: usize, len: usize) -> KernelResult<()> {
        let aligned = align_up(len, PAGE_SIZE);
        let mut inner = self.inner.lock();
        let Some(segment) = inner.mappings.get(&addr) else {
            return Err(EINVAL);
        };
        if segment.area.len != aligned {
            return Err(EINVAL);
        }
        inner.mappings.remove(&addr);
        Ok(())
    }

    pub fn mprotect(&self, addr: usize, len: usize, prot: usize) -> KernelResult<()> {
        let aligned = align_up(len, PAGE_SIZE);
        let mut inner = self.inner.lock();
        let Some(segment) = inner.mappings.get_mut(&addr) else {
            return Err(EINVAL);
        };
        if segment.area.len != aligned {
            return Err(EINVAL);
        }
        segment.area.prot = prot;
        Ok(())
    }

    pub fn brk(&self, new_break: Option<usize>) -> KernelResult<usize> {
        let mut inner = self.inner.lock();
        if let Some(new_break) = new_break {
            if new_break < USER_HEAP_BASE {
                return Err(EINVAL);
            }
            inner.program_break = align_up(new_break, 16);
        }
        Ok(inner.program_break)
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.mappings.clear();
        inner.program_break = USER_HEAP_BASE;
        inner.next_mapping_base = USER_MMAP_BASE;
    }

    pub fn read_bytes(&self, addr: usize, len: usize) -> KernelResult<Vec<u8>> {
        let inner = self.inner.lock();
        let (segment, offset) = find_segment(&inner.mappings, addr, len)?;
        match &segment.storage {
            SegmentStorage::Owned(bytes) => Ok(bytes[offset..offset + len].to_vec()),
            SegmentStorage::Host { ptr, .. } => unsafe {
                Ok(core::slice::from_raw_parts((ptr + offset) as *const u8, len).to_vec())
            },
        }
    }

    pub fn write_bytes(&self, addr: usize, bytes: &[u8]) -> KernelResult<()> {
        let mut inner = self.inner.lock();
        let (segment, offset) = find_segment_mut(&mut inner.mappings, addr, bytes.len())?;
        match &mut segment.storage {
            SegmentStorage::Owned(buffer) => {
                buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
            SegmentStorage::Host { ptr, len } => {
                if offset + bytes.len() > *len {
                    return Err(EFAULT);
                }
                unsafe {
                    ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (*ptr + offset) as *mut u8,
                        bytes.len(),
                    );
                }
                Ok(())
            }
        }
    }

    pub fn read_cstr(&self, addr: usize) -> KernelResult<String> {
        let mut out = Vec::new();
        for offset in 0..PAGE_SIZE {
            let byte = self.read_bytes(addr + offset, 1)?[0];
            if byte == 0 {
                return String::from_utf8(out).map_err(|_| EFAULT);
            }
            out.push(byte);
        }
        Err(EFAULT)
    }

    pub fn clone_private(&self) -> Self {
        let inner = self.inner.lock();
        let mut mappings = BTreeMap::new();
        for (start, segment) in &inner.mappings {
            let storage = match &segment.storage {
                SegmentStorage::Owned(bytes) => SegmentStorage::Owned(bytes.clone()),
                SegmentStorage::Host { ptr, len } => unsafe {
                    SegmentStorage::Owned(
                        core::slice::from_raw_parts(*ptr as *const u8, *len).to_vec(),
                    )
                },
            };
            mappings.insert(
                *start,
                Segment {
                    area: segment.area.clone(),
                    storage,
                },
            );
        }
        Self {
            inner: alloc::sync::Arc::new(Mutex::new(AddressSpaceInner {
                token: inner.token,
                mappings,
                program_break: inner.program_break,
                next_mapping_base: inner.next_mapping_base,
            })),
        }
    }

    pub fn load_static_elf(
        &self,
        image: &[u8],
        args: &[String],
        envs: &[String],
    ) -> KernelResult<LoadedImage> {
        ElfBinaryLoader::new().load(self, image, args, envs)
    }

    pub fn load_elf_image(
        &self,
        image: &[u8],
        args: &[String],
        envs: &[String],
    ) -> KernelResult<LoadedImage> {
        let header = ElfHeader::parse(image)?;
        if header.program_header_size != 56 || header.class != 2 || header.endianness != 1 {
            return Err(ENOEXEC);
        }
        if header.program_header_num == 0 {
            return Err(ENOEXEC);
        }

        self.clear();
        let mut highest_end = USER_HEAP_BASE;
        for index in 0..header.program_header_num {
            let offset = header.program_header_offset + index * header.program_header_size;
            let ph = ProgramHeader::parse(image, offset)?;
            if ph.segment_type != 1 {
                if ph.segment_type == 3 {
                    return Err(ENOEXEC);
                }
                continue;
            }
            if ph.file_size > ph.mem_size {
                return Err(ENOEXEC);
            }
            let data_end = ph.offset.checked_add(ph.file_size).ok_or(ENOEXEC)?;
            if data_end > image.len() {
                return Err(ENOEXEC);
            }
            let bytes = &image[ph.offset..data_end];
            self.map_fixed_bytes(ph.vaddr, bytes, ph.mem_size, ph.flags)?;
            highest_end = highest_end.max(align_up(ph.vaddr + ph.mem_size, PAGE_SIZE));
        }

        {
            let mut inner = self.inner.lock();
            inner.program_break = highest_end.max(USER_HEAP_BASE);
        }
        let stack_base = USER_STACK_TOP - USER_STACK_SIZE;
        let stack_image = build_initial_stack(args, envs, stack_base, USER_STACK_TOP)?;
        self.map_fixed_bytes(stack_base, &stack_image, USER_STACK_SIZE, DEFAULT_PROT)?;
        Ok(LoadedImage {
            entry: header.entry,
            stack_pointer: USER_STACK_TOP - stack_image.len(),
        })
    }

    fn map_owned(&self, addr: usize, bytes: Vec<u8>, prot: usize) -> KernelResult<()> {
        self.insert_segment(Segment {
            area: MappingArea {
                start: addr,
                len: bytes.len().max(1),
                prot,
            },
            storage: SegmentStorage::Owned(bytes),
        })
    }

    fn insert_segment(&self, segment: Segment) -> KernelResult<()> {
        let mut inner = self.inner.lock();
        if overlaps(&inner.mappings, segment.area.start, segment.area.len) {
            let overlapping = inner
                .mappings
                .iter()
                .filter_map(|(base, existing)| {
                    let end = segment.area.start + segment.area.len;
                    let existing_end = *base + existing.area.len;
                    (segment.area.start < existing_end && *base < end).then_some(*base)
                })
                .collect::<Vec<_>>();
            for base in overlapping {
                inner.mappings.remove(&base);
            }
        }
        inner.mappings.insert(segment.area.start, segment);
        Ok(())
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

#[derive(Clone, Copy, Debug)]
struct ElfHeader {
    class: u8,
    endianness: u8,
    entry: usize,
    program_header_offset: usize,
    program_header_size: usize,
    program_header_num: usize,
}

impl ElfHeader {
    fn parse(image: &[u8]) -> KernelResult<Self> {
        if image.len() < 64 || &image[..4] != b"\x7fELF" {
            return Err(ENOEXEC);
        }
        Ok(Self {
            class: image[4],
            endianness: image[5],
            entry: read_u64(image, 24)? as usize,
            program_header_offset: read_u64(image, 32)? as usize,
            program_header_size: read_u16(image, 54)? as usize,
            program_header_num: read_u16(image, 56)? as usize,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct ProgramHeader {
    segment_type: u32,
    flags: usize,
    offset: usize,
    vaddr: usize,
    file_size: usize,
    mem_size: usize,
}

impl ProgramHeader {
    fn parse(image: &[u8], offset: usize) -> KernelResult<Self> {
        if offset + 56 > image.len() {
            return Err(ENOEXEC);
        }
        Ok(Self {
            segment_type: read_u32(image, offset)?,
            flags: read_u32(image, offset + 4)? as usize,
            offset: read_u64(image, offset + 8)? as usize,
            vaddr: read_u64(image, offset + 16)? as usize,
            file_size: read_u64(image, offset + 32)? as usize,
            mem_size: read_u64(image, offset + 40)? as usize,
        })
    }
}

fn overlaps(mappings: &BTreeMap<usize, Segment>, start: usize, len: usize) -> bool {
    mappings.iter().any(|(base, segment)| {
        let end = start + len;
        let seg_end = *base + segment.area.len;
        start < seg_end && *base < end
    })
}

fn find_segment(
    mappings: &BTreeMap<usize, Segment>,
    addr: usize,
    len: usize,
) -> KernelResult<(&Segment, usize)> {
    mappings
        .iter()
        .find(|(base, segment)| **base <= addr && addr + len <= **base + segment.area.len)
        .map(|(base, segment)| (segment, addr - *base))
        .ok_or(EFAULT)
}

fn find_segment_mut(
    mappings: &mut BTreeMap<usize, Segment>,
    addr: usize,
    len: usize,
) -> KernelResult<(&mut Segment, usize)> {
    mappings
        .iter_mut()
        .find(|(base, segment)| **base <= addr && addr + len <= **base + segment.area.len)
        .map(|(base, segment)| (segment, addr - *base))
        .ok_or(EFAULT)
}

fn build_initial_stack(
    args: &[String],
    envs: &[String],
    stack_base: usize,
    stack_top: usize,
) -> KernelResult<Vec<u8>> {
    let pointer_size = size_of::<usize>();
    let strings = args.iter().chain(envs.iter()).collect::<Vec<_>>();
    let total_strings_len = strings.iter().map(|entry| entry.len() + 1).sum::<usize>();
    let pointer_count = 1 + args.len() + 1 + envs.len() + 1;
    let mut stack = vec![0u8; align_up(total_strings_len + pointer_count * pointer_size, 16)];

    let mut string_cursor = stack.len();
    let mut pointers = Vec::with_capacity(pointer_count);
    for entry in args {
        string_cursor -= entry.len() + 1;
        stack[string_cursor..string_cursor + entry.len()].copy_from_slice(entry.as_bytes());
        pointers.push(stack_top - (stack.len() - string_cursor));
    }
    pointers.push(0);
    for entry in envs {
        string_cursor -= entry.len() + 1;
        stack[string_cursor..string_cursor + entry.len()].copy_from_slice(entry.as_bytes());
        pointers.push(stack_top - (stack.len() - string_cursor));
    }
    pointers.push(0);

    let argc = args.len();
    let mut head = vec![0u8; pointer_size * (1 + pointers.len())];
    head[..pointer_size].copy_from_slice(&argc.to_le_bytes()[..pointer_size]);
    for (index, value) in pointers.iter().enumerate() {
        let start = pointer_size * (index + 1);
        head[start..start + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
    }

    let total = head.len() + (stack.len() - string_cursor);
    if total > USER_STACK_SIZE || stack_base >= stack_top {
        return Err(ENOMEM);
    }
    let mut out = vec![0u8; USER_STACK_SIZE];
    let start = USER_STACK_SIZE - total;
    out[start..start + head.len()].copy_from_slice(&head);
    out[start + head.len()..].copy_from_slice(&stack[string_cursor..]);
    Ok(out)
}

fn read_u16(bytes: &[u8], offset: usize) -> KernelResult<u16> {
    if offset + 2 > bytes.len() {
        return Err(ENOEXEC);
    }
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> KernelResult<u32> {
    if offset + 4 > bytes.len() {
        return Err(ENOEXEC);
    }
    Ok(u32::from_le_bytes(
        bytes[offset..offset + 4].try_into().unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> KernelResult<u64> {
    if offset + 8 > bytes.len() {
        return Err(ENOEXEC);
    }
    Ok(u64::from_le_bytes(
        bytes[offset..offset + 8].try_into().unwrap(),
    ))
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::{AddressSpace, EFAULT, ENOEXEC};

    const TEST_ELF: &[u8] = &[
        0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0xf3, 0, 1, 0, 0, 0,
        0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 64, 0, 56, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0, 0x78, 0, 0, 0, 0, 0, 0,
        0, 0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 4, 0, 0, 0, 0,
        0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0x13, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn address_space_round_trip() {
        let aspace = AddressSpace::new_user();
        aspace.install_bytes(0x1000, b"hello\0");
        assert_eq!(aspace.read_cstr(0x1000).unwrap(), "hello");

        let addr = aspace.map_anonymous(8192, 0).unwrap();
        aspace.write_bytes(addr, b"abc").unwrap();
        assert_eq!(aspace.read_bytes(addr, 3).unwrap(), b"abc");
        aspace.unmap(addr, 8192).unwrap();
    }

    #[test]
    fn unmapped_access_returns_efault() {
        let aspace = AddressSpace::new_user();
        assert_eq!(aspace.read_bytes(0xdead_beef, 4).unwrap_err(), EFAULT);
    }

    #[test]
    fn load_minimal_static_elf() {
        let aspace = AddressSpace::new_user();
        let loaded = aspace
            .load_static_elf(
                TEST_ELF,
                &[String::from("/bin/test")],
                &[String::from("A=B")],
            )
            .unwrap();
        assert_eq!(loaded.entry, 0x4000_1000);
        assert_eq!(aspace.read_bytes(0x4000_1000, 4).unwrap(), &[0x13, 0, 0, 0]);
    }

    #[test]
    fn reject_non_elf_images() {
        let aspace = AddressSpace::new_user();
        assert_eq!(
            aspace.load_static_elf(b"nope", &[], &[]).unwrap_err(),
            ENOEXEC
        );
    }
}
