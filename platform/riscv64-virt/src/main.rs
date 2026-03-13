#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::arch::global_asm;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
global_asm!(include_str!("entry.S"));

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn rust_main(hart_id: usize, dtb_pa: usize) -> ! {
    hal_riscv64_virt::bootstrap();
    kernel_core::boot_forever(kernel_core::BootInfo {
        hart_id,
        dtb_pa,
        platform: "riscv64-virt",
    });
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    hal_riscv64_virt::bootstrap();
    let _kernel = kernel_core::Kernel::bootstrap(kernel_core::BootInfo {
        hart_id: 0,
        dtb_pa: 0,
        platform: "riscv64-virt-host-stub",
    });
    println!("whuse host stub built successfully");
}

