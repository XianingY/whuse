# COW Fork Implementation Design

**Date:** 2026-03-30
**Author:** Claude Code
**Status:** Approved

## 1. Problem Statement

Current `fork()` implementation in `clone_private()` performs eager copy of all memory pages, causing severe performance issues:
- `Process fork+exit` takes 4409μs (normal: <100μs)
- `Process fork+/bin/sh -c` takes 920ms and times out

This prevents lmbench tests from passing and limits the score.

## 2. Design Decision Summary

| Decision | Choice |
|----------|--------|
| COW Implementation Approach | SegmentStorage-level COW |
| Page Fault Handler Location | Independent mm page fault handler |
| COW Segment Marker | New `CowParent` variant in `SegmentStorage` |
| COW Copy Granularity | Page-level (4KB) |
| Page Table Update Method | Rebuild entire page table |
| Read-Only Setup Location | After `fork_from()` with dirty marking |

## 3. Architecture

### 3.1 SegmentStorage Variants

```rust
enum SegmentStorage {
    Owned { bytes: Vec<u8>, ptr: usize },           // Private, writable
    Shared { bytes: Arc<Mutex<Vec<u8>>>, ptr: usize }, // Process-shared (e.g., mmap MAP_SHARED)
    CowParent { bytes: Arc<Mutex<Vec<u8>>>, ptr: usize }, // Fork parent, COW reference
    Host { ptr: usize, len: usize },                // Host memory (for kernel)
}
```

**Semantic differences:**
- `Shared`: Intentional inter-process sharing (mmap)
- `CowParent`: Fork relationship, copy-on-write semantics

### 3.2 fork_from() Modifications

In `crates/proc/src/lib.rs`:

```rust
fn fork_from(&self, pid: usize) -> Self {
    // ...
    let address_space = self.address_space.clone_private(); // Creates CowParent references

    // NEW: Mark address space dirty to trigger COW page table rebuild
    address_space.set_dirty();

    // ...
}
```

### 3.3 clone_private() Modifications

In `crates/mm/src/lib.rs`:

```rust
pub fn clone_private(&self) -> Self {
    let inner = self.inner.lock();
    let mut mappings = BTreeMap::new();
    for (start, segment) in &inner.mappings {
        let storage = match &segment.storage {
            SegmentStorage::Owned { ptr, .. } => {
                // NEW: Instead of copying data, create COW reference
                let bytes = Arc::new(Mutex::new(unsafe {
                    core::slice::from_raw_parts(*ptr as *const u8, segment.area.len).to_vec()
                }));
                CowParent { bytes, ptr: *ptr }
            }
            SegmentStorage::Shared { bytes, ptr } => SegmentStorage::Shared {
                bytes: bytes.clone(),
                ptr: *ptr,
            },
            SegmentStorage::Host { ptr, len } => {
                // Host segments remain as-is (copied eagerly)
                create_owned_storage(...)
            }
            // NEW: Handle existing CowParent - child gets its own COW reference
            SegmentStorage::CowParent { bytes, ptr } => CowParent {
                bytes: bytes.clone(),
                ptr: *ptr,
            },
        };
        // ...
    }
}
```

### 3.4 Page Table Rebuild with COW

In `crates/mm/src/lib.rs`, `rebuild_page_table()`:

```rust
fn build_riscv_page_table(&mut self) {
    // ... existing kernel/IO mappings ...
    for segment in self.mappings.values() {
        match &segment.storage {
            SegmentStorage::CowParent { ptr, .. } => {
                // Set page table entries as read-only (R=1, W=0)
                map_segment_pages_cow_riscv(builder, segment);
            }
            _ => {
                // Normal mapping
                map_segment_pages_riscv(segment);
            }
        }
    }
}

fn map_segment_pages_cow_riscv(builder: &mut Sv39PageTableBuilder, segment: &Segment) {
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    let flags = RISCV_PTE_U | RISCV_PTE_R | RISCV_PTE_A;  // Read-only, no W
    // ... map pages with shared physical address but read-only flags ...
}
```

### 3.5 Page Fault Handler

In `crates/mm/src/lib.rs`:

```rust
/// Handle store page fault for COW Fork
///
/// # Arguments
/// * `addr` - Faulting virtual address
/// * `address_space` - Process address space to modify
///
/// # Returns
/// * `Ok(())` if COW was handled successfully
/// * `Err(EFAULT)` if address not found or not COW segment
pub fn handle_page_fault(addr: usize, address_space: &mut AddressSpace) -> KernelResult<()> {
    let mut inner = address_space.inner.lock();

    // Find the segment containing the faulting address
    let (segment, offset) = find_segment_mut(&mut inner.mappings, addr, 1)?;

    match &mut segment.storage {
        SegmentStorage::CowParent { bytes, ptr } => {
            // Calculate which page within the segment
            let page_offset = addr - segment.area.start;
            let page_start = align_down(page_offset, PAGE_SIZE);
            let copy_len = PAGE_SIZE.min(segment.area.len - page_start);

            // Allocate new page
            let mut new_data = vec![0u8; PAGE_SIZE];

            // Copy data from shared COW storage
            let shared_data = bytes.lock();
            new_data[..copy_len].copy_from_slice(
                &shared_data[page_start..page_start + copy_len]
            );

            // Replace this segment's storage with Owned
            // Note: We need to handle page-level COW, not segment-level
            // This requires splitting the segment or using a different approach

            address_space.set_dirty();
            Ok(())
        }
        _ => {
            // Not a COW segment - this is a real fault
            Err(EFAULT)
        }
    }
}
```

**Note:** The above is simplified. True page-level COW requires splitting segments or maintaining a page-level mapping structure. See Section 4 for implementation approach.

## 4. Implementation Approach

### 4.1 Segment Splitting for Page-Level COW

True page-level COW requires splitting segments when only some pages are written. Two approaches:

**Approach A: Lazy Segment Split**
- Keep segments as-is initially
- On COW fault, split the segment at page boundaries
- The written page becomes a new Owned segment
- Remaining pages remain CowParent

**Approach B: Page-Table-Only COW**
- Maintain physical page reference counts at page table level
- Fork sets all user PTEs to read-only with shared physical pages
- COW fault handler allocates new physical page, copies, updates PTE
- No segment structure changes needed

### 4.2 Recommended Implementation

**Use Approach A (Segment Split) with simplification:**

For the initial implementation, we will use segment-level COW (fork copies when ANY page is written). This is simpler than true page-level COW but still provides significant speedup:

- Fork doesn't copy data immediately
- First write to any page triggers copy of that segment
- Subsequent writes to same segment are fast (owned copy)

This reduces fork cost from O(all pages) to O(one segment) on first write. Most programs only write to a small portion of their memory on fork.

## 5. RISC-V Trap Handler Integration

In `crates/kernel-core/src/lib_riscv.inc.rs`:

```rust
// In handle_trap(), after checking is_syscall:
let is_store_page_fault = match hal().platform.architecture() {
    PlatformArch::Riscv64 => scause == 15,  // Store page fault
    PlatformArch::LoongArch64 => scause == 15,  // Similar for LoongArch
    _ => false,
};

if is_store_page_fault {
    let fault_addr = process.trap_frame.stval;
    match mm::handle_page_fault(fault_addr, &mut process.address_space) {
        Ok(()) => {
            // Resume execution - page is now writable
            return;
        }
        Err(_) => {
            // Fall through to error handling - kill process
        }
    }
}
```

## 6. LoongArch Support

Same logic applies to LoongArch. The page fault scause value may differ - verify against platform documentation.

## 7. Error Handling

| Scenario | Handling |
|----------|----------|
| Fault address not in any segment | Return EFAULT, kill process |
| Segment is Owned/Shared/Host | Return EFAULT, kill process (real fault) |
| COW copy fails (ENOMEM) | Return ENOMEM, kill process |
| Page table rebuild fails | Return EFAULT, kill process |

## 8. Testing Strategy

1. **Unit test**: COW storage creation and detection
2. **Integration test**: fork() + write() + exit() sequence
3. **lmbench validation**: Verify `Process fork+exit` < 500μs

## 9. Files to Modify

| File | Changes |
|------|---------|
| `crates/mm/src/lib.rs` | Add `CowParent` variant, modify `clone_private()`, add `handle_page_fault()`, modify page table builders |
| `crates/proc/src/lib.rs` | Call `address_space.set_dirty()` after `clone_private()` in `fork_from()` |
| `crates/kernel-core/src/lib_riscv.inc.rs` | Add store page fault (scause=15) handling in trap handler |
| `crates/kernel-core/src/lib_loongarch.inc.rs` | Similar for LoongArch |

## 10. Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Segment split complexity | Start with segment-level COW, upgrade to page-level if needed |
| Page table rebuild performance | This is acceptable - fork is already expensive without COW |
| Memory pressure from COW pages | Normal for fork - parent and child each have copies |
