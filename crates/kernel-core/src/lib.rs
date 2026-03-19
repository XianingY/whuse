#![no_std]

#[cfg(target_arch = "riscv64")]
include!("lib_riscv.inc.rs");

#[cfg(target_arch = "loongarch64")]
include!("lib_loongarch.inc.rs");

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
include!("lib_riscv.inc.rs");
