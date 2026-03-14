#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::alloc::{GlobalAlloc, Layout};
#[cfg(target_os = "none")]
use core::arch::global_asm;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use core::ptr::null_mut;
#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "none")]
global_asm!(include_str!("entry.S"));

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

#[cfg(target_os = "none")]
#[repr(align(16))]
struct Heap([u8; HEAP_SIZE]);

#[cfg(target_os = "none")]
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[cfg(target_os = "none")]
struct BumpAllocator {
    offset: AtomicUsize,
}

#[cfg(target_os = "none")]
impl BumpAllocator {
    const fn new() -> Self {
        Self {
            offset: AtomicUsize::new(0),
        }
    }
}

#[cfg(target_os = "none")]
unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = core::ptr::addr_of_mut!(HEAP.0) as *mut u8 as usize;
        let mut current = self.offset.load(Ordering::Relaxed);
        loop {
            let aligned = align_up(base + current, layout.align());
            let next = aligned
                .checked_add(layout.size())
                .and_then(|value| value.checked_sub(base))
                .unwrap_or(HEAP_SIZE + 1);
            if next > HEAP_SIZE {
                return null_mut();
            }
            match self
                .offset
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::Relaxed)
            {
                Ok(_) => return aligned as *mut u8,
                Err(observed) => current = observed,
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn rust_main(hart_id: usize, dtb_pa: usize) -> ! {
    hal_riscv64_virt::bootstrap(dtb_pa);
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
    hal_riscv64_virt::bootstrap(0);
    let _kernel = kernel_core::Kernel::bootstrap(kernel_core::BootInfo {
        hart_id: 0,
        dtb_pa: 0,
        platform: "riscv64-virt-host-stub",
    });
    println!("whuse host stub built successfully");
}

#[cfg(target_os = "none")]
const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
