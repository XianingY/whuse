# OSCOMP Score Maximization: iozone/libcbench/lmbench/lua Fix Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix iozone, libcbench, lmbench, and lua scoring categories to produce non-zero scores on both RISC-V and LoongArch.

**Architecture:** Three-phase approach:
1. Phase 1: Fix iozone loader/runtime issues (RISC-V first, then LoongArch)
2. Phase 2: Fix libctest to unlock downstream categories (libcbench/lmbench)
3. Phase 3: Fix lua scoring

**Tech Stack:** Rust (whuse kernel), QEMU, OSCOMP testsuite, shell scripts

---

## Background Analysis

### Current Score State
- **Total:** 406 (LoongArch fix pending deployment should restore to ~560)
- **glibc-rv:** 155, **musl-rv:** 251 (working)
- **glibc-la:** 0, **musl-la:** 0 (LoongArch execution broken, fix deployed)

### Score Breakdown (086942cb submission)
| Category | Current | Target | Blocker |
|----------|---------|--------|---------|
| iozone | 0 | ? | "error while loading shared libraries: libc.so" |
| libcbench | 0 | ? | Downstream of libctest |
| lmbench | 0 | ? | Downstream of libctest |
| lua | 0 | ? | Downstream of libctest |

### Root Causes Identified
1. **iozone:** musl binaries load glibc libraries due to interpreter fallback order
2. **libctest:** pthread_cancel livelock blocks progression to later categories
3. **libcbench/lmbench/lua:** unreachable until libctest passes

---

## Phase 1: Fix iozone Loader/Runtime Issues

### Task 1: Verify iozone Failure Signature on RISC-V

**Files:**
- Check: `tools/dev/run_oscomp_stage2.sh`
- Check: `crates/syscall/src/lib.rs`

- [ ] **Step 1: Build oscomp images**

```bash
cd /home/wslootie/github/whuse
cargo xtask oscomp-images 2>&1
```

Expected: Both `sdcard-rv.img` and `sdcard-la.img` built successfully

- [ ] **Step 2: Run RISC-V iozone test**

```bash
cd /home/wslootie/github/whuse
timeout 600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-iozone.log 2>&1
```

- [ ] **Step 3: Check for loader error**

```bash
strings /tmp/rv-iozone.log | grep -E "error while loading|libc.so"
```

Expected evidence of loader error: `error while loading shared libraries: libc.so: cannot stat shared object`

- [ ] **Step 4: Verify execve interpreter fallback order**

Check current order in `crates/syscall/src/lib.rs`:
```bash
grep -A10 "for candidate in \[" crates/syscall/src/lib.rs | head -15
```

Expected: `/musl/lib/libc.so` should be BEFORE glibc fallbacks

---

### Task 2: Verify iozone Failure on LoongArch

- [ ] **Step 1: Run LoongArch iozone test**

```bash
cd /home/wslootie/github/whuse
timeout 600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-la.img" cargo xtask qemu-loongarch > /tmp/la-iozone.log 2>&1
```

- [ ] **Step 2: Check for loader error**

```bash
strings /tmp/la-iozone.log | grep -E "error while loading|libc.so"
```

---

### Task 3: Fix iozone on RISC-V (If Loader Error Persists)

**Files:**
- Modify: `crates/syscall/src/lib.rs` (interpreter fallback order)

**Current code (lines ~2385-2390):**
```rust
for candidate in [
    interp_path.as_str(),
    "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
    "/lib/ld-linux-riscv64-lp64d.so.1",
    "/musl/lib/libc.so",
] {
```

**Problem:** On RISC-V, glibc interpreter path is tried before musl.

**Fix:**
```rust
for candidate in [
    interp_path.as_str(),
    "/musl/lib/libc.so",
    "/glibc/lib/ld-linux-loongarch-lp64d.so.1",
    "/lib/ld-linux-riscv64-lp64d.so.1",
] {
```

- [ ] **Step 1: Modify execve interpreter fallback order**

Edit `crates/syscall/src/lib.rs` lines ~2385-2390

- [ ] **Step 2: Verify build**

```bash
cd /home/wslootie/github/whuse && make build-riscv 2>&1 | tail -10
```

Expected: Build succeeds

- [ ] **Step 3: Rebuild images**

```bash
cargo xtask oscomp-images 2>&1
```

- [ ] **Step 4: Retest iozone on RISC-V**

```bash
timeout 600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-iozone-fixed.log 2>&1
```

- [ ] **Step 5: Verify no loader error**

```bash
strings /tmp/rv-iozone-fixed.log | grep -E "error while loading"
```

Expected: No error output, iozone produces throughput numbers

---

### Task 4: Fix iozone on LoongArch

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs` (symlink creation)

**Problem:** On LoongArch, `/lib/riscv64-linux-gnu/*` symlinks to glibc cause musl binaries to load glibc libraries.

- [ ] **Step 1: Find and remove riscv64-linux-gnu symlinks in LoongArch**

Search for symlink creation in `crates/kernel-core/src/lib_loongarch.inc.rs`:
```bash
grep -n "riscv64-linux-gnu" crates/kernel-core/src/lib_loongarch.inc.rs | head -20
```

- [ ] **Step 2: Remove incorrect symlinks**

Remove lines creating `/lib/riscv64-linux-gnu/*` symlinks to glibc

- [ ] **Step 3: Verify build**

```bash
cd /home/wslootie/github/whuse && make build-loongarch 2>&1 | tail -10
```

Expected: Build succeeds

- [ ] **Step 4: Rebuild images and retest**

```bash
cargo xtask oscomp-images 2>&1
timeout 600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-la.img" cargo xtask qemu-loongarch > /tmp/la-iozone-fixed.log 2>&1
```

- [ ] **Step 5: Verify no loader error**

```bash
strings /tmp/la-iozone-fixed.log | grep -E "error while loading"
```

Expected: No error output

---

### Task 5: Commit iozone Fixes

- [ ] **Step 1: Commit RISC-V iozone fix**

```bash
git add crates/syscall/src/lib.rs
git commit -m "fix: prefer musl interpreter before glibc in execve fallback for RISC-V"
```

- [ ] **Step 2: Commit LoongArch iozone fix**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs
git commit -m "fix: remove riscv64-linux-gnu glibc symlinks from LoongArch runtime"
```

---

## Phase 2: Fix libctest to Unlock Downstream Categories

### Task 6: Analyze libctest Failure

**Context:** libctest blocks progression to libcbench/lmbench/lua

**Current Issue:** `pthread_cancel_points` causes livelock in post-cancel futex handling

- [ ] **Step 1: Run libctest and observe failure**

```bash
timeout 300s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-libctest.log 2>&1 &
sleep 180 && strings /tmp/rv-libctest.log | grep -E "pthread_cancel|libctest|futex" | tail -30
```

- [ ] **Step 2: Check AGENTS.md Section 10.1 for known issue**

```bash
grep -A30 "10.1 pthread_cancel" docs/superpowers/plans/*.md 2>/dev/null || grep -A30 "pthread_cancel" AGENTS.md
```

---

### Task 7: Implement libctest Cancellation Fix

**Files:**
- Modify: `crates/proc/src/lib.rs` (signal_frame_pending, cancellation_pending)
- Modify: `crates/kernel-core/src/lib.rs` (dispatch_pending_signals)
- Modify: `crates/syscall/src/lib.rs` (sys_futex interruption)

**Refer to:** AGENTS.md Section 10.1 for root cause analysis

- [ ] **Step 1: Review current cancellation handling**

Check `dispatch_pending_signals` in `crates/kernel-core/src/lib.rs`

- [ ] **Step 2: Review futex interruption in sys_futex**

Check `sys_futex` in `crates/syscall/src/lib.rs` for EINTR handling

- [ ] **Step 3: Implement minimal fix**

Based on Section 10.1 analysis, implement fix for:
1. Ensure SIGCANCEL properly interrupts blocking syscalls
2. Allow musl's pthread_exit to complete properly

- [ ] **Step 4: Verify build**

```bash
cd /home/wslootie/github/whuse && make build-riscv 2>&1 | tail -10
```

- [ ] **Step 5: Test libctest**

```bash
timeout 300s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-libctest-fixed.log 2>&1
strings /tmp/rv-libctest-fixed.log | grep "libctest.*end\|libctest.*pass\|libctest.*fail"
```

---

### Task 8: Commit libctest Fix

- [ ] **Step 1: Commit libctest fix**

```bash
git add crates/proc/src/lib.rs crates/kernel-core/src/lib.rs crates/syscall/src/lib.rs
git commit -m "fix: resolve pthread_cancel livelock in libctest"
```

---

## Phase 3: Fix lua Scoring

### Task 9: Analyze lua Failure

**Files:**
- Check: `tools/oscomp/ltp/score_whitelist.txt` (for lua tests if any)

- [ ] **Step 1: Run lua test**

```bash
timeout 120s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-lua.log 2>&1
strings /tmp/rv-lua.log | grep -E "lua|lua_testcode"
```

---

## Task 10: Commit lua Fix (Deferred to After libctest)

- [ ] **Step 1: Analyze lua failure output**

- [ ] **Step 2: Implement fix**

- [ ] **Step 3: Commit**

---

## Verification: Full Suite Run

### Task 11: Run Full RISC-V Suite

- [ ] **Step 1: Run complete RISC-V test**

```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-full.log 2>&1
```

- [ ] **Step 2: Check progression**

```bash
strings /tmp/rv-full.log | grep "whuse-oscomp-step-end" | tail -20
```

Expected sequence: basic → busybox → iozone → libctest → libc-bench → lmbench → lua

- [ ] **Step 3: Extract scores**

```bash
strings /tmp/rv-full.log | grep -E "TPASS|TFAIL|TBROK|summary"
```

---

### Task 12: Run Full LoongArch Suite

- [ ] **Step 1: Run complete LoongArch test**

```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-la.img" cargo xtask qemu-loongarch > /tmp/la-full.log 2>&1
```

- [ ] **Step 2: Check progression**

```bash
strings /tmp/la-full.log | grep "whuse-oscomp-step-end" | tail -20
```

- [ ] **Step 3: Extract scores**

```bash
strings /tmp/la-full.log | grep -E "TPASS|TFAIL|TBROK|summary"
```

---

## Summary of Changes

| Phase | Task | Files | Priority |
|-------|------|-------|----------|
| 1 | iozone RISC-V | `crates/syscall/src/lib.rs` | High |
| 1 | iozone LoongArch | `crates/kernel-core/src/lib_loongarch.inc.rs` | High |
| 2 | libctest fix | `crates/proc/src/lib.rs`, `crates/kernel-core/src/lib.rs`, `crates/syscall/src/lib.rs` | High |
| 3 | lua fix | TBD | Medium |

---

## Success Criteria

1. iozone produces throughput output without "error while loading shared libraries"
2. libctest completes without hard livelock
3. libcbench/lmbench/lua produce non-zero scores
4. Full suite reaches `whuse-oscomp-suite-done` without kernel panic
5. No regression in basic/busybox (current scores preserved)

---

## If Fix Is Insufficient

If after these changes iozone/libcbench/lmbench still fail:

1. **Check dynamic loader:** `/etc/ld.so.cache` may contain stale entries
2. **Check musl interpreter:** Verify musl binaries use `/lib/ld-musl-*.so.1`
3. **Consider runtime guard:** Skip glibc symlinks when running from musl paths
4. **Isolate libctest:** Run libctest in isolation to confirm cancellation fix works

---

## Test Execution Commands Reference

```bash
# Build
make build-riscv && make build-loongarch
cargo xtask oscomp-images

# RISC-V full suite
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-rv.img" cargo xtask qemu-riscv > /tmp/rv-full.log 2>&1

# LoongArch full suite
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$PWD/target/oscomp/sdcard-la.img" cargo xtask qemu-loongarch > /tmp/la-full.log 2>&1

# Check progression
strings /tmp/rv-full.log | grep "whuse-oscomp-step-end"
strings /tmp/la-full.log | grep "whuse-oscomp-step-end"

# Check for errors
strings /tmp/rv-full.log | grep "error while loading"
strings /tmp/la-full.log | grep "error while loading"
```
