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
global_asm!(include_str!("entry.S"));

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 224 * 1024 * 1024;

#[cfg(target_os = "none")]
#[repr(align(16))]
struct Heap([u8; HEAP_SIZE]);

#[cfg(target_os = "none")]
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[cfg(target_os = "none")]
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

    pub struct LinkedList {
        head: *mut usize,
    }

    impl LinkedList {
        pub const fn new() -> Self {
            Self {
                head: core::ptr::null_mut(),
            }
        }
        pub fn is_empty(&self) -> bool {
            self.head.is_null()
        }
        pub unsafe fn push(&mut self, ptr: *mut usize) {
            *ptr = self.head as usize;
            self.head = ptr;
        }
        pub fn pop(&mut self) -> Option<*mut usize> {
            if self.head.is_null() {
                None
            } else {
                let res = self.head;
                self.head = unsafe { *res as *mut usize };
                Some(res)
            }
        }
        pub fn iter_mut(&mut self) -> IterMut {
            IterMut {
                prev: &mut self.head as *mut *mut usize,
                curr: self.head,
            }
        }
    }

    pub struct IterMut {
        prev: *mut *mut usize,
        curr: *mut usize,
    }

    impl IterMut {
        pub fn pop(&mut self) -> Option<*mut usize> {
            if self.curr.is_null() {
                None
            } else {
                let res = self.curr;
                self.curr = unsafe { *res as *mut usize };
                unsafe { *self.prev = self.curr };
                Some(res)
            }
        }
        pub fn next(&mut self) {
            if !self.curr.is_null() {
                self.prev = self.curr as *mut *mut usize;
                self.curr = unsafe { *self.curr as *mut usize };
            }
        }
        pub fn value(&self) -> *mut usize {
            self.curr
        }
    }

    unsafe impl Send for LinkedList {}
    unsafe impl Sync for LinkedList {}

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
            let mut console = hal_api::ConsoleWriter;
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!("whuse: buddy heap init {:#x}-{:#x}\n", start, end),
            );
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
                    for j in (start_order + 1..order + 1).rev() {
                        let block = self.pop(j).unwrap();
                        let buddy = (block as usize + (1 << (j - 1))) as *mut usize;
                        self.push(j - 1, buddy);
                        self.push(j - 1, block);
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
            let mut order = size.trailing_zeros() as usize;
            let mut current = ptr.as_ptr() as usize;
            while order + 1 < ORDER {
                let buddy = current ^ (1usize << order);
                // Only merge if buddy is within the heap range.
                if buddy < self.start || buddy.saturating_add(1usize << order) > self.end {
                    break;
                }
                let mut found = false;
                let mut scan = self.free_list[order];
                let mut prev: *mut *mut usize = &mut self.free_list[order] as *mut *mut usize;
                while !scan.is_null() {
                    if scan as usize == buddy {
                        unsafe { *prev = *scan as *mut usize };
                        current = current.min(buddy);
                        order += 1;
                        found = true;
                        break;
                    }
                    unsafe {
                        prev = scan as *mut *mut usize;
                        scan = *scan as *mut usize;
                    }
                }
                if !found {
                    break;
                }
            }
            self.push(order, current as *mut usize);
        }
    }

    unsafe impl<const ORDER: usize> Send for Heap<ORDER> {}
    unsafe impl<const ORDER: usize> Sync for Heap<ORDER> {}
}

#[cfg(target_os = "none")]
use spin::Mutex;

#[cfg(target_os = "none")]
struct LockedBuddyAllocator<const ORDER: usize>(Mutex<buddy_allocator::Heap<ORDER>>);

#[cfg(target_os = "none")]
unsafe impl<const ORDER: usize> GlobalAlloc for LockedBuddyAllocator<ORDER> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut caller_ra = 0usize;
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("mv {}, ra", out(reg) caller_ra);
        }
        let cpu = hal_api::hal().cpu;
        let enabled = cpu.interrupts_enabled();
        if enabled {
            cpu.disable_interrupts();
        }
        let res = self
            .0
            .lock()
            .alloc(layout)
            .map(|p: core::ptr::NonNull<u8>| p.as_ptr())
            .unwrap_or(core::ptr::null_mut());
        if enabled {
            cpu.enable_interrupts();
        }
        if res.is_null() && layout.size() > 0 {
            let mut current_ra = 0usize;
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("mv {}, ra", out(reg) current_ra);
            }
            let mut console = hal_api::ConsoleWriter;
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!(
                    "whuse: alloc failed layout={:?} caller_ra={:#x} current_ra={:#x}\n",
                    layout, caller_ra, current_ra
                ),
            );
        }
        res
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !ptr.is_null() {
            let cpu = hal_api::hal().cpu;
            let enabled = cpu.interrupts_enabled();
            if enabled {
                cpu.disable_interrupts();
            }
            self.0
                .lock()
                .dealloc(core::ptr::NonNull::new_unchecked(ptr), layout);
            if enabled {
                cpu.enable_interrupts();
            }
        }
    }
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: LockedBuddyAllocator<32> =
    LockedBuddyAllocator(Mutex::new(buddy_allocator::Heap::new()));

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn rust_main(hart_id: usize, dtb_pa: usize) -> ! {
    hal_riscv64_virt::bootstrap(dtb_pa);
    unsafe {
        let start = core::ptr::addr_of_mut!(HEAP.0) as usize;
        ALLOCATOR.0.lock().init(start, start + HEAP_SIZE);
    }
    kernel_core::boot_forever(kernel_core::BootInfo {
        hart_id,
        dtb_pa,
        platform: "riscv64-virt",
    });
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let uart = 0x1000_0000usize as *mut u8;
    for &b in b"whuse: KERNEL PANIC: " {
        unsafe { core::ptr::write_volatile(uart, b) };
    }
    let mut console = hal_api::ConsoleWriter;
    let _ = core::fmt::Write::write_fmt(&mut console, format_args!("{}\n", info));
    hal_api::hal()
        .lifecycle
        .shutdown(hal_api::ShutdownReason::Failure)
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
