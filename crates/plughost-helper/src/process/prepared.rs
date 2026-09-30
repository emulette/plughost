//! Helper-owned routing plans and reusable block storage, built on the control thread.
use std::ops::{Add, Mul, Range};

use plughost_core::{
    AudioSource, AutomationEvent, ChannelAdaptation, MAX_AUDIO_CHANNELS, MAX_BLOCK_EVENTS,
    ParameterChange, PluginTiming, RoutedChainConfig, Sample, SampleFormat,
};

use super::audio_timing::DelayBank;

pub(crate) enum Prepared {
    F32(Routed<f32>),
    F64(Routed<f64>),
}

pub(crate) struct Routed<S: Sample> {
    pub slots: Vec<Slot<S>>,
    pub timings: Vec<PluginTiming>,
}

pub(crate) struct Slot<S: Sample> {
    pub input: Vec<Vec<S>>,
    pub output: Vec<Vec<S>>,
    pub changes: Vec<ParameterChange>,
    routes: Vec<Route>,
}
struct Route {
    index: usize,
    source: Source,
    destination: Range<usize>,
    adaptation: ChannelAdaptation,
}
enum Source {
    External(Range<usize>),
    Previous(Range<usize>),
    Silence,
}

fn channels<S: Sample>(count: usize, block: usize) -> Vec<Vec<S>> {
    (0..count).map(|_| Vec::with_capacity(block)).collect()
}

impl Prepared {
    /// Config has already been validated and ordered by negotiated native bus indices.
    pub fn new(config: &RoutedChainConfig, timings: &[PluginTiming]) -> Self {
        match config.sample_format {
            SampleFormat::F32 => Self::F32(Routed::new(config, timings)),
            SampleFormat::F64 => Self::F64(Routed::new(config, timings)),
        }
    }
}
impl<S: Sample> Routed<S> {
    fn new(config: &RoutedChainConfig, timings: &[PluginTiming]) -> Self {
        let slots = config
            .slots
            .iter()
            .enumerate()
            .map(|(slot, setup)| {
                let mut routes = Vec::new();
                let mut input_count = 0;
                for (index, route) in setup
                    .inputs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.bus.active)
                {
                    let source = match route.source {
                        AudioSource::External { bus } => {
                            let start: usize =
                                config.inputs[..bus].iter().map(|l| l.channels()).sum();
                            Source::External(start..start + config.inputs[bus].channels())
                        }
                        AudioSource::Previous { bus } => {
                            let outputs = &config.slots[slot - 1].outputs;
                            let start: usize = outputs
                                .iter()
                                .filter(|b| b.active)
                                .take_while(|b| b.id != bus)
                                .map(|b| b.layout.channels())
                                .sum();
                            let channels = outputs
                                .iter()
                                .find(|b| b.active && b.id == bus)
                                .unwrap()
                                .layout
                                .channels();
                            Source::Previous(start..start + channels)
                        }
                        AudioSource::Silence => Source::Silence,
                    };
                    let end = input_count + route.bus.layout.channels();
                    routes.push(Route {
                        index,
                        source,
                        destination: input_count..end,
                        adaptation: route.adaptation,
                    });
                    input_count = end;
                }
                let output_count = setup
                    .outputs
                    .iter()
                    .filter(|b| b.active)
                    .map(|b| b.layout.channels())
                    .sum();
                Slot {
                    input: channels(input_count, config.max_block_size),
                    output: channels(output_count, config.max_block_size),
                    changes: Vec::with_capacity(MAX_BLOCK_EVENTS),
                    routes,
                }
            })
            .collect();
        Self {
            slots,
            timings: timings.to_vec(),
        }
    }
}
impl<S: Sample> Slot<S> {
    pub fn begin(&mut self, slot: usize, frames: usize, automation: &[AutomationEvent]) {
        for channel in &mut self.output {
            channel.resize(frames, S::default());
            channel.fill(S::default());
        }
        self.changes.clear();
        // The request boundary checks the aggregate event budget before advancing any slot.
        self.changes.extend(
            automation
                .iter()
                .filter(|e| e.slot == slot)
                .map(|e| e.change),
        );
    }
}
impl Route {
    /// Whether the plugin can read this route's source directly: its channels arrive unchanged
    /// and undelayed. Other routes go through the slot's input scratch.
    fn direct<S: Sample>(&self, delays: &DelayBank<S>, slot: usize) -> bool {
        self.adaptation == ChannelAdaptation::Exact
            && !matches!(self.source, Source::Silence)
            && !delays.delays(slot, self.index)
    }
}

impl<S: Sample + Add<Output = S> + Mul<Output = S> + From<f32>> Slot<S> {
    /// Routes `external` input and the `previous` slot's output to this slot's inputs and calls
    /// `call` with them, the outputs `begin` cleared, and the slot's automation. Direct routes are
    /// passed by reference; the rest are adapted or delayed into scratch first.
    pub fn process<T>(
        &mut self,
        external: &[Vec<S>],
        previous: &[Vec<S>],
        frames: usize,
        delays: &mut DelayBank<S>,
        slot: usize,
        call: impl FnOnce(&[&[S]], &mut [&mut [S]], &[ParameterChange]) -> T,
    ) -> T {
        for route in &self.routes {
            if route.direct(delays, slot) {
                continue;
            }
            let target = &mut self.input[route.destination.clone()];
            for channel in target.iter_mut() {
                channel.resize(frames, S::default());
            }
            let source = match &route.source {
                Source::External(range) => Some(&external[range.clone()]),
                Source::Previous(range) => Some(&previous[range.clone()]),
                Source::Silence => None,
            };
            match (source, route.adaptation) {
                (Some(source), ChannelAdaptation::Exact) => {
                    for (out, input) in target.iter_mut().zip(source) {
                        out.copy_from_slice(input);
                    }
                }
                (Some(source), ChannelAdaptation::MonoToStereo) => {
                    for out in target.iter_mut() {
                        out.copy_from_slice(&source[0]);
                    }
                }
                (Some(source), ChannelAdaptation::StereoToMono) => {
                    for ((out, left), right) in target[0].iter_mut().zip(&source[0]).zip(&source[1])
                    {
                        *out = *left * S::from(0.5) + *right * S::from(0.5);
                    }
                }
                (None, _) => {
                    for out in target.iter_mut() {
                        out.fill(S::default());
                    }
                }
            }
            delays.apply(slot, route.index, target);
        }
        let mut inputs: [&[S]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
        for route in &self.routes {
            let direct = route.direct(delays, slot);
            for (channel, destination) in route.destination.clone().enumerate() {
                inputs[destination] = match &route.source {
                    Source::External(range) if direct => &external[range.start + channel],
                    Source::Previous(range) if direct => &previous[range.start + channel],
                    _ => &self.input[destination],
                };
            }
        }
        let input_count = self.input.len();
        let changes = &self.changes;
        with_outputs(&mut self.output, |outputs| {
            call(&inputs[..input_count], outputs, changes)
        })
    }
}

/// Borrow channel views on the stack. No cached Rust references outlive a synchronous native call.
pub(super) fn with_outputs<S, T>(
    output: &mut [Vec<S>],
    call: impl FnOnce(&mut [&mut [S]]) -> T,
) -> T {
    let mut outputs: [&mut [S]; MAX_AUDIO_CHANNELS] = std::array::from_fn(|_| &mut [][..]);
    let count = output.len();
    for (view, samples) in outputs.iter_mut().zip(output) {
        *view = samples;
    }
    call(&mut outputs[..count])
}

/// Borrow read-only channel views on the stack.
pub(super) fn with_inputs<S, T>(input: &[Vec<S>], call: impl FnOnce(&[&[S]]) -> T) -> T {
    let mut inputs: [&[S]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
    for (view, samples) in inputs.iter_mut().zip(input) {
        *view = samples;
    }
    call(&inputs[..input.len()])
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
