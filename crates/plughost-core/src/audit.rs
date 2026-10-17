//! Marks for the real-time audit of the helper's processing path: the host's code that processes
//! a block, the plugin's own code it calls, and the host callbacks the plugin makes from there.
//! Each mark holds until the `Scope` it returns drops.
//!
//! With the `realtime-audit` feature, RealtimeSanitizer reports every allocation, lock and system
//! call the host makes on that path, with its stack, on macOS when the build sets
//! `RTSAN_ENABLE=1`; plugin code is left out, and the host callbacks inside it are watched again.
//! On every platform `Allocator` also counts the host's allocations and frees there, which
//! `processing_allocations` returns, and `report` sums them up with the plugin queries the host
//! made. Without the feature the marks do nothing.

/// A mark, until it drops.
#[must_use]
pub struct Scope {
    #[cfg(feature = "realtime-audit")]
    _scope: enabled::Scope,
}

/// The host's processing of a block, or of a call made between blocks on the processing thread.
#[inline]
pub fn processing() -> Scope {
    Scope {
        #[cfg(feature = "realtime-audit")]
        _scope: enabled::Scope::enter(enabled::Context::Host),
    }
}

/// A call into the plugin, outside the audit.
#[inline]
pub fn plugin() -> Scope {
    Scope {
        #[cfg(feature = "realtime-audit")]
        _scope: enabled::Scope::enter(enabled::Context::Plugin),
    }
}

/// A query of the plugin's state (its tail, latency or a property), outside the audit but
/// counted: such calls belong on the main thread.
#[inline]
pub fn query() -> Scope {
    #[cfg(feature = "realtime-audit")]
    enabled::count_query();
    plugin()
}

/// A host callback the plugin makes, inside the audit again.
#[inline]
pub fn callback() -> Scope {
    Scope {
        #[cfg(feature = "realtime-audit")]
        _scope: enabled::Scope::enter(enabled::Context::Callback),
    }
}

#[cfg(feature = "realtime-audit")]
pub use enabled::{Allocator, processing_allocations, report};

#[cfg(feature = "realtime-audit")]
mod enabled {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// What the current thread runs.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Context {
        Outside,
        Host,
        Plugin,
        Callback,
    }

    thread_local! {
        static CONTEXT: Cell<Context> = const { Cell::new(Context::Outside) };
    }

    /// Allocations and frees of the host's processing code, and of its callbacks.
    static COUNTS: [[AtomicU64; 2]; 2] = [
        [AtomicU64::new(0), AtomicU64::new(0)],
        [AtomicU64::new(0), AtomicU64::new(0)],
    ];
    static QUERIES: AtomicU64 = AtomicU64::new(0);

    /// A context entered for the life of the value; it restores the enclosing one.
    pub struct Scope {
        enclosing: Context,
        entered: Context,
    }

    impl Scope {
        /// A plugin call outside processing, or a callback outside a plugin call, changes
        /// nothing.
        pub fn enter(context: Context) -> Scope {
            let enclosing = CONTEXT.get();
            let entered = match (enclosing, context) {
                (Context::Outside, Context::Host) => {
                    rtsan_standalone::realtime_enter();
                    Context::Host
                }
                (Context::Host | Context::Callback, Context::Plugin) => {
                    rtsan_standalone::disable();
                    Context::Plugin
                }
                (Context::Plugin, Context::Callback) => {
                    rtsan_standalone::enable();
                    Context::Callback
                }
                _ => enclosing,
            };
            CONTEXT.set(entered);
            Scope { enclosing, entered }
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            match (self.enclosing, self.entered) {
                (Context::Outside, Context::Host) => rtsan_standalone::realtime_exit(),
                (Context::Host | Context::Callback, Context::Plugin) => rtsan_standalone::enable(),
                (Context::Plugin, Context::Callback) => rtsan_standalone::disable(),
                _ => {}
            }
            CONTEXT.set(self.enclosing);
        }
    }

    pub fn count_query() {
        if matches!(CONTEXT.get(), Context::Host | Context::Callback) {
            QUERIES.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn count(free: bool) {
        let row = match CONTEXT.get() {
            Context::Host => 0,
            Context::Callback => 1,
            Context::Outside | Context::Plugin => return,
        };
        COUNTS[row][usize::from(free)].fetch_add(1, Ordering::Relaxed);
    }

    /// The system allocator, counting the host's allocations and frees while it processes.
    pub struct Allocator;

    // SAFETY: every call is delegated unchanged to the system allocator.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count(false);
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count(false);
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            count(true);
            unsafe { System.dealloc(pointer, layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            count(false);
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    /// Allocations and frees in the host's processing code so far, on any thread.
    pub fn processing_allocations() -> (u64, u64) {
        let [allocations, frees] = &COUNTS[0];
        (
            allocations.load(Ordering::Relaxed),
            frees.load(Ordering::Relaxed),
        )
    }

    /// The counts so far: allocations and frees in the host's processing code, then in its
    /// callbacks, and the plugin queries made while processing.
    pub fn report() -> String {
        let [[host, host_freed], [callback, callback_freed]] = COUNTS
            .each_ref()
            .map(|row| row.each_ref().map(|count| count.load(Ordering::Relaxed)));
        format!(
            "{} {host} allocations and {host_freed} frees in processing, {callback} allocations \
             and {callback_freed} frees in host callbacks, {} plugin queries",
            crate::messages::AUDIT,
            QUERIES.load(Ordering::Relaxed)
        )
    }
}
