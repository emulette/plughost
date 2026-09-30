//! Latency and tail follow connected audio paths. External inputs joining a delayed path are
//! delayed to that path's arrival time, so a sidechain and its main input refer to the same sample.

use std::collections::VecDeque;

use plughost_core::ipc::shared::Caller;
use plughost_core::render::Tail;
use plughost_core::{AudioSource, Failure, FailureKind, PluginTiming, RoutedChainConfig, Sample};
use plughost_formats::{BlockProcessor, Error};

use crate::calls::Calls;

/// One slot's native latency, tail and restart request. Every timing query in the helper goes
/// through here; chain totals come only from [`AudioTiming`].
pub(crate) fn plugin_timing(processor: &dyn BlockProcessor) -> Result<PluginTiming, Error> {
    Ok(PluginTiming {
        latency: processor.latency()?,
        tail: processor.tail()?,
        restart_required: processor.restart_required(),
    })
}

/// Every slot's timing, queried as `caller`. A failure names its slot.
pub(crate) fn chain_timings(
    processors: &[Box<dyn BlockProcessor>],
    calls: &Calls,
    caller: Caller,
) -> Result<Vec<PluginTiming>, (usize, Error)> {
    processors
        .iter()
        .enumerate()
        .map(|(slot, processor)| {
            let _call = calls.enter(caller, slot);
            plugin_timing(processor.as_ref()).map_err(|error| (slot, error))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct AudioTiming {
    native_latencies: Vec<u32>,
    previous: Vec<bool>,
    delays: Vec<Vec<u32>>,
    latency: u32,
}

impl AudioTiming {
    /// `config` was validated for `native.len()` slots when the chain was prepared.
    pub fn new(config: &RoutedChainConfig, native: &[PluginTiming]) -> Result<Self, Failure> {
        let mut latency = 0u32;
        let mut previous = Vec::with_capacity(native.len());
        let mut delays = Vec::with_capacity(native.len());
        for (setup, native) in config.slots.iter().zip(native) {
            let connected = setup.inputs.iter().any(|route| {
                route.bus.active && matches!(route.source, AudioSource::Previous { .. })
            });
            let arrival = if connected { latency } else { 0 };
            delays.push(
                setup
                    .inputs
                    .iter()
                    .map(|route| {
                        if route.bus.active && matches!(route.source, AudioSource::External { .. })
                        {
                            arrival
                        } else {
                            0
                        }
                    })
                    .collect(),
            );
            previous.push(connected);
            latency = arrival.checked_add(native.latency).ok_or_else(|| {
                Failure::new(FailureKind::Configuration, crate::errors::LATENCY_OVERFLOW)
            })?;
        }
        Ok(Self {
            native_latencies: native.iter().map(|timing| timing.latency).collect(),
            previous,
            delays,
            latency,
        })
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Plans `config` for the slots' current timing and evaluates the chain tail.
    pub fn plan(
        config: &RoutedChainConfig,
        native: &[PluginTiming],
    ) -> Result<(Self, Tail), Failure> {
        let plan = Self::new(config, native)?;
        let tail = plan.tail(native)?;
        Ok((plan, tail))
    }

    /// Re-evaluates current tails along the planned paths; a tail that overflows is infinite.
    /// Changed native latency requires a new configuration and fresh delay lines; silently keeping
    /// old alignment would corrupt audio.
    pub fn tail(&self, native: &[PluginTiming]) -> Result<Tail, Failure> {
        if native
            .iter()
            .zip(&self.native_latencies)
            .any(|(timing, expected)| timing.latency != *expected || timing.restart_required)
        {
            return Err(Failure::new(
                FailureKind::RestartRequired,
                crate::errors::AUDIO_LATENCY_CHANGED,
            ));
        }
        let mut tail = Tail::Samples(0);
        for (timing, previous) in native.iter().zip(&self.previous) {
            let incoming = if *previous { tail } else { Tail::Samples(0) };
            tail = match (incoming, timing.tail) {
                (Tail::Samples(input), Tail::Samples(plugin)) => input
                    .checked_add(plugin)
                    .map_or(Tail::Infinite, Tail::Samples),
                _ => Tail::Infinite,
            };
        }
        Ok(tail)
    }
}

/// Timing and its fully reserved histories are installed together after successful preparation.
pub(crate) struct Alignment {
    pub timing: AudioTiming,
    pub delays32: Option<DelayBank<f32>>,
    pub delays64: Option<DelayBank<f64>>,
}
impl Alignment {
    pub fn new(config: &RoutedChainConfig, timing: AudioTiming) -> Result<Self, Failure> {
        let (delays32, delays64) = match config.sample_format {
            plughost_core::SampleFormat::F32 => (Some(DelayBank::new(config, &timing)?), None),
            plughost_core::SampleFormat::F64 => (None, Some(DelayBank::new(config, &timing)?)),
        };
        Ok(Self {
            timing,
            delays32,
            delays64,
        })
    }
}

pub(crate) struct DelayBank<S: Sample> {
    routes: Vec<Vec<RouteDelay<S>>>,
}

struct RouteDelay<S: Sample> {
    delay: usize,
    channels: usize,
    queues: Vec<VecDeque<S>>,
}

impl<S: Sample> DelayBank<S> {
    /// Builds fresh downstream histories without modifying this bank. The final bank's total
    /// sample budget is checked before allocation, including unaffected routes.
    pub fn replacement(
        &self,
        replaced: usize,
        timing: &AudioTiming,
    ) -> Result<DelayReplacement<S>, Failure> {
        let start = replaced + 1;
        let end = (start..self.routes.len())
            .find(|&slot| !timing.previous[slot])
            .unwrap_or(self.routes.len());
        check_budget::<S>(self.routes.iter().enumerate().flat_map(|(slot, routes)| {
            routes.iter().enumerate().map(move |(index, route)| {
                let delay = if (start..end).contains(&slot) {
                    timing.delays[slot][index] as usize
                } else {
                    route.delay
                };
                (route.channels, delay)
            })
        }))?;
        let routes = (start..end)
            .map(|slot| {
                self.routes[slot]
                    .iter()
                    .zip(&timing.delays[slot])
                    .map(|(route, &delay)| RouteDelay::new(route.channels, delay as usize))
                    .collect()
            })
            .collect();
        Ok(DelayReplacement { start, routes })
    }

    /// Commits already prepared histories; unaffected queues retain their pending samples.
    pub fn replace_slot(&mut self, replacement: DelayReplacement<S>) {
        for (old, new) in self.routes[replacement.start..]
            .iter_mut()
            .zip(replacement.routes)
        {
            *old = new;
        }
    }

    /// Reserves the complete declared alignment before any sample is processed. Queues begin
    /// empty to represent leading silence without initializing the entire sample history.
    pub fn new(config: &RoutedChainConfig, timing: &AudioTiming) -> Result<Self, Failure> {
        let shape = || {
            config
                .slots
                .iter()
                .zip(&timing.delays)
                .map(|(setup, delays)| {
                    setup.inputs.iter().zip(delays).map(|(input, &delay)| {
                        (
                            if input.bus.active {
                                input.bus.layout.channels()
                            } else {
                                0
                            },
                            delay as usize,
                        )
                    })
                })
        };
        check_budget::<S>(shape().flatten())?;
        let routes = shape()
            .map(|routes| {
                routes
                    .map(|(channels, delay)| RouteDelay::new(channels, delay))
                    .collect()
            })
            .collect();
        Ok(Self { routes })
    }

    /// Whether `route` (its full index in SlotAudioConfig.inputs) delays its audio.
    pub fn delays(&self, slot: usize, route: usize) -> bool {
        self.routes[slot][route].delay != 0
    }

    /// Delays one route's prepared channels in place.
    pub fn apply(&mut self, slot: usize, route: usize, channels: &mut [Vec<S>]) {
        let route = &mut self.routes[slot][route];
        if route.delay == 0 {
            return;
        }
        for (channel, queue) in channels.iter_mut().zip(&mut route.queues) {
            for sample in channel {
                let input = *sample;
                *sample = if queue.len() == route.delay {
                    queue.pop_front().unwrap()
                } else {
                    S::default()
                };
                queue.push_back(input);
            }
        }
    }
}

pub(crate) struct DelayReplacement<S: Sample> {
    start: usize,
    routes: Vec<Vec<RouteDelay<S>>>,
}

/// Declared delays come from plugins, so their total storage is bounded before it is reserved.
fn check_budget<S: Sample>(shape: impl Iterator<Item = (usize, usize)>) -> Result<(), Failure> {
    let mut bytes = 0usize;
    for (channels, delay) in shape {
        bytes = channels
            .checked_mul(delay)
            .and_then(|samples| samples.checked_mul(size_of::<S>()))
            .and_then(|added| bytes.checked_add(added))
            .filter(|&bytes| bytes <= plughost_core::MAX_ALIGNMENT_BYTES)
            .ok_or_else(|| {
                Failure::new(FailureKind::Configuration, crate::errors::ALIGNMENT_STORAGE)
            })?;
    }
    Ok(())
}

impl<S: Sample> RouteDelay<S> {
    fn new(channels: usize, delay: usize) -> Self {
        Self {
            delay,
            channels,
            queues: (0..if delay == 0 { 0 } else { channels })
                .map(|_| VecDeque::with_capacity(delay))
                .collect(),
        }
    }
}

#[cfg(test)]
#[path = "audio_timing_tests.rs"]
mod tests;
