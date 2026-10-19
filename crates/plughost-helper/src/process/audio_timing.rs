//! Latency and tail follow connected audio paths. External inputs joining a delayed path are
//! delayed to that path's arrival time, so a sidechain and its main input refer to the same sample.

use std::borrow::Borrow;

use plughost_core::ipc::shared::Caller;
use plughost_core::render::Tail;
use plughost_core::{
    AudioInputRoute, AudioSource, Failure, FailureKind, PluginTiming, RoutedChainConfig, Sample,
};
use plughost_formats::{Error, HostedPlugin};

use crate::calls::Calls;

/// Every slot's timing, read on the main thread, which also refreshes what the slots' processors
/// report to the processing thread. A failure names its slot. Chain totals come only from
/// [`AudioTiming`].
pub(crate) fn chain_timings<P: Borrow<dyn HostedPlugin>>(
    plugins: &[P],
    calls: &Calls,
) -> Result<Vec<PluginTiming>, (usize, Error)> {
    plugins
        .iter()
        .enumerate()
        .map(|(slot, plugin)| {
            let _call = calls.enter(Caller::Main, slot);
            plugin.borrow().timing().map_err(|error| (slot, error))
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

    /// Plans `config` for the slots' current timing and evaluates the chain tail. A slot that
    /// asked to be prepared again fails with its index.
    pub fn plan(
        config: &RoutedChainConfig,
        native: &[PluginTiming],
    ) -> Result<(Self, Tail), (Option<usize>, Failure)> {
        let plan = Self::new(config, native).map_err(|failure| (None, failure))?;
        let tail = plan
            .tail(native)
            .map_err(|(slot, failure)| (Some(slot), failure))?;
        Ok((plan, tail))
    }

    /// Re-evaluates current tails along the planned paths; a tail that overflows is infinite.
    /// A plugin keeps the latency it was activated with until it is prepared again or reset, and
    /// every activation plans the chain again; a latency the plan does not have, like a restart
    /// request, needs a new configuration and fresh delay lines, and fails with its slot.
    pub fn tail(&self, native: &[PluginTiming]) -> Result<Tail, (usize, Failure)> {
        self.applied(native, None)?;
        Ok(self.current_tail(native))
    }

    /// Fails with the first slot but `except` whose latency is not the planned one or that asked
    /// to be prepared again, as [`AudioTiming::tail`] does.
    pub fn applied(
        &self,
        native: &[PluginTiming],
        except: Option<usize>,
    ) -> Result<(), (usize, Failure)> {
        let differs = native
            .iter()
            .zip(&self.native_latencies)
            .enumerate()
            .position(|(slot, (timing, expected))| {
                Some(slot) != except && (timing.latency != *expected || timing.restart_required)
            });
        match differs {
            Some(slot) => Err((slot, restart_required())),
            None => Ok(()),
        }
    }

    /// The chain tail of the slots' current tails, whether or not they still match the plan.
    pub fn current_tail(&self, native: &[PluginTiming]) -> Tail {
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
        tail
    }
}

/// The failure of a slot that needs a new configuration before it processes again.
pub(crate) fn restart_required() -> Failure {
    Failure::new(
        FailureKind::RestartRequired,
        crate::errors::AUDIO_LATENCY_CHANGED,
    )
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
    queues: Vec<Vec<S>>,
    position: usize,
}

impl<S: Sample> DelayBank<S> {
    /// Commits prepared histories, which take the replaced ones for the control thread to drop;
    /// unaffected queues retain their pending samples.
    pub fn replace_slot(&mut self, replacement: &mut DelayReplacement<S>) {
        for (old, new) in self.routes[replacement.start..]
            .iter_mut()
            .zip(&mut replacement.routes)
        {
            std::mem::swap(old, new);
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
        let position = route.position;
        for (channel, history) in channels.iter_mut().zip(&mut route.queues) {
            // Fill the reserved history lazily, preserving the initial delay as silence.
            let leading = (route.delay - history.len()).min(channel.len());
            history.extend_from_slice(&channel[..leading]);
            channel[..leading].fill(S::default());
            let mut position = position;
            let mut offset = leading;
            while offset < channel.len() {
                let count = (route.delay - position).min(channel.len() - offset);
                channel[offset..offset + count]
                    .swap_with_slice(&mut history[position..position + count]);
                offset += count;
                position += count;
                if position == route.delay {
                    position = 0;
                }
            }
            route.position = position;
        }
    }
}

pub(crate) struct DelayReplacement<S: Sample> {
    start: usize,
    routes: Vec<Vec<RouteDelay<S>>>,
}

impl<S: Sample> DelayReplacement<S> {
    /// Fresh downstream histories for a bank of `config` planned with `current`, whose slot
    /// `replaced` changes the plan to `timing`. Built on the control thread from the plans
    /// alone; the final bank's total sample budget is checked before allocation, including
    /// unaffected routes.
    pub fn new(
        config: &RoutedChainConfig,
        current: &AudioTiming,
        replaced: usize,
        timing: &AudioTiming,
    ) -> Result<Self, Failure> {
        let slots = config.slots.len();
        let start = replaced + 1;
        let end = (start..slots)
            .find(|&slot| !timing.previous[slot])
            .unwrap_or(slots);
        let channels = |route: &AudioInputRoute| {
            if route.bus.active {
                route.bus.layout.channels()
            } else {
                0
            }
        };
        check_budget::<S>(config.slots.iter().enumerate().flat_map(|(slot, setup)| {
            let delays = if (start..end).contains(&slot) {
                &timing.delays[slot]
            } else {
                &current.delays[slot]
            };
            setup
                .inputs
                .iter()
                .zip(delays)
                .map(move |(route, &delay)| (channels(route), delay as usize))
        }))?;
        let routes = (start..end)
            .map(|slot| {
                config.slots[slot]
                    .inputs
                    .iter()
                    .zip(&timing.delays[slot])
                    .map(|(route, &delay)| RouteDelay::new(channels(route), delay as usize))
                    .collect()
            })
            .collect();
        Ok(DelayReplacement { start, routes })
    }
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
            queues: (0..if delay == 0 { 0 } else { channels })
                .map(|_| Vec::with_capacity(delay))
                .collect(),
            position: 0,
        }
    }
}

#[cfg(test)]
#[path = "audio_timing_tests.rs"]
mod tests;
