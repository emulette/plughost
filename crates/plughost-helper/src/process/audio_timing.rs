//! Latency and tail follow connected audio paths. External inputs joining a delayed path are
//! delayed to that path's arrival time, so a sidechain and its main input refer to the same sample.
//! A slot's dry input, which its host bypass plays, is delayed by the slot's latency, so turning
//! host bypass on or off changes neither the chain's latency nor its alignment.

use std::borrow::Borrow;

use plughost_core::ipc::shared::Caller;
use plughost_core::render::Tail;
use plughost_core::{
    AudioInputRoute, AudioSource, Failure, FailureKind, PluginTiming, RoutedChainConfig, Sample,
};
use plughost_formats::{Error, HostedPlugin};

use super::prepared::DryPath;
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

    /// Plans `config` for the slots' current timing and evaluates the chain tail with the slots
    /// whose host bypass is on `bypassed`. A slot that asked to be prepared again fails with its
    /// index.
    pub fn plan(
        config: &RoutedChainConfig,
        native: &[PluginTiming],
        bypassed: &[bool],
    ) -> Result<(Self, Tail), (Option<usize>, Failure)> {
        let plan = Self::new(config, native).map_err(|failure| (None, failure))?;
        let tail = plan
            .tail(native, bypassed)
            .map_err(|(slot, failure)| (Some(slot), failure))?;
        Ok((plan, tail))
    }

    /// Re-evaluates current tails along the planned paths; a tail that overflows is infinite.
    /// A plugin keeps the latency it was activated with until it is prepared again or reset, and
    /// every activation plans the chain again; a latency the plan does not have, like a restart
    /// request, needs a new configuration and fresh delay lines, and fails with its slot.
    pub fn tail(
        &self,
        native: &[PluginTiming],
        bypassed: &[bool],
    ) -> Result<Tail, (usize, Failure)> {
        self.applied(native, None)?;
        Ok(self.current_tail(native, bypassed))
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

    /// The chain tail of the slots' current tails, whether or not they still match the plan. A
    /// slot whose host bypass is on (`bypassed`) passes its input on and adds no tail of its own.
    pub fn current_tail(&self, native: &[PluginTiming], bypassed: &[bool]) -> Tail {
        let mut tail = Tail::Samples(0);
        for ((timing, previous), &bypassed) in native.iter().zip(&self.previous).zip(bypassed) {
            let incoming = if *previous { tail } else { Tail::Samples(0) };
            let own = if bypassed {
                Tail::Samples(0)
            } else {
                timing.tail
            };
            tail = match (incoming, own) {
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
    /// Reserves the routes' delays and the dry lines of the slots with a `dry` path, those whose
    /// host bypass is on (`bypassed`) first (see [`DelayBank::new`]).
    pub fn new(
        config: &RoutedChainConfig,
        timing: AudioTiming,
        dry: &[Option<DryPath>],
        bypassed: &[bool],
    ) -> Result<Self, Failure> {
        let (delays32, delays64) = match config.sample_format {
            plughost_core::SampleFormat::F32 => {
                (Some(DelayBank::new(config, &timing, dry, bypassed)?), None)
            }
            plughost_core::SampleFormat::F64 => {
                (None, Some(DelayBank::new(config, &timing, dry, bypassed)?))
            }
            _ => return Err(crate::errors::sample_format()),
        };
        Ok(Self {
            timing,
            delays32,
            delays64,
        })
    }

    /// Whether each slot has a dry line, without which its host bypass cannot be turned on.
    pub fn dry_lines(&self) -> Vec<bool> {
        match (&self.delays32, &self.delays64) {
            (Some(bank), _) => bank.dry_lines(),
            (_, Some(bank)) => bank.dry_lines(),
            (None, None) => Vec::new(),
        }
    }
}

pub(crate) struct DelayBank<S: Sample> {
    routes: Vec<Vec<RouteDelay<S>>>,
    /// Each slot's dry line for host bypass, as long as its planned latency: `None` for a slot
    /// without a dry path, or whose line did not fit in the budget.
    dry: Vec<Option<RouteDelay<S>>>,
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
        if let Some(line) = &mut replacement.dry {
            std::mem::swap(&mut self.dry[replacement.start - 1], line);
        }
    }

    /// Reserves the complete declared alignment before any sample is processed. Queues begin
    /// empty to represent leading silence without initializing the entire sample history.
    ///
    /// Each slot with a `dry` path also gets a dry line, unless the budget has no room for it:
    /// the lines of the slots whose host bypass is on (`bypassed`) must fit with the routes, and
    /// the others are reserved in slot order while they fit. A slot left without one cannot turn
    /// host bypass on, which keeps a plugin's huge latency from failing a chain that never
    /// bypasses it.
    pub fn new(
        config: &RoutedChainConfig,
        timing: &AudioTiming,
        dry: &[Option<DryPath>],
        bypassed: &[bool],
    ) -> Result<Self, Failure> {
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
        let mut bytes = add_budget::<S>(0, shape().flatten())?;
        let lines = dry_shapes::<S>(&mut bytes, &timing.native_latencies, dry, bypassed)?;
        let routes = shape()
            .map(|routes| {
                routes
                    .map(|(channels, delay)| RouteDelay::new(channels, delay))
                    .collect()
            })
            .collect();
        let dry = lines
            .into_iter()
            .map(|line| line.map(|(channels, delay)| RouteDelay::new(channels, delay)))
            .collect();
        Ok(Self { routes, dry })
    }

    /// Whether `route` (its full index in SlotAudioConfig.inputs) delays its audio.
    pub fn delays(&self, slot: usize, route: usize) -> bool {
        self.routes[slot][route].delay != 0
    }

    /// Delays one route's prepared channels in place.
    pub fn apply(&mut self, slot: usize, route: usize, channels: &mut [Vec<S>]) {
        self.routes[slot][route].apply(channels);
    }

    /// Whether the slot's dry line keeps a history, which it must be fed for even while the
    /// slot's host bypass is off.
    pub fn dry_history(&self, slot: usize) -> bool {
        self.dry[slot].as_ref().is_some_and(|line| line.delay != 0)
    }

    /// Delays the slot's dry input in place by the slot's latency, or returns false for a slot
    /// without a dry line.
    pub fn apply_dry(&mut self, slot: usize, channels: &mut [Vec<S>]) -> bool {
        match &mut self.dry[slot] {
            Some(line) => {
                line.apply(channels);
                true
            }
            None => false,
        }
    }

    fn dry_lines(&self) -> Vec<bool> {
        self.dry.iter().map(Option::is_some).collect()
    }
}

pub(crate) struct DelayReplacement<S: Sample> {
    start: usize,
    routes: Vec<Vec<RouteDelay<S>>>,
    /// The replaced slot's new dry line, when its latency changed; otherwise it keeps its line
    /// and the dry history in it.
    dry: Option<Option<RouteDelay<S>>>,
}

impl<S: Sample> DelayReplacement<S> {
    /// Fresh downstream histories for a bank of `config` planned with `current`, whose slot
    /// `replaced` changes the plan to `timing`. Built on the control thread from the plans
    /// alone; the final bank's total sample budget is checked before allocation, including
    /// unaffected routes and the dry lines the slots have (`lines`). The replaced slot keeps its
    /// dry line when its latency stays, so a slot whose host bypass is on plays on without a
    /// gap; another latency gets it a fresh line, reserved as [`DelayBank::new`] does.
    pub fn new(
        config: &RoutedChainConfig,
        current: &AudioTiming,
        replaced: usize,
        timing: &AudioTiming,
        dry: &[Option<DryPath>],
        bypassed: &[bool],
        lines: &[bool],
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
        let mut bytes = add_budget::<S>(
            0,
            config.slots.iter().enumerate().flat_map(|(slot, setup)| {
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
            }),
        )?;
        let retimed = current.native_latencies[replaced] != timing.native_latencies[replaced];
        let kept = dry.iter().enumerate().filter_map(|(slot, path)| {
            let path = path.as_ref().filter(|_| lines[slot])?;
            (slot != replaced || !retimed)
                .then(|| (path.output.len(), current.native_latencies[slot] as usize))
        });
        bytes = add_budget::<S>(bytes, kept)?;
        let line = match &dry[replaced] {
            Some(path) if retimed => {
                let shape = (
                    path.output.len(),
                    timing.native_latencies[replaced] as usize,
                );
                match add_budget::<S>(bytes, std::iter::once(shape)) {
                    Ok(_) => Some(Some(RouteDelay::new(shape.0, shape.1))),
                    Err(failure) if bypassed[replaced] => return Err(failure),
                    Err(_) => Some(None),
                }
            }
            _ => None,
        };
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
        Ok(DelayReplacement {
            start,
            routes,
            dry: line,
        })
    }

    /// Whether the replaced slot has a dry line after the replacement, when it gets a new one.
    pub fn dry_line(&self) -> Option<bool> {
        self.dry.as_ref().map(Option::is_some)
    }
}

/// The dry lines, as channels and length, of the slots with a `dry` path, within what `bytes`
/// of the budget is left: a slot whose host bypass is on (`bypassed`) must get its line, the
/// others get theirs in slot order while it fits.
fn dry_shapes<S: Sample>(
    bytes: &mut usize,
    latencies: &[u32],
    dry: &[Option<DryPath>],
    bypassed: &[bool],
) -> Result<Vec<Option<(usize, usize)>>, Failure> {
    let shape = |slot: usize| {
        dry[slot]
            .as_ref()
            .map(|path| (path.output.len(), latencies[slot] as usize))
    };
    let required = (0..dry.len())
        .filter(|&slot| bypassed[slot])
        .filter_map(shape);
    *bytes = add_budget::<S>(*bytes, required)?;
    Ok((0..dry.len())
        .map(|slot| {
            let line = shape(slot)?;
            if bypassed[slot] {
                return Some(line);
            }
            let total = add_budget::<S>(*bytes, std::iter::once(line)).ok()?;
            *bytes = total;
            Some(line)
        })
        .collect())
}

/// Declared delays come from plugins, so their total storage is bounded before it is reserved:
/// adds `shape` to the `bytes` already counted, or fails past the budget.
fn add_budget<S: Sample>(
    mut bytes: usize,
    shape: impl Iterator<Item = (usize, usize)>,
) -> Result<usize, Failure> {
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
    Ok(bytes)
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

    /// Delays `channels` in place.
    fn apply(&mut self, channels: &mut [Vec<S>]) {
        if self.delay == 0 {
            return;
        }
        let position = self.position;
        for (channel, history) in channels.iter_mut().zip(&mut self.queues) {
            // Fill the reserved history lazily, preserving the initial delay as silence.
            let leading = (self.delay - history.len()).min(channel.len());
            history.extend_from_slice(&channel[..leading]);
            channel[..leading].fill(S::default());
            let mut position = position;
            let mut offset = leading;
            while offset < channel.len() {
                let count = (self.delay - position).min(channel.len() - offset);
                channel[offset..offset + count]
                    .swap_with_slice(&mut history[position..position + count]);
                offset += count;
                position += count;
                if position == self.delay {
                    position = 0;
                }
            }
            self.position = position;
        }
    }
}

#[cfg(test)]
#[path = "audio_timing_tests.rs"]
mod tests;
