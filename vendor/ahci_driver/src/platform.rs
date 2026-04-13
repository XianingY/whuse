use core::{alloc::Layout, arch::asm};

use alloc::alloc::alloc;

unsafe extern "C" {
    pub safe fn ahci_printf(fmt: *const u8, _: ...) -> i32;

    // 物理地址转换为uncached虚拟地址
    pub safe fn ahci_phys_to_uncached(pa: u64) -> u64;

    // cached虚拟地址转换为物理地址
    // ahci dma可以接受64位的物理地址
    pub safe fn ahci_virt_to_phys(va: u64) -> u64;

    pub safe fn ahci_mdelay(ms: u64);
}

pub fn ahci_malloc_align(size: u64, align: u32) -> u64 {
    unsafe { alloc(Layout::from_size_align_unchecked(size as _, align as _)) as u64 }
}

// 同步dcache中所有cached和uncached访存请求
pub fn ahci_sync_dcache() {
    unsafe {
        asm!("dbar 0");
    }
}
