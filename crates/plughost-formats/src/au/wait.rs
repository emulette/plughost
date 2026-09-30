//! Waiting for Audio Unit completion handlers. They can be delivered through the calling thread's
//! run loop (the main queue on the main thread), so the wait keeps the run loop running instead
//! of blocking.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};

const SLICE: Duration = Duration::from_millis(10);

/// A value a completion handler fills in on some other thread or queue.
pub struct Slot<T> {
    value: Arc<Mutex<Option<T>>>,
}

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Slot<T> {
        Slot {
            value: Arc::clone(&self.value),
        }
    }
}

impl<T> Slot<T> {
    pub fn new() -> Slot<T> {
        Slot {
            value: Arc::new(Mutex::new(None)),
        }
    }

    pub fn fill(&self, value: T) {
        *self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
    }

    /// Runs the current thread's run loop until the value arrives. The helper's supervision
    /// bounds how long a plugin may take.
    pub fn wait(&self) -> T {
        loop {
            if let Some(value) = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                return value;
            }
            // SAFETY: the default mode is a valid run loop mode.
            unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, SLICE.as_secs_f64(), true) };
        }
    }
}

/// Runs the current thread's run loop for `duration`, delivering notifications and timers.
pub fn run_loop_for(duration: Duration) {
    let until = std::time::Instant::now() + duration;
    while std::time::Instant::now() < until {
        // SAFETY: the default mode is a valid run loop mode.
        unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, SLICE.as_secs_f64(), true) };
    }
}
