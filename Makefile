build:
	cargo xtask build-riscv

check:
	cargo xtask check

qemu:
	cargo xtask qemu-riscv

test:
	cargo test --workspace --exclude whuse-riscv64-virt

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
