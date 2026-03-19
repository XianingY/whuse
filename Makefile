.PHONY: all build check qemu test build-riscv build-loongarch qemu-riscv qemu-loongarch \
	oscomp-riscv oscomp-loongarch oscomp-riscv-contest oscomp-loongarch-contest \
	oscomp-riscv-host oscomp-loongarch-host oscomp-images parallel-setup \
	stage1-riscv stage1-loongarch stage1-both stage2-riscv stage2-riscv-3x \
	stage2-loongarch-chain stage2-loongarch-gate-smoke-120 stage2-loongarch-gate-300 \
	stage2-loongarch-full-3600 qemu-clean-loongarch package-kernels contest-selfcheck

XTASK := cargo run --manifest-path tools/xtask/Cargo.toml --

all: build-riscv build-loongarch package-kernels

build:
	$(XTASK) build-riscv

check:
	$(XTASK) check

qemu:
	$(XTASK) qemu-riscv

test:
	cargo test -p proc -p task -p mm -p vfs -p fs-ext4 -p syscall

build-riscv:
	$(XTASK) build-riscv

build-loongarch:
	$(XTASK) build-loongarch

package-kernels:
	cp target/riscv64gc-unknown-none-elf/debug/whuse-riscv64-virt kernel-rv
	cp target/loongarch64-unknown-none-softfloat/debug/whuse-loongarch64-virt kernel-la

qemu-riscv:
	$(XTASK) qemu-riscv

qemu-loongarch:
	$(XTASK) qemu-loongarch

oscomp-riscv:
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-riscv

oscomp-loongarch:
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-loongarch

oscomp-riscv-contest:
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-riscv

oscomp-loongarch-contest:
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-loongarch

oscomp-riscv-host:
	WHUSE_QEMU_MODE=host $(XTASK) oscomp-riscv

oscomp-loongarch-host:
	WHUSE_QEMU_MODE=host $(XTASK) oscomp-loongarch

oscomp-images:
	$(XTASK) oscomp-images

contest-selfcheck:
	$(XTASK) contest-selfcheck

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

stage2-riscv:
	TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} tools/dev/run_oscomp_stage2.sh riscv

stage2-riscv-3x:
	RUNS=$${RUNS:-3} TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} WHUSE_STAGE2_IMAGE_POLICY=$${WHUSE_STAGE2_IMAGE_POLICY:-auto} WHUSE_OSCOMP_COMPAT=$${WHUSE_OSCOMP_COMPAT:-0} WHUSE_STAGE2_STOP_ON_SUITE_DONE=$${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1} tools/dev/run_oscomp_stage2_3x.sh

qemu-clean-loongarch:
	tools/dev/cleanup_stale_qemu.sh
