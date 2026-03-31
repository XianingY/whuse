#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::alloc::{GlobalAlloc, Layout};
#[cfg(target_os = "none")]
use core::arch::global_asm;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use core::ptr::{null_mut, NonNull};
#[cfg(target_os = "none")]
use spin::Mutex;

#[cfg(target_os = "none")]
global_asm!(include_str!("entry.S"));

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 192 * 1024 * 1024;

#[cfg(target_os = "none")]
#[repr(align(16))]
struct Heap([u8; HEAP_SIZE]);

#[cfg(target_os = "none")]
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[cfg(target_os = "none")]
pub mod buddy_allocator {
    use core::alloc::Layout;
    use core::cmp::min;
    use core::mem::size_of;
    use core::ptr::NonNull;

    fn prev_power_of_two(num: usize) -> usize {
        if num == 0 {
            return 0;
        }
        1 << (usize::BITS as usize - num.leading_zeros() as usize - 1)
    }

    pub struct Heap<const ORDER: usize> {
        free_list: [*mut usize; ORDER],
        start: usize,
        end: usize,
    }

    impl<const ORDER: usize> Heap<ORDER> {
        pub const fn new() -> Self {
            Self {
                free_list: [core::ptr::null_mut(); ORDER],
                start: 0,
                end: 0,
            }
        }

        fn push(&mut self, order: usize, ptr: *mut usize) {
            unsafe {
                *ptr = self.free_list[order] as usize;
                self.free_list[order] = ptr;
            }
        }

        fn pop(&mut self, order: usize) -> Option<*mut usize> {
            let res = self.free_list[order];
            if res.is_null() {
                None
            } else {
                unsafe {
                    self.free_list[order] = *res as *mut usize;
                }
                Some(res)
            }
        }

        pub unsafe fn init(&mut self, mut start: usize, mut end: usize) {
            start = (start + size_of::<usize>() - 1) & (!size_of::<usize>() + 1);
            end &= !size_of::<usize>() + 1;
            self.start = start;
            self.end = end;
            let mut current = start;
            while current + size_of::<usize>() <= end {
                let lowbit = current & (!current + 1);
                let size = min(lowbit, prev_power_of_two(end - current));
                self.push(size.trailing_zeros() as usize, current as *mut usize);
                current += size;
            }
        }

        pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, ()> {
            let size = layout
                .size()
                .max(layout.align())
                .max(size_of::<usize>())
                .next_power_of_two();
            let start_order = size.trailing_zeros() as usize;
            for order in start_order..ORDER {
                if !self.free_list[order].is_null() {
                    for split in (start_order + 1..order + 1).rev() {
                        let block = self.pop(split).unwrap();
                        let buddy = (block as usize + (1 << (split - 1))) as *mut usize;
                        self.push(split - 1, buddy);
                        self.push(split - 1, block);
                    }
                    return Ok(NonNull::new(self.pop(start_order).unwrap() as *mut u8).unwrap());
                }
            }
            Err(())
        }

        pub fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
            let size = layout
                .size()
                .max(layout.align())
                .max(size_of::<usize>())
                .next_power_of_two();
            let order = size.trailing_zeros() as usize;
            let current = ptr.as_ptr() as usize;
            if current < self.start || current.saturating_add(1usize << order) > self.end {
                return;
            }
            self.push(order, current as *mut usize);
        }
    }
}

#[cfg(target_os = "none")]
unsafe impl<const ORDER: usize> Send for buddy_allocator::Heap<ORDER> {}
#[cfg(target_os = "none")]
unsafe impl<const ORDER: usize> Sync for buddy_allocator::Heap<ORDER> {}

#[cfg(target_os = "none")]
struct LockedBuddyAllocator<const ORDER: usize>(Mutex<buddy_allocator::Heap<ORDER>>);

#[cfg(target_os = "none")]
unsafe impl<const ORDER: usize> GlobalAlloc for LockedBuddyAllocator<ORDER> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let res = self
            .0
            .lock()
            .alloc(layout)
            .map(|p| p.as_ptr())
            .unwrap_or(null_mut());
        if res.is_null() && layout.size() > 0 {
            let mut console = hal_api::ConsoleWriter;
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!(
                    "whuse: loongarch alloc failed size={} align={}\n",
                    layout.size(),
                    layout.align()
                ),
            );
        }
        res
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }
        self.0.lock().dealloc(NonNull::new_unchecked(ptr), layout);
    }
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: LockedBuddyAllocator<32> =
    LockedBuddyAllocator(Mutex::new(buddy_allocator::Heap::new()));

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn rust_main(hart_id: usize, dtb_pa: usize) -> ! {
    unsafe {
        let start = core::ptr::addr_of_mut!(HEAP.0) as usize;
        ALLOCATOR.0.lock().init(start, start + HEAP_SIZE);
    }
    hal_loongarch64_virt::bootstrap(dtb_pa);
    kernel_core::boot_forever(kernel_core::BootInfo {
        hart_id,
        dtb_pa,
        platform: "loongarch64-virt",
    });
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let mut console = hal_api::ConsoleWriter;
    let _ = core::fmt::Write::write_fmt(&mut console, format_args!("whuse: PANIC {}\n", info));
    hal_api::hal()
        .lifecycle
        .shutdown(hal_api::ShutdownReason::Failure)
}

#[cfg(not(target_os = "none"))]
fn main() {
    hal_loongarch64_virt::bootstrap(0);
    let _kernel = kernel_core::Kernel::bootstrap(kernel_core::BootInfo {
        hart_id: 0,
        dtb_pa: 0,
        platform: "loongarch64-virt-host-stub",
    });
    println!("whuse loongarch host stub built successfully");
}
