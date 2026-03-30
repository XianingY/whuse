# Whuse Operational Playbook (Portable + AI-Readable)

This document is the operator runbook for Whuse OSCOMP testing.

- Goal: keep the workflow **portable**, **actionable**, and **machine-readable**.
- Scope: process only (no kernel semantic changes in this document).
- Repository root is assumed as current working directory (`$REPO_ROOT`).

## 1) Environment Prerequisites

### 1.1 Required tools

- Rust toolchain matching `rust-toolchain.toml`
- `cargo`
- QEMU:
  - `qemu-system-riscv64`
  - `qemu-system-loongarch64`
- ext4/image tooling (for image prep validation):
  - `debugfs`
  - `xz`
- Optional (for testsuits image build fallback): `docker`

### 1.2 Quick version probe

```bash
cargo --version
qemu-system-riscv64 --version
qemu-system-loongarch64 --version
debugfs -V
xz --version
docker --version
docker run --rm docker.educg.net/cg/os-contest:20260104 qemu-system-riscv64 --version
docker run --rm docker.educg.net/cg/os-contest:20260104 qemu-system-loongarch64 --version
```

Contest baseline in this repo follows the measured image output above
(currently QEMU `10.0.2` on `os-contest:20260104`), not older document snapshots.

## 2) Variables and Conventions

Use variables/relative paths only. Do not hardcode machine-private absolute paths.

```bash
export REPO_ROOT="$(pwd)"
export TESTSUITS_DIR="${WHUSE_OSCOMP_TESTSUITS_DIR:-$(dirname "$REPO_ROOT")/testsuits-for-oskernel}"
export RV_IMG="$REPO_ROOT/target/oscomp/sdcard-rv.img"
export LA_IMG="$REPO_ROOT/target/oscomp/sdcard-la.img"
export XTASK="cargo run --manifest-path $REPO_ROOT/tools/xtask/Cargo.toml --"
```

Contest-safe rule: do not rely on `cargo xtask` alias resolution. Prefer
`make ...` targets or explicit `${XTASK} <cmd>`.

Key runtime env vars:

- `WHUSE_DISK_IMAGE`: override QEMU disk image path.
- `WHUSE_EXTRA_DISK_IMAGE`: optional second disk (`disk.img` / `disk-la.img`) path.
- `WHUSE_OSCOMP_TESTSUITS_DIR`: override testsuits directory used by `cargo xtask oscomp-images`.
- `WHUSE_OSCOMP_SKIP_BUILD=1`: skip `make sdcard` when preparing oscomp images.
- `WHUSE_OSCOMP_DOCKER_IMAGE`: docker image for testsuits `make sdcard` fallback.
- `WHUSE_OSCOMP_COMPAT`: suite mode switch (`0`=real execution flow, `1`=compat fallback).
- `WHUSE_QEMU_MODE`: `contest` (docker qemu) or `host` (local qemu).
- `WHUSE_QEMU_RISCV_MEM`: RISC-V QEMU RAM size (default `1G`).
- `WHUSE_QEMU_LOONGARCH_MEM`: LoongArch QEMU RAM size (default `1G`).
- `WHUSE_LTP_PROFILE`: LTP mode (`score`=whitelist/blacklist high-yield, `full`=broader execution).
- `WHUSE_LTP_WHITELIST`: LTP score whitelist path (default `/musl/ltp_score_whitelist.txt`).
- `WHUSE_LTP_BLACKLIST`: LTP score blacklist path (default `/musl/ltp_score_blacklist.txt`).

## 3) Standard Command Blocks

### 3.1 Baseline build/check

```bash
make test
make check
make build-riscv
make build-loongarch
make contest-selfcheck
```

### 3.2 Prepare SD-card images (dual-arch)

```bash
${XTASK} oscomp-images
```

Expected output image paths:

- `target/oscomp/sdcard-rv.img`
- `target/oscomp/sdcard-la.img`

### 3.3 Run full suite (RISC-V / LoongArch)

```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$RV_IMG" ${XTASK} oscomp-riscv > /tmp/rv-full.log 2>&1
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$LA_IMG" ${XTASK} oscomp-loongarch > /tmp/la-full.log 2>&1
```

LoongArch contest profile is `-kernel kernel-la` based. Bootrom/loader remains host-debug only.

### 3.4 Log extraction quick checks

```bash
strings /tmp/rv-full.log | grep "whuse-oscomp-step-\|whuse-oscomp-suite-done\|panic\|pid 1 (init)"
```

Use `strings` first — QEMU log output is binary-mixed; plain `grep` will report "binary file matches" with no output.

### 3.5 LTP-focused run (RISC-V)

```bash
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score tools/dev/run_oscomp_stage2.sh ltp-riscv
```

## 4) Current Flow (可复现现状 / Default Today)

Current recommended default for validation is real execution (`WHUSE_OSCOMP_COMPAT=0`) and contest-mode QEMU:

- `WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}`
- `WHUSE_QEMU_MODE=contest`

Host quick mode remains available via `WHUSE_QEMU_MODE=host`.

### 4.1 Current RISC-V/LoongArch full run (repro baseline)

Preconditions:

- `cargo xtask oscomp-images` succeeded, and `$RV_IMG` / `$LA_IMG` exist.
- QEMU binary is available.

Commands:

```bash
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask oscomp-riscv > /tmp/rv-current.log 2>&1
timeout 3600s env WHUSE_QEMU_MODE=contest WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask oscomp-loongarch > /tmp/la-current.log 2>&1
```

Acceptance markers:

- Must contain:
  - `whuse-oscomp-script-start`
  - `whuse-oscomp-step-begin:busybox_testcode.sh`
  - `whuse-oscomp-step-end:busybox_testcode.sh:*`
  - `whuse-oscomp-suite-done`
- Compat mode often contains (expected only if `WHUSE_OSCOMP_COMPAT=1`):
  - `whuse-oscomp-step-skip:*:compat-hang`

Failure handling:

- If image lock appears (`disk image ... is currently in use by pid ...`):
  - stop prior QEMU process, rerun.
- If kernel panic or `pid 1 (init)` crash appears:
  - mark run as invalid, collect trap context, move to kernel semantic debugging.

### 4.2 Current default step order (documented behavior)

The suite script order is:

1. `time-test` (missing file -> explicit skip marker)
2. `busybox_testcode.sh` (compat script when `WHUSE_OSCOMP_COMPAT=1`)
3. `iozone_testcode.sh`
4. `libctest_testcode.sh`
5. `libc-bench`
6. `lmbench_testcode.sh`
7. `lua_testcode.sh`
8. `unixbench_testcode.sh`
9. `netperf_testcode.sh`
10. `iperf_testcode.sh`
11. `cyclic_testcode.sh` (or fallback to `cyclictest_testcode.sh`)

### 4.3 Current known status (as of 2026-03-30)

**Branch**: `dev` at commit `023004e`

**RISC-V musl-rv Test Results** (verified 2026-03-30):
- ✅ `time-test` — skip (missing binary, expected)
- ✅ `basic_testcode.sh` — completes with exit 0
- ✅ `busybox_testcode.sh` — completes with exit 0
- ✅ `iozone_testcode.sh` — completes with exit 0
- ✅ `libctest_testcode.sh` — completes with exit 0
- ✅ `libc-bench` — completes with exit 0
- ✅ `lua_testcode.sh` — completes with exit 0
- ❌ `lmbench_testcode.sh` — starts but times out (no COW fork optimization)
- ⏭️ `netperf_testcode.sh` — skipped (per user request)
- ⏭️ `iperf_testcode.sh` — skipped (per user request)  
- ⏭️ `cyclictest_testcode.sh` — skipped (per user request)

**Score**: ~525 points (lmbench not passing)

**Key Issues**:

1. **COW Fork Not Implemented**: `clone_private()` in `crates/mm/src/lib.rs` does eager copy of all memory pages on fork(). This is correct but slow, causing lmbench to timeout.

2. **COW Fork Attempt Failed**: Previous attempt to implement COW (Copy-On-Write) fork failed because:
   - Page fault handler (scause=15) couldn't find faulting address in segment mappings
   - Segment lookup returned `None` for stack pages
   - Would require careful debugging of the segment boundary checking logic

3. **EINTR Livelock Fix**: `023004e` includes EINTR counter to detect and force-exit pthread_cancel livelocks.

### 4.4 Scoring Summary

| Test | musl-rv | glibc-rv | musl-la | glibc-la |
|------|----------|----------|----------|----------|
| basic | ✅ | ✅ | ✅ | ✅ |
| busybox | ✅ | ✅ | ✅ | ✅ |
| iozone | ✅ | ✅ | ✅ | ✅ |
| libctest | ✅ | N/A | ✅ | N/A |
| libc-bench | ✅ | ✅ | ✅ | ✅ |
| lua | ✅ | ✅ | ✅ | ✅ |
| lmbench | ❌ timeout | ❌ | ❌ | ❌ |
| netperf | ⏭️ | ⏭️ | ⏭️ | ⏭️ |
| iperf | ⏭️ | ⏭️ | ⏭️ | ⏭️ |
| cyclictest | ⏭️ | ⏭️ | ⏭️ | ⏭️ |

- ✅ = passes with exit 0
- ❌ = fails/times out
- ⏭️ = skipped per user request
- N/A = not scored per contest rules

## 5) Target Flow (理想真实执行 / Real Execution)

Target policy: disable compat-by-default semantics during verification runs.

### 5.1 Real execution run (no compat shortcuts)

Preconditions:

- Same as current flow.
- Explicitly disable compat for this run.

Commands:

```bash
timeout 3600s env WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask qemu-riscv > /tmp/rv-target.log 2>&1
timeout 3600s env WHUSE_OSCOMP_COMPAT=0 WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask qemu-loongarch > /tmp/la-target.log 2>&1
```

Acceptance markers:

- Must contain:
  - begin/end markers for each suite step
  - `whuse-oscomp-suite-done`
- Must NOT contain:
  - `panic`
  - `pid 1 (init)` crash signature
  - "fake timeout without real execution" style behavior

Failure handling:

- If blocked at one group:
  - inspect top failing syscall patterns (`ENOSYS/EINVAL`) + process name + group marker.
- If loader/map failures (e.g., ld-musl traps):
  - prioritize `mmap/mprotect/munmap/brk` + dynamic loader path.

### 5.2 Throughput objective (after stable completion)

- Phase 1: full sequence completes and system stays alive.
- Phase 2: reduce `basic/busybox/lua` fail/error toward zero.
- Phase 3: reduce failures in heavy groups (`iozone/libctest/lmbench/unixbench/netperf/iperf/cyclictest`).

## 6) Validation Rules (Machine-Readable)

Use these checks for any run log:

```bash
# required suite closure
strings /tmp/rv-*.log | grep "whuse-oscomp-suite-done"

# crash signatures (must be empty)
strings /tmp/rv-*.log | grep "panic\|pid 1 (init).*trap\|trapped with scause"

# step progression
strings /tmp/rv-*.log | grep "whuse-oscomp-step-begin\|whuse-oscomp-step-end\|whuse-oscomp-step-timeout\|whuse-oscomp-step-skip"
```

Pass/Fail policy:

- PASS (flow): reaches `whuse-oscomp-suite-done` and no kernel panic/init-crash.
- FAIL (flow): missing suite-done, or panic/init-crash present.
- QUALITY score: count `testcase .* fail|error` lines per group for trend tracking.

## 7) Fault Triage Table

| Symptom | Fast Check | Likely Cause | Next Action |
|---|---|---|---|
| `disk image ... in use by pid` | check running QEMU | stale emulator instance | stop holder process, rerun |
| `whuse-oscomp-step-skip:*:compat-hang` | check `WHUSE_OSCOMP_COMPAT` | compat default enabled | rerun with `WHUSE_OSCOMP_COMPAT=0` for real execution |
| `ld-musl-*.so.1` trap or early user trap | inspect surrounding step/process markers | loader/memory mapping semantics gap | prioritize `mmap/mprotect/munmap/brk` path |
| flow stalls around busybox large-tree ops | inspect step timeout + process name | heavy directory traversal or syscall semantics | profile hot syscalls; optimize VFS/ext4 read/stat path |
| preflight missing `/musl/...` file | inspect `oscomp preflight missing required path` logs | incomplete image content | rebuild testsuits image / validate with `cargo xtask oscomp-images` |
| `grep` reports "binary file matches" on log | use `strings /tmp/rv-*.log \| grep ...` instead | QEMU log contains binary escape sequences | always pipe through `strings` before grepping |
| `make build-riscv` reports `Finished (0.0Xs)` but changes not reflected | `touch` modified `.rs` files then rebuild | cargo incremental cache not invalidated | `touch crates/*/src/lib.rs && make build-riscv` |
| libctest hangs at `pthread_cancel_points` | see Section 10 | mutex deadlock in signal/FUTEX interaction | see active fix in Section 10 |

## 8) Migration Checklist (New Machine)

### 8.1 Host-first path

Preconditions:

- host has required tools in Section 1.
- testsuits repo available at `$TESTSUITS_DIR`.

Commands:

```bash
make check
make build-riscv
make build-loongarch
cargo xtask oscomp-images
timeout 120s env WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask qemu-riscv > /tmp/rv-smoke.log 2>&1
timeout 120s env WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask qemu-loongarch > /tmp/la-smoke.log 2>&1
```

Acceptance markers:

- both smoke logs show kernel enter + suite script start markers.

Failure handling:

- if host `make sdcard` fails in testsuits, use docker fallback (Section 8.2).

### 8.2 Docker fallback for testsuits image build

Preconditions:

- docker available and runnable by current user.

Commands:

```bash
export WHUSE_OSCOMP_DOCKER_IMAGE="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20260104}"
cargo xtask oscomp-images
```

Acceptance markers:

- both `target/oscomp/sdcard-rv.img` and `target/oscomp/sdcard-la.img` exist and pass xtask validation.

Failure handling:

- verify docker can pull/run image;
- verify testsuits path mount permissions;
- retry with explicit `WHUSE_OSCOMP_TESTSUITS_DIR`.

## 9) Source of Testsuits

- Official repository: [oscomp/testsuits-for-oskernel](https://github.com/oscomp/testsuits-for-oskernel)
- Keep local testsuits in sync before image rebuild when debugging image-content mismatches.

## 10) Known Blocking Issues and Active Fixes

### 10.1 pthread_cancel_points Cancellation Livelock (RESOLVED)

**Status**: Resolved in `023004e`.

**What was the issue**: The cancelled thread would livelock in repeated `FUTEX_WAIT -> -EINTR` handling on `__tl_lock`.

**Fix applied**: Added EINTR counter in `crates/proc/src/lib.rs`. When EINTR count >= 1000 for a thread, `force_thread_exit` is set to break the livelock.

**Current behavior**: `libctest_testcode.sh` now completes with exit 0.

### 10.2 COW Fork Implementation (WORKING but lmbench still slow)

**Status**: COW Fork is implemented and working, but lmbench times out due to page table rebuild overhead.

**What was implemented**:
1. Added `CowParent` variant to `SegmentStorage` enum
2. Modified `clone_private()` to mark pages as read-only (COW)
3. Added scause=15 (store page fault) handler in `lib_riscv.inc.rs`
4. Implemented `handle_page_fault()` to copy page data and remap writable

**Current behavior**:
- COW faults are properly handled (see "whuse: COW fault handled" in logs)
- basic, busybox, iozone, libctest, lua tests all pass
- lmbench times out due to COW page table rebuild overhead

**lmbench timeout issue**:
- After each COW fault, `set_dirty()` is called which marks the entire address space dirty
- On next `token()` call, the entire page table is rebuilt
- lmbench does many fork operations, each triggering COW faults on data pages
- Each COW fault causes a full page table rebuild, making lmbench extremely slow

**Fixes applied**:
1. Removed EAGAIN check for COW fork (commit 8de03de) - this was causing fork to fail for large address spaces
2. COW pages now have execute permission (commit dba3071) - fixes scause=12 on code execution

**Remaining optimization needed**:
- Instead of rebuilding the entire page table after each COW fault, update just the specific PTE
- This requires adding a method to Sv39PageTableBuilder to update a single PTE

**Files involved**:
- `crates/mm/src/lib.rs` — `SegmentStorage::CowParent`, `handle_page_fault()`, `promote_cow_segment()`
- `crates/kernel-core/src/lib_riscv.inc.rs` — scause=15 handler
- `crates/proc/src/lib.rs` — `fork_process_from_current()`

### 10.4 Kernel Idle Timer Infrastructure (COMPLETED)

**Status**: Fully implemented and working.

The kernel spin loop previously ran with `sstatus.SIE=0`, preventing timer interrupts from firing when all user threads were blocked. This caused the FUTEX timed-wait expiry and deadlock detection paths to never execute.

Implemented:

- `hal().cpu.enable_interrupts()` called before `spin_loop()` / `wfi` in the idle path.
- `__whuse_kernel_trap_entry` assembly in `crates/hal-riscv64-virt/src/lib.rs`: saves all registers, calls `__whuse_kernel_trap_handler`, restores and `sret`s.
- `KERNEL_TRAP_HANDLER: AtomicUsize` static: stores the Rust callback pointer.
- `set_kernel_timer_callback` on `VirtCpu` (RISC-V): stores callback, sets `stvec` to kernel handler.
- `stvec` is restored to `__whuse_kernel_trap_entry` after `run_user()` returns (in `VirtCpu::run_user`).
- `KERNEL_IDLE_TIMER_TICKS: AtomicU64` global counter; `kernel_idle_timer_cb()` increments it and rearms the timer.
- `kernel_idle_timer_cb` registered in `Kernel::bootstrap()`.
- Spin loop drains `KERNEL_IDLE_TIMER_TICKS` and performs: timed-wait expiry, deadlock detection, and signal-wake checks.
- Stubs added for LoongArch (`crates/hal-loongarch64-virt/src/lib.rs`) and host-test CPU (`crates/syscall/src/lib.rs` `TestCpu`).

Verified: `[IDLE-TMR:1]` appears in QEMU log, confirming kernel timer fires during blocked-all-threads scenarios.

### 10.5 mm::read_cstr regression (COMPLETED)

**Status**: Fixed.

`read_cstr` was reading `chunk_len = PAGE_SIZE` bytes per iteration. For small mappings (e.g., a 6-byte `"hello\0"` string), `find_segment` would return `EFAULT` when the chunk extended past the mapped region.

Fix: added a fallback path — if the chunk read fails with `EFAULT`, retry reading 1 byte at a time until a NUL is found or another error occurs.

File: `crates/mm/src/lib.rs`.

## 11) Active Debug Log Markers

These log lines are intentionally present in the current build for diagnostics. Remove after the relevant issue is resolved.

| Marker | Source | Meaning |
|---|---|---|
| `[IDLE-TMR:N]` | kernel-core spin loop | Kernel idle timer fired N times since last drain |
| `whuse-debug: dispatch_pending_signals tid=X pending=Y signum=Z clear_child_tid=A tid_address=B` | kernel-core | Signal dispatch attempt for thread X with thread-exit pointers |
| `whuse: dispatching sig N tid=X handler=H ...` | kernel-core | Signal frame written; sepc redirected to handler H |
| `whuse-debug: FUTEX_WAIT tid=X uaddr=A val=V` | syscall | Thread X entering FUTEX_WAIT (registered in queue) |
| `whuse-debug: FUTEX_WAIT EINTR tid=X addr=A pending=P` | syscall | FUTEX_WAIT interrupted by pending signal |
| `whuse-debug: FUTEX_WAIT EAGAIN tid=X ...` | syscall | FUTEX_WAIT rejected because value already changed |
| `whuse-debug: kill/tkill target=X sig=S caller_tid=Y` | syscall | Signal delivery from Y to X |
| `whuse: idle-tick ready=R blocked=B all_futex=F` | kernel-core | Idle tick: R runnable, B blocked, all-futex deadlock flag |
| `[SPIN:N]` / `[LOOP:N]` | kernel-core | Spin loop / run-forever iteration counters |
| `whuse: syscall enter ...` | syscall | Broad syscall tracing (entry point) |
| `whuse: syscall exit ...` | syscall | Broad syscall tracing (exit point) |
| `whuse: run_user sepc=... sp=...` | hal-riscv | HAL context switch entry diagnostic |
| `whuse: trap return scause=...` | hal-riscv | HAL context switch exit diagnostic |

### 10.6 Buddy Allocator & Memory Overlap Fix (COMPLETED)

**Status**: Verified stable.

**Issue**: System stalled after `init` bootstrap due to memory collision between the kernel static `HEAP` (managed by `BuddyAllocator`) and the `mm` crate's page allocator. The page allocator was consuming DRAM starting from `0x80200000`, which the kernel binary also occupied.

**Fix**:
1.  Implemented a robust `BuddyAllocator` in `platform/riscv64-virt/src/main.rs`.
2.  Added `PROVIDE(end = .);` to `linker.ld` to identify the kernel end.
3.  Updated `hal-riscv64-virt` to dynamically adjust `MEMORY_MAP` during bootstrap, starting usable DRAM at the kernel `end`.
4.  Implemented actual CSR manipulation (`csrs/csrc sie`) for interrupt control in `VirtCpu` to protect allocator locks.

**Result**: Init process now successfully enters user mode and performs syscalls (`brk`, `mmap`, `execve`).

### 10.7 Universal Syscall Tracing (COMPLETED)

**Status**: Enabled.

**Improvement**: Moved syscall tracing from individual syscall implementations to the central `SyscallDispatcher::dispatch` in `crates/syscall/src/lib.rs`. 
- Ensures all syscalls (including unknown/unsupported) are logged with arguments and return values.
- Significantly improved observability into `init` and `busybox` behavior.
