//! Global allocator that tracks the peak heap size (performance measurement).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

impl Counting {
    /// Largest heap size so far in bytes, over all threads.
    pub fn peak() -> usize {
        PEAK.load(Relaxed)
    }

    fn grow(bytes: usize) {
        let now = CURRENT.fetch_add(bytes, Relaxed) + bytes;
        PEAK.fetch_max(now, Relaxed);
    }
}

// SAFETY: every call forwards to the system allocator with the caller's arguments.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            Self::grow(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            Self::grow(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            if size >= layout.size() {
                Self::grow(size - layout.size());
            } else {
                CURRENT.fetch_sub(layout.size() - size, Relaxed);
            }
        }
        moved
    }
}
