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
const USER_STACK_SIZE: usize = 0x80_000;
const USER_STACK_TOP: usize = 0x7fff_f000;
const USER_MMAP_BASE: usize = 0x5000_0000;
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
    Owned { bytes: Vec<u8>, ptr: usize },
    Host { ptr: usize, len: usize },
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
        self.map_owned(start, vec![0; aligned], prot)?;
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

        let new_break = align_up(requested_break, 16);
        let old_break = self.inner.lock().program_break;
        if new_break > old_break {
            let map_start = align_up(old_break, PAGE_SIZE);
            let map_end = align_up(new_break, PAGE_SIZE);
            if map_end > map_start {
                self.map_fixed_bytes(map_start, &[], map_end - map_start, DEFAULT_PROT)?;
            }
        } else if new_break < old_break {
            let unmap_start = align_up(new_break, PAGE_SIZE);
            let unmap_end = align_up(old_break, PAGE_SIZE);
            if unmap_end > unmap_start {
                let mut inner = self.inner.lock();
                unmap_range_inner(&mut inner, unmap_start, unmap_end - unmap_start)?;
            }
        }

        self.inner.lock().program_break = new_break;
        Ok(new_break)
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
        let mut cursor = addr;
        let mut remaining = len;
        while remaining > 0 {
            let (segment, offset) = find_segment(&inner.mappings, cursor, 1)?;
            let available = segment.area.len.saturating_sub(offset);
            let take = available.min(remaining);
            match &segment.storage {
                SegmentStorage::Owned { ptr, .. } => unsafe {
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
            }
            cursor = cursor.checked_add(take).ok_or(EFAULT)?;
            remaining -= take;
        }
        Ok(out)
    }

    pub fn write_bytes(&self, addr: usize, bytes: &[u8]) -> KernelResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock();
        let mut cursor = addr;
        let mut written = 0usize;
        while written < bytes.len() {
            let (segment, offset) = find_segment_mut(&mut inner.mappings, cursor, 1)?;
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
                SegmentStorage::Owned { ptr, .. } => {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(*ptr as *const u8, segment.area.len).to_vec()
                    };
                    create_owned_storage(*start, bytes)
                }
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
        let header = ElfHeader::parse(image)?;
        let phdr_addr = find_phdr_vaddr(&header, image)?;
        if header.program_header_size != 56 || header.class != 2 || header.endianness != 1 {
            return Err(ENOEXEC);
        }
        if header.program_header_num == 0 {
            return Err(ENOEXEC);
        }

        self.clear();
        let mut highest_end = USER_HEAP_BASE;
        let mut load_segments = 0usize;
        for index in 0..header.program_header_num {
            let offset = header.program_header_offset + index * header.program_header_size;
            let ph = ProgramHeader::parse(image, offset)?;
            if ph.segment_type != 1 {
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
            self.map_fixed_bytes(ph.vaddr, bytes, ph.mem_size, elf_flags_to_prot(ph.flags))?;
            let seg_end = ph.vaddr.checked_add(ph.mem_size).ok_or(ENOEXEC)?;
            highest_end = highest_end.max(align_up(seg_end, PAGE_SIZE));
            load_segments += 1;
        }
        if load_segments == 0 {
            return Err(ENOEXEC);
        }

        {
            let mut inner = self.inner.lock();
            inner.program_break = highest_end.max(USER_HEAP_BASE);
        }
        let stack_top = USER_STACK_TOP;
        let stack_base = stack_top - USER_STACK_SIZE;
        self.map_fixed_bytes(stack_base, &[], USER_STACK_SIZE, DEFAULT_PROT)?;
        let auxv = [
            (3usize, phdr_addr),                  // AT_PHDR
            (4usize, header.program_header_size), // AT_PHENT
            (5usize, header.program_header_num),  // AT_PHNUM
            (6usize, PAGE_SIZE),                  // AT_PAGESZ
            (7usize, 0usize),                     // AT_BASE
            (8usize, 0usize),                     // AT_FLAGS
            (9usize, header.entry),               // AT_ENTRY
            (11usize, 0usize),                    // AT_UID
            (12usize, 0usize),                    // AT_EUID
            (13usize, 0usize),                    // AT_GID
            (14usize, 0usize),                    // AT_EGID
            (23usize, 0usize),                    // AT_SECURE
        ];
        let stack_image = build_initial_stack(args, envs, stack_top, &auxv)?;
        let used = stack_image.len();
        self.write_bytes(stack_top - used, &stack_image)?;
        Ok(LoadedImage {
            entry: header.entry,
            stack_pointer: stack_top - used,
        })
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
            map_segment_pages_riscv(&mut builder, segment);
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
            map_segment_pages_loongarch(&mut builder, segment);
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

fn riscv_make_leaf_pte(paddr: usize, flags: u64) -> u64 {
    ((paddr >> 12) as u64) << 10 | flags | RISCV_PTE_V
}

fn riscv_make_table_pte(paddr: usize) -> u64 {
    ((paddr >> 12) as u64) << 10 | RISCV_PTE_V
}

fn riscv_segment_pte_flags(prot: usize) -> u64 {
    let mut flags = RISCV_PTE_U | RISCV_PTE_A | RISCV_PTE_D;
    if prot & 0b001 != 0 {
        flags |= RISCV_PTE_R;
    }
    if prot & 0b010 != 0 {
        flags |= RISCV_PTE_W;
    }
    if prot & 0b100 != 0 {
        flags |= RISCV_PTE_X;
    }
    if flags & (RISCV_PTE_R | RISCV_PTE_W | RISCV_PTE_X) == 0 {
        flags |= RISCV_PTE_R | RISCV_PTE_W;
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
        SegmentStorage::Host { ptr, .. } => {
            ((ptr & (PAGE_SIZE - 1)) == page_offset).then_some(ptr.saturating_sub(page_offset))
        }
    }
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
    stack_top: usize,
    auxv: &[(usize, usize)],
) -> KernelResult<Vec<u8>> {
    let pointer_size = size_of::<usize>();
    let strings = args.iter().chain(envs.iter()).collect::<Vec<_>>();
    let total_strings_len = strings.iter().map(|entry| entry.len() + 1).sum::<usize>();
    let pointer_count = 1 + args.len() + 1 + envs.len() + 1 + (auxv.len() + 1) * 2;
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

    let mut head = vec![0u8; pointer_size * pointer_count];
    let argc = args.len();
    let mut cursor = 0usize;
    head[..pointer_size].copy_from_slice(&argc.to_le_bytes()[..pointer_size]);
    cursor += pointer_size;
    for value in &pointers {
        head[cursor..cursor + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
    }
    for &(key, value) in auxv {
        head[cursor..cursor + pointer_size].copy_from_slice(&key.to_le_bytes()[..pointer_size]);
        cursor += pointer_size;
        head[cursor..cursor + pointer_size].copy_from_slice(&value.to_le_bytes()[..pointer_size]);
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
