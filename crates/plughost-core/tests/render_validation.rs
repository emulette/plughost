//! Exercise caller-supplied render options through the public rendering boundary.
use plughost_core::render::{Process, RenderOptions, Tail, TailPolicy, render};
use plughost_core::{FailureKind, MidiEvent, RenderError};

#[derive(Default)]
struct Instrument {
    calls: usize,
}

impl Process<f32> for Instrument {
    type Error = RenderError;
    fn sample_rate(&self) -> f64 {
        48_000.0
    }
    fn max_block_size(&self) -> usize {
        16
    }
    fn input_channels(&self) -> usize {
        0
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn latency(&self) -> Result<u32, RenderError> {
        Ok(0)
    }
    fn tail(&self) -> Result<Tail, RenderError> {
        Ok(Tail::Infinite)
    }
    fn process(
        &mut self,
        _context: &plughost_core::BlockContext,
        _: &[&[f32]],
        output: &mut [&mut [f32]],
        _: &[plughost_core::AutomationEvent],
        _: &[MidiEvent],
        _produced: &mut Vec<MidiEvent>,
    ) -> Result<(), RenderError> {
        self.calls += 1;
        output[0].fill(0.25);
        Ok(())
    }
}

fn reported(seconds: f64) -> RenderOptions {
    RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: seconds,
    }
}

fn rejected(processor: &mut Instrument, frames: usize, options: &RenderOptions) {
    let error = render(processor, &[], frames, &[], options).unwrap_err();
    assert_eq!(error.kind(), FailureKind::InvalidInput);
    assert_eq!(
        processor.calls, 0,
        "invalid input must not reach processing"
    );
}

#[test]
fn invalid_options_are_rejected_before_processing() {
    let silence = |threshold, hold_seconds| RenderOptions {
        tail: TailPolicy::UntilSilence {
            threshold,
            hold_seconds,
        },
        max_tail_seconds: 0.0,
    };
    for options in [
        reported(f64::NAN),
        silence(-0.1, 0.0),
        silence(0.0, f64::INFINITY),
    ] {
        rejected(&mut Instrument::default(), 1, &options);
    }
}

#[test]
fn fractional_durations_round_to_samples_without_changing_valid_output() {
    for (seconds, tail) in [(0.49 / 48_000.0, 0), (0.51 / 48_000.0, 1)] {
        let mut processor = Instrument::default();
        let result = render(&mut processor, &[], 3, &[], &reported(seconds)).unwrap();
        assert_eq!(result.channels, [vec![0.25; 3 + tail]]);
        assert_eq!(result.tail, tail);
    }
}

#[test]
fn zero_limits_and_finite_thresholds_above_unity_are_valid() {
    let mut processor = Instrument::default();
    let options = RenderOptions {
        tail: TailPolicy::UntilSilence {
            threshold: 2.0,
            hold_seconds: 0.0,
        },
        max_tail_seconds: 0.0,
    };
    let result = render(&mut processor, &[], 3, &[], &options).unwrap();
    assert_eq!(result.channels, [vec![0.25; 3]]);
    assert_eq!(result.tail, 0);
    let empty = render(&mut processor, &[], 0, &[], &options).unwrap();
    assert!(empty.channels[0].is_empty());
    assert_eq!(processor.calls, 1);
}
