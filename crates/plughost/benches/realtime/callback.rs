//! The audio callback thread: the synthetic device's calls on the realtime endpoint, and what it
//! measures. With `--rate-change` the control thread hands it a new endpoint between callbacks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use plughost::{
    AutomationEvent, BlockContext, FallbackReason, MAX_BLOCK_EVENTS, ParameterChange,
    RealtimeEndpoint, RealtimeOutput,
};
use plughost_core::ipc::shared::clock;

use crate::{GAIN, driver};

/// What the callback thread measured.
pub struct Measured {
    /// How late the driver woke for each callback, in clock ticks.
    pub delays: Vec<u64>,
    /// How long each callback took, in clock ticks.
    pub callbacks: Vec<u64>,
    /// How long the helper took for each output played.
    pub completions: Vec<Duration>,
    /// Grid slots the driver skipped after waking late.
    pub skipped: u64,
    /// Callbacks that played the fallback, by reason: stopped, priming, missed, resync.
    pub fallbacks: [u64; 4],
    /// The shortest quantum the callbacks had to finish within.
    pub shortest: Duration,
}

/// The rate and quantum of a callback profile.
#[derive(Clone, Copy)]
pub struct Profile {
    pub rate: f64,
    pub quantum: usize,
}

impl Profile {
    pub fn duration(self) -> Duration {
        Duration::from_secs_f64(self.quantum as f64 / self.rate)
    }
}

/// An endpoint for the callback thread to take over, with the profile it was created for.
pub struct Handover {
    pub endpoint: RealtimeEndpoint,
    pub profile: Profile,
}

/// What the callback gives the endpoint besides the audio.
#[derive(Clone, Copy)]
pub struct Inputs {
    /// Eight gain points per quantum.
    pub automation: bool,
    /// One block of more events than a block may hold whenever `Shared::overflow` is set.
    pub overflow: bool,
}

/// What the control thread shares with the callback thread. The callback thread publishes its
/// counts after each callback, outside the time it measures.
#[derive(Default)]
pub struct Shared {
    /// Ends the callbacks.
    pub stop: AtomicBool,
    /// Makes the next callback submit more events than a block may hold; the callback clears it.
    pub overflow: AtomicBool,
    fallbacks: AtomicU64,
    processed: AtomicU64,
}

impl Shared {
    /// Callbacks that played the fallback so far.
    pub fn fallbacks(&self) -> u64 {
        self.fallbacks.load(Ordering::Relaxed)
    }

    /// Callbacks that played the chain's output so far.
    pub fn processed(&self) -> u64 {
        self.processed.load(Ordering::Relaxed)
    }
}

/// How `--rate-change` replaces the callback thread's endpoint: the driver takes the next one
/// from `next` between two callbacks, on its own thread outside the time it measures, and sends
/// the one it leaves to `retired`, so that the control thread frees it.
pub struct Feed {
    pub next: Receiver<Handover>,
    pub retired: Sender<RealtimeEndpoint>,
}

/// Uniform white noise in [-0.25, 0.25), the same on every run.
fn noise(frames: usize) -> Vec<Vec<f32>> {
    let mut state = 1u32;
    (0..2)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5) / 2.0
                })
                .collect()
        })
        .collect()
}

/// `points` points on the gain of slot 0 across a quantum.
fn automation(quantum: usize, points: usize) -> Vec<AutomationEvent> {
    (0..points)
        .map(|point| AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: GAIN,
                offset: point * quantum / points,
                value: if point % 2 == 0 { 0.25 } else { 0.75 },
            },
        })
        .collect()
}

/// An endpoint with the buffers and the grid of quanta of its profile.
struct Lane {
    endpoint: RealtimeEndpoint,
    profile: Profile,
    period: u64,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    points: Vec<AutomationEvent>,
    /// An over-budget block's events, with `--overflow`.
    excess: Vec<AutomationEvent>,
    /// The grid's first slot, and the slot the next callback is due at.
    start: u64,
    slot: u64,
}

impl Lane {
    fn new(handover: Handover, inputs: Inputs) -> Lane {
        let Handover { endpoint, profile } = handover;
        let period = clock::ticks(profile.duration());
        driver::schedule(period);
        Lane {
            endpoint,
            profile,
            period,
            input: noise(profile.quantum),
            output: noise(profile.quantum),
            points: automation(profile.quantum, if inputs.automation { 8 } else { 0 }),
            excess: automation(
                profile.quantum,
                if inputs.overflow {
                    MAX_BLOCK_EVENTS + 1
                } else {
                    0
                },
            ),
            start: clock::now() + period,
            slot: 0,
        }
    }
}

/// Calls the endpoint on the grid of quanta from a realtime thread, `callbacks` times at most,
/// or until `shared.stop`; and takes each endpoint `feed` hands it.
pub fn drive(
    first: Handover,
    callbacks: usize,
    inputs: Inputs,
    shared: &Shared,
    feed: Option<Feed>,
) -> Measured {
    let mut lane = Lane::new(first, inputs);
    let mut produced = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut measured = Measured {
        delays: Vec::with_capacity(callbacks),
        callbacks: Vec::with_capacity(callbacks),
        completions: Vec::with_capacity(callbacks),
        skipped: 0,
        fallbacks: [0; 4],
        shortest: lane.profile.duration(),
    };
    let mut processed = 0;
    for _ in 0..callbacks {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        if let Some(feed) = &feed
            && let Ok(next) = feed.next.try_recv()
        {
            measured.shortest = measured.shortest.min(next.profile.duration());
            let old = std::mem::replace(&mut lane, Lane::new(next, inputs));
            let _ = feed.retired.send(old.endpoint);
        }
        let due = lane.start + lane.slot * lane.period;
        driver::wait_until(due);
        // The endpoint finds an over-budget block invalid, plays its fallback and resyncs.
        let points = if shared.overflow.swap(false, Ordering::Relaxed) {
            &lane.excess
        } else {
            &lane.points
        };
        let woke = clock::now();
        let [input_left, input_right] = &lane.input[..] else {
            unreachable!("the chain has two input channels");
        };
        let [left, right] = &mut lane.output[..] else {
            unreachable!("the chain has two output channels");
        };
        let played = lane.endpoint.process_audio_f32(
            &BlockContext::new(lane.profile.quantum),
            &[input_left, input_right],
            &mut [left, right],
            points,
            &[],
            &mut produced,
        );
        let done = clock::now();
        measured.delays.push(woke.saturating_sub(due));
        measured.callbacks.push(done.saturating_sub(woke));
        match played {
            Ok(RealtimeOutput::Processed) => {
                measured.completions.extend(lane.endpoint.last_completion());
                processed += 1;
                shared.processed.store(processed, Ordering::Relaxed);
            }
            Ok(RealtimeOutput::Fallback(reason)) => {
                let index = match reason {
                    FallbackReason::Stopped => 0,
                    FallbackReason::Priming => 1,
                    FallbackReason::Missed => 2,
                    _ => 3,
                };
                measured.fallbacks[index] += 1;
            }
            // The driver passes valid input; a failure would show as a resync.
            Ok(_) | Err(_) => measured.fallbacks[3] += 1,
        }
        shared
            .fallbacks
            .store(measured.fallbacks.iter().sum(), Ordering::Relaxed);
        let next = (clock::now().saturating_sub(lane.start) / lane.period + 1).max(lane.slot + 1);
        measured.skipped += next - lane.slot - 1;
        lane.slot = next;
    }
    measured
}
