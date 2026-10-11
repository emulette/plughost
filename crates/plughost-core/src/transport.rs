//! Caller-owned timeline positions. Processing elapsed time belongs to the native processor.

use serde::{Deserialize, Serialize};

use crate::InputError;

/// Musical meter; numerator must be nonzero and denominator a power of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    /// Number of denominator note units in a bar.
    pub numerator: u16,
    /// Note unit, such as 4 for quarter notes or 8 for eighth notes.
    pub denominator: u16,
}

/// Musical positions are quarter notes, including for compound meters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopRegion {
    /// Start position in quarter notes.
    pub start: f64,
    /// Exclusive loop end in quarter notes; greater than `start`.
    pub end: f64,
    /// Loop start on the caller’s absolute sample timeline.
    pub sample_start: i64,
    /// Exclusive loop end in samples; greater than `sample_start`.
    pub sample_end: i64,
}

/// Caller-supplied start and numbering of the current musical bar.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarPosition {
    /// Start position in quarter notes.
    pub start: f64,
    /// Caller-supplied bar number; advancement increments it at bar boundaries.
    pub number: i32,
}

/// A snapshot at the first sample of a block. `None` fields are unavailable, never synthesized.
/// A seek is a discontinuity between consecutive snapshots. Loop wrapping is caller-owned.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Transport {
    /// Signed absolute sample position of the block’s first frame.
    pub sample_position: i64,
    /// Absolute musical position in quarter notes, when available.
    pub beat_position: Option<f64>,
    /// Current bar start and number; requires an available beat position.
    pub bar_position: Option<BarPosition>,
    /// Finite, positive quarter notes per minute, when available.
    pub tempo: Option<f64>,
    /// Current musical meter, when available.
    pub time_signature: Option<TimeSignature>,
    /// Whether transport advances while samples are processed.
    pub playing: bool,
    /// Active loop boundaries, when available; wrapping remains caller-owned.
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

    /// Checks scalar ranges and consistency without querying a plugin.
    /// `Transport::validate_at_rate` additionally checks sample positions at the prepared rate.
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
    /// Frames to process, independent of audio channel count; may be zero.
    pub frames: usize,
    /// Snapshot at the first frame, or `None` when time information is unavailable.
    pub transport: Option<Transport>,
}

impl BlockContext {
    /// Constructs a block duration with no transport snapshot.
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

    /// Checks that the frame count fits native block lengths and validates any transport.
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
