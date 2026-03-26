# OSCOMP Real Score Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore real score-bearing execution for `basic`, `busybox`, `iozone`, `libcbench`, `libctest`, `lmbench`, `ltp`, and `lua` across `musl-rv`, `glibc-rv`, `musl-la`, and `glibc-la`.

**Architecture:** Keep RISC-V on the official dual-runtime suite, restore LoongArch to the same official suite model, and remove scheduler/debug shortcuts that spuriously wake blocked futex waiters. Use official testcase output and runtime-prefixed markers as the only acceptance criteria.

**Tech Stack:** Rust kernel (`kernel-core`, `syscall`, `proc`, `vfs`), QEMU contest runners, testsuits-for-oskernel images.

---

### Task 1: Restore official LoongArch dual-runtime suite

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs`

- [ ] **Step 1: Add LoongArch official-suite `only_step` support mirroring RISC-V**
- [ ] **Step 2: Switch LoongArch suite selection from stage2 synthetic helper to `OSCOMP_OFFICIAL_SUITE_SCRIPT`**
- [ ] **Step 3: Run LoongArch targeted `basic_testcode.sh` and verify real `musl/glibc` runtime markers**

### Task 2: Remove spurious blocked-task wakeups

**Files:**
- Modify: `crates/kernel-core/src/lib_riscv.inc.rs`
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs`

- [ ] **Step 1: Remove unconditional `whuse-coop` wake-all block from timer-tick path**
- [ ] **Step 2: Keep only explicit deadlock/signal wake logic**
- [ ] **Step 3: Re-run real `libcbench`/`libctest` reproductions and verify progress moved forward**

### Task 3: Verify score-bearing groups with official outputs

**Files:**
- Modify: `crates/syscall/src/lib.rs` if libcbench/libctest still block after Task 2
- Test: `/tmp/rv-real-*.log`, `/tmp/la-real-*.log`

- [ ] **Step 1: Run targeted official-suite commands for `basic`, `busybox`, `iozone`, `libcbench`, `libctest`, `lmbench`, `ltp`, `lua`**
- [ ] **Step 2: Verify real testcase output (`Pass!`, `testcase ... success`, benchmark lines) instead of synthetic markers**
- [ ] **Step 3: Only if still blocked, patch the earliest real kernel semantic failure and re-run**
