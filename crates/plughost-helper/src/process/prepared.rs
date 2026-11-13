//! Helper-owned routing plans and reusable block storage, built on the control thread.
use std::ops::{Add, Mul, Range};

use plughost_core::{
    AudioBusInfo, AudioBusRole, AudioDirection, AudioSource, AutomationEvent, ChannelAdaptation,
    Layout, MAX_BLOCK_EVENTS, ParameterChange, PluginTiming, RoutedChainConfig, Sample,
    SampleFormat, SlotAudioConfig,
};

use super::audio_timing::DelayBank;
use super::bypass::Fade;
use super::channel_views::ChannelViews;

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
    input_views: ChannelViews<S>,
    output_views: ChannelViews<S>,
    /// What host bypass plays in place of the plugin's main output, for a slot that has a
    /// [`DryPath`].
    dry: Option<Dry<S>>,
}

/// Where a slot's host bypass takes its dry input and puts it: the main input bus, by its full
/// index in `SlotAudioConfig::inputs`, the main output's channels among the slot's active
/// outputs, and the adaptation between their layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DryPath {
    pub input: usize,
    pub output: Range<usize>,
    pub adaptation: ChannelAdaptation,
}

impl DryPath {
    /// The dry path of a slot prepared as `setup`, whose inputs and outputs are in native index
    /// order, with the buses `buses` its plugin reported. The main buses are the first of each
    /// direction with the main role. A slot whose main input or output is missing or inactive,
    /// or whose main layouts are neither the same nor mono and stereo, has none: host bypass
    /// does not substitute one.
    pub fn new(setup: &SlotAudioConfig, buses: &[AudioBusInfo]) -> Option<DryPath> {
        let main = |direction| {
            buses
                .iter()
                .find(|bus| bus.direction == direction && bus.role == AudioBusRole::Main)
                .map(|bus| bus.id)
        };
        let input_id = main(AudioDirection::Input)?;
        let output_id = main(AudioDirection::Output)?;
        let input = setup
            .inputs
            .iter()
            .position(|route| route.bus.active && route.bus.id == input_id)?;
        let outputs = setup.outputs.iter().filter(|bus| bus.active);
        let start: usize = outputs
            .clone()
            .take_while(|bus| bus.id != output_id)
            .map(|bus| bus.layout.channels())
            .sum();
        let output = outputs.clone().find(|bus| bus.id == output_id)?.layout;
        let adaptation = match (setup.inputs[input].bus.layout, output) {
            (source, target) if source == target => ChannelAdaptation::Exact,
            (Layout::Mono, Layout::Stereo) => ChannelAdaptation::MonoToStereo,
            (Layout::Stereo, Layout::Mono) => ChannelAdaptation::StereoToMono,
            _ => return None,
        };
        Some(DryPath {
            input,
            output: start..start + output.channels(),
            adaptation,
        })
    }
}

/// A slot's dry path, by the position of its main input route among the slot's routes, with the
/// scratch the adapted, delayed dry input goes through.
struct Dry<S: Sample> {
    route: usize,
    output: Range<usize>,
    adaptation: ChannelAdaptation,
    scratch: Vec<Vec<S>>,
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
    /// Config has already been validated and ordered by negotiated native bus indices; `dry`
    /// holds each slot's dry path, planned for it.
    pub fn new(
        config: &RoutedChainConfig,
        timings: &[PluginTiming],
        dry: &[Option<DryPath>],
    ) -> Result<Self, plughost_core::Failure> {
        // What processing reads of each route, so that it never meets one it cannot follow.
        let supported = config
            .slots
            .iter()
            .flat_map(|slot| &slot.inputs)
            .all(|route| {
                matches!(
                    route.source,
                    AudioSource::External { .. }
                        | AudioSource::Previous { .. }
                        | AudioSource::Silence
                ) && matches!(
                    route.adaptation,
                    ChannelAdaptation::Exact
                        | ChannelAdaptation::MonoToStereo
                        | ChannelAdaptation::StereoToMono
                )
            });
        if !supported {
            return Err(plughost_core::Failure::new(
                plughost_core::FailureKind::Configuration,
                crate::errors::ROUTING_UNSUPPORTED,
            ));
        }
        Ok(match config.sample_format {
            SampleFormat::F32 => Self::F32(Routed::new(config, timings, dry)),
            SampleFormat::F64 => Self::F64(Routed::new(config, timings, dry)),
            _ => return Err(crate::errors::sample_format()),
        })
    }
}
impl<S: Sample> Routed<S> {
    fn new(config: &RoutedChainConfig, timings: &[PluginTiming], dry: &[Option<DryPath>]) -> Self {
        let slots = config
            .slots
            .iter()
            .zip(dry)
            .enumerate()
            .map(|(slot, (setup, dry))| {
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
                        // Refused by `Prepared::new`.
                        _ => unreachable!(),
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
                let dry = dry.as_ref().map(|path| Dry {
                    route: routes
                        .iter()
                        .position(|route| route.index == path.input)
                        .unwrap(),
                    output: path.output.clone(),
                    adaptation: path.adaptation,
                    scratch: channels(path.output.len(), config.max_block_size),
                });
                Slot {
                    input: channels(input_count, config.max_block_size),
                    output: channels(output_count, config.max_block_size),
                    changes: Vec::with_capacity(MAX_BLOCK_EVENTS),
                    input_views: ChannelViews::new(input_count),
                    output_views: ChannelViews::new(output_count),
                    routes,
                    dry,
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
            match source {
                Some(source) => adapt(source, route.adaptation, target),
                None => {
                    for out in target.iter_mut() {
                        out.fill(S::default());
                    }
                }
            }
            delays.apply(slot, route.index, target);
        }
        let inputs = self.routes.iter().flat_map(|route| {
            let direct = route.direct(delays, slot);
            let channels = match &route.source {
                Source::External(range) if direct => &external[range.clone()],
                Source::Previous(range) if direct => &previous[range.clone()],
                _ => &self.input[route.destination.clone()],
            };
            channels.iter().map(Vec::as_slice)
        });
        let changes = &self.changes;
        self.input_views.with_inputs(inputs, |inputs| {
            self.output_views
                .with_outputs(&mut self.output, |outputs| call(inputs, outputs, changes))
        })
    }

    /// Plays host bypass after the plugin processed: the main output becomes the main input the
    /// plugin received, adapted to the main output's layout and delayed by the slot's dry line,
    /// and the other outputs fall silent. `fade.mix` is how far that has gone, from 0 (the
    /// plugin's output) to 1 (the dry input); it moves toward `fade.enabled` by `fade.step` per
    /// frame, crossfading linearly, and stays where it is when there. The dry line is fed while host bypass is off
    /// too, so that it has the history to play when it turns on. A slot without a dry path or
    /// line, whose host bypass the main thread keeps off, is left as the plugin made it.
    pub fn bypass(
        &mut self,
        external: &[Vec<S>],
        previous: &[Vec<S>],
        frames: usize,
        delays: &mut DelayBank<S>,
        slot: usize,
        fade: Fade<'_>,
    ) {
        let Fade { enabled, mix, step } = fade;
        let Some(dry) = &mut self.dry else {
            return;
        };
        let off = !enabled && *mix == 0.0;
        if off && !delays.dry_history(slot) {
            return;
        }
        let route = &self.routes[dry.route];
        let source = match &route.source {
            Source::External(range) if route.direct(delays, slot) => &external[range.clone()],
            Source::Previous(range) if route.direct(delays, slot) => &previous[range.clone()],
            _ => &self.input[route.destination.clone()],
        };
        for channel in &mut dry.scratch {
            channel.resize(frames, S::default());
        }
        adapt(source, dry.adaptation, &mut dry.scratch);
        if !delays.apply_dry(slot, &mut dry.scratch) || off || frames == 0 {
            return;
        }
        let target = if enabled { 1.0 } else { 0.0 };
        if *mix == target {
            // Settled on the dry input: copied, so that a plugin's output, even one that is
            // not finite, does not reach it.
            for (index, channel) in self.output.iter_mut().enumerate() {
                if dry.output.contains(&index) {
                    channel.copy_from_slice(&dry.scratch[index - dry.output.start]);
                } else {
                    channel.fill(S::default());
                }
            }
            return;
        }
        let start = *mix;
        let direction = if enabled { step } else { -step };
        let gain = |frame: usize| (start + direction * (frame + 1) as f32).clamp(0.0, 1.0);
        for (index, channel) in self.output.iter_mut().enumerate() {
            let source = dry
                .output
                .contains(&index)
                .then(|| &dry.scratch[index - dry.output.start]);
            for (frame, sample) in channel.iter_mut().enumerate() {
                let gain = gain(frame);
                let dry = source.map_or(S::default(), |source| source[frame]);
                // The ends are exact: the plugin's output untouched, or the dry input alone.
                if gain == 1.0 {
                    *sample = dry;
                } else if gain > 0.0 {
                    *sample = *sample * S::from(1.0 - gain) + dry * S::from(gain);
                }
            }
        }
        *mix = gain(frames - 1);
    }

    /// Borrows this slot's completed output for publication, reusing its channel-view storage.
    pub fn with_output<T>(&mut self, call: impl FnOnce(&[&[S]]) -> T) -> T {
        self.output_views
            .with_inputs(self.output.iter().map(Vec::as_slice), call)
    }
}

/// Copies `source` into `target` with `adaptation`; `Prepared::new` refuses other adaptations.
fn adapt<S: Sample + Add<Output = S> + Mul<Output = S> + From<f32>>(
    source: &[Vec<S>],
    adaptation: ChannelAdaptation,
    target: &mut [Vec<S>],
) {
    match adaptation {
        ChannelAdaptation::MonoToStereo => {
            for out in target.iter_mut() {
                out.copy_from_slice(&source[0]);
            }
        }
        ChannelAdaptation::StereoToMono => {
            for ((out, left), right) in target[0].iter_mut().zip(&source[0]).zip(&source[1]) {
                *out = *left * S::from(0.5) + *right * S::from(0.5);
            }
        }
        _ => {
            for (out, input) in target.iter_mut().zip(source) {
                out.copy_from_slice(input);
            }
        }
    }
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
