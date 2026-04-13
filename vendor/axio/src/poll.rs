use core::task::Context;

use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct IoEvents: u16 {
        const IN     = 0x0001;
        const PRI    = 0x0002;
        const OUT    = 0x0004;
        const ERR    = 0x0008;
        const HUP    = 0x0010;
        const NVAL   = 0x0020;

        const RDNORM = 0x0040;
        const RDBAND = 0x0080;
        const WRNORM = 0x0100;
        const WRBAND = 0x0200;

        const MSG    = 0x0400;
        const REMOVE = 0x1000;
        const RDHUP  = 0x2000;

        /// Events that are always polled even without specifying them.
        const ALWAYS_POLL = Self::ERR.bits() | Self::HUP.bits();
    }
}

/// Trait for types that can be polled for I/O events.
pub trait Pollable {
    /// Polls the type for I/O events.
    fn poll(&self) -> IoEvents;

    /// Registers wakers for I/O events.
    fn register(&self, context: &mut Context<'_>, events: IoEvents);
}

#[cfg(feature = "alloc")]
struct Entry {
    waker: core::task::Waker,
    next: *mut Entry,
}

// TODO: optimize
/// A lock-free structure for waking up tasks that are waiting for I/O events.
#[cfg(feature = "alloc")]
pub struct PollSet {
    head: core::sync::atomic::AtomicPtr<Entry>,
}

#[cfg(feature = "alloc")]
impl Default for PollSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
impl PollSet {
    pub const fn new() -> Self {
        Self {
            head: core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub fn register(&self, waker: &core::task::Waker) {
        let entry = alloc::boxed::Box::leak(alloc::boxed::Box::new(Entry {
            waker: waker.clone(),
            next: core::ptr::null_mut(),
        }));

        entry.next = self.head.load(core::sync::atomic::Ordering::Acquire);
        loop {
            match self.head.compare_exchange_weak(
                entry.next,
                entry,
                core::sync::atomic::Ordering::Release,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => entry.next = current,
            }
        }
    }

    pub fn wake(&self) -> usize {
        let mut count = 0;
        let mut head = self
            .head
            .swap(core::ptr::null_mut(), core::sync::atomic::Ordering::AcqRel);
        while !head.is_null() {
            let entry = unsafe { alloc::boxed::Box::from_raw(head) };
            entry.waker.wake();
            count += 1;
            head = entry.next;
        }
        count
    }
}

#[cfg(feature = "alloc")]
impl Drop for PollSet {
    fn drop(&mut self) {
        // Ensure all entries are dropped
        self.wake();
    }
}

#[cfg(feature = "alloc")]
impl alloc::task::Wake for PollSet {
    fn wake(self: alloc::sync::Arc<Self>) {
        self.as_ref().wake();
    }

    fn wake_by_ref(self: &alloc::sync::Arc<Self>) {
        self.as_ref().wake();
    }
}
