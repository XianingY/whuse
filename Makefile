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

stage1-both:
	tools/dev/run_oscomp_stage1.sh both

stage2-riscv:
	TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} tools/dev/run_oscomp_stage2.sh riscv

stage2-riscv-3x:
	RUNS=$${RUNS:-3} TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} WHUSE_STAGE2_IMAGE_POLICY=$${WHUSE_STAGE2_IMAGE_POLICY:-auto} WHUSE_OSCOMP_COMPAT=$${WHUSE_OSCOMP_COMPAT:-0} WHUSE_STAGE2_STOP_ON_SUITE_DONE=$${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1} tools/dev/run_oscomp_stage2_3x.sh
