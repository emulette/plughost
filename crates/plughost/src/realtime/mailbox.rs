//! The mailbox through which the control thread hands the callback a value, such as a longer dry
//! delay line or a recovered chain's ring, and the callback hands back the value it replaced: the
//! callback swaps values without allocating, freeing or waiting, and the control thread frees
//! what the callback replaced.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// Marks, in the pointer's lowest bit, a value the callback handed back rather than an offer.
const RETURNED: usize = 1;

/// One slot holding nothing, a value the control thread offers, or the value the callback
/// replaced when it took the last offer. The callback can always take an offer, so a value the
/// control thread offers is never held up by one it has not freed yet.
pub(crate) struct Mailbox<T> {
    slot: AtomicPtr<T>,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Mailbox<T> {
        Mailbox {
            slot: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

fn returned<T>(pointer: *mut T) -> bool {
    pointer.addr() & RETURNED != 0
}

/// Frees an offer or a handed back value.
///
/// # Safety
///
/// `pointer` is null, or came from `Box::into_raw`, possibly marked, and nothing else owns it.
unsafe fn free<T>(pointer: *mut T) {
    let pointer = pointer.map_addr(|address| address & !RETURNED);
    if !pointer.is_null() {
        // SAFETY: forwarded from the caller.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

impl<T> Mailbox<T> {
    /// Control thread: offers `value`, freeing an offer the callback has not taken or what it
    /// handed back.
    pub fn send(&self, value: Box<T>) {
        const { assert!(align_of::<T>() > RETURNED) };
        let old = self.slot.swap(Box::into_raw(value), Ordering::AcqRel);
        // SAFETY: the slot holds only pointers from `Box::into_raw`, and the swap took sole
        // ownership.
        unsafe { free(old) };
    }

    /// Control thread: frees what the callback handed back.
    pub fn reclaim(&self) {
        let current = self.slot.load(Ordering::Acquire);
        if returned(current)
            && self
                .slot
                .compare_exchange(
                    current,
                    ptr::null_mut(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            // SAFETY: as in `send`; the exchange took sole ownership.
            unsafe { free(current) };
        }
    }

    /// Callback: replaces `current` with the offered value, if there is one, and hands `current`
    /// back for the control thread to free.
    pub fn receive(&self, current: &mut Box<T>) {
        const { assert!(align_of::<T>() > RETURNED) };
        let offered = self.slot.load(Ordering::Acquire);
        if offered.is_null() || returned(offered) {
            return;
        }
        // SAFETY: `current` is moved out here and written back below; nothing between panics.
        let old = Box::into_raw(unsafe { ptr::read(current) });
        let handed_back = old.map_addr(|address| address | RETURNED);
        let taken = self
            .slot
            .compare_exchange(offered, handed_back, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        // The control thread replaced the offer meanwhile; `current` stays.
        let next = if taken { offered } else { old };
        // SAFETY: `next` came from `Box::into_raw`: the offer the exchange took sole ownership
        // of, or `current`'s own value.
        unsafe { ptr::write(current, Box::from_raw(next)) };
    }
}

impl<T> Drop for Mailbox<T> {
    fn drop(&mut self) {
        let value = self.slot.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: the slot holds only pointers from `Box::into_raw`, now owned by nothing else.
        unsafe { free(value) };
    }
}

// SAFETY: the mailbox moves boxed values between threads, so it is as sendable as they are.
unsafe impl<T: Send> Send for Mailbox<T> {}
unsafe impl<T: Send> Sync for Mailbox<T> {}
