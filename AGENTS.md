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
rg -n "whuse-oscomp-step-(begin|end|skip|timeout)|whuse-oscomp-suite-done|panic|pid 1 \\(init\\)" /tmp/rv-full.log /tmp/la-full.log
```

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
rg -n "whuse-oscomp-suite-done" /tmp/rv-*.log /tmp/la-*.log

# crash signatures (must be empty)
rg -n "panic|pid 1 \\(init\\).*trap|trapped with scause" /tmp/rv-*.log /tmp/la-*.log

# step progression
rg -n "whuse-oscomp-step-begin|whuse-oscomp-step-end|whuse-oscomp-step-timeout|whuse-oscomp-step-skip" /tmp/rv-*.log /tmp/la-*.log
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
