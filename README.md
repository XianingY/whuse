# Whuse

Whuse is a Rust rewrite of the original RuOK kernel project, rebuilt as a
self-contained workspace rooted in this repository.

The project now tracks both `riscv64` and `loongarch64` with OSCOMP-oriented
build and run entrypoints.

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
```

## Run Modes

`xtask` supports two QEMU modes:

- `contest` (default for `oscomp-*`): run QEMU inside the contest docker image.
- `host` (default for `qemu-*`): run QEMU directly on host tools.

Environment controls:

- `WHUSE_QEMU_MODE=contest|host`
- `WHUSE_DISK_IMAGE=<path>` for primary disk
- `WHUSE_EXTRA_DISK_IMAGE=<path>` for second disk
- `WHUSE_OSCOMP_TESTSUITS_DIR=<path>` testsuits source
- `WHUSE_OSCOMP_DOCKER_IMAGE=<image>` contest image (default `docker.educg.net/cg/os-contest:20260104`)
- `WHUSE_OSCOMP_COMPAT=0` for real execution flow

## Competition-Aligned Entry Points

```bash
make oscomp-riscv-contest
make oscomp-loongarch-contest
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
