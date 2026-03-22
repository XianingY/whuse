.PHONY: all build check qemu test build-riscv build-loongarch qemu-riscv qemu-loongarch \
	oscomp-riscv oscomp-loongarch oscomp-riscv-contest oscomp-loongarch-contest \
	oscomp-riscv-host oscomp-loongarch-host oscomp-images parallel-setup \
	stage1-riscv stage1-loongarch stage1-both stage2-riscv stage2-riscv-3x \
	stage2-riscv-ltp \
	stage2-loongarch-chain stage2-loongarch-gate-smoke-120 stage2-loongarch-gate-300 \
	stage2-loongarch-full-3600 qemu-clean-loongarch package-kernels contest-selfcheck \
	prepare-cargo-config

XTASK := cargo run --manifest-path tools/xtask/Cargo.toml --

all: prepare-cargo-config build-riscv build-loongarch package-kernels

prepare-cargo-config:
	@mkdir -p .cargo
	@cmp -s cargo_config.toml .cargo/config.toml || cp cargo_config.toml .cargo/config.toml

build: prepare-cargo-config
	$(XTASK) build-riscv

check: prepare-cargo-config
	$(XTASK) check

qemu: prepare-cargo-config
	$(XTASK) qemu-riscv

test: prepare-cargo-config
	cargo test -p proc -p task -p mm -p vfs -p fs-ext4 -p syscall

build-riscv: prepare-cargo-config
	$(XTASK) build-riscv

build-loongarch: prepare-cargo-config
	$(XTASK) build-loongarch

package-kernels:
	@test -f kernel-rv
	@test -f kernel-la

qemu-riscv: prepare-cargo-config
	$(XTASK) qemu-riscv

qemu-loongarch: prepare-cargo-config
	$(XTASK) qemu-loongarch

oscomp-riscv: prepare-cargo-config
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-riscv

oscomp-loongarch: prepare-cargo-config
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-loongarch

oscomp-riscv-contest: prepare-cargo-config
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-riscv

oscomp-loongarch-contest: prepare-cargo-config
	WHUSE_QEMU_MODE=contest $(XTASK) oscomp-loongarch

oscomp-riscv-host: prepare-cargo-config
	WHUSE_QEMU_MODE=host $(XTASK) oscomp-riscv

oscomp-loongarch-host: prepare-cargo-config
	WHUSE_QEMU_MODE=host $(XTASK) oscomp-loongarch

oscomp-images: prepare-cargo-config
	$(XTASK) oscomp-images

contest-selfcheck: prepare-cargo-config
	$(XTASK) contest-selfcheck

parallel-setup:
	tools/dev/setup_parallel_worktrees.sh

stage1-riscv: prepare-cargo-config
	tools/dev/run_oscomp_stage1.sh riscv

stage1-loongarch: prepare-cargo-config
	tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-chain: prepare-cargo-config
	TIMEOUT_SECS=1200 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=chain-fast WHUSE_STAGE2_REAL_PHASE=full WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-gate-smoke-120: prepare-cargo-config
	TIMEOUT_SECS=120 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=gate WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=smoke tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-gate-300: prepare-cargo-config
	TIMEOUT_SECS=300 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=gate WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage2-loongarch-full-3600: prepare-cargo-config
	TIMEOUT_SECS=3600 WHUSE_STAGE1_USE_IMAGE_COPY=1 WHUSE_OSCOMP_SKIP_BUILD=1 WHUSE_OSCOMP_COMPAT=0 WHUSE_STAGE2_TIMEOUT_PROFILE=real WHUSE_STAGE2_REAL_PHASE=full WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=full tools/dev/run_oscomp_stage1.sh loongarch

stage1-both: prepare-cargo-config
	tools/dev/run_oscomp_stage1.sh both

stage2-riscv: prepare-cargo-config
	TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} tools/dev/run_oscomp_stage2.sh riscv

stage2-riscv-ltp: prepare-cargo-config
	TIMEOUT_SECS=$${TIMEOUT_SECS:-2400} WHUSE_LTP_PROFILE=$${WHUSE_LTP_PROFILE:-score} tools/dev/run_oscomp_stage2.sh ltp-riscv

stage2-riscv-3x: prepare-cargo-config
	RUNS=$${RUNS:-3} TIMEOUT_SECS=$${TIMEOUT_SECS:-3600} WHUSE_STAGE2_IMAGE_POLICY=$${WHUSE_STAGE2_IMAGE_POLICY:-auto} WHUSE_OSCOMP_COMPAT=$${WHUSE_OSCOMP_COMPAT:-0} WHUSE_STAGE2_STOP_ON_SUITE_DONE=$${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1} tools/dev/run_oscomp_stage2_3x.sh

qemu-clean-loongarch:
	tools/dev/cleanup_stale_qemu.sh
