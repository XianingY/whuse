build:
	cargo xtask build

check:
	cargo xtask check

qemu:
	cargo xtask qemu

test:
	cargo test --workspace --exclude whuse-riscv64-virt

