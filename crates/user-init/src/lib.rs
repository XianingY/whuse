#![cfg_attr(not(test), no_std)]

extern crate alloc;

use proc::Process;
use vfs::{KernelResult, KernelVfs};

pub const INIT_BANNER: &str = "whuse: init process bootstrapped\n";

pub fn seed_filesystem(vfs: &mut KernelVfs) -> KernelResult<()> {
    vfs.create_file("/", "/etc/motd", INIT_BANNER.as_bytes())?;
    vfs.create_file("/", "/bin/init", b"builtin-init")?;
    vfs.create_file("/", "/proc/version", b"whuse-riscv64-virt")?;
    Ok(())
}

pub fn seed_process(process: &mut Process) {
    process.address_space.install_bytes(0x1000, b"/etc/motd\0");
    process.address_space.install_bytes(0x2000, b"/tmp/boot.log\0");
    process.address_space.install_bytes(0x3000, b"hello from init\n\0");
    process.address_space.install_bytes(0x4000, &[0; 256]);
}

