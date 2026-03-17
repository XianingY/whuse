# Whuse

Whuse is a Rust rewrite of the original RuOK kernel project, rebuilt as a
self-contained workspace rooted in this repository.

The first implementation target is `riscv64 + qemu virt`, with a modular
crate layout that mirrors the original HAL/kernel/process/filesystem/syscall
split while using Rust traits and ownership boundaries.

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

## Commands

```bash
make build
make check
make qemu
make test
```

`make qemu` expects `cargo` and `qemu-system-riscv64` to be installed and
available in `PATH`.

## Parallel Stage1 Workflow

Use worktrees + branches to run `riscv64` and `loongarch64` in parallel:

```bash
make parallel-setup
```

This provisions:

- `integration/stage1` in the main repo
- `arch/riscv-stage1` in `../whuse-rv`
- `arch/loongarch-stage1` in `../whuse-la`

Stage1 validation (real execution mode, `WHUSE_OSCOMP_COMPAT=0`) is available
as:

```bash
make stage1-riscv
make stage1-loongarch
make stage1-both
```

The stage1 runner writes independent logs under `/tmp/rv-stage1-*.log` and
`/tmp/la-stage1-*.log`, and checks required `step-begin/step-end` markers up to
the `iozone` phase gates.
