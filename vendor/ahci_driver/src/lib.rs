#![no_std]
#![allow(dead_code, unused_assignments, unused_mut, non_camel_case_types)]

extern crate alloc;

mod drv_ahci;
mod libahci;
mod libata;
mod platform;

pub use drv_ahci::{ahci_init, ahci_sata_read_common, ahci_sata_write_common};
pub use libahci::ahci_device;
