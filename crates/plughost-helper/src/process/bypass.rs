//! Each slot's host bypass on the processing thread: whether it is on, and how far the crossfade
//! from the plugin's output to the slot's delayed dry input has gone. While the realtime worker
//! runs, a switch crossfades over [`RAMP`]; otherwise it takes effect from the next block's first
//! frame.

/// How long a host bypass switch crossfades while the realtime worker runs, in seconds.
const RAMP: f64 = 0.01;

#[derive(Default)]
pub(crate) struct HostBypass {
    enabled: Vec<bool>,
    /// Per slot: 0 plays the plugin's output, 1 the dry input.
    mix: Vec<f32>,
}

impl HostBypass {
    /// The slots' host bypass `enabled`, already in effect.
    pub fn new(enabled: Vec<bool>) -> HostBypass {
        let mix = enabled
            .iter()
            .map(|&on| if on { 1.0 } else { 0.0 })
            .collect();
        HostBypass { enabled, mix }
    }

    /// Turns the slot's host bypass on or off, crossfading when `ramp` is set.
    pub fn set(&mut self, slot: usize, enabled: bool, ramp: bool) {
        self.enabled[slot] = enabled;
        if !ramp {
            self.mix[slot] = if enabled { 1.0 } else { 0.0 };
        }
    }

    /// Ends the crossfades under way, as when the realtime worker stops or the chain is reset.
    pub fn settle(&mut self) {
        for (mix, &enabled) in self.mix.iter_mut().zip(&self.enabled) {
            *mix = if enabled { 1.0 } else { 0.0 };
        }
    }

    pub fn enabled(&self) -> &[bool] {
        &self.enabled
    }

    /// The slot's host bypass for a block, whose crossfade moves by `step` per frame.
    pub fn slot(&mut self, slot: usize, step: f32) -> Fade<'_> {
        Fade {
            enabled: self.enabled[slot],
            mix: &mut self.mix[slot],
            step,
        }
    }
}

/// One slot's host bypass while a block is processed: whether it is on, how far its crossfade
/// has gone, which the block moves on, and how far the crossfade moves per frame.
pub(crate) struct Fade<'a> {
    pub enabled: bool,
    pub mix: &'a mut f32,
    pub step: f32,
}

/// How far a crossfade moves per frame at `sample_rate`.
pub(crate) fn step(sample_rate: f64) -> f32 {
    (1.0 / (sample_rate * RAMP)) as f32
}
