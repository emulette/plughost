//! The benchmark's global allocator counts heap allocations across all application threads, so
//! each render reports how often the host allocated. Plugin heaps are not counted.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

struct Counting;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

// SAFETY: every call is delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// Allocations and reallocations since process start.
pub fn count() -> u64 {
    ALLOCATIONS.load(Relaxed)
}
