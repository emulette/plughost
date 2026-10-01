//! Caller-owned timeline positions. Processing elapsed time belongs to the native processor.

use serde::{Deserialize, Serialize};

use crate::InputError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u16,
    pub denominator: u16,
}

/// Musical positions are quarter notes, including for compound meters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopRegion {
    pub start: f64,
    pub end: f64,
    pub sample_start: i64,
    pub sample_end: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarPosition {
    pub start: f64,
    pub number: i32,
}

/// A snapshot at the first sample of a block. `None` fields are unavailable, never synthesized.
/// A seek is a discontinuity between consecutive snapshots. Loop wrapping is caller-owned.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Transport {
    pub sample_position: i64,
    pub beat_position: Option<f64>,
    pub bar_position: Option<BarPosition>,
    pub tempo: Option<f64>,
    pub time_signature: Option<TimeSignature>,
    pub playing: bool,
    pub loop_region: Option<LoopRegion>,
}

impl Transport {
    /// A position with no musical time, tempo, meter or loop.
    pub fn new(sample_position: i64, playing: bool) -> Transport {
        Transport {
            sample_position,
            beat_position: None,
            bar_position: None,
            tempo: None,
            time_signature: None,
            playing,
            loop_region: None,
        }
    }

    pub fn validate(&self) -> Result<(), InputError> {
        // CLAP represents beats/seconds as signed 31-bit fixed point. Keep positions in the
        // common representable range instead of silently saturating at the format boundary.
        let beat = |v: f64| v.is_finite() && (-4_294_967_296.0..4_294_967_296.0).contains(&v);
        if self.beat_position.is_some_and(|v| !beat(v))
            || self.bar_position.is_some_and(|v| {
                !beat(v.start) || self.beat_position.is_none_or(|position| v.start > position)
            })
            || self.tempo.is_some_and(|v| !v.is_finite() || v <= 0.0)
            || self
                .time_signature
                .is_some_and(|v| v.numerator == 0 || !v.denominator.is_power_of_two())
            || self.loop_region.is_some_and(|v| {
                !beat(v.start)
                    || !beat(v.end)
                    || v.start >= v.end
                    || v.sample_start >= v.sample_end
                    || self.beat_position.is_none()
            })
        {
            return Err(InputError::Transport);
        }
        Ok(())
    }

    /// Check representability at the configured rate before calling any native processor.
    pub fn validate_at_rate(&self, sample_rate: f64) -> Result<(), InputError> {
        crate::config::validate_processing(sample_rate, 1)?;
        self.validate()?;
        let valid = |position: i64| {
            (-4_294_967_296.0..4_294_967_296.0).contains(&(position as f64 / sample_rate))
        };
        if !valid(self.sample_position)
            || self
                .loop_region
                .is_some_and(|region| !valid(region.sample_start) || !valid(region.sample_end))
        {
            return Err(InputError::Transport);
        }
        Ok(())
    }

    /// Advance within a constant-tempo segment. Stopped transport stays in place.
    pub fn advanced(self, frames: usize, sample_rate: f64) -> Result<Self, InputError> {
        crate::config::validate_processing(sample_rate, 1)?;
        self.validate_at_rate(sample_rate)?;
        if !self.playing {
            return Ok(self);
        }
        let mut next = self;
        next.sample_position = self
            .sample_position
            .checked_add(i64::try_from(frames).map_err(|_| InputError::Transport)?)
            .ok_or(InputError::Transport)?;
        if let (Some(beat), Some(tempo)) = (self.beat_position, self.tempo) {
            let beats = beat + frames as f64 / sample_rate * tempo / 60.0;
            next.beat_position = Some(beats);
            if let (Some(bar), Some(meter)) = (self.bar_position, self.time_signature) {
                let length = f64::from(meter.numerator) * 4.0 / f64::from(meter.denominator);
                let count = ((beats - bar.start) / length).floor();
                let number = f64::from(bar.number) + count;
                if !number.is_finite() || number < i32::MIN as f64 || number > i32::MAX as f64 {
                    return Err(InputError::Transport);
                }
                next.bar_position = Some(BarPosition {
                    start: bar.start + count * length,
                    number: number as i32,
                });
            } else if frames != 0 {
                next.bar_position = None;
            }
        } else if frames != 0 {
            next.beat_position = None;
            next.bar_position = None;
            next.loop_region = None;
        }
        next.validate_at_rate(sample_rate)?;
        Ok(next)
    }
}

/// Explicit block duration, independent of the presence of audio channels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BlockContext {
    pub frames: usize,
    pub transport: Option<Transport>,
}

impl BlockContext {
    pub const fn new(frames: usize) -> Self {
        Self {
            frames,
            transport: None,
        }
    }

    /// The same block at `transport`.
    pub const fn with_transport(self, transport: Transport) -> Self {
        Self {
            transport: Some(transport),
            ..self
        }
    }

    pub fn validate(&self) -> Result<(), InputError> {
        if self.frames > i32::MAX as usize {
            return Err(InputError::BlockSize);
        }
        if let Some(transport) = self.transport {
            transport.validate()?;
        }
        Ok(())
    }
}
