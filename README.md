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

