//! The dry fallback's delay line, and the mailbox through which the control thread hands the
//! callback a longer one: the callback swaps storage without allocating or freeing, and leaves
//! what it replaced for the control thread to free.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use plughost_core::Sample;

/// The callback's input history, per channel, in a ring of a fixed capacity.
pub(crate) struct DryDelay {
    channels: Vec<Vec<f64>>,
    write: usize,
    delay: usize,
}

impl DryDelay {
    /// History for `channels` channels, enough for a delay of `capacity - quantum` frames.
    pub fn new(channels: usize, capacity: usize) -> DryDelay {
        DryDelay {
            channels: vec![vec![0.0; capacity]; channels],
            write: 0,
            delay: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Delays by `delay` frames; the control thread made the capacity room for it.
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay.min(self.capacity());
    }

    /// Appends one callback's input; silence when the input had another shape.
    pub fn push<S: Sample>(&mut self, input: Option<&[&[S]]>, frames: usize) {
        let capacity = self.capacity();
        if capacity == 0 {
            return;
        }
        for (index, history) in self.channels.iter_mut().enumerate() {
            let source = input.and_then(|input| input.get(index));
            for frame in 0..frames {
                history[(self.write + frame) % capacity] =
                    source.map_or(0.0, |source| source[frame].to_f64());
            }
        }
        self.write = (self.write + frames) % capacity;
    }

    /// Frame `frame` of the callback just pushed, `frames` long, delayed.
    pub fn delayed(&self, channel: usize, frame: usize, frames: usize) -> f64 {
        let capacity = self.capacity();
        let Some(history) = self.channels.get(channel) else {
            return 0.0;
        };
        if capacity == 0 {
            return 0.0;
        }
        let position = (self.write + capacity * 2 + frame - frames - self.delay) % capacity;
        history[position]
    }
}

/// One value on its way from the control thread to the callback, and one on its way back.
pub(crate) struct Mailbox<T> {
    incoming: AtomicPtr<T>,
    retired: AtomicPtr<T>,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Mailbox<T> {
        Mailbox {
            incoming: AtomicPtr::new(ptr::null_mut()),
            retired: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

impl<T> Mailbox<T> {
    /// Control thread: offers `value`, replacing one the callback has not taken.
    pub fn send(&self, value: Box<T>) {
        let old = self.incoming.swap(Box::into_raw(value), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: only `send` stores into `incoming`, always from `Box::into_raw`, and the
            // swap took sole ownership.
            drop(unsafe { Box::from_raw(old) });
        }
        self.reclaim();
    }

    /// Control thread: frees what the callback replaced.
    pub fn reclaim(&self) {
        let old = self.retired.swap(ptr::null_mut(), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: only `receive` stores into `retired`, from `Box::into_raw`.
            drop(unsafe { Box::from_raw(old) });
        }
    }

    /// Callback: replaces `current` with an offered value, unless what it replaced last has
    /// not been freed yet, in which case the offer waits for a later call.
    pub fn receive(&self, current: &mut Box<T>) {
        if !self.retired.load(Ordering::Acquire).is_null() {
            return;
        }
        let offered = self.incoming.swap(ptr::null_mut(), Ordering::AcqRel);
        if offered.is_null() {
            return;
        }
        // SAFETY: as in `send`; the swap took sole ownership.
        let old = std::mem::replace(current, unsafe { Box::from_raw(offered) });
        self.retired.store(Box::into_raw(old), Ordering::Release);
    }
}

impl<T> Drop for Mailbox<T> {
    fn drop(&mut self) {
        for slot in [&self.incoming, &self.retired] {
            let value = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !value.is_null() {
                // SAFETY: both slots hold only pointers from `Box::into_raw`.
                drop(unsafe { Box::from_raw(value) });
            }
        }
    }
}

// SAFETY: the mailbox moves boxed values between threads, so it is as sendable as they are.
unsafe impl<T: Send> Send for Mailbox<T> {}
unsafe impl<T: Send> Sync for Mailbox<T> {}
