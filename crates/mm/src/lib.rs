#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
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
const USER_STACK_SIZE: usize = 0x80_000;
const USER_STACK_TOP: usize = 0x7fff_f000;
const USER_MMAP_BASE: usize = 0x5000_0000;
const USER_MMAP_GUARD_GAP: usize = PAGE_SIZE * 512;
const DEFAULT_PROT: usize = 0b11;
const PAGE_SIZE: usize = 4096;
const DEBUG_LARGE_SEGMENT_ALLOC: bool = false;

const RISCV_PTE_V: u64 = 1 << 0;
const RISCV_PTE_R: u64 = 1 << 1;
const RISCV_PTE_W: u64 = 1 << 2;
const RISCV_PTE_X: u64 = 1 << 3;
const RISCV_PTE_U: u64 = 1 << 4;
const RISCV_PTE_A: u64 = 1 << 6;
const RISCV_PTE_D: u64 = 1 << 7;

const LA_PTE_V: u64 = 1 << 0;
const LA_PTE_D: u64 = 1 << 1;
const LA_PTE_PLVL: u64 = 1 << 2;
const LA_PTE_PLVH: u64 = 1 << 3;
const LA_PTE_MATL: u64 = 1 << 4;
const LA_PTE_MATH: u64 = 1 << 5;
const LA_PTE_GH: u64 = 1 << 6;
const LA_PTE_P: u64 = 1 << 7;
const LA_PTE_W: u64 = 1 << 8;
const LA_PTE_NR: u64 = 1 << 61;
const LA_PTE_NX: u64 = 1 << 62;
const LA_PTE_ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

const RISCV_MMIO_BASE: usize = 0x0200_0000;
const RISCV_MMIO_SIZE: usize = 0x1e00_0000;
const LOONGARCH_MMIO_BASE: usize = 0x1000_0000;
const LOONGARCH_MMIO_SIZE: usize = 0x1000_0000;
const LOONGARCH_PHYS_BASE: usize = 0x9000_0000;
const LOONGARCH_PHYS_SIZE: usize = 512 * 1024 * 1024;

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
    Owned {
        bytes: Vec<u8>,
        ptr: usize,
    },
    Shared {
        bytes: alloc::sync::Arc<Mutex<Vec<u8>>>,
        ptr: usize,
    },
    CowParent {
        bytes: alloc::sync::Arc<Mutex<Vec<u8>>>,
        ptr: usize,
    },
    Host {
        ptr: usize,
        len: usize,
    },
}

#[derive(Clone, Debug)]
struct Segment {
    area: MappingArea,
    storage: SegmentStorage,
}

#[repr(align(4096))]
#[derive(Clone, Debug)]
struct PageTablePage([u64; 512]);

impl PageTablePage {
    fn new() -> Self {
        Self([0; 512])
    }
}

#[derive(Clone, Debug)]
struct PageTableSpace {
    root_phys: usize,
    pages: Vec<alloc::boxed::Box<PageTablePage>>,
}

#[derive(Clone, Debug)]
struct AddressSpaceInner {
    token: VmSpaceToken,
    mappings: BTreeMap<usize, Segment>,
    program_break: usize,
    next_mapping_base: usize,
    page_table: Option<PageTableSpace>,
    dirty: bool,
}

#[derive(Clone, Debug)]
pub struct AddressSpace {
    inner: alloc::sync::Arc<Mutex<AddressSpaceInner>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoadedImage {
    pub entry: usize,
    pub stack_pointer: usize,
    pub load_bias: usize,
    pub phdr_addr: usize,
    pub phnum: usize,
    pub phent: usize,
    pub interp_base: usize,
    pub interp_entry: usize,
    pub program_entry: usize,
    pub is_dyn: bool,
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
        let inner = AddressSpaceInner {
            token: VmSpaceToken(0),
            mappings: BTreeMap::new(),
            program_break: USER_HEAP_BASE,
            next_mapping_base: USER_MMAP_BASE,
            page_table: None,
            dirty: true,
        };
        Self {
            inner: alloc::sync::Arc::new(Mutex::new(inner)),
        }
    }

    pub fn token(&self) -> VmSpaceToken {
        let mut inner = self.inner.lock();
        if inner.dirty {
            inner.rebuild_page_table();
            inner.dirty = false;
        }
        inner.token
    }

    /// Mark the address space as dirty, forcing page table rebuild on next token() call.
    pub fn set_dirty(&self) {
        let mut inner = self.inner.lock();
        inner.dirty = true;
    }

    pub fn map_anonymous(&self, len: usize, prot: usize) -> KernelResult<usize> {
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let start = {
            let mut inner = self.inner.lock();
            let mut start = align_up(inner.next_mapping_base, PAGE_SIZE);
            loop {
                if let Some(overlap_end) = first_overlap_end(&inner.mappings, start, aligned) {
                    start = align_up(overlap_end, PAGE_SIZE);
                    continue;
                }
                let next = start.checked_add(aligned).ok_or(ENOMEM)?;
                inner.next_mapping_base = next;
                break start;
            }
        };
        let mut zeros = Vec::new();
        zeros.try_reserve_exact(aligned).map_err(|_| ENOMEM)?;
        zeros.resize(aligned, 0);
        self.map_owned(start, zeros, prot)?;
        Ok(start)
    }

    pub fn map_anonymous_shared(&self, len: usize, prot: usize) -> KernelResult<usize> {
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let start = {
            let mut inner = self.inner.lock();
            let mut start = align_up(inner.next_mapping_base, PAGE_SIZE);
            loop {
                if let Some(overlap_end) = first_overlap_end(&inner.mappings, start, aligned) {
                    start = align_up(overlap_end, PAGE_SIZE);
                    continue;
                }
                let next = start.checked_add(aligned).ok_or(ENOMEM)?;
                inner.next_mapping_base = next;
                break start;
            }
        };
        let mut zeros = Vec::new();
        zeros.try_reserve_exact(aligned).map_err(|_| ENOMEM)?;
        zeros.resize(aligned, 0);
        self.map_shared(start, zeros, prot)?;
        Ok(start)
    }

    pub fn map_anonymous_at(&self, addr: usize, len: usize, prot: usize) -> KernelResult<usize> {
        if len == 0 || addr & (PAGE_SIZE - 1) != 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        self.map_fixed_bytes(addr, &[], aligned, prot)?;
        Ok(addr)
    }

    pub fn map_anonymous_shared_at(
        &self,
        addr: usize,
        len: usize,
        prot: usize,
    ) -> KernelResult<usize> {
        if len == 0 || addr & (PAGE_SIZE - 1) != 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        self.map_fixed_shared_bytes(addr, &[], aligned, prot)?;
        Ok(addr)
    }

    pub fn is_range_available(&self, addr: usize, len: usize) -> KernelResult<bool> {
        if len == 0 || addr & (PAGE_SIZE - 1) != 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let inner = self.inner.lock();
        Ok(!overlaps(&inner.mappings, addr, aligned))
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
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(mem_len).map_err(|_| ENOMEM)?;
        buffer.resize(mem_len, 0);
        buffer[..bytes.len()].copy_from_slice(bytes);
        self.map_owned(addr, buffer, prot)
    }

    pub fn map_fixed_shared_bytes(
        &self,
        addr: usize,
        bytes: &[u8],
        mem_len: usize,
        prot: usize,
    ) -> KernelResult<()> {
        if mem_len == 0 || bytes.len() > mem_len {
            return Err(EINVAL);
        }
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(mem_len).map_err(|_| ENOMEM)?;
        buffer.resize(mem_len, 0);
        buffer[..bytes.len()].copy_from_slice(bytes);
        self.map_shared(addr, buffer, prot)
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
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let mut inner = self.inner.lock();
        unmap_range_inner(&mut inner, addr, aligned)
    }

    pub fn mprotect(&self, addr: usize, len: usize, prot: usize) -> KernelResult<()> {
        if len == 0 {
            return Err(EINVAL);
        }
        let aligned = align_up(len, PAGE_SIZE);
        let end = addr.checked_add(aligned).ok_or(EINVAL)?;
        let mut inner = self.inner.lock();
        let keys = inner.mappings.keys().copied().collect::<Vec<_>>();
        for key in keys {
            let Some(segment) = inner.mappings.remove(&key) else {
                continue;
            };
            let seg_start = segment.area.start;
            let seg_end = seg_start + segment.area.len;
            if end <= seg_start || addr >= seg_end {
                inner.mappings.insert(seg_start, segment);
                continue;
            }
            if addr > seg_start {
                let left_len = addr - seg_start;
                let left = slice_segment(&segment, seg_start, left_len, segment.area.prot);
                inner.mappings.insert(left.area.start, left);
            }

            let mid_start = seg_start.max(addr);
            let mid_end = seg_end.min(end);
            let mid_len = mid_end - mid_start;
            let mid = slice_segment(&segment, mid_start, mid_len, prot);
            inner.mappings.insert(mid.area.start, mid);

            if end < seg_end {
                let right_start = end;
                let right_len = seg_end - end;
                let right = slice_segment(&segment, right_start, right_len, segment.area.prot);
                inner.mappings.insert(right.area.start, right);
            }
        }
        inner.dirty = true;
        Ok(())
    }

    pub fn brk(&self, new_break: Option<usize>) -> KernelResult<usize> {
        let Some(requested_break) = new_break else {
            return Ok(self.inner.lock().program_break);
        };
        if requested_break < USER_HEAP_BASE {
            return Err(EINVAL);
        }

        let old_break = self.inner.lock().program_break;
        if requested_break > old_break {
            let map_start = align_up(old_break, PAGE_SIZE);
            let map_end = align_up(requested_break, PAGE_SIZE);
            if map_end > map_start {
                let overlaps_existing = {
                    let inner = self.inner.lock();
                    overlaps(&inner.mappings, map_start, map_end - map_start)
                };
                if overlaps_existing {
                    return Ok(old_break);
                }
                let mut page = map_start;
                while page < map_end {
                    let next = (page + PAGE_SIZE).min(map_end);
                    let page_missing = {
                        let inner = self.inner.lock();
                        !overlaps(&inner.mappings, page, next - page)
                    };
                    if page_missing {
                        self.map_fixed_bytes(page, &[], next - page, DEFAULT_PROT)?;
                    }
                    page = next;
                }
            }
        } else if requested_break < old_break {
            let unmap_start = align_up(requested_break, PAGE_SIZE);
            let unmap_end = align_up(old_break, PAGE_SIZE);
            if unmap_end > unmap_start {
                let mut inner = self.inner.lock();
                unmap_range_inner(&mut inner, unmap_start, unmap_end - unmap_start)?;
            }
        }

        self.inner.lock().program_break = requested_break;
        Ok(requested_break)
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.mappings.clear();
        inner.program_break = USER_HEAP_BASE;
        inner.next_mapping_base = USER_MMAP_BASE;
        inner.dirty = true;
    }

    pub fn read_bytes(&self, addr: usize, len: usize) -> KernelResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let inner = self.inner.lock();
        let mut out = Vec::new();
        out.try_reserve(len).map_err(|_| ENOMEM)?;
        let mut cursor = addr;
        let mut remaining = len;
        while remaining > 0 {
            let (segment, offset) = find_segment(&inner.mappings, cursor, 1)?;
            if !segment_allows_read(segment) {
                return Err(EFAULT);
            }
            let available = segment.area.len.saturating_sub(offset);
            let take = available.min(remaining);
            if take == 0 {
                return Err(EFAULT);
            }
            match &segment.storage {
                SegmentStorage::Owned { ptr, .. } => unsafe {
                    out.extend_from_slice(core::slice::from_raw_parts(
                        (ptr + offset) as *const u8,
                        take,
                    ));
                },
                SegmentStorage::Shared { ptr, .. } => unsafe {
                    out.extend_from_slice(core::slice::from_raw_parts(
                        (ptr + offset) as *const u8,
                        take,
                    ));
                },
                SegmentStorage::Host { ptr, .. } => unsafe {
                    out.extend_from_slice(core::slice::from_raw_parts(
                        (ptr + offset) as *const u8,
                        take,
                    ));
                },
                SegmentStorage::CowParent { bytes, ptr } => unsafe {
                    let _shared = bytes.lock();
                    out.extend_from_slice(core::slice::from_raw_parts(
                        (*ptr + offset) as *const u8,
                        take,
                    ));
                },
            }
            cursor = cursor.checked_add(take).ok_or(EFAULT)?;
            remaining -= take;
        }
        Ok(out)
    }

    pub fn describe_addr(&self, addr: usize) -> String {
        let inner = self.inner.lock();
        if let Some((_, segment)) = inner.mappings.range(..=addr).next_back() {
            let start = segment.area.start;
            let end = start.saturating_add(segment.area.len);
            if addr < end {
                let kind = if addr >= USER_STACK_TOP.saturating_sub(USER_STACK_SIZE * 4) {
                    "stack"
                } else if addr >= USER_MMAP_BASE {
                    "mmap"
                } else if addr >= USER_HEAP_BASE {
                    "brk"
                } else {
                    "elf"
                };
                return format!(
                    "mapped kind={} start={:#x} end={:#x} prot={:#x}",
                    kind, start, end, segment.area.prot
                );
            }
        }
        let prev = inner
            .mappings
            .range(..addr)
            .next_back()
            .map(|(_, segment)| {
                format!(
                    "{:#x}-{:#x}/prot={:#x}",
                    segment.area.start,
                    segment.area.start.saturating_add(segment.area.len),
                    segment.area.prot
                )
            })
            .unwrap_or_else(|| "none".to_string());
        let next = inner
            .mappings
            .range(addr..)
            .next()
            .map(|(_, segment)| {
                format!(
                    "{:#x}-{:#x}/prot={:#x}",
                    segment.area.start,
                    segment.area.start.saturating_add(segment.area.len),
                    segment.area.prot
                )
            })
            .unwrap_or_else(|| "none".to_string());
        format!(
            "unmapped prev={} next={} brk={:#x} next_mmap={:#x}",
            prev, next, inner.program_break, inner.next_mapping_base
        )
    }

    pub fn debug_segments(&self, start: usize, len: usize) -> String {
        if len == 0 {
            return "<empty>".to_string();
        }
        let end = start.saturating_add(len);
        let inner = self.inner.lock();
        let mut parts = Vec::new();
        for segment in inner.mappings.values() {
            let seg_start = segment.area.start;
            let seg_end = seg_start.saturating_add(segment.area.len);
            if seg_start < end && start < seg_end {
                parts.push(format!(
                    "{:#x}-{:#x}/prot={:#x}",
                    seg_start, seg_end, segment.area.prot
                ));
            }
        }
        if parts.is_empty() {
            "<none>".to_string()
        } else {
            parts.join(",")
        }
    }

    pub fn write_bytes(&self, addr: usize, bytes: &[u8]) -> KernelResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock();
        let mut cursor = addr;
        let mut written = 0usize;
        while written < bytes.len() {
            let mut promoted = false;
            let take = {
                let (segment, offset) = find_segment_mut(&mut inner.mappings, cursor, 1)?;
                if !segment_allows_write(segment) {
                    return Err(EFAULT);
                }
                if matches!(segment.storage, SegmentStorage::CowParent { .. }) {
                    promote_cow_segment(segment)?;
                    promoted = true;
                }
                let available = segment.area.len.saturating_sub(offset);
                let take = available.min(bytes.len() - written);
                match &mut segment.storage {
                    SegmentStorage::Owned { ptr, .. } => unsafe {
                        ptr::copy_nonoverlapping(
                            bytes[written..written + take].as_ptr(),
                            (*ptr + offset) as *mut u8,
                            take,
                        );
                    },
                    SegmentStorage::Shared { ptr, .. } => unsafe {
                        ptr::copy_nonoverlapping(
                            bytes[written..written + take].as_ptr(),
                            (*ptr + offset) as *mut u8,
                            take,
                        );
                    },
                    SegmentStorage::Host { ptr, len } => {
                        if offset + take > *len {
                            return Err(EFAULT);
                        }
                        unsafe {
                            ptr::copy_nonoverlapping(
                                bytes[written..written + take].as_ptr(),
                                (*ptr + offset) as *mut u8,
                                take,
                            );
                        }
                    }
                    SegmentStorage::CowParent { .. } => unreachable!(),
                }
                take
            };
            if promoted {
                inner.dirty = true;
            }
            cursor = cursor.checked_add(take).ok_or(EFAULT)?;
            written += take;
        }
        Ok(())
    }

    pub fn read_cstr(&self, addr: usize) -> KernelResult<String> {
        const MAX_STR_LEN: usize = 16 * PAGE_SIZE;
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset < MAX_STR_LEN {
            let page_offset = (addr + offset) & (PAGE_SIZE - 1);
            let chunk_len = (PAGE_SIZE - page_offset).min(MAX_STR_LEN - offset);
            match self.read_bytes(addr + offset, chunk_len) {
                Ok(chunk) => {
                    if let Some(nul) = chunk.iter().position(|&b| b == 0) {
                        out.extend_from_slice(&chunk[..nul]);
                        return String::from_utf8(out).map_err(|_| EFAULT);
                    }
                    out.extend_from_slice(&chunk);
                    offset += chunk.len();
                }
                Err(_) => {
                    // Fallback for short mappings that cannot satisfy the
                    // whole chunk in one shot: probe byte-by-byte.
                    let mut progressed = 0usize;
                    while progressed < chunk_len && offset < MAX_STR_LEN {
                        let byte = match self.read_bytes(addr + offset, 1) {
                            Ok(bytes) => bytes[0],
                            Err(_) => {
                                if out.is_empty() {
                                    return Err(EFAULT);
                                }
                                return Err(EFAULT);
                            }
                        };
                        if byte == 0 {
                            return String::from_utf8(out).map_err(|_| EFAULT);
                        }
                        out.push(byte);
                        offset += 1;
                        progressed += 1;
                    }
                }
            }
        }
        Err(EFAULT)
    }

    pub fn clone_private(&self) -> Self {
        let inner = self.inner.lock();
        let mut mappings = BTreeMap::new();
        for (start, segment) in &inner.mappings {
            let storage = match &segment.storage {
                SegmentStorage::Owned { ptr, .. } => create_cow_storage(
                    *start,
                    unsafe { core::slice::from_raw_parts(*ptr as *const u8, segment.area.len) }
                        .to_vec(),
                ),
                SegmentStorage::Shared { bytes, ptr } => SegmentStorage::Shared {
                    bytes: bytes.clone(),
                    ptr: *ptr,
                },
                SegmentStorage::CowParent { bytes, ptr } => SegmentStorage::CowParent {
                    bytes: bytes.clone(),
                    ptr: *ptr,
                },
                SegmentStorage::Host { ptr, len } => unsafe {
                    create_owned_storage(
                        *start,
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
                token: VmSpaceToken(0),
                mappings,
                program_break: inner.program_break,
                next_mapping_base: inner.next_mapping_base,
                page_table: None,
                dirty: true,
            })),
        }
    }

    pub fn is_shared(&self) -> bool {
        alloc::sync::Arc::strong_count(&self.inner) > 1
    }

    /// Handle store page fault for COW Fork
    ///
    /// When a child process writes to a COW page, this is called to:
    /// 1. Find the COW segment
    /// 2. Copy the data to a new owned page
    /// 3. Convert the segment to Owned storage
    ///
    /// # Arguments
    /// * `addr` - Faulting virtual address
    /// * `address_space` - Process address space to modify
    ///
    /// Handle a store page fault for COW Fork.
    /// Finds the faulting address in the segment mappings, promotes the COW
    /// segment to owned, and rebuilds the page table.
    ///
    /// # Returns
    /// * `Ok(())` if COW was handled successfully
    /// * `Err(EFAULT)` if address not found or not COW segment
    pub fn handle_page_fault(addr: usize, address_space: &mut AddressSpace) -> KernelResult<()> {
        let mut inner = address_space.inner.lock();

        // Find the segment containing the faulting address
        let segment_start = inner
            .mappings
            .iter()
            .find(|(_, segment)| segment_contains_addr(segment, addr))
            .or_else(|| {
                inner
                    .mappings
                    .iter()
                    .find(|(_, segment)| segment_page_contains_addr(segment, addr))
            })
            .map(|(start, _)| *start);

        let Some(seg_start) = segment_start else {
            return Err(EFAULT);
        };

        // Get mutable references to modify the segment
        let segment = inner.mappings.get_mut(&seg_start).ok_or(EFAULT)?;

        // Check if this is actually a COW segment
        if !matches!(segment.storage, SegmentStorage::CowParent { .. }) {
            return Err(EFAULT);
        }

        // Promote the segment (changes CowParent to Owned)
        promote_cow_segment(segment)?;

        drop(inner);
        address_space.set_dirty();
        Ok(())
    }

    pub fn estimated_private_clone_bytes(&self) -> usize {
        let inner = self.inner.lock();
        inner.mappings.values().fold(0usize, |acc, segment| {
            let page_offset = segment.area.start & (PAGE_SIZE - 1);
            let seg_len = segment.area.len.max(1);
            let map_len = align_up(page_offset + seg_len, PAGE_SIZE);
            acc.saturating_add(map_len.saturating_add(PAGE_SIZE))
        })
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
        self.load_elf_images(
            image,
            None,
            args,
            envs,
            args.first().map(|arg| arg.as_str()),
        )
    }

    pub fn load_elf_images(
        &self,
        program_image: &[u8],
        interp_image: Option<&[u8]>,
        args: &[String],
        envs: &[String],
        execfn: Option<&str>,
    ) -> KernelResult<LoadedImage> {
        self.clear();
        let program = self.map_elf_segments(program_image)?;
        let interp = if let Some(image) = interp_image {
            Some(self.map_elf_segments(image)?)
        } else {
            None
        };

        {
            let mut inner = self.inner.lock();
            inner.program_break = program
                .highest_end
                .max(interp.as_ref().map(|image| image.highest_end).unwrap_or(0))
                .max(USER_HEAP_BASE);
            if interp.is_some() {
                // Reserve space for the dynamic loader and libc mmaps so brk/sbrk
                // growth tests can expand without colliding with adjacent RX pages.
                inner.next_mapping_base = inner
                    .next_mapping_base
                    .max(inner.program_break.saturating_add(USER_MMAP_GUARD_GAP));
            }
        }
        let stack_top = USER_STACK_TOP;
        let stack_base = stack_top - USER_STACK_SIZE;
        self.map_fixed_bytes(stack_base, &[], USER_STACK_SIZE, DEFAULT_PROT)?;
        let auxv = [
            (3usize, program.phdr_addr),                                // AT_PHDR
            (4usize, program.phent),                                    // AT_PHENT
            (5usize, program.phnum),                                    // AT_PHNUM
            (6usize, PAGE_SIZE),                                        // AT_PAGESZ
            (7usize, interp.map(|image| image.load_bias).unwrap_or(0)), // AT_BASE
            (8usize, 0usize),                                           // AT_FLAGS
            (9usize, program.entry),                                    // AT_ENTRY
            (11usize, 0usize),                                          // AT_UID
            (12usize, 0usize),                                          // AT_EUID
            (13usize, 0usize),                                          // AT_GID
            (14usize, 0usize),                                          // AT_EGID
            (16usize, 0usize),                                          // AT_HWCAP
            (17usize, 100usize),                                        // AT_CLKTCK
            (23usize, 0usize),                                          // AT_SECURE
        ];
        let stack_image = build_initial_stack(args, envs, stack_top, &auxv, execfn)?;
        let used = stack_image.len();
        self.write_bytes(stack_top - used, &stack_image)?;
        Ok(LoadedImage {
            entry: interp.map(|image| image.entry).unwrap_or(program.entry),
            stack_pointer: stack_top - used,
            load_bias: program.load_bias,
            phdr_addr: program.phdr_addr,
            phnum: program.phnum,
            phent: program.phent,
            interp_base: interp.map(|image| image.load_bias).unwrap_or(0),
            interp_entry: interp.map(|image| image.entry).unwrap_or(0),
            program_entry: program.entry,
            is_dyn: program.is_dyn,
        })
    }

    fn map_elf_segments(&self, image: &[u8]) -> KernelResult<MappedElfImage> {
        const PT_LOAD: u32 = 1;
        const ET_EXEC: u16 = 2;
        const ET_DYN: u16 = 3;

        let header = ElfHeader::parse(image)?;
        if header.program_header_size != 56 || header.class != 2 || header.endianness != 1 {
            return Err(ENOEXEC);
        }
        if header.program_header_num == 0 {
            return Err(ENOEXEC);
        }

        let mut min_vaddr = usize::MAX;
        let mut max_vaddr_end = 0usize;
        let mut load_segments = 0usize;
        for index in 0..header.program_header_num {
            let offset = header.program_header_offset + index * header.program_header_size;
            let ph = ProgramHeader::parse(image, offset)?;
            if ph.segment_type != PT_LOAD {
                continue;
            }
            if ph.file_size > ph.mem_size {
                return Err(ENOEXEC);
            }
            min_vaddr = min_vaddr.min(ph.vaddr);
            max_vaddr_end = max_vaddr_end.max(ph.vaddr.checked_add(ph.mem_size).ok_or(ENOEXEC)?);
            load_segments += 1;
        }
        if load_segments == 0 {
            return Err(ENOEXEC);
        }

        let load_bias = match header.elf_type {
            ET_EXEC => 0usize,
            ET_DYN => self.reserve_dynamic_load_bias(min_vaddr, max_vaddr_end)?,
            _ => return Err(ENOEXEC),
        };

        for index in 0..header.program_header_num {
            let offset = header.program_header_offset + index * header.program_header_size;
            let ph = ProgramHeader::parse(image, offset)?;
            if ph.segment_type != PT_LOAD {
                continue;
            }
            if ph.file_size > ph.mem_size {
                return Err(ENOEXEC);
            }
            let data_end = ph.offset.checked_add(ph.file_size).ok_or(ENOEXEC)?;
            if data_end > image.len() {
                return Err(ENOEXEC);
            }
            let mapped_vaddr = ph.vaddr.checked_add(load_bias).ok_or(ENOEXEC)?;
            let seg_start = align_down(mapped_vaddr, PAGE_SIZE);
            let page_offset = mapped_vaddr - seg_start;
            let file_page_start = if (ph.offset & (PAGE_SIZE - 1)) != page_offset {
                if ph.offset < PAGE_SIZE && page_offset == 0 {
                    ph.offset
                } else {
                    return Err(ENOEXEC);
                }
            } else {
                align_down(ph.offset, PAGE_SIZE)
            };
            // PT_LOAD segments are page-granular once mapped into userspace.
            // The bytes between the logical end of the segment and the end of
            // its last page must remain accessible/zero-filled, otherwise PIE
            // data/bss users that touch objects in the tail of that page will
            // spuriously fault with EFAULT.
            let seg_mem_len = align_up(
                page_offset.checked_add(ph.mem_size).ok_or(ENOEXEC)?,
                PAGE_SIZE,
            );
            let mut seg_bytes = Vec::new();
            seg_bytes
                .try_reserve_exact(page_offset + ph.file_size)
                .map_err(|_| ENOMEM)?;
            seg_bytes.extend_from_slice(&image[file_page_start..data_end]);
            self.map_fixed_bytes(
                seg_start,
                &seg_bytes,
                seg_mem_len,
                elf_flags_to_prot(ph.flags),
            )?;
        }

        let phdr_addr = find_phdr_vaddr(&header, image)?
            .checked_add(load_bias)
            .ok_or(ENOEXEC)?;
        let entry = header.entry.checked_add(load_bias).ok_or(ENOEXEC)?;
        Ok(MappedElfImage {
            entry,
            load_bias,
            phdr_addr,
            phnum: header.program_header_num,
            phent: header.program_header_size,
            highest_end: align_up(
                max_vaddr_end.checked_add(load_bias).ok_or(ENOEXEC)?,
                PAGE_SIZE,
            ),
            is_dyn: header.elf_type == ET_DYN,
        })
    }

    fn reserve_dynamic_load_bias(
        &self,
        min_vaddr: usize,
        max_vaddr_end: usize,
    ) -> KernelResult<usize> {
        let load_floor = align_down(min_vaddr, PAGE_SIZE);
        let span = align_up(
            max_vaddr_end.saturating_sub(load_floor).max(PAGE_SIZE),
            PAGE_SIZE,
        );
        let mut inner = self.inner.lock();
        let mut base = align_up(inner.next_mapping_base, PAGE_SIZE);
        loop {
            if let Some(overlap_end) = first_overlap_end(&inner.mappings, base, span) {
                base = align_up(overlap_end, PAGE_SIZE);
                continue;
            }
            inner.next_mapping_base = base.checked_add(span).ok_or(ENOMEM)?;
            return base.checked_sub(load_floor).ok_or(ENOMEM);
        }
    }

    fn map_owned(&self, addr: usize, bytes: Vec<u8>, prot: usize) -> KernelResult<()> {
        let len = bytes.len().max(1);
        self.insert_segment(Segment {
            area: MappingArea {
                start: addr,
                len,
                prot,
            },
            storage: create_owned_storage(addr, bytes),
        })
    }

    fn map_shared(&self, addr: usize, bytes: Vec<u8>, prot: usize) -> KernelResult<()> {
        let len = bytes.len().max(1);
        self.insert_segment(Segment {
            area: MappingArea {
                start: addr,
                len,
                prot,
            },
            storage: create_shared_storage(addr, bytes),
        })
    }

    pub fn map_shared_existing(
        &self,
        addr: usize,
        data: alloc::sync::Arc<Mutex<Vec<u8>>>,
        prot: usize,
    ) -> KernelResult<usize> {
        let len = data.lock().len().max(1);
        self.insert_segment(Segment {
            area: MappingArea {
                start: addr,
                len,
                prot,
            },
            storage: SegmentStorage::Shared {
                bytes: data,
                ptr: addr,
            },
        })?;
        Ok(addr)
    }

    fn insert_segment(&self, segment: Segment) -> KernelResult<()> {
        let mut inner = self.inner.lock();
        if overlaps(&inner.mappings, segment.area.start, segment.area.len) {
            unmap_range_inner(&mut inner, segment.area.start, segment.area.len)?;
        }
        inner.mappings.insert(segment.area.start, segment);
        inner.dirty = true;
        Ok(())
    }
}

impl AddressSpaceInner {
    fn rebuild_page_table(&mut self) {
        if cfg!(target_arch = "riscv64") {
            self.build_riscv_page_table();
            return;
        }
        if cfg!(target_arch = "loongarch64") {
            self.build_loongarch_page_table();
            return;
        }
        self.token = VmSpaceToken(0);
        self.page_table = None;
    }

    /// Update a single PTE in the existing RISC-V Sv39 page table.
    /// Returns true on success, false if the page table doesn't exist or PTE not found.
    fn update_pte_riscv(&mut self, vaddr: usize, paddr: usize, flags: u64) -> bool {
        let page_table = match &mut self.page_table {
            Some(pt) => pt,
            None => return false,
        };

        // Sv39 VPN indices
        let vpn2 = (vaddr >> 30) & 0x1ff;
        let vpn1 = (vaddr >> 21) & 0x1ff;
        let vpn0 = (vaddr >> 12) & 0x1ff;

        // Get root page table (first page in the vec)
        if page_table.pages.is_empty() {
            return false;
        }

        // Walk down the page table
        // Level 2: root
        let pte2 = page_table.pages[0].0[vpn2];
        if pte2 & RISCV_PTE_V == 0 {
            return false;
        }
        // Not a leaf
        if pte2 & (RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_X) != 0 {
            return false;
        }
        let l1_phys = ((pte2 >> 10) as usize) << 12;

        // Level 1
        let pte1 = unsafe { &mut (*(l1_phys as *mut PageTablePage)).0 }[vpn1];
        if pte1 & RISCV_PTE_V == 0 {
            return false;
        }
        if pte1 & (RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_X) != 0 {
            return false;
        }
        let l0_phys = ((pte1 >> 10) as usize) << 12;

        // Level 0 - update the PTE
        let pte_array = unsafe { &mut (*(l0_phys as *mut PageTablePage)).0 };
        pte_array[vpn0] = ((paddr >> 12) as u64) << 10 | flags | RISCV_PTE_V;
        true
    }

    fn update_pte_loongarch(&mut self, vaddr: usize, paddr: usize, flags: u64) -> bool {
        let page_table = match &mut self.page_table {
            Some(pt) => pt,
            None => return false,
        };

        let dir3 = (vaddr >> 39) & 0x1ff;
        let dir2 = (vaddr >> 30) & 0x1ff;
        let dir1 = (vaddr >> 21) & 0x1ff;
        let pt = (vaddr >> 12) & 0x1ff;

        if page_table.pages.is_empty() {
            return false;
        }

        let root = &page_table.pages[0].0;
        let pte3 = root[dir3];
        if pte3 == 0 || pte3 & (LA_PTE_P | LA_PTE_V) != 0 {
            return false;
        }
        let l2_phys = (pte3 as usize) & !(PAGE_SIZE - 1);

        let pte2 = unsafe { &mut (*(l2_phys as *mut PageTablePage)).0 }[dir2];
        if pte2 == 0 || pte2 & (LA_PTE_P | LA_PTE_V) != 0 {
            return false;
        }
        let l1_phys = (pte2 as usize) & !(PAGE_SIZE - 1);

        let pte1 = unsafe { &mut (*(l1_phys as *mut PageTablePage)).0 }[dir1];
        if pte1 == 0 || pte1 & (LA_PTE_P | LA_PTE_V) != 0 {
            return false;
        }
        let l0_phys = (pte1 as usize) & !(PAGE_SIZE - 1);

        let pte_array = unsafe { &mut (*(l0_phys as *mut PageTablePage)).0 };
        pte_array[pt] = loong_make_leaf_pte(paddr, flags);
        true
    }

    /// Get a mutable reference to a page table page given its physical address.
    fn build_riscv_page_table(&mut self) {
        let mut builder = Sv39PageTableBuilder::new();
        builder.map_identity_2m(
            0x8000_0000,
            0x1_0000_0000,
            RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_X | RISCV_PTE_A | RISCV_PTE_D,
        );
        builder.map_identity_2m(
            RISCV_MMIO_BASE,
            RISCV_MMIO_BASE + RISCV_MMIO_SIZE,
            RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_A | RISCV_PTE_D,
        );
        for segment in self.mappings.values() {
            let is_cow = matches!(&segment.storage, SegmentStorage::CowParent { .. });
            if is_cow {
                map_segment_pages_cow_riscv(&mut builder, segment);
            } else {
                map_segment_pages_riscv(&mut builder, segment);
            }
        }
        let root_phys = builder.root_phys();
        self.token = VmSpaceToken(root_phys);
        self.page_table = Some(PageTableSpace {
            root_phys,
            pages: builder.into_pages(),
        });
    }

    fn build_loongarch_page_table(&mut self) {
        let mut builder = LoongPageTableBuilder::new();
        builder.map_identity_2m(
            LOONGARCH_PHYS_BASE,
            LOONGARCH_PHYS_BASE + LOONGARCH_PHYS_SIZE,
            loong_kernel_pte_flags(true, true, true, false),
        );
        builder.map_identity_2m(
            LOONGARCH_MMIO_BASE,
            LOONGARCH_MMIO_BASE + LOONGARCH_MMIO_SIZE,
            loong_kernel_pte_flags(true, true, false, true),
        );
        for segment in self.mappings.values() {
            let is_cow = matches!(&segment.storage, SegmentStorage::CowParent { .. });
            if is_cow {
                map_segment_pages_cow_loongarch(&mut builder, segment);
            } else {
                map_segment_pages_loongarch(&mut builder, segment);
            }
        }
        let root_phys = builder.root_phys();
        self.token = VmSpaceToken(root_phys);
        self.page_table = Some(PageTableSpace {
            root_phys,
            pages: builder.into_pages(),
        });
    }
}

struct Sv39PageTableBuilder {
    root_phys: usize,
    pages: Vec<alloc::boxed::Box<PageTablePage>>,
}

impl Sv39PageTableBuilder {
    fn new() -> Self {
        let mut root = alloc::boxed::Box::new(PageTablePage::new());
        let root_phys = (&mut *root as *mut PageTablePage) as usize;
        Self {
            root_phys,
            pages: vec![root],
        }
    }

    fn root_phys(&self) -> usize {
        self.root_phys
    }

    fn into_pages(self) -> Vec<alloc::boxed::Box<PageTablePage>> {
        self.pages
    }

    fn map_identity_2m(&mut self, start: usize, end: usize, flags: u64) {
        let mut cursor = align_down(start, 1 << 21);
        let limit = align_up(end, 1 << 21);
        while cursor < limit {
            self.map_2m(cursor, cursor, flags);
            cursor += 1 << 21;
        }
    }

    fn map_2m(&mut self, vaddr: usize, paddr: usize, flags: u64) {
        if (vaddr | paddr) & ((1 << 21) - 1) != 0 {
            return;
        }
        let vpn2 = (vaddr >> 30) & 0x1ff;
        let vpn1 = (vaddr >> 21) & 0x1ff;
        let l1_phys = self.ensure_next_table(self.root_phys, vpn2);
        let pte = riscv_make_leaf_pte(paddr, flags);
        let table = self.table_mut(l1_phys);
        table[vpn1] = pte;
    }

    fn map_4k(&mut self, vaddr: usize, paddr: usize, flags: u64) {
        if (vaddr | paddr) & (PAGE_SIZE - 1) != 0 {
            return;
        }
        let vpn2 = (vaddr >> 30) & 0x1ff;
        let vpn1 = (vaddr >> 21) & 0x1ff;
        let vpn0 = (vaddr >> 12) & 0x1ff;

        let l1_phys = self.ensure_next_table(self.root_phys, vpn2);
        if l1_phys == 0 {
            return;
        }
        let l0_phys = self.ensure_next_table(l1_phys, vpn1);
        if l0_phys == 0 {
            return;
        }
        let table = self.table_mut(l0_phys);
        table[vpn0] = riscv_make_leaf_pte(paddr, flags);
    }

    fn ensure_next_table(&mut self, table_phys: usize, index: usize) -> usize {
        let existing = self.table_mut(table_phys)[index];
        if existing & RISCV_PTE_V != 0 && existing & (RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_X) == 0
        {
            return ((existing >> 10) as usize) << 12;
        }
        if existing & RISCV_PTE_V != 0 {
            return 0;
        }
        let mut next = alloc::boxed::Box::new(PageTablePage::new());
        let next_phys = (&mut *next as *mut PageTablePage) as usize;
        self.pages.push(next);
        self.table_mut(table_phys)[index] = riscv_make_table_pte(next_phys);
        next_phys
    }

    fn table_mut(&mut self, phys: usize) -> &mut [u64; 512] {
        unsafe { &mut (*(phys as *mut PageTablePage)).0 }
    }
}

struct LoongPageTableBuilder {
    root_phys: usize,
    pages: Vec<alloc::boxed::Box<PageTablePage>>,
}

impl LoongPageTableBuilder {
    fn new() -> Self {
        let mut root = alloc::boxed::Box::new(PageTablePage::new());
        let root_phys = (&mut *root as *mut PageTablePage) as usize;
        Self {
            root_phys,
            pages: vec![root],
        }
    }

    fn root_phys(&self) -> usize {
        self.root_phys
    }

    fn into_pages(self) -> Vec<alloc::boxed::Box<PageTablePage>> {
        self.pages
    }

    fn map_identity_2m(&mut self, start: usize, end: usize, flags: u64) {
        let mut cursor = align_down(start, 1 << 21);
        let limit = align_up(end, 1 << 21);
        while cursor < limit {
            self.map_2m(cursor, cursor, flags);
            cursor += 1 << 21;
        }
    }

    fn map_2m(&mut self, vaddr: usize, paddr: usize, flags: u64) {
        if (vaddr | paddr) & ((1 << 21) - 1) != 0 {
            return;
        }
        let dir3 = (vaddr >> 39) & 0x1ff;
        let dir2 = (vaddr >> 30) & 0x1ff;
        let dir1 = (vaddr >> 21) & 0x1ff;

        let l2_phys = self.ensure_next_table(self.root_phys, dir3);
        if l2_phys == 0 {
            return;
        }
        let l1_phys = self.ensure_next_table(l2_phys, dir2);
        if l1_phys == 0 {
            return;
        }
        let table = self.table_mut(l1_phys);
        table[dir1] = loong_make_leaf_pte(paddr, flags | LA_PTE_GH);
    }

    fn map_4k(&mut self, vaddr: usize, paddr: usize, flags: u64) {
        if (vaddr | paddr) & (PAGE_SIZE - 1) != 0 {
            return;
        }
        let dir3 = (vaddr >> 39) & 0x1ff;
        let dir2 = (vaddr >> 30) & 0x1ff;
        let dir1 = (vaddr >> 21) & 0x1ff;
        let pt = (vaddr >> 12) & 0x1ff;

        let l2_phys = self.ensure_next_table(self.root_phys, dir3);
        if l2_phys == 0 {
            return;
        }
        let l1_phys = self.ensure_next_table(l2_phys, dir2);
        if l1_phys == 0 {
            return;
        }
        let l0_phys = self.ensure_next_table(l1_phys, dir1);
        if l0_phys == 0 {
            return;
        }
        let table = self.table_mut(l0_phys);
        table[pt] = loong_make_leaf_pte(paddr, flags);
    }

    fn ensure_next_table(&mut self, table_phys: usize, index: usize) -> usize {
        let existing = self.table_mut(table_phys)[index];
        if existing == 0 {
            let mut next = alloc::boxed::Box::new(PageTablePage::new());
            let next_phys = (&mut *next as *mut PageTablePage) as usize;
            self.pages.push(next);
            self.table_mut(table_phys)[index] = loong_make_table_pte(next_phys);
            return next_phys;
        }
        if existing & (LA_PTE_P | LA_PTE_V) != 0 {
            return 0;
        }
        (existing as usize) & !(PAGE_SIZE - 1)
    }

    fn table_mut(&mut self, phys: usize) -> &mut [u64; 512] {
        unsafe { &mut (*(phys as *mut PageTablePage)).0 }
    }
}

fn map_segment_pages_riscv(builder: &mut Sv39PageTableBuilder, segment: &Segment) {
    let Some(phys_base) = segment_phys_base(segment) else {
        return;
    };
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    if (0x8000_0000..0x1_0000_0000).contains(&start) && phys_base == start {
        return;
    }
    let flags = riscv_segment_pte_flags(segment.area.prot);
    let mut vaddr = start;
    while vaddr < end {
        builder.map_4k(vaddr, phys_base + (vaddr - start), flags);
        vaddr += PAGE_SIZE;
    }
}

fn map_segment_pages_loongarch(builder: &mut LoongPageTableBuilder, segment: &Segment) {
    let Some(phys_base) = segment_phys_base(segment) else {
        return;
    };
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    if (LOONGARCH_PHYS_BASE..LOONGARCH_PHYS_BASE + LOONGARCH_PHYS_SIZE).contains(&start)
        && phys_base == start
    {
        return;
    }
    let flags = loong_segment_pte_flags(segment.area.prot);
    let mut vaddr = start;
    while vaddr < end {
        builder.map_4k(vaddr, phys_base + (vaddr - start), flags);
        vaddr += PAGE_SIZE;
    }
}

fn map_segment_pages_cow_riscv(builder: &mut Sv39PageTableBuilder, segment: &Segment) {
    let phys_base = match segment_phys_base(segment) {
        Some(base) => base,
        None => return,
    };
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    // COW pages are read-only but executable if original segment allowed exec:
    // R=1, W=0, X=original segment's X, U=1, A=1, D=0
    // Write attempts will trigger store page fault (scause=15) which COW handles
    let exec = segment.area.prot & 0b100 != 0;
    let flags = RISCV_PTE_U | RISCV_PTE_R | RISCV_PTE_A | if exec { RISCV_PTE_X } else { 0 };
    let mut vaddr = start;
    while vaddr < end {
        builder.map_4k(vaddr, phys_base + (vaddr - start), flags);
        vaddr += PAGE_SIZE;
    }
}

fn map_segment_pages_cow_loongarch(builder: &mut LoongPageTableBuilder, segment: &Segment) {
    let phys_base = match segment_phys_base(segment) {
        Some(base) => base,
        None => return,
    };
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    // COW pages: read-only, no W flag
    let flags = loong_pte_flags(true, false, segment.area.prot & 0b100 != 0, true, false);
    let mut vaddr = start;
    while vaddr < end {
        builder.map_4k(vaddr, phys_base + (vaddr - start), flags);
        vaddr += PAGE_SIZE;
    }
}

fn riscv_make_leaf_pte(paddr: usize, flags: u64) -> u64 {
    ((paddr >> 12) as u64) << 10 | flags | RISCV_PTE_V
}

fn riscv_make_table_pte(paddr: usize) -> u64 {
    ((paddr >> 12) as u64) << 10 | RISCV_PTE_V
}

fn riscv_segment_pte_flags(prot: usize) -> u64 {
    let mut flags = RISCV_PTE_A | RISCV_PTE_D;
    let mut read = prot & 0b001 != 0;
    let write = prot & 0b010 != 0;
    let exec = prot & 0b100 != 0;
    if !read && !write && !exec {
        // Preserve a present mapping for PROT_NONE but keep it inaccessible
        // to user mode by clearing U.
        return flags | RISCV_PTE_R | RISCV_PTE_W;
    }
    flags |= RISCV_PTE_U;
    // RISC-V leaf PTEs do not allow execute-only encodings (R=0,W=0,X=1).
    // Align with Linux-style mappings: executable pages are at least readable.
    if exec {
        read = true;
    }
    if read {
        flags |= RISCV_PTE_R;
    }
    if write {
        flags |= RISCV_PTE_W;
    }
    if exec {
        flags |= RISCV_PTE_X;
    }
    flags
}

fn loong_make_leaf_pte(paddr: usize, flags: u64) -> u64 {
    ((paddr as u64) & LA_PTE_ADDR_MASK) | flags
}

fn loong_make_table_pte(paddr: usize) -> u64 {
    (paddr as u64) & LA_PTE_ADDR_MASK
}

fn loong_kernel_pte_flags(read: bool, write: bool, exec: bool, device: bool) -> u64 {
    loong_pte_flags(read, write, exec, false, device)
}

fn loong_segment_pte_flags(prot: usize) -> u64 {
    let mut read = prot & 0b001 != 0;
    let write = prot & 0b010 != 0;
    let exec = prot & 0b100 != 0;
    if !read && !write && !exec {
        // Keep the mapping present but supervisor-only for PROT_NONE.
        return loong_pte_flags(true, false, false, false, false);
    }
    // Keep user mappings permissive enough for toolchains that request
    // write-only or execute-only pages during loader transitions.
    if write || exec {
        read = true;
    }
    loong_pte_flags(read, write, exec, true, false)
}

fn loong_pte_flags(read: bool, write: bool, exec: bool, user: bool, device: bool) -> u64 {
    let mut flags = LA_PTE_V | LA_PTE_P;
    if !read {
        flags |= LA_PTE_NR;
    }
    if write {
        flags |= LA_PTE_W | LA_PTE_D;
    }
    if !exec {
        flags |= LA_PTE_NX;
    }
    if user {
        flags |= LA_PTE_PLVL | LA_PTE_PLVH;
    }
    if device {
        flags |= LA_PTE_MATH;
    } else {
        flags |= LA_PTE_MATL;
    }
    flags
}

fn segment_phys_base(segment: &Segment) -> Option<usize> {
    let page_offset = segment.area.start & (PAGE_SIZE - 1);
    match segment.storage {
        SegmentStorage::Owned { ptr, .. } => Some(ptr.saturating_sub(page_offset)),
        SegmentStorage::Shared { ptr, .. } => Some(ptr.saturating_sub(page_offset)),
        SegmentStorage::CowParent { ptr, .. } => Some(ptr.saturating_sub(page_offset)),
        SegmentStorage::Host { ptr, .. } => {
            ((ptr & (PAGE_SIZE - 1)) == page_offset).then_some(ptr.saturating_sub(page_offset))
        }
    }
}

fn segment_page_contains_addr(segment: &Segment, addr: usize) -> bool {
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(
        segment.area.start.saturating_add(segment.area.len),
        PAGE_SIZE,
    );
    start <= addr && addr < end
}

fn segment_contains_addr(segment: &Segment, addr: usize) -> bool {
    let start = segment.area.start;
    let end = start.saturating_add(segment.area.len);
    start <= addr && addr < end
}

fn promote_cow_segment(segment: &mut Segment) -> KernelResult<()> {
    if segment.area.prot & 0b010 == 0 {
        return Err(EFAULT);
    }

    let current_data = match &segment.storage {
        SegmentStorage::CowParent { ptr, .. } => unsafe {
            core::slice::from_raw_parts(*ptr as *const u8, segment.area.len).to_vec()
        },
        _ => return Err(EFAULT),
    };

    segment.storage = create_owned_storage(segment.area.start, current_data);
    Ok(())
}

fn create_cow_storage(addr: usize, mut data: Vec<u8>) -> SegmentStorage {
    if data.is_empty() {
        data.push(0);
    }
    let len = data.len();
    let page_offset = addr & (PAGE_SIZE - 1);
    let map_len = align_up(page_offset + len, PAGE_SIZE);
    let total = map_len + PAGE_SIZE;
    let mut bytes = vec![0u8; total];
    let raw_ptr = bytes.as_mut_ptr() as usize;
    let aligned_base = align_up(raw_ptr, PAGE_SIZE);
    let ptr = aligned_base + page_offset;
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, len);
    }
    let bytes = alloc::sync::Arc::new(Mutex::new(bytes));
    SegmentStorage::CowParent { bytes, ptr }
}

fn create_owned_storage(addr: usize, mut data: Vec<u8>) -> SegmentStorage {
    if data.is_empty() {
        data.push(0);
    }
    let len = data.len();
    let page_offset = addr & (PAGE_SIZE - 1);
    let map_len = align_up(page_offset + len, PAGE_SIZE);
    let total = map_len + PAGE_SIZE;
    if DEBUG_LARGE_SEGMENT_ALLOC && total >= 700_000 {
        let mut console = hal_api::ConsoleWriter;
        let _ = core::fmt::Write::write_fmt(
            &mut console,
            format_args!(
                "whuse-debug: create_owned_storage huge total={} addr={:#x} len={} map_len={} page_off={}\n",
                total, addr, len, map_len, page_offset
            ),
        );
    }
    let mut bytes = vec![0u8; total];
    let raw_ptr = bytes.as_mut_ptr() as usize;
    let aligned_base = align_up(raw_ptr, PAGE_SIZE);
    let ptr = aligned_base + page_offset;
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, len);
    }
    SegmentStorage::Owned { bytes, ptr }
}

fn create_shared_storage(addr: usize, mut data: Vec<u8>) -> SegmentStorage {
    if data.is_empty() {
        data.push(0);
    }
    let len = data.len();
    let page_offset = addr & (PAGE_SIZE - 1);
    let map_len = align_up(page_offset + len, PAGE_SIZE);
    let total = map_len + PAGE_SIZE;
    let mut bytes = vec![0u8; total];
    let raw_ptr = bytes.as_mut_ptr() as usize;
    let aligned_base = align_up(raw_ptr, PAGE_SIZE);
    let ptr = aligned_base + page_offset;
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, len);
    }
    SegmentStorage::Shared {
        bytes: alloc::sync::Arc::new(Mutex::new(bytes)),
        ptr,
    }
}

fn elf_flags_to_prot(flags: u32) -> usize {
    let mut prot = 0usize;
    if flags & 0b100 != 0 {
        prot |= 0b001;
    }
    if flags & 0b010 != 0 {
        prot |= 0b010;
    }
    if flags & 0b001 != 0 {
        prot |= 0b100;
    }
    if prot == 0 {
        DEFAULT_PROT
    } else {
        prot
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
    elf_type: u16,
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
            elf_type: read_u16(image, 16)?,
            entry: read_u64(image, 24)? as usize,
            program_header_offset: read_u64(image, 32)? as usize,
            program_header_size: read_u16(image, 54)? as usize,
            program_header_num: read_u16(image, 56)? as usize,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MappedElfImage {
    entry: usize,
    load_bias: usize,
    phdr_addr: usize,
    phnum: usize,
    phent: usize,
    highest_end: usize,
    is_dyn: bool,
}

#[derive(Clone, Copy, Debug)]
struct ProgramHeader {
    segment_type: u32,
    flags: u32,
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
            flags: read_u32(image, offset + 4)?,
            offset: read_u64(image, offset + 8)? as usize,
            vaddr: read_u64(image, offset + 16)? as usize,
            file_size: read_u64(image, offset + 32)? as usize,
            mem_size: read_u64(image, offset + 40)? as usize,
        })
    }
}

fn find_phdr_vaddr(header: &ElfHeader, image: &[u8]) -> KernelResult<usize> {
    let phoff = header.program_header_offset;
    for index in 0..header.program_header_num {
        let offset = header.program_header_offset + index * header.program_header_size;
        let ph = ProgramHeader::parse(image, offset)?;
        if ph.segment_type != 1 || ph.file_size == 0 {
            continue;
        }
        let file_end = ph.offset.checked_add(ph.file_size).ok_or(ENOEXEC)?;
        if phoff < ph.offset || phoff >= file_end {
            continue;
        }
        let delta = phoff - ph.offset;
        return ph.vaddr.checked_add(delta).ok_or(ENOEXEC);
    }
    Ok(0)
}

fn overlaps(mappings: &BTreeMap<usize, Segment>, start: usize, len: usize) -> bool {
    let end = start.saturating_add(len);
    mappings.iter().any(|(base, segment)| {
        let seg_end = *base + segment.area.len;
        start < seg_end && *base < end
    })
}

fn first_overlap_end(
    mappings: &BTreeMap<usize, Segment>,
    start: usize,
    len: usize,
) -> Option<usize> {
    let end = start.saturating_add(len);
    mappings.iter().find_map(|(base, segment)| {
        let seg_end = *base + segment.area.len;
        (start < seg_end && *base < end).then_some(seg_end)
    })
}

fn range_fully_mapped(mappings: &BTreeMap<usize, Segment>, start: usize, len: usize) -> bool {
    let end = start.saturating_add(len);
    let mut cursor = start;
    while cursor < end {
        let Some((base, segment)) = mappings.range(..=cursor).next_back() else {
            return false;
        };
        let seg_end = *base + segment.area.len;
        if cursor < *base || cursor >= seg_end {
            return false;
        }
        cursor = seg_end.min(end);
    }
    true
}

fn unmap_range_inner(inner: &mut AddressSpaceInner, start: usize, len: usize) -> KernelResult<()> {
    if len == 0 {
        return Err(EINVAL);
    }
    let end = start.checked_add(len).ok_or(EINVAL)?;
    let keys = inner.mappings.keys().copied().collect::<Vec<_>>();
    let mut changed = false;
    for key in keys {
        let Some(segment) = inner.mappings.remove(&key) else {
            continue;
        };
        let seg_start = segment.area.start;
        let seg_end = seg_start + segment.area.len;
        if end <= seg_start || start >= seg_end {
            inner.mappings.insert(seg_start, segment);
            continue;
        }
        changed = true;
        if start > seg_start {
            let left_len = start - seg_start;
            let left = slice_segment(&segment, seg_start, left_len, segment.area.prot);
            inner.mappings.insert(left.area.start, left);
        }
        if end < seg_end {
            let right_start = end;
            let right_len = seg_end - end;
            let right = slice_segment(&segment, right_start, right_len, segment.area.prot);
            inner.mappings.insert(right.area.start, right);
        }
    }
    if changed {
        inner.dirty = true;
    }
    Ok(())
}

fn slice_segment(segment: &Segment, start: usize, len: usize, prot: usize) -> Segment {
    Segment {
        area: MappingArea { start, len, prot },
        storage: slice_segment_storage(segment, start, len),
    }
}

fn slice_segment_storage(segment: &Segment, start: usize, len: usize) -> SegmentStorage {
    let offset = start - segment.area.start;
    match &segment.storage {
        SegmentStorage::Owned { ptr, .. } => {
            let bytes =
                unsafe { core::slice::from_raw_parts((*ptr + offset) as *const u8, len).to_vec() };
            create_owned_storage(start, bytes)
        }
        SegmentStorage::Shared { bytes, ptr } => SegmentStorage::Shared {
            bytes: bytes.clone(),
            ptr: ptr.saturating_add(offset),
        },
        SegmentStorage::CowParent { bytes, ptr } => SegmentStorage::CowParent {
            bytes: bytes.clone(),
            ptr: ptr.saturating_add(offset),
        },
        SegmentStorage::Host { ptr, .. } => SegmentStorage::Host {
            ptr: ptr.saturating_add(offset),
            len,
        },
    }
}

fn find_segment(
    mappings: &BTreeMap<usize, Segment>,
    addr: usize,
    len: usize,
) -> KernelResult<(&Segment, usize)> {
    let end = addr.checked_add(len).ok_or(EFAULT)?;
    mappings
        .iter()
        .find(|(base, segment)| {
            let seg_start = **base;
            let Some(seg_end) = seg_start.checked_add(segment.area.len) else {
                return false;
            };
            seg_start <= addr && end <= seg_end
        })
        .map(|(base, segment)| (segment, addr - *base))
        .ok_or(EFAULT)
}

fn find_segment_mut(
    mappings: &mut BTreeMap<usize, Segment>,
    addr: usize,
    len: usize,
) -> KernelResult<(&mut Segment, usize)> {
    let end = addr.checked_add(len).ok_or(EFAULT)?;
    mappings
        .iter_mut()
        .find(|(base, segment)| {
            let seg_start = **base;
            let Some(seg_end) = seg_start.checked_add(segment.area.len) else {
                return false;
            };
            seg_start <= addr && end <= seg_end
        })
        .map(|(base, segment)| (segment, addr - *base))
        .ok_or(EFAULT)
}

fn segment_allows_read(segment: &Segment) -> bool {
    segment.area.prot & 0b001 != 0
}

fn segment_allows_write(segment: &Segment) -> bool {
    segment.area.prot & 0b010 != 0
}

fn build_initial_stack(
    args: &[String],
    envs: &[String],
    stack_top: usize,
    auxv: &[(usize, usize)],
    execfn: Option<&str>,
) -> KernelResult<Vec<u8>> {
    const AT_RANDOM: usize = 25;
    const AT_EXECFN: usize = 31;
    const AUX_RANDOM_BYTES: [u8; 16] = [
        0x57, 0x68, 0x75, 0x73, 0x65, 0x2d, 0x72, 0x76, 0x2d, 0x61, 0x75, 0x78, 0x76, 0x2d, 0x31,
        0x36,
    ];

    let pointer_size = size_of::<usize>();
    let total_strings_len = args
        .iter()
        .chain(envs.iter())
        .map(|entry| entry.len() + 1)
        .sum::<usize>()
        + execfn.map(|value| value.len() + 1).unwrap_or(0)
        + AUX_RANDOM_BYTES.len();
    let auxv_len = auxv.len() + usize::from(execfn.is_some()) + 1;
    let pointer_count = 1 + args.len() + 1 + envs.len() + 1 + (auxv_len + 1) * 2;
    let mut stack = vec![0u8; align_up(total_strings_len + pointer_count * pointer_size, 16)];

    let mut string_cursor = stack.len();
    let mut arg_ptrs = Vec::with_capacity(args.len());
    let mut env_ptrs = Vec::with_capacity(envs.len());
    let mut push_blob = |blob: &[u8]| -> usize {
        string_cursor -= blob.len();
        stack[string_cursor..string_cursor + blob.len()].copy_from_slice(blob);
        stack_top - (stack.len() - string_cursor)
    };

    for entry in args {
        let mut bytes = Vec::with_capacity(entry.len() + 1);
        bytes.extend_from_slice(entry.as_bytes());
        bytes.push(0);
        arg_ptrs.push(push_blob(&bytes));
    }
    for entry in envs {
        let mut bytes = Vec::with_capacity(entry.len() + 1);
        bytes.extend_from_slice(entry.as_bytes());
        bytes.push(0);
        env_ptrs.push(push_blob(&bytes));
    }
    let execfn_addr = execfn.map(|value| {
        let mut bytes = Vec::with_capacity(value.len() + 1);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        push_blob(&bytes)
    });
    let random_addr = push_blob(&AUX_RANDOM_BYTES);

    let mut head = vec![0u8; pointer_size * pointer_count];
    let argc = args.len();
    let mut cursor = 0usize;
    head[..pointer_size].copy_from_slice(&argc.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    for value in &arg_ptrs {
        head[cursor..cursor + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
    }
    head[cursor..cursor + pointer_size].copy_from_slice(&0usize.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    for value in &env_ptrs {
        head[cursor..cursor + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
    }
    head[cursor..cursor + pointer_size].copy_from_slice(&0usize.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;

    for &(key, value) in auxv {
        head[cursor..cursor + pointer_size].copy_from_slice(&key.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
        head[cursor..cursor + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
    }
    head[cursor..cursor + pointer_size].copy_from_slice(&AT_RANDOM.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    head[cursor..cursor + pointer_size].copy_from_slice(&random_addr.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    if let Some(execfn_addr) = execfn_addr {
        head[cursor..cursor + pointer_size]
            .copy_from_slice(&AT_EXECFN.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
        head[cursor..cursor + pointer_size]
            .copy_from_slice(&execfn_addr.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
    }
    head[cursor..cursor + pointer_size].copy_from_slice(&0usize.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    head[cursor..cursor + pointer_size].copy_from_slice(&0usize.to_le_bytes()[..pointer_size]);

    let strings_len = stack.len() - string_cursor;
    let total = head.len() + strings_len;
    if total > USER_STACK_SIZE {
        return Err(ENOMEM);
    }
    let aligned_total = align_up(total, 16);
    let pad = aligned_total - total;
    let mut out = vec![0u8; aligned_total];
    out[..head.len()].copy_from_slice(&head);
    out[head.len() + pad..].copy_from_slice(&stack[string_cursor..]);
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

fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::{
        loong_segment_pte_flags, map_segment_pages_cow_loongarch, map_segment_pages_cow_riscv,
        riscv_segment_pte_flags, segment_phys_base, AddressSpace, LoongPageTableBuilder,
        PageTablePage, PageTableSpace, SegmentStorage, Sv39PageTableBuilder, VmSpaceToken, EFAULT,
        ENOEXEC, PAGE_SIZE, LA_PTE_W, RISCV_PTE_R, RISCV_PTE_W, RISCV_PTE_X,
    };
    use alloc::vec::Vec;

    const TEST_ELF: &[u8] = &[
        0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0xf3, 0, 1, 0, 0, 0,
        0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 64, 0, 56, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0, 0x78, 0, 0, 0, 0, 0, 0,
        0, 0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 0x00, 0x10, 0x00, 0x40, 0, 0, 0, 0, 4, 0, 0, 0, 0,
        0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0x13, 0, 0, 0, 0, 0, 0, 0,
    ];

    fn write_u16_le(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64_le(buf: &mut [u8], offset: usize, value: u64) {
        buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn build_unaligned_second_load_elf() -> Vec<u8> {
        let mut elf = vec![0u8; 0x2000];

        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // ELFCLASS64
        elf[5] = 1; // little-endian
        elf[6] = 1; // current version

        write_u16_le(&mut elf, 16, 2); // ET_EXEC
        write_u16_le(&mut elf, 18, 0xf3); // EM_RISCV
        write_u32_le(&mut elf, 20, 1); // EV_CURRENT
        write_u64_le(&mut elf, 24, 0x4000_0080); // e_entry
        write_u64_le(&mut elf, 32, 64); // e_phoff
        write_u16_le(&mut elf, 52, 64); // e_ehsize
        write_u16_le(&mut elf, 54, 56); // e_phentsize
        write_u16_le(&mut elf, 56, 2); // e_phnum

        // First PT_LOAD covers only the first page of the file.
        write_u32_le(&mut elf, 64, 1); // PT_LOAD
        write_u32_le(&mut elf, 68, 0b101); // R|X
        write_u64_le(&mut elf, 72, 0x0); // p_offset
        write_u64_le(&mut elf, 80, 0x4000_0000); // p_vaddr
        write_u64_le(&mut elf, 88, 0x4000_0000); // p_paddr
        write_u64_le(&mut elf, 96, 0x1000); // p_filesz
        write_u64_le(&mut elf, 104, 0x1000); // p_memsz
        write_u64_le(&mut elf, 112, 0x1000); // p_align

        // Second PT_LOAD starts mid-page and should preserve bytes from the
        // aligned file page prefix instead of replacing them with zeros.
        write_u32_le(&mut elf, 120, 1); // PT_LOAD
        write_u32_le(&mut elf, 124, 0b110); // R|W
        write_u64_le(&mut elf, 128, 0x1800); // p_offset
        write_u64_le(&mut elf, 136, 0x4000_1800); // p_vaddr
        write_u64_le(&mut elf, 144, 0x4000_1800); // p_paddr
        write_u64_le(&mut elf, 152, 0x200); // p_filesz
        write_u64_le(&mut elf, 160, 0x200); // p_memsz
        write_u64_le(&mut elf, 168, 0x1000); // p_align

        for (index, byte) in elf[0x1000..0x1800].iter_mut().enumerate() {
            *byte = ((index as u8) ^ 0x5a).wrapping_add(1);
        }
        for (index, byte) in elf[0x1800..0x1a00].iter_mut().enumerate() {
            *byte = 0xc0 | ((index as u8) & 0x3f);
        }

        elf
    }

    fn build_minimal_dynamic_elf(mem_size: usize, flags: u32) -> Vec<u8> {
        let mut elf = vec![0u8; 0x1000];
        let file_size = elf.len() as u64;
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // 64-bit
        elf[5] = 1; // little-endian
        elf[6] = 1; // current version
        write_u16_le(&mut elf, 16, 3); // ET_DYN
        write_u16_le(&mut elf, 18, 243); // EM_RISCV
        write_u32_le(&mut elf, 20, 1); // EV_CURRENT
        write_u64_le(&mut elf, 24, 0x100); // e_entry
        write_u64_le(&mut elf, 32, 64); // e_phoff
        write_u16_le(&mut elf, 52, 64); // e_ehsize
        write_u16_le(&mut elf, 54, 56); // e_phentsize
        write_u16_le(&mut elf, 56, 1); // e_phnum

        write_u32_le(&mut elf, 64, 1); // PT_LOAD
        write_u32_le(&mut elf, 68, flags);
        write_u64_le(&mut elf, 72, 0x0); // p_offset
        write_u64_le(&mut elf, 80, 0x0); // p_vaddr
        write_u64_le(&mut elf, 88, 0x0); // p_paddr
        write_u64_le(&mut elf, 96, file_size); // p_filesz
        write_u64_le(&mut elf, 104, mem_size as u64); // p_memsz
        write_u64_le(&mut elf, 112, PAGE_SIZE as u64); // p_align

        elf[0x100..0x104].copy_from_slice(&[0x13, 0, 0, 0]);
        elf
    }

    #[test]
    fn address_space_round_trip() {
        let aspace = AddressSpace::new_user();
        aspace.install_bytes(0x1000, b"hello\0");
        assert_eq!(aspace.read_cstr(0x1000).unwrap(), "hello");

        let addr = aspace.map_anonymous(8192, 0b11).unwrap();
        aspace.write_bytes(addr, b"abc").unwrap();
        assert_eq!(aspace.read_bytes(addr, 3).unwrap(), b"abc");
        aspace.unmap(addr, 8192).unwrap();
    }

    #[test]
    fn read_cstr_across_short_segments() {
        let aspace = AddressSpace::new_user();
        aspace.install_bytes(0x2fff, b"A");
        aspace.install_bytes(0x3000, b"BC\0");
        assert_eq!(aspace.read_cstr(0x2fff).unwrap(), "ABC");
    }

    #[test]
    fn read_and_write_bytes_across_adjacent_segments() {
        let aspace = AddressSpace::new_user();
        aspace.install_bytes(0x4000, b"ab");
        aspace.install_bytes(0x4002, b"cd");
        assert_eq!(aspace.read_bytes(0x4001, 3).unwrap(), b"bcd");
        aspace.write_bytes(0x4001, b"XYZ").unwrap();
        assert_eq!(aspace.read_bytes(0x4000, 4).unwrap(), b"aXYZ");
    }

    #[test]
    fn read_bytes_respects_read_permission() {
        let aspace = AddressSpace::new_user();
        aspace.map_fixed_bytes(0x5000, b"deny", PAGE_SIZE, 0).unwrap();
        assert_eq!(aspace.read_bytes(0x5000, 4), Err(EFAULT));
        assert_eq!(aspace.read_cstr(0x5000), Err(EFAULT));
    }

    #[test]
    fn write_bytes_respects_write_permission() {
        let aspace = AddressSpace::new_user();
        aspace
            .map_fixed_bytes(0x6000, b"ro", PAGE_SIZE, 0b001)
            .unwrap();
        assert_eq!(aspace.write_bytes(0x6000, b"x"), Err(EFAULT));
    }

    #[test]
    fn brk_preserves_requested_break_without_extra_alignment() {
        let aspace = AddressSpace::new_user();
        let base = aspace.brk(None).unwrap();
        let requested = base + PAGE_SIZE * 2 - 1;

        assert_eq!(aspace.brk(Some(requested)).unwrap(), requested);
        aspace.write_bytes(requested, b"X").unwrap();
        assert_eq!(aspace.read_bytes(requested, 1).unwrap(), b"X");
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
    fn dynamic_brk_growth_does_not_overwrite_interp_rx_segment() {
        let aspace = AddressSpace::new_user();
        let program = build_minimal_dynamic_elf(PAGE_SIZE, 0b101);
        let interp = build_minimal_dynamic_elf(PAGE_SIZE * 3, 0b101);
        let loaded = aspace
            .load_elf_images(
                &program,
                Some(&interp),
                &[String::from("/bin/test")],
                &[],
                Some("/bin/test"),
            )
            .unwrap();

        let interp_text = loaded.interp_base + 0x100;
        let interp_end = loaded.interp_base + PAGE_SIZE * 3;

        assert_eq!(
            aspace.read_bytes(interp_text, 4).unwrap(),
            [0x13, 0, 0, 0],
            "interpreter text must be readable before brk growth"
        );

        aspace.brk(Some(interp_end + PAGE_SIZE - 1)).unwrap();

        assert_eq!(
            aspace.read_bytes(interp_text, 4).unwrap(),
            [0x13, 0, 0, 0],
            "brk growth must not overwrite interpreter text"
        );
        assert!(
            aspace.describe_addr(interp_text).contains("prot=0x5"),
            "interpreter text must remain executable after brk growth: {}",
            aspace.describe_addr(interp_text)
        );
    }

    #[test]
    fn dynamic_elf_keeps_future_mmaps_away_from_brk() {
        let aspace = AddressSpace::new_user();
        let program = build_minimal_dynamic_elf(PAGE_SIZE, 0b101);
        let interp = build_minimal_dynamic_elf(PAGE_SIZE * 3, 0b101);
        let _loaded = aspace
            .load_elf_images(
                &program,
                Some(&interp),
                &[String::from("/bin/test")],
                &[],
                Some("/bin/test"),
            )
            .unwrap();

        let brk_base = aspace.brk(None).unwrap();
        let mmap_base = aspace.map_anonymous(PAGE_SIZE, 0b101).unwrap();

        assert!(
            mmap_base >= brk_base + super::USER_MMAP_GUARD_GAP,
            "dynamic ELF should reserve a gap between brk and later mmaps: brk={:#x} mmap={:#x}",
            brk_base,
            mmap_base
        );
    }

    #[test]
    fn brk_growth_does_not_unmap_existing_mmap_segment() {
        let aspace = AddressSpace::new_user();
        let initial_brk = aspace.brk(None).unwrap();
        let mmap_base = initial_brk + PAGE_SIZE;

        aspace
            .map_fixed_bytes(mmap_base, b"LIBC", PAGE_SIZE, 0b101)
            .unwrap();
        assert_eq!(aspace.read_bytes(mmap_base, 4).unwrap(), b"LIBC");

        let result = aspace.brk(Some(mmap_base + PAGE_SIZE)).unwrap();

        assert_eq!(
            result, initial_brk,
            "brk must refuse growth that would overlap an existing mapping"
        );
        assert_eq!(
            aspace.read_bytes(mmap_base, 4).unwrap(),
            b"LIBC",
            "existing mmap segment must remain intact after rejected brk growth"
        );
        assert!(
            aspace.describe_addr(mmap_base).contains("prot=0x5"),
            "mmap segment must remain executable after rejected brk growth: {}",
            aspace.describe_addr(mmap_base)
        );
    }

    #[test]
    fn brk_growth_after_heap_mprotect_can_reclaim_heap_vma() {
        let aspace = AddressSpace::new_user();
        let initial_brk = aspace.brk(None).unwrap();
        let first_request = initial_brk + PAGE_SIZE * 2 - 1;

        assert_eq!(aspace.brk(Some(first_request)).unwrap(), first_request);
        aspace
            .mprotect(initial_brk + PAGE_SIZE, PAGE_SIZE, 0b001)
            .unwrap();

        let second_request = first_request + PAGE_SIZE;
        assert_eq!(aspace.brk(Some(second_request)).unwrap(), second_request);
        assert_eq!(
            aspace.read_bytes(second_request - 1, 1).unwrap(),
            vec![0],
            "heap growth after mprotect should still expose zeroed memory"
        );
    }

    #[test]
    fn brk_growth_can_extend_across_existing_heap_pages() {
        let aspace = AddressSpace::new_user();
        let initial_brk = aspace.brk(None).unwrap();
        let first_request = initial_brk + PAGE_SIZE * 2 - 1;

        assert_eq!(aspace.brk(Some(first_request)).unwrap(), first_request);

        let second_request = first_request + PAGE_SIZE;
        assert_eq!(aspace.brk(Some(second_request)).unwrap(), second_request);
    }

    #[test]
    fn unaligned_second_load_preserves_aligned_file_page_prefix() {
        let aspace = AddressSpace::new_user();
        let elf = build_unaligned_second_load_elf();

        aspace
            .load_static_elf(&elf, &[String::from("/bin/test")], &[])
            .unwrap();

        assert_eq!(
            aspace.read_bytes(0x4000_1000, 32).unwrap(),
            elf[0x1000..0x1020].to_vec()
        );
        assert_eq!(
            aspace.read_bytes(0x4000_1800, 32).unwrap(),
            elf[0x1800..0x1820].to_vec()
        );
    }

    #[test]
    fn reject_non_elf_images() {
        let aspace = AddressSpace::new_user();
        assert_eq!(
            aspace.load_static_elf(b"nope", &[], &[]).unwrap_err(),
            ENOEXEC
        );
    }

    fn riscv_leaf_pte(builder: &mut Sv39PageTableBuilder, vaddr: usize) -> u64 {
        let vpn2 = (vaddr >> 30) & 0x1ff;
        let vpn1 = (vaddr >> 21) & 0x1ff;
        let vpn0 = (vaddr >> 12) & 0x1ff;

        let l2 = builder.table_mut(builder.root_phys);
        let l1_phys = ((l2[vpn2] >> 10) as usize) << 12;
        let l1 = builder.table_mut(l1_phys);
        let l0_phys = ((l1[vpn1] >> 10) as usize) << 12;
        let l0 = builder.table_mut(l0_phys);
        l0[vpn0]
    }

    #[test]
    fn prot_none_riscv_leaf_is_present_but_not_user_accessible() {
        let flags = riscv_segment_pte_flags(0);
        assert_eq!(flags & super::RISCV_PTE_U, 0);
        assert_ne!(flags & super::RISCV_PTE_R, 0);
        assert_ne!(flags & super::RISCV_PTE_W, 0);
    }

    #[test]
    fn prot_none_loong_leaf_is_present_but_not_user_accessible() {
        let flags = loong_segment_pte_flags(0);
        assert_eq!(flags & (super::LA_PTE_PLVL | super::LA_PTE_PLVH), 0);
        assert_eq!(flags & super::LA_PTE_NR, 0);
        assert_ne!(flags & super::LA_PTE_NX, 0);
    }

    #[test]
    fn cow_exec_segments_keep_x_and_clear_w_in_riscv_pte() {
        let parent = AddressSpace::new_user();
        parent
            .map_fixed_bytes(0x4010, &[0x13, 0, 0, 0], 4, 0b111)
            .unwrap();
        let child = parent.clone_private();
        let segment = child.inner.lock().mappings.get(&0x4010).cloned().unwrap();

        assert!(matches!(segment.storage, SegmentStorage::CowParent { .. }));

        let mut builder = Sv39PageTableBuilder::new();
        map_segment_pages_cow_riscv(&mut builder, &segment);
        let pte = riscv_leaf_pte(&mut builder, 0x4010);

        assert_ne!(pte & RISCV_PTE_R, 0);
        assert_ne!(pte & RISCV_PTE_X, 0);
        assert_eq!(pte & RISCV_PTE_W, 0);
    }

    #[test]
    fn cow_fault_rejects_read_only_private_segments() {
        let parent = AddressSpace::new_user();
        parent.map_fixed_bytes(0x4003, b"A", 1, 0b101).unwrap();
        let mut child = parent.clone_private();

        assert_eq!(
            AddressSpace::handle_page_fault(0x4003, &mut child).unwrap_err(),
            EFAULT
        );
        assert!(matches!(
            child.inner.lock().mappings.get(&0x4003).unwrap().storage,
            SegmentStorage::CowParent { .. }
        ));
    }

    #[test]
    fn cow_fault_matches_full_mapped_page_span() {
        let parent = AddressSpace::new_user();
        parent.map_fixed_bytes(0x4003, b"A", 1, 0b011).unwrap();
        let mut child = parent.clone_private();

        AddressSpace::handle_page_fault(0x4fff, &mut child).unwrap();
        assert!(matches!(
            child.inner.lock().mappings.get(&0x4003).unwrap().storage,
            SegmentStorage::Owned { .. }
        ));
    }

    #[test]
    fn write_bytes_promotes_writable_cow_segments() {
        let parent = AddressSpace::new_user();
        parent.map_fixed_bytes(0x5000, b"abcd", 4, 0b011).unwrap();
        let child = parent.clone_private();

        child.write_bytes(0x5000, b"Z").unwrap();

        assert_eq!(parent.read_bytes(0x5000, 4).unwrap(), b"abcd");
        assert_eq!(child.read_bytes(0x5000, 4).unwrap(), b"Zbcd");
        assert!(matches!(
            child.inner.lock().mappings.get(&0x5000).unwrap().storage,
            SegmentStorage::Owned { .. }
        ));
    }

    #[test]
    fn child_cow_reads_survive_parent_unmap() {
        let parent = AddressSpace::new_user();
        parent
            .map_fixed_bytes(0x5000, b"./brk\0arg0\0", PAGE_SIZE, 0b011)
            .unwrap();
        let child = parent.clone_private();

        parent.unmap(0x5000, PAGE_SIZE).unwrap();

        assert_eq!(child.read_cstr(0x5000).unwrap(), "./brk");
        assert_eq!(child.read_cstr(0x5006).unwrap(), "arg0");
    }

    #[test]
    fn cow_fault_prefers_logical_segment_before_page_span_fallback() {
        let parent = AddressSpace::new_user();
        parent
            .map_fixed_bytes(0x4000, &[0u8; 0x400], 0x400, 0b001)
            .unwrap();
        parent
            .map_fixed_bytes(0x4800, &[0u8; 0x400], 0x400, 0b011)
            .unwrap();
        let mut child = parent.clone_private();

        AddressSpace::handle_page_fault(0x4800, &mut child).unwrap();

        assert!(matches!(
            child.inner.lock().mappings.get(&0x4800).unwrap().storage,
            SegmentStorage::Owned { .. }
        ));
    }

    #[test]
    fn riscv_cow_fault_can_update_leaf_pte_in_place() {
        let parent = AddressSpace::new_user();
        parent.map_fixed_bytes(0x5000, b"abcd", 4, 0b011).unwrap();
        let child = parent.clone_private();
        let segment = child.inner.lock().mappings.get(&0x5000).cloned().unwrap();
        let phys_base = segment_phys_base(&segment).unwrap();
        let expected_flags = riscv_segment_pte_flags(segment.area.prot);
        let mut builder = Sv39PageTableBuilder::new();
        map_segment_pages_cow_riscv(&mut builder, &segment);
        let root_phys = builder.root_phys();
        let page_table = PageTableSpace {
            root_phys,
            pages: builder.into_pages(),
        };

        let mut inner = child.inner.lock();
        inner.page_table = Some(page_table);
        inner.token = VmSpaceToken(root_phys);
        inner.dirty = false;
        assert!(inner.update_pte_riscv(0x5000, phys_base, expected_flags));
        assert!(!inner.dirty);
        let pte = {
            let page_table = inner.page_table.as_ref().unwrap();
            let vpn2 = (0x5000 >> 30) & 0x1ff;
            let vpn1 = (0x5000 >> 21) & 0x1ff;
            let vpn0 = (0x5000 >> 12) & 0x1ff;
            let l2 = &page_table.pages[0].0;
            let l1_phys = ((l2[vpn2] >> 10) as usize) << 12;
            let l1 = unsafe { &(*(l1_phys as *const PageTablePage)).0 };
            let l0_phys = ((l1[vpn1] >> 10) as usize) << 12;
            let l0 = unsafe { &(*(l0_phys as *const PageTablePage)).0 };
            l0[vpn0]
        };
        assert_ne!(pte & RISCV_PTE_W, 0);
    }

    #[test]
    fn loongarch_cow_fault_can_update_leaf_pte_in_place() {
        let parent = AddressSpace::new_user();
        parent.map_fixed_bytes(0x5000, b"abcd", 4, 0b011).unwrap();
        let child = parent.clone_private();
        let segment = child.inner.lock().mappings.get(&0x5000).cloned().unwrap();
        let phys_base = segment_phys_base(&segment).unwrap();
        let expected_flags = loong_segment_pte_flags(segment.area.prot);
        let mut builder = LoongPageTableBuilder::new();
        map_segment_pages_cow_loongarch(&mut builder, &segment);
        let root_phys = builder.root_phys();
        let page_table = PageTableSpace {
            root_phys,
            pages: builder.into_pages(),
        };

        let mut inner = child.inner.lock();
        inner.page_table = Some(page_table);
        inner.token = VmSpaceToken(root_phys);
        inner.dirty = false;
        assert!(inner.update_pte_loongarch(0x5000, phys_base, expected_flags));
        assert!(!inner.dirty);
        let pte = {
            let page_table = inner.page_table.as_ref().unwrap();
            let dir3 = (0x5000 >> 39) & 0x1ff;
            let dir2 = (0x5000 >> 30) & 0x1ff;
            let dir1 = (0x5000 >> 21) & 0x1ff;
            let pt = (0x5000 >> 12) & 0x1ff;
            let l2 = &page_table.pages[0].0;
            let l1_phys = (l2[dir3] as usize) & !(PAGE_SIZE - 1);
            let l1 = unsafe { &(*(l1_phys as *const PageTablePage)).0 };
            let l0_phys = (l1[dir2] as usize) & !(PAGE_SIZE - 1);
            let l0 = unsafe { &(*(l0_phys as *const PageTablePage)).0 };
            let leaf_phys = (l0[dir1] as usize) & !(PAGE_SIZE - 1);
            let leaf = unsafe { &(*(leaf_phys as *const PageTablePage)).0 };
            leaf[pt]
        };
        assert_ne!(pte & LA_PTE_W, 0);
    }
}
