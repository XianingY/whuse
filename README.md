# Whuse - OSKernel2026

Whuse is a Rust rewrite of the original RuOK kernel project, rebuilt as a
self-contained workspace rooted in this repository.

The project now tracks both `riscv64` and `loongarch64` with OSCOMP-oriented
build and run entrypoints.

## Competition Baseline (2026-01-04 Image)

All primary validation in this repository is aligned to:

- Contest image: `docker.educg.net/cg/os-contest:20260104`
- Testsuits branch: `pre-2025`
- Contest runner mode: `WHUSE_QEMU_MODE=contest`
- Real flow mode: `WHUSE_OSCOMP_COMPAT=0`

Current measured QEMU in the contest image is `10.0.2` for both
`qemu-system-riscv64` and `qemu-system-loongarch64`.

## Rust Toolchain Policy

To avoid rustup rolling updates during contest compile, this repository pins
Rust to `1.94.0` with `profile = "minimal"` in
`rust-toolchain.toml` instead of using floating `stable`.

## Xtask Invocation Policy (Contest-Safe)

Contest builds must not depend on Cargo alias loading from `.cargo/config.toml`.
This repository's build scripts call xtask explicitly via:

```bash
cargo run --manifest-path tools/xtask/Cargo.toml -- <command>
```

`make` targets already use this explicit form internally.
## Workspace layout

- `crates/hal-api`: shared HAL traits and global registration
- `crates/hal-riscv64-virt`: RISC-V `virt` platform implementation skeleton
- `crates/kernel-core`: boot flow, logging, panic path, and kernel wiring
- `crates/mm`: frame allocator and user address-space model
- `crates/task`: cooperative task scheduler model
- `crates/proc`: process table and file descriptor state
- `crates/vfs`: in-memory root filesystem, devfs, procfs-lite, and mounts
- `crates/syscall`: RISC-V ABI syscall dispatcher with Phase 1 handlers
- `crates/user-init`: built-in init seed data and boot-time filesystem setup
- `platform/riscv64-virt`: platform binary and RISC-V entry assembly
- `tools/xtask`: build/check/qemu helper entrypoints

## Build Outputs (Competition)

The competition runner executes `make all` and expects:

- `kernel-rv`
- `kernel-la`

Optional extra disk images:

- `disk.img` (RISC-V extra disk)
- `disk-la.img` (LoongArch extra disk)

## Commands

```bash
make all
make check
make test
make oscomp-images
make contest-selfcheck
```

## Run Modes

`xtask` supports two QEMU modes:

- `contest` (default for `oscomp-*`): run QEMU inside the contest docker image.
- `host` (default for `qemu-*`): run QEMU directly on host tools.

Competition scoring should use `contest` mode only.

Environment controls:

- `WHUSE_QEMU_MODE=contest|host`
- `WHUSE_QEMU_RISCV_MEM=<size>` RISC-V QEMU memory (default `1G`)
- `WHUSE_QEMU_LOONGARCH_MEM=<size>` LoongArch QEMU memory (default `1G`)
- `WHUSE_DISK_IMAGE=<path>` for primary disk
- `WHUSE_EXTRA_DISK_IMAGE=<path>` for second disk
- `WHUSE_OSCOMP_TESTSUITS_DIR=<path>` testsuits source
- `WHUSE_OSCOMP_DOCKER_IMAGE=<image>` contest image (default `docker.educg.net/cg/os-contest:20260104`)
- `WHUSE_OSCOMP_COMPAT=0` for real execution flow

## Competition-Aligned Entry Points

```bash
make oscomp-riscv-contest
make oscomp-loongarch-contest
make contest-selfcheck
```

Host quick mode:

```bash
make oscomp-riscv-host
make oscomp-loongarch-host
```

## Performance Gate (Merge Policy)

Before merging feature branches into `master`, compare each architecture against
its own baseline:

- `step-begin/step-end` coverage must not regress.
- Any step previously ending with `:0` must not regress to non-zero.
- Runtime regression tolerance is within `<=3%` jitter.

## Competition Flow Notes

- `make all` always produces `kernel-rv` and `kernel-la` at repository root.
- Contest LoongArch boot path uses `-kernel kernel-la` (not bootrom/loader).
- Test execution is scan-driven: the kernel discovers `*_testcode.sh` from disk,
  runs them serially, emits `step-begin/end/timeout/skip`, and prints
  group `START/END` markers for scoring.

| Item | Previous | Contest-aligned now |
| --- | --- | --- |
| LoongArch contest boot | bootrom + loader | direct `-kernel kernel-la` |
| Testsuite execution | fixed built-in order | disk scan `*_testcode.sh` (serial) |
| Default oscomp mode | implicit/mixed | explicit `contest` |
| Baseline self-check | manual | `make contest-selfcheck` |
