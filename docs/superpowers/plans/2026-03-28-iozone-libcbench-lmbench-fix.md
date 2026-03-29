# iozone/libcbench/lmbench Score Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix iozone/libcbench/lmbench scoring 0 across all configurations by ensuring musl binaries load musl libraries instead of glibc.

**Architecture:** Two-part fix:
1. Fix execve interpreter fallback order (syscall) - universal
2. Remove riscv64-linux-gnu glibc symlinks from LoongArch runtime (LoongArch-specific)

**Tech Stack:** Rust (whuse kernel), QEMU, OSCOMP testsuite

---

## Problem Analysis

From log analysis:
```
./iozone: error while loading shared libraries: libc.so: cannot stat shared object: Error 20
```

**Root Cause Chain:**
1. Kernel's `prepare_oscomp_runtime_layout` creates symlinks like `/lib/riscv64-linux-gnu/tls/libc.so -> /glibc/lib/libc.so.6`
2. When musl iozone runs, its dynamic linker probes `/lib/riscv64-linux-gnu/tls/libc.so`
3. This path exists (as glibc!) so musl links against glibc libc
4. glibc libc is incompatible with musl-linked binary → Error 20

**Key Files to Modify:**
- `crates/syscall/src/lib.rs` - interpreter fallback order (ALL architectures)
- `crates/kernel-core/src/lib_loongarch.inc.rs` - symlink creation (LoongArch ONLY)
- `crates/kernel-core/src/lib_riscv.inc.rs` - symlink creation (RISC-V - verify only)

---

## Task 1: Fix Interpreter Fallback Order in execve (ALL Architectures)

**Files:**
- Modify: `crates/syscall/src/lib.rs:2385-2390`

**Current Code (lines 2385-2390):**
```rust
for candidate in [
    interp_path.as_str(),
    "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
    "/lib/ld-linux-riscv64-lp64d.so.1",
    "/musl/lib/libc.so",
] {
```

**Problem:** For LoongArch, the glibc interpreter is tried BEFORE musl. The ELF's own PT_INTERP should be tried first, but musl should be tried before glibc fallbacks.

- [ ] **Step 1: Modify execve interpreter fallback order**

Change lines 2385-2390 to put musl BEFORE glibc fallbacks:

```rust
for candidate in [
    interp_path.as_str(),
    "/musl/lib/libc.so",
    "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
    "/lib/ld-linux-riscv64-lp64d.so.1",
] {
```

- [ ] **Step 2: Verify build**

Run: `make build-loongarch 2>&1 | tail -20`
Expected: Build succeeds without errors

Run: `make build-riscv 2>&1 | tail -20`
Expected: Build succeeds without errors

- [ ] **Step 3: Commit**

```bash
git add crates/syscall/src/lib.rs
git commit -m "fix: prefer musl interpreter before glibc in execve fallback"
```

---

## Task 2: Remove Risky Glibc Compatibility Symlinks from LoongArch

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs`

**Problem:** On LoongArch, creating `/lib/riscv64-linux-gnu/*` directories and symlinks to glibc is incorrect. Musl binaries on LoongArch should NOT search riscv64 paths.

**Changes Required:**

1. **REMOVE these directory creation lines (~2777-2778):**
```rust
"/lib/riscv64-linux-gnu",
"/lib/riscv64-linux-gnu/tls",
```

2. **REMOVE these symlink lines (~2867-2872):**
```rust
("/lib/riscv64-linux-gnu/libc.so.6", "/glibc/lib/libc.so.6"),
("/lib/riscv64-linux-gnu/libm.so.6", "/glibc/lib/libm.so.6"),
("/lib/riscv64-linux-gnu/libc.so", "/glibc/lib/libc.so.6"),
("/lib/riscv64-linux-gnu/libm.so", "/glibc/lib/libm.so.6"),
("/lib/riscv64-linux-gnu/tls/libc.so", "/glibc/lib/libc.so.6"),
("/lib/riscv64-linux-gnu/tls/libm.so", "/glibc/lib/libm.so.6"),
```

- [ ] **Step 1: Read lib_loongarch.inc.rs around lines 2770-2920 to confirm exact line numbers**

- [ ] **Step 2: Remove riscv64-linux-gnu directory creation (lines ~2777-2778)**

- [ ] **Step 3: Remove riscv64-linux-gnu -> glibc symlinks (lines ~2867-2872)**

- [ ] **Step 4: Verify build**

Run: `make build-loongarch 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 5: Commit**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs
git commit -m "fix: remove riscv64-linux-gnu glibc symlinks from LoongArch runtime"
```

---

## Task 3: Verify RISC-V Symlinks Are Not Affecting Musl

**Files:**
- Read: `crates/kernel-core/src/lib_riscv.inc.rs` (NO modifications unless issues found)

**Analysis:** On RISC-V, the `/lib/riscv64-linux-gnu/tls/libc.so -> /glibc/lib/libc.so.6` symlinks ARE correct for glibc binaries. The question is whether musl binaries on RISC-V are affected.

**Musl on RISC-V should use:**
- `/lib/ld-musl-riscv64.so.1 -> /musl/lib/libc.so` (line ~2838)

**Decision Table:**
| Condition | Action |
|-----------|--------|
| Musl binaries use `/lib/ld-musl-riscv64.so.1` | No change needed |
| Musl binaries incorrectly probe `/lib/riscv64-linux-gnu/*` | Add runtime guard to skip glibc symlinks for musl paths |

- [ ] **Step 1: Verify musl on RISC-V uses ld-musl-riscv64.so.1**

The existing symlink `/lib/ld-musl-riscv64.so.1 -> /musl/lib/libc.so` (line 2838) should handle musl correctly.

**Expected:** NO changes needed for RISC-V if musl uses ld-musl-riscv64.so.1.

- [ ] **Step 2: Document findings**

If no changes needed, document that RISC-V musl uses correct loader path.

---

## Task 4: Build OSComp Images

- [ ] **Step 1: Build images**

Run: `cargo xtask oscomp-images 2>&1`
Expected: Both sdcard-rv.img and sdcard-la.img built successfully

---

## Task 5: Regression Testing - RISC-V

**Run RISC-V full test and verify no regressions:**

- [ ] **Step 1: Run RISC-V test suite**

Run:
```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$REPO_ROOT/target/oscomp/sdcard-rv.img" cargo xtask oscomp-riscv > /tmp/rv-test.log 2>&1
```

**Success Criteria:**
- `whuse-oscomp-suite-done` present in log
- No kernel panic or `pid 1 (init)` crash
- `basic-musl` and `basic-glibc` groups complete (no regression)
- `busybox-musl` and `busybox-glibc` groups complete (no regression)
- `iozone-musl` and `iozone-glibc` produce output (not "error while loading")
- `libctest-musl` completes or shows pthread_cancel (expected, not full hang)
- `whuse-oscomp-step-end:iozone*` markers present

**Check for regression:**
Run: `strings /tmp/rv-test.log | grep "whuse-oscomp-step-end:basic"`
Expected: Shows both musl and glibc basic tests ending

---

## Task 6: Regression Testing - LoongArch

**Run LoongArch full test and verify no regressions:**

- [ ] **Step 1: Run LoongArch test suite**

Run:
```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$REPO_ROOT/target/oscomp/sdcard-la.img" cargo xtask oscomp-loongarch > /tmp/la-test.log 2>&1
```

**Success Criteria:**
- `whuse-oscomp-suite-done` present in log
- No kernel panic or `pid 1 (init)` crash
- `basic-musl` group completes (no regression from line 36 in score: 36)
- `iozone-musl` produces output (not "error while loading")
- `whuse-oscomp-step-end:iozone*` markers present

**Check for regression:**
Run: `strings /tmp/la-test.log | grep "whuse-oscomp-step-end:basic"`
Expected: Shows musl basic test ending

---

## Task 7: Score Verification

**Extract score markers from logs:**

- [ ] **Step 1: Check RISC-V scores**

Run: `strings /tmp/rv-test.log | grep "iozone.*throughput\|libcbench\|lmbench" | head -20`

- [ ] **Step 2: Check LoongArch scores**

Run: `strings /tmp/la-test.log | grep "iozone.*throughput\|libcbench\|lmbench" | head -20`

---

## Summary of Changes

| File | Change | Scope |
|------|--------|-------|
| `crates/syscall/src/lib.rs:2385-2390` | Put `/musl/lib/libc.so` before glibc fallbacks | ALL architectures |
| `crates/kernel-core/src/lib_loongarch.inc.rs` | Remove `/lib/riscv64-linux-gnu/*` directories and symlinks | LoongArch ONLY |
| `crates/kernel-core/src/lib_riscv.inc.rs` | No changes (verify musl uses ld-musl-riscv64.so.1) | RISC-V verification |

---

## Success Criteria

1. iozone produces output without "error while loading shared libraries"
2. libcbench produces numeric scores (or at least runs)
3. lmbench produces numeric scores (or at least runs)
4. basic tests on both architectures still pass (no regression from current scores)
5. busybox tests on both architectures still pass (no regression)
6. No new compilation errors

---

## If Fix Is Insufficient

If after these changes iozone/libcbench/lmbench still fail:

1. **Check PT_INTERP**: Verify musl iozone binary's actual interpreter path
2. **Check ld.so.cache**: `/etc/ld.so.cache` might contain stale entries
3. **Consider runtime guard**: If musl binary runs from `/musl/`, skip glibc symlinks
