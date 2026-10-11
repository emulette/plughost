//! Reusable, exactly sized storage for references borrowed only during a synchronous call.
use std::ptr::NonNull;

use plughost_core::Sample;

pub(super) struct ChannelViews<S: Sample> {
    channels: Vec<*mut [S]>,
}

// SAFETY: these are scratch pointers, not retained borrows. Every entry exposed by either
// callback is overwritten from that call's live slices first; old pointers are never read.
// Moving the storage between control and processing threads cannot access the old pointees.
unsafe impl<S: Sample> Send for ChannelViews<S> {}

impl<S: Sample> ChannelViews<S> {
    pub fn new(channels: usize) -> Self {
        let empty = std::ptr::slice_from_raw_parts_mut(NonNull::<S>::dangling().as_ptr(), 0);
        Self {
            channels: vec![empty; channels],
        }
    }

    pub fn with_inputs<'a, T>(
        &mut self,
        input: impl Iterator<Item = &'a [S]>,
        call: impl FnOnce(&[&[S]]) -> T,
    ) -> T {
        let mut count = 0;
        for (index, channel) in input.enumerate() {
            self.channels[index] = (channel as *const [S]).cast_mut();
            count += 1;
        }
        // SAFETY: references and raw pointers have the same layout. Every entry in this prefix
        // was just written from a live shared slice. References exist only for the callback,
        // which cannot let a borrow escape; the stored pointers remain raw after it returns.
        let views =
            unsafe { std::slice::from_raw_parts(self.channels.as_ptr().cast::<&[S]>(), count) };
        call(views)
    }

    pub fn with_outputs<T>(
        &mut self,
        output: &mut [Vec<S>],
        call: impl FnOnce(&mut [&mut [S]]) -> T,
    ) -> T {
        for (index, channel) in output.iter_mut().enumerate() {
            self.channels[index] = channel.as_mut_slice() as *mut [S];
        }
        // SAFETY: every prefix entry came from one uniquely borrowed Vec's live slice. Their
        // allocations are disjoint, the reference array is exclusively borrowed, and both it
        // and its channel borrows end with this synchronous callback. No reference is cached.
        let views = unsafe {
            std::slice::from_raw_parts_mut(
                self.channels.as_mut_ptr().cast::<&mut [S]>(),
                output.len(),
            )
        };
        call(views)
    }
}
