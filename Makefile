build:
	cargo xtask build-riscv

check:
	cargo xtask check

qemu:
	cargo xtask qemu-riscv

test:
	cargo test -p proc -p task -p mm -p vfs -p fs-ext4 -p syscall

build-riscv:
	cargo xtask build-riscv

build-loongarch:
	cargo xtask build-loongarch

qemu-riscv:
	cargo xtask qemu-riscv

qemu-loongarch:
	cargo xtask qemu-loongarch

oscomp-riscv:
	cargo xtask oscomp-riscv

oscomp-loongarch:
	cargo xtask oscomp-loongarch

oscomp-images:
	cargo xtask oscomp-images

parallel-setup:
	tools/dev/setup_parallel_worktrees.sh

stage1-riscv:
	tools/dev/run_oscomp_stage1.sh riscv

stage1-loongarch:
	tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-chain:
	TIMEOUT_SECS=1200 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=chain-fast WHUSE_STAGE2_REAL_PHASE=full WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-gate-smoke-120:
	TIMEOUT_SECS=120 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=gate WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=smoke tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-gate-300:
	TIMEOUT_SECS=300 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=gate WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-full-3600:
	TIMEOUT_SECS=3600 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=full WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage1-both:
	tools/dev/run_oscomp_stage1.sh both

qemu-clean-loongarch:
	tools/dev/cleanup_stale_qemu.sh
