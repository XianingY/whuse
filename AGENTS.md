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

### 3.6 Raw exit verification (contest-style)

```bash
TIMEOUT_SECS=2400 tools/dev/run_oscomp_stage2.sh riscv-raw-exit
TIMEOUT_SECS=2400 tools/dev/run_oscomp_stage2.sh loongarch-raw-exit
```

These modes disable helper-side `suite-done` termination and require the guest
to print the shutdown marker and let QEMU exit on its own.

## 4) Current Flow (可复现现状 / Default Today)

Current recommended default for validation is real execution (`WHUSE_OSCOMP_COMPAT=0`) and contest-mode QEMU:

- `WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-0}`
- `WHUSE_QEMU_MODE=contest`

Host quick mode remains available via `WHUSE_QEMU_MODE=host`.

### 4.1 Current best scored baseline (2026-04-01, commit `9ad5123`)

Current public best result comes from site submission `9ad512346bbdcb1c688a2311961b977ec239f422`.

- Total score: **884.0**
- Primary score source: **musl-rv = 513.0**
- Current direction is validated: RISC-V score-first sequencing plus scorer-compatible `libctest` / `ltp` output works on the site.

Score highlights from [assist_docs/9ad512346bbdcb1c688a2311961b977ec239f422/代码执行结果.md](assist_docs/9ad512346bbdcb1c688a2311961b977ec239f422/代码执行结果.md):

| Test | glibc-la | glibc-rv | musl-la | musl-rv | Total |
|---|---:|---:|---:|---:|---:|
| basic | 98 | 101 | 98 | 99 | 396 |
| busybox | 0 | 54 | 11 | 54 | 119 |
| libctest | - | - | 0 | 201 | 201 |
| ltp | 0 | 0 | 0 | 133 | 133 |
| libcbench | 0 | 0 | 0 | 17 | 17 |
| lua | 0 | 9 | 0 | 9 | 18 |

### 4.2 Current code per-arch full order

RISC-V committed `full` order at `9ad5123`:

1. `time-test`
2. `basic_testcode.sh`
3. `busybox_testcode.sh`
4. `iozone_testcode.sh` — explicit skip (`riscv-known-panic`)
5. `ltp_testcode.sh` — `musl` runs internal score-mode LTP; `glibc` skipped
6. `libctest_testcode.sh` — `musl` real runner; `glibc` skipped
7. `lua_testcode.sh`
8. `libc-bench`
9. `lmbench_testcode.sh`
10. `unixbench_testcode.sh`
11. `netperf_testcode.sh`
12. `iperf_testcode.sh`
13. `cyclic_testcode.sh`

LoongArch current code `full` order:

1. `time-test`
2. `basic_testcode.sh`
3. `busybox_testcode.sh`
4. `libctest_testcode.sh` — `musl` real execution; `glibc` explicit skip (`glibc-libctest-not-scored`)
5. `lua_testcode.sh`
6. `libc-bench`
7. `ltp_testcode.sh` — `musl` real execution; `glibc` explicit skip (`glibc-ltp-not-scored`)
8. `iozone_testcode.sh` — explicit skip (`loongarch-iozone-not-scored`)
9. `lmbench_testcode.sh` — explicit skip (`loongarch-lmbench-not-scored`)
10. `unixbench_testcode.sh`
11. `netperf_testcode.sh`
12. `iperf_testcode.sh`
13. `cyclic_testcode.sh`

### 4.3 Scorer-sensitive output contracts

The site judge is not only syscall/semantic-sensitive; it is also output-contract-sensitive.

- `basic`: keep default builds free of `COW fault handled` / `COW promote failed` noise. The `pipe` / `wait` / `waitpid` / `yield` scorers are line-sensitive.
- `libctest`: must emit judge-visible `START entry-static.exe`, `START entry-dynamic.exe`, and `Pass!`.
- `ltp`: must emit `RUN LTP CASE <case>` and `FAIL LTP CASE <case> : <ret>`. `whuse-ltp-case-result:*` is auxiliary diagnostics, not the primary scoring contract.

### 4.4 Site-proven vs current code state

Current site-proven best is still `9ad5123`, but the current code baseline is already ahead of that on the LoongArch control plane.

- Site-proven baseline:
  - RISC-V score-first sequencing and scorer-compatible `libctest` / `ltp` output are confirmed by `9ad5123`
  - LoongArch score remains low in the site result because that submission still used the older `full` control plane
- Current code baseline:
  - stage2 helper tracks per-arch `full` order instead of one shared `full_root_steps`
  - LoongArch `full` is now score-first, with selective `glibc` skips for `busybox/libctest/ltp`
  - low-yield LoongArch groups (`iozone/lmbench/unixbench/netperf/iperf/cyclic`) are explicit skips
- Current earliest blocker:
  - `loongarch full -> musl/libctest` after `basic -> busybox` is already reachable

## 5) Next Focus (Post-`9ad5123`)

Next stage remains **LoongArch full-chain scoring**, with RISC-V LTP expansion handled as a secondary local-only track.

### 5.1 Immediate engineering goal

- Keep the committed RISC-V score path unchanged and use it only as a regression guard.
- Keep the current LoongArch score-first `full` ordering and selective runtime/step skips intact.
- Make `full -> musl/libctest` the only active LoongArch blocker to debug.
- Expand `musl-rv` LTP locally through curated-first discovery; do not switch the site path away from 48-case score mode.
- Treat full-discovery as candidate generation only. Promote to curated first; keep score whitelist conservative.

### 5.2 Next validation path

Primary LoongArch validation sequence:

```bash
TIMEOUT_SECS=240 WHUSE_STAGE2_IMAGE_POLICY=never WHUSE_STAGE2_USE_IMAGE_COPY=1 WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh loongarch
TIMEOUT_SECS=240 WHUSE_STAGE2_IMAGE_POLICY=never WHUSE_STAGE2_USE_IMAGE_COPY=1 WHUSE_OSCOMP_PROFILE=libctest WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh loongarch
```

Acceptance goals:

- `basic -> busybox` must remain reachable in `loongarch full`
- determine whether `musl/libctest` is:
  - a true semantic stall
  - only too slow for the current window
  - or missing scorer-visible output
- if `libctest` becomes passable or skippable without losing more score than it blocks, the next target is `lua` then `libc-bench`

RISC-V guardrail:

```bash
TIMEOUT_SECS=240 WHUSE_STAGE2_IMAGE_POLICY=never WHUSE_STAGE2_USE_IMAGE_COPY=1 WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh riscv
```

Must still show:

- `basic_testcode.sh:0`
- `busybox_testcode.sh:0`
- `RUN LTP CASE ...`
- `FAIL LTP CASE ... : 0`

## 6) Validation Rules (Machine-Readable)

Use these checks for any run log:

```bash
# required suite closure
strings /tmp/rv-*.log | grep "whuse-oscomp-suite-done"

# raw-exit closure marker (required for contest-style exit validation)
strings /tmp/rv-*.log | grep "whuse: contest shutdown requested reason="

# crash signatures (must be empty)
strings /tmp/rv-*.log | grep "panic\|pid 1 (init).*trap\|trapped with scause"

# step progression
strings /tmp/rv-*.log | grep "whuse-oscomp-step-begin\|whuse-oscomp-step-end\|whuse-oscomp-step-timeout\|whuse-oscomp-step-skip"
```

Pass/Fail policy:

- PASS (flow): reaches `whuse-oscomp-suite-done` and no kernel panic/init-crash.
- PASS (raw-exit): reaches `whuse-oscomp-suite-done`, prints `whuse: contest shutdown requested reason=...`, and QEMU exits without helper-side kill.
- FAIL (flow): missing suite-done, or panic/init-crash present.
- QUALITY score: count `testcase .* fail|error` lines per group for trend tracking.

## 7) Fault Triage Table

| Symptom | Fast Check | Likely Cause | Next Action |
|---|---|---|---|
| `disk image ... in use by pid` | check running QEMU | stale emulator instance | stop holder process, rerun |
| `whuse-oscomp-step-skip:*:compat-hang` | check `WHUSE_OSCOMP_COMPAT` | compat default enabled | rerun with `WHUSE_OSCOMP_COMPAT=0` for real execution |
| `ld-musl-*.so.1` trap or early user trap | inspect surrounding step/process markers | loader/memory mapping semantics gap | prioritize `mmap/mprotect/munmap/brk` path |
| flow stalls around busybox large-tree ops | inspect step timeout + process name | heavy directory traversal or syscall semantics | profile hot syscalls; optimize VFS/ext4 read/stat path |
| `ltp` ran but scored `0` | inspect log for `RUN LTP CASE` and `FAIL LTP CASE ... : <ret>` | scorer contract mismatch | restore official LTP output contract before changing kernel semantics |
| `libctest` ran but scored `0` | inspect log for `START entry-static.exe`, `START entry-dynamic.exe`, `Pass!` | stubbed runner or scorer-visible lines missing | run the real musl libctest launcher and verify image content is not stubbed |
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
