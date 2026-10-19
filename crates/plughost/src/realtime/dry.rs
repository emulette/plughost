//! The dry fallback's delay line. The control thread hands the callback a longer one through the
//! state's mailbox.

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
