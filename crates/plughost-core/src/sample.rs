use std::ops::Range;

use crate::config::SampleFormat;

/// An audio sample type the plugin formats can process: `f32` or `f64`. It is sealed: only this
/// crate implements it, so a sample's `FORMAT` always names its own type.
pub trait Sample: sealed::Sealed + Copy + Default + Send + Sync + 'static {
    /// Native sample precision corresponding to this Rust type.
    const FORMAT: SampleFormat;

    /// Converts a sample for peak measurement and silence detection.
    fn to_f64(self) -> f64;
}

mod sealed {
    /// Implemented by `f32` and `f64` only.
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

impl Sample for f32 {
    const FORMAT: SampleFormat = SampleFormat::F32;

    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl Sample for f64 {
    const FORMAT: SampleFormat = SampleFormat::F64;

    fn to_f64(self) -> f64 {
        self
    }
}

/// What a block of audio holds, as rendering checks each block it delivers, and realtime
/// processing is to. Internal to the plughost crates; exempt from semver.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// The largest magnitude among the finite samples, 0 without any.
    pub peak: f64,
    /// The first frame at which a channel holds a NaN or infinite sample, as an index into the
    /// channels, not into the measured range.
    pub non_finite: Option<usize>,
}

/// Measures `frames` of each channel.
#[doc(hidden)]
pub fn level<S: Sample, C: AsRef<[S]>>(channels: &[C], frames: Range<usize>) -> Level {
    let mut level = Level {
        peak: 0.0,
        non_finite: None,
    };
    for channel in channels {
        for (frame, sample) in frames.clone().zip(&channel.as_ref()[frames.clone()]) {
            let sample = sample.to_f64();
            if sample.is_finite() {
                level.peak = level.peak.max(sample.abs());
            } else if level.non_finite.is_none_or(|first| frame < first) {
                level.non_finite = Some(frame);
            }
        }
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_finds_the_earliest_non_finite_frame_of_any_channel_and_the_finite_peak() {
        let channels = [
            [0.25f32, -0.5, 0.0, f32::NAN],
            [0.0, 0.0, f32::INFINITY, -0.75],
        ];
        assert_eq!(
            level(&channels, 0..4),
            Level {
                peak: 0.75,
                non_finite: Some(2),
            }
        );
        // Only the frames asked for count, and frames keep their index in the channels.
        assert_eq!(
            level(&channels, 0..2),
            Level {
                peak: 0.5,
                non_finite: None,
            }
        );
        assert_eq!(
            level(&channels, 3..4),
            Level {
                peak: 0.75,
                non_finite: Some(3),
            }
        );
    }
}
