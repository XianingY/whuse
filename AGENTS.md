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
```

## 2) Variables and Conventions

Use variables/relative paths only. Do not hardcode machine-private absolute paths.

```bash
export REPO_ROOT="$(pwd)"
export TESTSUITS_DIR="${WHUSE_OSCOMP_TESTSUITS_DIR:-$(dirname "$REPO_ROOT")/testsuits-for-oskernel}"
export RV_IMG="$REPO_ROOT/target/oscomp/sdcard-rv.img"
export LA_IMG="$REPO_ROOT/target/oscomp/sdcard-la.img"
```

Key runtime env vars:

- `WHUSE_DISK_IMAGE`: override QEMU disk image path.
- `WHUSE_OSCOMP_TESTSUITS_DIR`: override testsuits directory used by `cargo xtask oscomp-images`.
- `WHUSE_OSCOMP_SKIP_BUILD=1`: skip `make sdcard` when preparing oscomp images.
- `WHUSE_OSCOMP_DOCKER_IMAGE`: docker image for testsuits `make sdcard` fallback.
- `WHUSE_OSCOMP_COMPAT`: suite mode switch (`1`=current compat-heavy flow, `0`=target real execution flow).

## 3) Standard Command Blocks

### 3.1 Baseline build/check

```bash
make test
make check
make build-riscv
make build-loongarch
```

### 3.2 Prepare SD-card images (dual-arch)

```bash
cargo xtask oscomp-images
```

Expected output image paths:

- `target/oscomp/sdcard-rv.img`
- `target/oscomp/sdcard-la.img`

### 3.3 Run full suite (RISC-V / LoongArch)

```bash
timeout 3600s env WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask qemu-riscv > /tmp/rv-full.log 2>&1
timeout 3600s env WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask qemu-loongarch > /tmp/la-full.log 2>&1
```

### 3.4 Log extraction quick checks

```bash
strings /tmp/rv-full.log | grep "whuse-oscomp-step-\|whuse-oscomp-suite-done\|panic\|pid 1 (init)"
```

Use `strings` first — QEMU log output is binary-mixed; plain `grep` will report "binary file matches" with no output.

## 4) Current Flow (可复现现状 / Default Today)

Current default is compat-heavy because suite script sets:

- `WHUSE_OSCOMP_COMPAT=${WHUSE_OSCOMP_COMPAT:-1}`

That means without explicit override, runtime uses compat behavior.

### 4.1 Current RISC-V/LoongArch full run (repro baseline)

Preconditions:

- `cargo xtask oscomp-images` succeeded, and `$RV_IMG` / `$LA_IMG` exist.
- QEMU binary is available.

Commands:

```bash
timeout 3600s env WHUSE_DISK_IMAGE="$RV_IMG" cargo xtask qemu-riscv > /tmp/rv-current.log 2>&1
timeout 3600s env WHUSE_DISK_IMAGE="$LA_IMG" cargo xtask qemu-loongarch > /tmp/la-current.log 2>&1
```

Acceptance markers:

- Must contain:
  - `whuse-oscomp-script-start`
  - `whuse-oscomp-step-begin:busybox_testcode.sh`
  - `whuse-oscomp-step-end:busybox_testcode.sh:*`
  - `whuse-oscomp-suite-done`
- Compat mode often contains (expected under current default):
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

### 4.3 Current known status (as of last verified run)

Steps confirmed completing with `step-end` marker:

- `time-test` — skip (missing binary, expected)
- `busybox_testcode.sh` — completes
- `iozone_testcode.sh` — completes
- `libctest_testcode.sh` — **HANGS** at `pthread_cancel_points` test (see Section 10)

Steps not yet reached due to libctest hang:

- `libc-bench`, `lmbench_testcode.sh`, `lua_testcode.sh`, `unixbench_testcode.sh`, `netperf_testcode.sh`, `iperf_testcode.sh`, `cyclic_testcode.sh`

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
export WHUSE_OSCOMP_DOCKER_IMAGE="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20250614}"
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

### 10.1 pthread_cancel_points deadlock (libctest, RISC-V)

**Status**: Fix implemented, verification in progress.

**Symptom**: The `libctest_testcode.sh` step starts `pthread_cancel_points` and then hangs permanently. All subsequent suite steps are never reached. The QEMU log shows `[IDLE-TMR:1]` firing repeatedly but no progress.

**Root cause chain**:

1. Thread 125 (joiner) sends SIGCANCEL (sig 33) to thread 126, then calls `FUTEX_WAIT` on `0x50022b30` (detach_state, waiting for thread 126 to finish).
2. Thread 126 receives SIGCANCEL. The kernel dispatches the signal frame: `sepc` is redirected to `__cancel` (musl's cancellation handler at `0x47840`). `pending_signals` bit for sig 33 is **cleared** at this point (consumed during dispatch).
3. Thread 126 runs `sret` and begins executing `__cancel`. The handler internally calls `pthread_exit` → acquires `__tl_lock` → calls `FUTEX_WAIT` on `0xaec68` (`__tl_lock` futex).
4. `__tl_lock` is held by thread 125 (which called `pthread_join` → holds the lock while waiting for thread 126).
5. **Deadlock**: thread 125 holds `__tl_lock`, waits for thread 126. Thread 126 needs `__tl_lock`, waits for thread 125 to release it.

The prior fix attempt (idle-timer + `futex_blocked_with_pending_signal_tids`) did not resolve this because `pending_signals` is already 0 by the time thread 126 is blocked — the signal was consumed during dispatch.

**Fix implemented** (`signal_frame_pending` flag):

Added `signal_frame_pending: bool` to `Process` in `crates/proc/src/lib.rs`.

- Set to `true` in `dispatch_pending_signals` in `crates/kernel-core/src/lib.rs` immediately after the signal frame is written and `sepc` is redirected.
- Cleared to `false` in `sys_rt_sigreturn` in `crates/syscall/src/lib.rs`.
- In `FUTEX_WAIT` handler (`crates/syscall/src/lib.rs`): both the re-entry path and the fresh-entry path now check `signal_frame_pending || pending_signals != 0` and return `-EINTR` immediately if true.

The intended effect: when thread 126's `__cancel` handler calls `FUTEX_WAIT` on `__tl_lock`, the kernel returns `EINTR` immediately instead of blocking. The `__cancel` / `pthread_exit` path in musl is designed to handle `EINTR` from cancellation-point futexes and retry, or it will call `__do_cancel` directly. This breaks the deadlock.

**Current gap (still debugging)**:

Despite the fix being compiled in, the `FUTEX_WAIT tid=126 uaddr=0xaec68` is still entered (seen in log). This means `signal_frame_pending` is `false` when thread 126 makes this FUTEX_WAIT call. Investigation ongoing.

Possible explanations to check:

1. `dispatch_pending_signals` is called for thread 126 when a **different thread** is "current" — i.e., the process pointer is correct but the `signal_frame_pending` write goes to a cloned/stale copy.
2. There is a second `dispatch_pending_signals` call later that re-dispatches and overwrites the state without re-setting `signal_frame_pending` (e.g., a double-dispatch path).
3. The signal frame for thread 126 is set up while thread 126 is not yet "current" (it was woken remotely and its frame modified directly), and by the time it becomes current and runs, some path has reset `signal_frame_pending`.
4. `__cancel` itself calls `rt_sigreturn` before reaching the `FUTEX_WAIT` — clearing `signal_frame_pending` prematurely.

**Debug commands**:

```bash
# Check sequence around dispatch and FUTEX_WAIT
strings /tmp/rv-test.log | grep -A 20 "dispatching sig 33"

# Check if FUTEX_WAIT EINTR ever fires for tid=126
strings /tmp/rv-test.log | grep "FUTEX_WAIT EINTR.*tid=126"

# Check if rt_sigreturn fires before the FUTEX_WAIT
strings /tmp/rv-test.log | grep "rt_sigreturn\|sigreturn"
```

**Files involved**:

- `crates/proc/src/lib.rs` — `Process::signal_frame_pending` field
- `crates/kernel-core/src/lib.rs` — `dispatch_pending_signals` sets the flag
- `crates/syscall/src/lib.rs` — `FUTEX_WAIT` checks flag; `sys_rt_sigreturn` clears it

### 10.2 Kernel idle timer infrastructure (COMPLETED)

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

### 10.3 mm::read_cstr regression (COMPLETED)

**Status**: Fixed.

`read_cstr` was reading `chunk_len = PAGE_SIZE` bytes per iteration. For small mappings (e.g., a 6-byte `"hello\0"` string), `find_segment` would return `EFAULT` when the chunk extended past the mapped region.

Fix: added a fallback path — if the chunk read fails with `EFAULT`, retry reading 1 byte at a time until a NUL is found or another error occurs.

File: `crates/mm/src/lib.rs`.

## 11) Active Debug Log Markers

These log lines are intentionally present in the current build for diagnostics. Remove after the relevant issue is resolved.

| Marker | Source | Meaning |
|---|---|---|
| `[IDLE-TMR:N]` | kernel-core spin loop | Kernel idle timer fired N times since last drain |
| `whuse-debug: dispatch_pending_signals tid=X pending=Y signum=Z` | kernel-core | Signal dispatch attempt for thread X |
| `whuse: dispatching sig N tid=X handler=H ...` | kernel-core | Signal frame written; sepc redirected to handler H |
| `whuse-debug: FUTEX_WAIT tid=X uaddr=A val=V` | syscall | Thread X entering FUTEX_WAIT (registered in queue) |
| `whuse-debug: FUTEX_WAIT EINTR tid=X addr=A pending=P` | syscall | FUTEX_WAIT interrupted by pending signal |
| `whuse-debug: FUTEX_WAIT EAGAIN tid=X ...` | syscall | FUTEX_WAIT rejected because value already changed |
| `whuse-debug: kill/tkill target=X sig=S caller_tid=Y` | syscall | Signal delivery from Y to X |
| `whuse: idle-tick ready=R blocked=B all_futex=F` | kernel-core | Idle tick: R runnable, B blocked, all-futex deadlock flag |
| `[SPIN:N]` / `[LOOP:N]` | kernel-core | Spin loop / run-forever iteration counters |
