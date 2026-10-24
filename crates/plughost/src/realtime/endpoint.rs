//! The endpoint's callback. Each call submits the callback's quantum and plays the output of the
//! quantum submitted `latency` callbacks before, or the fallback when it is missing. The call
//! allocates nothing, takes no lock and waits for nothing: it copies into and out of the ring,
//! rings the doorbell, updates atomic counters, and keeps track of the notes it delivered.

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use plughost_core::internal::level;
use plughost_core::ipc::shared::clock;
use plughost_core::ipc::shared::realtime::{Outcome, Ring};
use plughost_core::{
    AutomationEvent, BlockContext, Events, FailureKind, InputError, RoutedChainConfig, Sample,
    SampleFormat,
};

use super::dry::DryDelay;
use super::notes::Notes;
use super::{Fallback, RealtimeConfig, State};
use crate::Error;

/// What a callback played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RealtimeOutput {
    /// The chain's output for the quantum submitted `latency` callbacks before.
    Processed,
    /// The fallback.
    Fallback(FallbackReason),
}

/// Why a callback played the fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FallbackReason {
    /// The endpoint is not running.
    Stopped,
    /// The endpoint started less than `latency` callbacks ago, so no output is due yet. The first
    /// output plays as is, without a crossfade from this fallback.
    Priming,
    /// The helper had not completed the output in time.
    Missed,
    /// The endpoint waits for [`crate::Chain::resync_realtime`], to be stopped, the chain reset
    /// and the endpoint started again, or to be recovered; or a resync or recovery is under way.
    /// The first output of the new generation plays as is, as after a start.
    Resync,
}

/// The audio callback's side of a chain's realtime endpoint, from [`crate::Chain::realtime_endpoint`].
/// Move it to the callback's thread and call `process_audio_f32` or `process_audio_f64` there,
/// in the chain's prepared precision, once per callback. Drop it on a control thread: dropping
/// releases shared memory. [`crate::Chain::recover_realtime`] moves the endpoint to the chain it
/// recovers in a new helper, while the callback keeps this value.
///
/// Call it once per quantum's worth of time, as an audio device calls back: the helper works on
/// a quantum until the call `latency` quanta later, so quanta processed in a burst find the
/// previous ones unfinished and play the fallback ([`FallbackReason::Missed`]). Ask the device
/// for callbacks of one quantum; an application that cuts each device callback into several
/// quanta needs a latency of as many quanta as a callback holds.
pub struct RealtimeEndpoint {
    /// Boxed so that recovery can hand the callback another chain's ring without allocating.
    ring: Box<Ring>,
    state: Arc<State>,
    quantum: usize,
    latency: u64,
    fallback: Fallback,
    format: SampleFormat,
    sample_rate: f64,
    input_channels: usize,
    output_channels: usize,
    event_inputs: usize,
    event_outputs: usize,
    slots: usize,
    generation: u64,
    sequence: u64,
    /// The previous callback played the chain's output.
    wet: bool,
    /// The previous callback played the fallback for a missed output, which the next output
    /// crossfades from. The first output after a start plays as is.
    crossfade: bool,
    /// Each output channel's last played sample, which a fallback ramps from.
    last: Vec<f64>,
    /// Ticks from submission to completion of the output the last callback played.
    completion: Option<u64>,
    dry: Option<Box<DryDelay>>,
    /// The chain latency of the generation's first output. A replacement that changes it waits
    /// for a new generation rather than go on in this one's timeline.
    planned: Option<u32>,
    notes: Notes,
    /// The notes earlier outputs started wait for the new generation's first callback that
    /// returns `Ok` to end them.
    ending: bool,
}

impl std::fmt::Debug for RealtimeEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeEndpoint")
            .field("quantum", &self.quantum)
            .field("latency", &self.latency)
            .field("fallback", &self.fallback)
            .field("generation", &self.generation)
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

/// The samples the ring carries.
trait Wire: Sample {
    fn from_f64(value: f64) -> Self;
    fn submit(
        ring: &mut Ring,
        generation: u64,
        sequence: u64,
        context: &BlockContext,
        input: &[&[Self]],
        automation: &[AutomationEvent],
        events: &Events,
    ) -> io::Result<()>;
    fn read(
        ring: &mut Ring,
        generation: u64,
        sequence: u64,
        output: &mut [&mut [Self]],
        event_ports: usize,
        produced: &mut Events,
    ) -> io::Result<Option<(Outcome, usize)>>;
}

macro_rules! wire {
    ($sample:ty, $submit:ident, $read:ident) => {
        impl Wire for $sample {
            fn from_f64(value: f64) -> Self {
                value as $sample
            }
            fn submit(
                ring: &mut Ring,
                generation: u64,
                sequence: u64,
                context: &BlockContext,
                input: &[&[Self]],
                automation: &[AutomationEvent],
                events: &Events,
            ) -> io::Result<()> {
                ring.$submit(generation, sequence, context, input, automation, events)
            }
            fn read(
                ring: &mut Ring,
                generation: u64,
                sequence: u64,
                output: &mut [&mut [Self]],
                event_ports: usize,
                produced: &mut Events,
            ) -> io::Result<Option<(Outcome, usize)>> {
                ring.$read(generation, sequence, output, event_ports, produced)
            }
        }
    };
}
wire!(f32, submit_f32, read_f32);
wire!(f64, submit_f64, read_f64);

impl RealtimeEndpoint {
    pub(crate) fn new(
        ring: Ring,
        state: Arc<State>,
        config: &RealtimeConfig,
        chain: &RoutedChainConfig,
        dry: Option<Box<DryDelay>>,
    ) -> RealtimeEndpoint {
        let output_channels = chain.output_channels();
        RealtimeEndpoint {
            ring: Box::new(ring),
            state,
            quantum: config.quantum,
            latency: config.latency as u64,
            fallback: config.fallback,
            format: chain.sample_format,
            sample_rate: chain.sample_rate,
            input_channels: chain.input_channels(),
            output_channels,
            event_inputs: chain.event_inputs,
            event_outputs: chain.event_outputs(),
            slots: chain.slots.len(),
            generation: 0,
            sequence: 0,
            wet: false,
            crossfade: false,
            last: vec![0.0; output_channels],
            completion: None,
            dry,
            planned: None,
            notes: Notes::new(),
            ending: false,
        }
    }

    /// Validates the callback's events and automation as a chain block, without allocating.
    fn validate(
        &self,
        context: &BlockContext,
        automation: &[AutomationEvent],
        events: &Events,
    ) -> Result<(), InputError> {
        context.validate()?;
        if let Some(transport) = context.transport {
            transport.validate_at_rate(self.sample_rate)?;
        }
        plughost_core::internal::validate_event_budget(automation.len(), events)?;
        if let Some(event) = events.iter().find(|event| event.port >= self.event_inputs) {
            return Err(InputError::EventPort { port: event.port });
        }
        if !plughost_core::internal::events_fit(events, context.frames) {
            return Err(InputError::Events);
        }
        if automation.iter().any(|event| {
            event.slot >= self.slots
                || !plughost_core::internal::changes_fit(&[event.change], context.frames)
        }) || automation
            .windows(2)
            .any(|pair| pair[0].change.offset > pair[1].change.offset)
        {
            return Err(InputError::Automation);
        }
        Ok(())
    }

    fn process<S: Wire>(
        &mut self,
        context: &BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[AutomationEvent],
        events: &Events,
        produced: &mut Events,
    ) -> Result<RealtimeOutput, Error> {
        let _audit = plughost_core::audit::processing();
        let state = Arc::clone(&self.state);
        let _call = state.enter();
        let played = self.serve(context, input, output, automation, events, produced);
        // The new generation's plugins will not end the notes earlier outputs started. A
        // generation's first `latency` callbacks play no output, and a callback that fails sets
        // `resync` until the next generation, so no output of this one has added notes yet.
        if played.is_ok() && std::mem::take(&mut self.ending) {
            let dropped = self.notes.end_all(produced);
            self.state
                .dropped_events
                .fetch_add(dropped, Ordering::Relaxed);
        }
        played
    }

    fn serve<S: Wire>(
        &mut self,
        context: &BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[AutomationEvent],
        events: &Events,
        produced: &mut Events,
    ) -> Result<RealtimeOutput, Error> {
        produced.clear();
        if S::FORMAT != self.format {
            return Err(Error::Input {
                slot: None,
                error: InputError::AudioSampleFormat,
            });
        }
        let frames = context.frames;
        if output.len() != self.output_channels
            || output.iter().any(|channel| channel.len() != frames)
        {
            return Err(Error::Input {
                slot: None,
                error: InputError::AudioBuffers,
            });
        }
        State::count(&self.state.callbacks);
        // A new generation's dry delay line holds this callback's input already.
        let generation = self.state.generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.begin(generation);
        }
        let input_fits = input.len() == self.input_channels
            && input.iter().all(|channel| channel.len() == frames);
        if let Some(dry) = &mut self.dry {
            dry.push(input_fits.then_some(input), frames);
        }
        if self.state.closed.load(Ordering::Acquire) {
            return Ok(self.fallback(output, FallbackReason::Stopped));
        }
        if !self.state.running.load(Ordering::Acquire) {
            return Ok(self.fallback(output, FallbackReason::Stopped));
        }
        if self.state.resync.load(Ordering::Relaxed) || self.state.halted(self.generation) {
            return Ok(self.fallback(output, FallbackReason::Resync));
        }
        if frames != self.quantum {
            State::count(&self.state.size_mismatches);
            self.state.resync.store(true, Ordering::Relaxed);
            return Ok(self.fallback(output, FallbackReason::Resync));
        }
        let invalid = if input_fits {
            self.validate(context, automation, events)
                .map_err(|error| Error::Input { slot: None, error })
        } else {
            Err(Error::Input {
                slot: None,
                error: InputError::AudioBuffers,
            })
        };
        if let Err(error) = invalid {
            // A quantum that is not submitted breaks the pipeline like an overflow.
            self.state.resync.store(true, Ordering::Relaxed);
            self.fallback(output, FallbackReason::Resync);
            return Err(error);
        }
        let sequence = self.sequence + 1;
        if !self.ring.lane_free(sequence) {
            State::count(&self.state.overflows);
            self.state.resync.store(true, Ordering::Relaxed);
            return Ok(self.fallback(output, FallbackReason::Resync));
        }
        let submitted = S::submit(
            &mut self.ring,
            generation,
            sequence,
            context,
            input,
            automation,
            events,
        );
        if submitted.is_err() {
            self.state.fail(None, FailureKind::Protocol);
            return Ok(self.fallback(output, FallbackReason::Resync));
        }
        self.sequence = sequence;
        if sequence <= self.latency {
            return Ok(self.fallback(output, FallbackReason::Priming));
        }
        let read = S::read(
            &mut self.ring,
            generation,
            sequence - self.latency,
            output,
            self.event_outputs,
            produced,
        );
        match read {
            Ok(Some((
                Outcome::Done {
                    latency, elapsed, ..
                },
                dropped,
            ))) => {
                if *self.planned.get_or_insert(latency) != latency {
                    produced.clear();
                    self.state.resync.store(true, Ordering::Relaxed);
                    return Ok(self.fallback(output, FallbackReason::Resync));
                }
                State::count(&self.state.processed);
                self.notes.record(produced);
                self.completion = Some(elapsed);
                self.state
                    .dropped_events
                    .fetch_add(dropped as u64, Ordering::Relaxed);
                self.state.latency.store(latency, Ordering::Relaxed);
                self.play(output);
                if level(output, 0..frames).non_finite.is_some() {
                    State::count(&self.state.non_finite_outputs);
                }
                Ok(RealtimeOutput::Processed)
            }
            Ok(Some((Outcome::Failed { slot, kind }, _))) => {
                produced.clear();
                // A slot the chain does not have is the helper's fault, not the slot's.
                match slot {
                    Some(slot) if slot >= self.slots => {
                        self.state.fail(None, FailureKind::Protocol);
                    }
                    _ => self.state.fail(slot, kind),
                }
                Ok(self.fallback(output, FallbackReason::Resync))
            }
            Ok(None) => {
                produced.clear();
                State::count(&self.state.missed);
                Ok(self.fallback(output, FallbackReason::Missed))
            }
            Err(_) => {
                produced.clear();
                self.state.fail(None, FailureKind::Protocol);
                Ok(self.fallback(output, FallbackReason::Resync))
            }
        }
    }

    /// How long the helper took to complete the output the last callback played, from its
    /// submission; `None` when the callback played the fallback. Call it outside the callback, or
    /// keep the value for later: the conversion reads the system's timebase.
    pub fn last_completion(&self) -> Option<Duration> {
        self.completion.map(clock::duration)
    }

    /// Follows a new generation, whose quanta start again from 1, in the ring of the chain that
    /// recovered the endpoint when recovery started it.
    fn begin(&mut self, generation: u64) {
        self.generation = generation;
        self.sequence = 0;
        self.planned = None;
        self.ending = true;
        self.state.ring.receive(&mut self.ring);
        if let Some(dry) = &mut self.dry {
            self.state.dry.receive(dry);
            dry.set_delay(self.state.dry_delay.load(Ordering::Relaxed) as usize);
        }
    }

    /// The fallback's sample at `frame` of a callback `frames` long.
    fn fallback_sample(&self, channel: usize, frame: usize, frames: usize) -> f64 {
        match (&self.dry, self.fallback) {
            (Some(dry), Fallback::Dry) => dry.delayed(channel, frame, frames),
            _ => 0.0,
        }
    }

    /// Plays the fallback, ramping over the callback from the last sample played, or 0 when it
    /// was not finite, when the previous callback played the chain's output.
    fn fallback<S: Wire>(
        &mut self,
        output: &mut [&mut [S]],
        reason: FallbackReason,
    ) -> RealtimeOutput {
        for (index, channel) in output.iter_mut().enumerate() {
            let frames = channel.len();
            let start = self.fallback_sample(index, 0, frames);
            let held = if self.last[index].is_finite() {
                self.last[index]
            } else {
                0.0
            };
            for (frame, sample) in channel.iter_mut().enumerate() {
                let mut value = self.fallback_sample(index, frame, frames);
                if self.wet {
                    let remaining = 1.0 - (frame + 1) as f64 / frames as f64;
                    value += (held - start) * remaining;
                }
                *sample = S::from_f64(value);
            }
            if let Some(&last) = channel.last() {
                self.last[index] = last.to_f64();
            }
        }
        self.wet = false;
        self.crossfade = reason == FallbackReason::Missed;
        self.completion = None;
        RealtimeOutput::Fallback(reason)
    }

    /// Plays the chain's output, which the ring wrote into `output`, crossfading from the
    /// fallback when the previous callback played it for a missed output.
    fn play<S: Wire>(&mut self, output: &mut [&mut [S]]) {
        for (index, channel) in output.iter_mut().enumerate() {
            let frames = channel.len();
            if self.crossfade {
                for frame in 0..frames {
                    let gain = (frame + 1) as f64 / frames as f64;
                    let fallback = self.fallback_sample(index, frame, frames);
                    channel[frame] =
                        S::from_f64(fallback * (1.0 - gain) + channel[frame].to_f64() * gain);
                }
            }
            if let Some(&last) = channel.last() {
                self.last[index] = last.to_f64();
            }
        }
        self.wet = true;
        self.crossfade = false;
    }
}

macro_rules! endpoint_process {
    ($sample:ty, $method:ident) => {
        impl RealtimeEndpoint {
            /// Serves one audio callback in the chain's prepared precision: submits `input`,
            /// `automation` and `events`, which address the chain as in
            /// [`crate::Chain::process_audio_f32`], and plays into `output` the chain's output
            /// for the quantum submitted `latency` callbacks before, or the fallback. Each slice
            /// is the callback's frames long, which must be the quantum. `produced` is replaced
            /// with that output's events, system exclusive ones included, up to the room it has
            /// reserved: pass [`crate::Events::for_block`] to receive them all without
            /// allocating. The first callback that returns `Ok` after a start, a resync or a
            /// recovery puts there instead a note off at offset 0 for each
            /// note earlier outputs started and did not end, in the form it started in: a MIDI
            /// note off, or a note off with the note's ID.
            ///
            /// Invalid input is an error and starts a resync, like a callback of another size;
            /// the fallback then plays. Wrong output buffers or precision are an error and leave
            /// `output` unchanged.
            pub fn $method(
                &mut self,
                context: &BlockContext,
                input: &[&[$sample]],
                output: &mut [&mut [$sample]],
                automation: &[AutomationEvent],
                events: &Events,
                produced: &mut Events,
            ) -> Result<RealtimeOutput, Error> {
                self.process(context, input, output, automation, events, produced)
            }
        }
    };
}
endpoint_process!(f32, process_audio_f32);
endpoint_process!(f64, process_audio_f64);
