//! Counts heap requests made by the current thread, so a test can assert that a call does not
//! allocate. Each test binary that declares `mod support;` installs this as its global allocator.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static REQUESTS: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn count() {
    REQUESTS.with(|requests| requests.set(requests.get() + 1));
}

// SAFETY: every call is delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// Runs `call` and returns its result with the allocations, reallocations, and deallocations the
/// current thread made meanwhile.
pub fn heap_requests<T>(call: impl FnOnce() -> T) -> (T, u64) {
    let before = REQUESTS.with(Cell::get);
    let value = call();
    (value, REQUESTS.with(Cell::get) - before)
}
