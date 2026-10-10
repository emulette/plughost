//! The plugin slot each helper thread is calling, kept in a small mapping the application owns.
//!
//! The helper records a slot before each call into a plugin and clears it afterwards. Recording is
//! a plain atomic store into shared memory, so announcing a call costs no system call. After the
//! helper crashes or stops responding, the application reads what each thread was doing.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use super::memory::{Handle, Memory};

/// A helper thread that calls into plugins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// Lifecycle, parameter, state and editor calls, and the event loop.
    Main = 0,
    /// Audio processing.
    Processing = 1,
}

const BYTES: usize = 2 * size_of::<u64>();

pub struct Activity {
    pub(super) memory: Memory,
}

impl Activity {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            memory: Memory::new(BYTES)?,
        })
    }

    /// # Safety
    ///
    /// `handle` must be the shared memory of a peer's [`Activity`].
    pub(super) unsafe fn open(handle: Handle) -> io::Result<Self> {
        Ok(Self {
            // SAFETY: forwarded from the caller.
            memory: unsafe { Memory::open(handle, BYTES) }?,
        })
    }

    fn word(&self, caller: Caller) -> &AtomicU64 {
        // SAFETY: the page-aligned mapping holds one word per caller, has a fixed length and
        // lives as long as `self`. Both processes access it only through these atomics.
        unsafe {
            &*self
                .memory
                .as_ptr()
                .cast::<AtomicU64>()
                .add(caller as usize)
        }
    }

    /// Records that `caller` is about to call into the plugin in `slot`.
    pub fn enter(&self, caller: Caller, slot: usize) {
        self.word(caller).store(slot as u64 + 1, Ordering::Release);
    }

    /// Records that `caller` returned from the plugin.
    pub fn leave(&self, caller: Caller) {
        self.word(caller).store(0, Ordering::Release);
    }

    /// The slot `caller` is calling, if it is inside a plugin.
    pub fn slot(&self, caller: Caller) -> Option<usize> {
        let value = self.word(caller).load(Ordering::Acquire);
        value
            .checked_sub(1)
            .and_then(|slot| usize::try_from(slot).ok())
    }
}
