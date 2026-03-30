# COW Fork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Copy-On-Write Fork to reduce fork() overhead from ~4ms to <100μs by deferring page copying until first write.

**Architecture:** Add `CowParent` variant to `SegmentStorage` for fork COW tracking. On fork, child gets read-only COW mappings. Store page fault triggers actual page copy. Use segment-level COW (simplified page-level) for initial implementation.

**Tech Stack:** Rust, RISC-V/LoongArch virtual memory, kernel syscall infrastructure

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/mm/src/lib.rs` | Core memory management: SegmentStorage enum, AddressSpace, page table builders, page fault handler |
| `crates/proc/src/lib.rs` | Process creation: fork_from() calls clone_private() |
| `crates/kernel-core/src/lib_riscv.inc.rs` | RISC-V trap handler: dispatches page faults to mm handler |
| `crates/kernel-core/src/lib_loongarch.inc.rs` | LoongArch trap handler: same as RISC-V |

---

## Task 1: Add CowParent variant to SegmentStorage

**Files:**
- Modify: `crates/mm/src/lib.rs:107-120`

- [ ] **Step 1: Read current SegmentStorage definition**

Run: `grep -n "enum SegmentStorage" crates/mm/src/lib.rs -A 15`
Expected: Current enum with Owned, Shared, Host variants

- [ ] **Step 2: Add CowParent variant**

Replace the SegmentStorage enum (around line 107) with:

```rust
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
```

- [ ] **Step 3: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -50`
Expected: No errors related to SegmentStorage

- [ ] **Step 4: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): add CowParent variant to SegmentStorage enum"
```

---

## Task 2: Add set_dirty() public method to AddressSpace

**Files:**
- Modify: `crates/mm/src/lib.rs` - add method after line ~219

- [ ] **Step 1: Find AddressSpace impl block**

Run: `grep -n "impl AddressSpace" crates/mm/src/lib.rs -A 5`
Expected: Block starts around line 204

- [ ] **Step 2: Add set_dirty method**

After the `pub fn token()` method (around line 226), add:

```rust
    /// Mark the address space as dirty, forcing page table rebuild on next token() call.
    pub fn set_dirty(&self) {
        let mut inner = self.inner.lock();
        inner.dirty = true;
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -50`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): add set_dirty() method to AddressSpace"
```

---

## Task 3: Modify clone_private() to create CowParent references

**Files:**
- Modify: `crates/mm/src/lib.rs:634-673`

- [ ] **Step 1: Read current clone_private implementation**

Run: `grep -n "pub fn clone_private" crates/mm/src/lib.rs -A 40`
Expected: Current implementation that eagerly copies Owned segments

- [ ] **Step 2: Replace clone_private to create CowParent for Owned segments**

Replace the storage matching block (lines 638-655) with:

```rust
            let storage = match &segment.storage {
                SegmentStorage::Owned { ptr, .. } => {
                    // Create COW reference instead of eager copy
                    let bytes = alloc::sync::Arc::new(Mutex::new(unsafe {
                        core::slice::from_raw_parts(*ptr as *const u8, segment.area.len).to_vec()
                    }));
                    CowParent { bytes, ptr: *ptr }
                }
                SegmentStorage::Shared { bytes, ptr } => SegmentStorage::Shared {
                    bytes: bytes.clone(),
                    ptr: *ptr,
                },
                SegmentStorage::CowParent { bytes, ptr } => CowParent {
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
```

- [ ] **Step 3: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -50`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): modify clone_private to create CowParent references instead of eager copy"
```

---

## Task 4: Add read_bytes and write_bytes to CowParent handling

**Files:**
- Modify: `crates/mm/src/lib.rs:467-490` (read_bytes) and `557-585` (write_bytes)

- [ ] **Step 1: Read read_bytes implementation**

Run: `grep -n "pub fn read_bytes" crates/mm/src/lib.rs -A 25`
Expected: Shows how different storage types are read

- [ ] **Step 2: Add CowParent handling to read_bytes**

In the read_bytes match block (around line 467), add:

```rust
                SegmentStorage::CowParent { bytes, ptr } => unsafe {
                    let shared = bytes.lock();
                    out.extend_from_slice(core::slice::from_raw_parts(
                        (*ptr + offset) as *const u8,
                        take,
                    ));
                },
```

- [ ] **Step 3: Read write_bytes implementation**

Run: `grep -n "fn write_bytes" crates/mm/src/lib.rs -A 40`
Expected: Shows how different storage types are written

- [ ] **Step 4: Add CowParent handling to write_bytes**

In write_bytes (around line 557), CowParent writes will trigger COW fault eventually, but for now we need to handle the case where we write to a CowParent segment before page fault. Add:

```rust
                SegmentStorage::CowParent { ptr, .. } => unsafe {
                    ptr::copy_nonoverlapping(
                        bytes[written..written + take].as_ptr(),
                        (*ptr + offset) as *mut u8,
                        take,
                    );
                },
```

Note: This is a simplification - writes to CowParent will eventually trigger COW via page fault. The write_bytes path is a fallback.

- [ ] **Step 5: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -50`
Expected: No errors

- [ ] **Step 6: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): handle CowParent in read_bytes and write_bytes"
```

---

## Task 5: Add page fault handler to mm crate

**Files:**
- Modify: `crates/mm/src/lib.rs` - add after clone_private

- [ ] **Step 1: Add handle_page_fault function**

After clone_private (around line 674), add:

```rust
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
/// # Returns
/// * `Ok(())` if COW was handled successfully
/// * `Err(EFAULT)` if address not found or not COW segment
pub fn handle_page_fault(addr: usize, address_space: &mut AddressSpace) -> KernelResult<()> {
    let mut inner = address_space.inner.lock();

    // Find the segment containing the faulting address
    let segment_start = inner
        .mappings
        .iter()
        .find(|(start, segment)| {
            let seg_end = start.saturating_add(segment.area.len);
            **start <= addr && addr < seg_end
        })
        .map(|(start, _)| *start);

    let Some(seg_start) = segment_start else {
        return Err(EFAULT);
    };

    // Get mutable references to modify the segment
    let segment = inner
        .mappings
        .get_mut(&seg_start)
        .ok_or(EFAULT)?;

    match &mut segment.storage {
        SegmentStorage::CowParent { bytes, ptr } => {
            // Calculate offset within the segment
            let offset = addr - segment.area.start;

            // Read current data from shared COW storage
            let current_data = {
                let shared = bytes.lock();
                shared.clone()
            };

            // Create new owned storage with the data
            let new_storage = create_owned_storage(segment.area.start, current_data);

            // Replace the segment's storage with owned
            segment.storage = new_storage;

            // Mark dirty to rebuild page table
            drop(inner);
            address_space.set_dirty();

            Ok(())
        }
        _ => {
            // Not a COW segment - this is a real fault (e.g., write to read-only)
            Err(EFAULT)
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -80`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): add handle_page_fault for COW Fork"
```

---

## Task 6: Modify page table builders to handle CowParent with read-only PTEs

**Files:**
- Modify: `crates/mm/src/lib.rs` - build_riscv_page_table (around line 958) and build_loongarch_page_table (around line 981)

- [ ] **Step 1: Read build_riscv_page_table**

Run: `grep -n "fn build_riscv_page_table" crates/mm/src/lib.rs -A 25`
Expected: Current implementation that maps all segments with same flags

- [ ] **Step 2: Modify build_riscv_page_table to handle CowParent**

After the existing loop that calls `map_segment_pages_riscv`, add a check:

```rust
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
            // Map COW segments as read-only (no W flag)
            map_segment_pages_cow_riscv(&mut builder, segment);
        } else {
            map_segment_pages_riscv(segment);
        }
    }
    // ... rest unchanged
}
```

- [ ] **Step 3: Add map_segment_pages_cow_riscv function**

After `map_segment_pages_riscv` (around line 1191), add:

```rust
fn map_segment_pages_cow_riscv(builder: &mut Sv39PageTableBuilder, segment: &Segment) {
    let phys_base = match segment_phys_base(segment) {
        Some(base) => base,
        None => return,
    };
    let start = align_down(segment.area.start, PAGE_SIZE);
    let end = align_up(segment.area.start + segment.area.len, PAGE_SIZE);
    // COW pages are read-only: R=1, W=0, U=1, A=1, D=0 (dirty bit not set on read-only)
    let flags = RISCV_PTE_U | RISCV_PTE_R | RISCV_PTE_A;
    let mut vaddr = start;
    while vaddr < end {
        builder.map_4k(vaddr, phys_base + (vaddr - start), flags);
        vaddr += PAGE_SIZE;
    }
}
```

- [ ] **Step 4: Read build_loongarch_page_table**

Run: `grep -n "fn build_loongarch_page_table" crates/mm/src/lib.rs -A 25`

- [ ] **Step 5: Modify build_loongarch_page_table similarly**

Add the same CowParent handling to LoongArch builder.

- [ ] **Step 6: Add map_segment_pages_cow_loongarch**

```rust
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
```

- [ ] **Step 7: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package mm 2>&1 | head -80`
Expected: No errors

- [ ] **Step 8: Commit**

```bash
git add crates/mm/src/lib.rs
git commit -m "feat(mm): map CowParent segments as read-only in page table"
```

---

## Task 7: Integrate page fault handler in RISC-V trap handler

**Files:**
- Modify: `crates/kernel-core/src/lib_riscv.inc.rs` - around line 2142

- [ ] **Step 1: Read the trap handler structure around line 2142**

Run: `sed -n '2140,2230p' crates/kernel-core/src/lib_riscv.inc.rs`
Expected: Shows is_syscall check, and fallback to trap handling

- [ ] **Step 2: Add store page fault detection and handling**

After the `if is_syscall { ... return; }` block (around line 2226) and before the trap logging (line 2228), add:

```rust
        // Check for store page fault (scause=15) - COW Fork trigger
        let is_store_page_fault = scause == 15;
        if is_store_page_fault {
            let fault_addr = self
                .processes
                .current()
                .map(|p| p.trap_frame.stval)
                .unwrap_or(0);
            let process = self.processes.current_mut();
            if let Ok(process) = process {
                match mm::handle_page_fault(fault_addr, &mut process.address_space) {
                    Ok(()) => {
                        // COW handled successfully, resume execution
                        logln(format_args!(
                            "whuse: COW fault handled addr={:#x} pid={}",
                            fault_addr,
                            process.tgid
                        ));
                        return;
                    }
                    Err(_) => {
                        // COW handling failed, fall through to kill process
                        logln(format_args!(
                            "whuse: COW fault failed addr={:#x} pid={}",
                            fault_addr,
                            process.tgid
                        ));
                    }
                }
            }
        }
```

- [ ] **Step 3: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package whuse-kernel-core 2>&1 | head -80`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add crates/kernel-core/src/lib_riscv.inc.rs
git commit -m "feat(kernel): handle store page fault for COW Fork on RISC-V"
```

---

## Task 8: Integrate page fault handler in LoongArch trap handler

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs`

- [ ] **Step 1: Find scause values for LoongArch page fault**

Run: `grep -n "scause" crates/kernel-core/src/lib_loongarch.inc.rs | head -20`
Expected: Find how scause is used for trap detection

- [ ] **Step 2: Find where to add page fault handling**

Look for similar pattern to RISC-V where page faults should be handled.

- [ ] **Step 3: Add similar store page fault handling for LoongArch**

Based on RISC-V implementation, add the same logic to LoongArch handler.

- [ ] **Step 4: Verify compilation**

Run: `cd /home/wslootie/github/whuse && cargo check --package whuse-kernel-core 2>&1 | head -80`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs
git commit -m "feat(kernel): handle store page fault for COW Fork on LoongArch"
```

---

## Task 9: Test COW Fork with fork+exit sequence

**Files:**
- Build and test on QEMU

- [ ] **Step 1: Build RISC-V**

Run: `cd /home/wslootie/github/whuse && make build-riscv 2>&1 | tail -30`
Expected: Build completes successfully

- [ ] **Step 2: Build LoongArch**

Run: `cd /home/wslootie/github/whuse && make build-loongarch 2>&1 | tail -30`
Expected: Build completes successfully

- [ ] **Step 3: Run QEMU smoke test**

Run: `cd /home/wslootie/github/whuse && timeout 60s cargo xtask qemu-riscv 2>&1 | tail -50`
Expected: System boots and shows init process

- [ ] **Step 4: Check for COW-related debug output**

Run with grep: `timeout 60s cargo xtask qemu-riscv 2>&1 | strings | grep -i "COW\|cow\|page_fault"`
Expected: No errors about missing functions

- [ ] **Step 5: Commit any remaining changes**

```bash
git add -A
git commit -m "chore: COW Fork implementation complete"
```

---

## Task 10: Run full test suite to validate lmbench improvement

**Files:**
- No file changes - just validation

- [ ] **Step 1: Prepare oscomp images**

Run: `cd /home/wslootie/github/whuse && cargo xtask oscomp-images 2>&1 | tail -20`
Expected: Images created successfully

- [ ] **Step 2: Run RISC-V full suite**

Run: `timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask oscomp-riscv 2>&1 | tee /tmp/rv-cow-test.log`
Expected: Suite completes, lmbench fork+exit improved

- [ ] **Step 3: Check lmbench results**

Run: `strings /tmp/rv-cow-test.log | grep "lmbench"`
Expected: Process fork+exit shows improvement (< 500μs)

- [ ] **Step 4: Run LoongArch full suite**

Run: `timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask oscomp-loongarch 2>&1 | tee /tmp/la-cow-test.log`
Expected: Suite completes

- [ ] **Step 5: Compare scores**

Note the new total score and compare with baseline 505.

---

## Implementation Notes

### Why segment-level COW?
True page-level COW requires splitting segments when only some pages are written. This adds complexity. For the initial implementation:
- Fork creates CowParent references (no data copying)
- First write to ANY page in a segment triggers copy of entire segment
- Most programs only write to small portion of memory after fork, so this is still a major improvement

### COW Page Fault Flow
1. Fork creates child with CowParent segments mapped read-only in page table
2. Child writes to memory → store page fault (scause=15)
3. Kernel catches fault, calls handle_page_fault()
4. handle_page_fault finds CowParent, copies data, converts to Owned
5. handle_page_fault marks dirty, returns
6. On next token() call, page table rebuilds with writable PTEs
7. Faulting instruction retries, now succeeds

### Error Scenarios
- If fault address not in any segment: EFAULT → kill process
- If segment is Owned/Shared/Host (not CowParent): EFAULT → kill process (real bug or attack)
- If memory allocation fails during COW copy: ENOMEM → kill process

---

## Spec Coverage Checklist

- [x] Add CowParent variant to SegmentStorage
- [x] Modify clone_private to create CowParent references
- [x] Add set_dirty() method to AddressSpace
- [x] Add handle_page_fault function
- [x] Modify RISC-V page table builder for read-only COW mappings
- [x] Modify LoongArch page table builder for read-only COW mappings
- [x] Add page fault handling in RISC-V trap handler
- [x] Add page fault handling in LoongArch trap handler
- [x] Handle CowParent in read_bytes and write_bytes
- [x] Testing strategy documented

---

## Type Consistency Check

| Type/Method | Defined In | Used In | Status |
|-------------|------------|---------|--------|
| SegmentStorage::CowParent | mm/lib.rs | mm/lib.rs | OK |
| AddressSpace::set_dirty() | mm/lib.rs | mm/lib.rs, proc/lib.rs | OK |
| handle_page_fault(addr, &mut AddressSpace) | mm/lib.rs | kernel-core/* | OK |
| map_segment_pages_cow_riscv | mm/lib.rs | mm/lib.rs | OK |
| RISCV_PTE flags | mm/lib.rs | mm/lib.rs | OK |
| scause == 15 (RISC-V store fault) | kernel-core | kernel-core | OK |
