//! Offline render rules shared by every processing path: the output is aligned with the input by
//! removing the reported latency, and the tail after the input is rendered as well.

mod session;
mod stream;
mod timeline;
pub use session::{RenderSession, SessionState};
pub use timeline::{AutomationRamp, RenderSchedule, TransportChange};

use serde::{Deserialize, Serialize};

use crate::errors::RenderError;
use crate::event::{MidiEvent, events_fit};
use crate::sample::Sample;

/// The tail length a processor reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tail {
    Samples(u32),
    Infinite,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TailPolicy {
    /// Render the tail length the processor reports. An infinite tail renders up to the maximum.
    Reported,
    /// Render until the output has stayed at or below `threshold` (linear peak) for
    /// `hold_seconds`, then cut after the last sample above it. Both values must be finite and
    /// non-negative; zero is allowed, and a linear peak threshold may exceed one. Plugins often report a wrong
    /// tail, so this is the default.
    UntilSilence { threshold: f64, hold_seconds: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderOptions {
    pub tail: TailPolicy,
    /// Upper bound on the rendered tail, whatever the policy. Must be finite and non-negative.
    pub max_tail_seconds: f64,
}

impl Default for RenderOptions {
    fn default() -> RenderOptions {
        RenderOptions {
            tail: TailPolicy::UntilSilence {
                // -96 dBFS
                threshold: 1.585e-5,
                hold_seconds: 1.0,
            },
            max_tail_seconds: 30.0,
        }
    }
}

impl RenderOptions {
    fn validate(&self) -> Result<(), RenderError> {
        if !self.max_tail_seconds.is_finite() || self.max_tail_seconds < 0.0 {
            return Err(RenderError::TailDuration);
        }
        if let TailPolicy::UntilSilence {
            threshold,
            hold_seconds,
        } = self.tail
        {
            if !threshold.is_finite() || threshold < 0.0 {
                return Err(RenderError::SilenceThreshold);
            }
            if !hold_seconds.is_finite() || hold_seconds < 0.0 {
                return Err(RenderError::SilenceHold);
            }
        }
        Ok(())
    }
}

/// A prepared processor the render rules can drive.
pub trait Process<S: Sample> {
    type Error: From<RenderError>;

    fn sample_rate(&self) -> f64;
    fn max_block_size(&self) -> usize;
    fn input_channels(&self) -> usize;
    fn output_channels(&self) -> usize;
    fn latency(&self) -> Result<u32, Self::Error>;
    fn tail(&self) -> Result<Tail, Self::Error>;
    /// Validate all scheduled targets before the render advances DSP state. Implementors with
    /// automation must check slot/parameter identity, write access and automatable metadata.
    fn validate_automation(
        &mut self,
        automation: &[crate::AutomationEvent],
    ) -> Result<(), Self::Error> {
        if automation.is_empty() {
            Ok(())
        } else {
            Err(RenderError::Input(crate::InputError::Automation).into())
        }
    }
    /// Processes one block. Every input and output slice has the same length, at most
    /// `max_block_size`; automation and MIDI are in offset order, inside the block. Points hold
    /// their values from their offsets, including equal-offset input order. Native adapters own
    /// any subdivision needed to avoid implicit interpolation between point edits. `produced` is
    /// replaced with the block's output events in offset order, with block offsets.
    fn process(
        &mut self,
        context: &crate::BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[crate::AutomationEvent],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Self::Error>;
}

/// What a stream delivers at once: aligned output audio, and the output events processed since
/// the previous delivery. Event offsets count from the start of the input, in the processing
/// timeline; they are not shifted by the latency removed from the audio.
#[derive(Clone, Copy, Debug)]
pub struct Delivery<'a, S> {
    /// One slice per output channel, all of one length, which may be zero.
    pub audio: &'a [&'a [S]],
    pub events: &'a [MidiEvent],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rendered<S> {
    /// Output channels aligned with the input: `frames + tail` samples each.
    pub channels: Vec<Vec<S>>,
    /// Output events in offset order, counted from the start of the input without latency
    /// alignment, including events processed during the tail.
    pub events: Vec<MidiEvent>,
    /// The latency that was removed, in samples.
    pub latency: u32,
    /// Samples rendered after the end of the input.
    pub tail: usize,
}

/// Renders `frames` samples of `input` (one slice per input channel, none for an instrument)
/// through the processor, with `events` placed by offset from the start of the input, in offset
/// order and before `frames`.
///
/// The latency is read before rendering and after every block; a change is an error because the
/// alignment would no longer hold. Under [`TailPolicy::Reported`], a change that alters the
/// planned tail length is an error for the same reason; [`TailPolicy::UntilSilence`] ignores the
/// reported tail.
/// Options, processor timing, sample conversion and buffer lengths are validated before calling
/// `process`. Durations round to the nearest sample. Invalid or unrepresentable values are errors,
/// not clamped limits. Representable lengths do not guarantee enough memory is available.
pub fn render<S: Sample, P: Process<S>>(
    processor: &mut P,
    input: &[&[S]],
    frames: usize,
    events: &[MidiEvent],
    options: &RenderOptions,
) -> Result<Rendered<S>, P::Error> {
    render_with_schedule(
        processor,
        input,
        frames,
        events,
        options,
        &RenderSchedule::default(),
    )
}

/// Renders a caller-defined timeline in bounded blocks, splitting at transport changes and the
/// combined event budget. Ramps expand into exact sample points within each block. Native
/// adapters preserve point semantics, subdividing internally when required by their format.
/// Seeks change the transport only; call reset explicitly to discard DSP history.
pub fn render_with_schedule<S: Sample, P: Process<S>>(
    processor: &mut P,
    input: &[&[S]],
    frames: usize,
    events: &[MidiEvent],
    options: &RenderOptions,
    schedule: &RenderSchedule<'_>,
) -> Result<Rendered<S>, P::Error> {
    let mut channels = vec![Vec::new(); processor.output_channels()];
    let mut produced = Vec::new();
    let result = render_stream(
        processor,
        RenderInput {
            audio: input,
            frames,
            events,
            schedule: *schedule,
        },
        options,
        |delivery| {
            for (channel, samples) in channels.iter_mut().zip(delivery.audio) {
                channel.extend_from_slice(samples);
            }
            produced.extend_from_slice(delivery.events);
        },
        || false,
    )?;
    Ok(Rendered {
        channels,
        events: produced,
        latency: result.latency,
        tail: result.tail,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct RenderInput<'a, S> {
    pub audio: &'a [&'a [S]],
    pub frames: usize,
    pub events: &'a [MidiEvent],
    pub schedule: RenderSchedule<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    Complete,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderProgress {
    pub status: RenderStatus,
    /// Native frames processed, including latency and tail. Cancellation never interrupts a call.
    pub processed_frames: usize,
    pub output_frames: usize,
    pub latency: u32,
    pub tail: usize,
}

/// Delivers aligned output without retaining the whole result. Memory is bounded by the prepared
/// block size plus the configured silence hold. Cancellation is checked before each submitted block;
/// delivered samples remain valid. DSP state has advanced by `processed_frames`. Reset before a
/// new independent render. This function does not reset the caller's processor implicitly.
///
/// The prepared `max_block_size` is also the submission limit. Larger offline blocks can reduce
/// helper round trips, but increase scratch storage and the amount of processing before the next
/// cancellation check. Transport changes and the event budget may shorten a block. No subsequent
/// block is submitted ahead of that check. Silence detection trims delivered output sample by
/// sample; DSP may already have advanced through the rest of the submitted block.
pub fn render_stream<S: Sample, P: Process<S>>(
    processor: &mut P,
    request: RenderInput<'_, S>,
    options: &RenderOptions,
    mut consume: impl FnMut(Delivery<'_, S>),
    mut cancelled: impl FnMut() -> bool,
) -> Result<RenderProgress, P::Error> {
    let RenderInput {
        audio: input,
        frames,
        events,
        schedule,
    } = request;
    let expected = processor.input_channels();
    if input.len() != expected {
        return Err(RenderError::InputChannels {
            expected,
            actual: input.len(),
        }
        .into());
    }
    if input.iter().any(|channel| channel.len() != frames) {
        return Err(RenderError::InputLength.into());
    }
    if !events_fit(events, frames) {
        return Err(RenderError::Events.into());
    }

    let latency = processor.latency()?;
    let sample_rate = processor.sample_rate();
    let block = processor.max_block_size();
    crate::config::validate_processing(sample_rate, block).map_err(RenderError::Input)?;
    options.validate()?;
    let max_tail = seconds_to_samples(options.max_tail_seconds, sample_rate)?;
    let body = frames
        .checked_add(latency as usize)
        .ok_or(RenderError::LengthOverflow)?;
    let reported = matches!(options.tail, TailPolicy::Reported);
    let reported_tail = |tail: Tail| match tail {
        Tail::Samples(samples) => (samples as usize).min(max_tail),
        Tail::Infinite => max_tail,
    };
    let tail = if reported {
        reported_tail(processor.tail()?)
    } else {
        max_tail
    };
    let end = body.checked_add(tail).ok_or(RenderError::LengthOverflow)?;
    std::alloc::Layout::array::<S>(end).map_err(|_| RenderError::LengthOverflow)?;
    schedule
        .validate(frames, end, sample_rate)
        .map_err(RenderError::Input)?;
    schedule
        .validate_capacity(events)
        .map_err(RenderError::Input)?;
    let targets: Vec<_> = schedule
        .automation
        .iter()
        .copied()
        .chain(schedule.ramps.iter().map(|ramp| crate::AutomationEvent {
            slot: ramp.slot,
            change: crate::ParameterChange {
                id: ramp.id,
                offset: ramp.start,
                value: ramp.from,
            },
        }))
        .collect();
    processor.validate_automation(&targets)?;
    let silence = match options.tail {
        TailPolicy::UntilSilence {
            threshold,
            hold_seconds,
        } => Some((threshold, seconds_to_samples(hold_seconds, sample_rate)?)),
        TailPolicy::Reported => None,
    };

    let mut input_block = vec![vec![S::default(); block]; input.len()];
    let mut output_block = vec![vec![S::default(); block]; processor.output_channels()];
    let mut output = stream::Output::new(output_block.len(), silence);
    let mut status = RenderStatus::Complete;
    let mut position = 0;
    let mut pending = events;
    let mut block_events = Vec::with_capacity(crate::MAX_BLOCK_EVENTS);
    let mut automation = Vec::with_capacity(crate::MAX_BLOCK_EVENTS);
    let mut produced = Vec::with_capacity(crate::MAX_BLOCK_EVENTS);
    let mut produced_since = Vec::new();

    while position < end {
        if cancelled() {
            status = RenderStatus::Cancelled;
            break;
        }
        let boundary = schedule.next_transport(position).unwrap_or(end).min(end);
        let limit = position + block.min(boundary - position);
        let block_end = schedule.fill_block(position, limit, pending, &mut automation);
        let count = block_end - position;
        let start = position.min(frames);
        let available = count.min(frames - start);
        for (buffer, source) in input_block.iter_mut().zip(input) {
            buffer[..available].copy_from_slice(&source[start..start + available]);
            buffer[available..count].fill(S::default());
        }
        for buffer in &mut output_block {
            buffer[..count].fill(S::default());
        }
        let inputs: Vec<&[S]> = input_block.iter().map(|b| &b[..count]).collect();
        let mut outputs: Vec<&mut [S]> = output_block.iter_mut().map(|b| &mut b[..count]).collect();
        let due = pending
            .iter()
            .take_while(|event| event.offset < position + count)
            .count();
        block_events.clear();
        block_events.extend(pending[..due].iter().map(|event| MidiEvent {
            offset: event.offset - position,
            ..event.clone()
        }));
        pending = &pending[due..];
        let context = crate::BlockContext {
            frames: count,
            transport: schedule
                .transport_at(position, sample_rate)
                .map_err(RenderError::Input)?,
        };
        processor.process(
            &context,
            &inputs,
            &mut outputs,
            &automation,
            &block_events,
            &mut produced,
        )?;
        produced_since.extend(produced.drain(..).map(|event| MidiEvent {
            offset: event.offset + position,
            ..event
        }));
        let now = processor.latency()?;
        if now != latency {
            return Err(RenderError::LatencyChanged {
                before: latency,
                after: now,
            }
            .into());
        }
        // Only a reported tail sets the render length; silence detection ignores it.
        if reported && reported_tail(processor.tail()?) != tail {
            return Err(RenderError::TailChanged.into());
        }
        let done = output.accept(&output_block, count, position, latency as usize, body);
        position += count;
        output.deliver(&mut produced_since, &mut consume);
        if done {
            break;
        }
    }
    output.deliver(&mut produced_since, &mut consume);
    Ok(RenderProgress {
        status,
        processed_frames: position,
        output_frames: output.emitted,
        latency,
        tail: output.emitted.saturating_sub(frames),
    })
}

fn seconds_to_samples(seconds: f64, sample_rate: f64) -> Result<usize, RenderError> {
    let samples = (seconds * sample_rate).round();
    // The exclusive upper bound is exactly representable as f64, unlike usize::MAX on 64-bit.
    let upper_bound = (usize::MAX as u128 + 1) as f64;
    if !samples.is_finite() || samples >= upper_bound {
        return Err(RenderError::LengthOverflow);
    }
    Ok(samples as usize)
}

#[cfg(test)]
mod tests;
