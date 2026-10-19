//! Isolated realtime processing. A [`RealtimeEndpoint`] that the application's audio callback
//! owns hands each callback's quantum to the chain's helper through shared memory, and returns
//! the quantum the helper completed `latency` quanta earlier. The callback never waits for the
//! helper: when that output is missing it plays the configured [`Fallback`], ramping into and
//! out of it over one quantum. The chain starts, resynchronizes and stops the endpoint from the
//! application's control thread and reports its [`RealtimeStatus`].

mod dry;
mod endpoint;
mod mailbox;
mod notes;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};
use std::time::Duration;

use plughost_core::FailureKind;
use plughost_core::ipc::shared::realtime::Ring;

pub use endpoint::{FallbackReason, RealtimeEndpoint, RealtimeOutput};

/// What the endpoint plays while the helper's output is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fallback {
    /// Silence.
    Silence,
    /// The callback's input, delayed by the endpoint's whole latency so it lines up with the
    /// processed output. Needs as many output channels as input channels; for effects.
    Dry,
}

/// The shape of a realtime endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RealtimeConfig {
    /// The frames of every callback, at most the chain's prepared maximum block size. Each
    /// callback is one quantum; a callback of another size makes the endpoint resynchronize.
    pub quantum: usize,
    /// Quanta between submitting a quantum and playing its output: 1, 2 or 3. The endpoint adds
    /// `latency * quantum` frames to the chain's latency.
    pub latency: usize,
    /// What plays while the helper's output is missing.
    pub fallback: Fallback,
}

impl RealtimeConfig {
    /// Quanta of `quantum` frames with one quantum of added latency and a silent fallback.
    pub const fn new(quantum: usize) -> RealtimeConfig {
        RealtimeConfig {
            quantum,
            latency: 1,
            fallback: Fallback::Silence,
        }
    }

    /// Sets the added latency in quanta.
    pub const fn with_latency(mut self, latency: usize) -> RealtimeConfig {
        self.latency = latency;
        self
    }

    /// Sets the fallback.
    pub const fn with_fallback(mut self, fallback: Fallback) -> RealtimeConfig {
        self.fallback = fallback;
        self
    }
}

/// The last quantum the helper reported failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RealtimeFailure {
    /// Zero-based plugin slot when attributable to one slot; otherwise `None`.
    pub slot: Option<usize>,
    /// What failed.
    pub kind: FailureKind,
}

/// What a realtime endpoint did since it was created, read from the control thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RealtimeStatus {
    /// The endpoint processes: started and not stopped.
    pub running: bool,
    /// The endpoint plays its fallback until [`crate::Chain::resync_realtime`], until it is
    /// stopped, the chain reset and the endpoint started again, or until
    /// [`crate::Chain::recover_realtime`] succeeds: after an overflow, a callback of another size,
    /// invalid input, a failed quantum, a replacement that changed the chain's latency, a resync
    /// that did not complete, or while a recovery is under way.
    pub resync: bool,
    /// Callbacks the endpoint served.
    pub callbacks: u64,
    /// Callbacks that played the chain's output.
    pub processed: u64,
    /// Callbacks whose output the helper had not completed in time; they played the fallback.
    pub missed: u64,
    /// Callbacks that found no free lane because the helper fell behind; each starts a resync.
    pub overflows: u64,
    /// Callbacks whose size was not the quantum; each starts a resync.
    pub size_mismatches: u64,
    /// Quanta the helper reported failed; each starts a resync.
    pub failures: u64,
    /// The most recent of those failures.
    pub last_failure: Option<RealtimeFailure>,
    /// Quanta the helper completed later than `latency` quanta after their submission, whether
    /// or not the callback still waited for them.
    pub late: u64,
    /// Output events left out: system exclusive events, which the callback cannot copy without
    /// allocating, and events beyond `produced`'s capacity. A new generation's note offs for the
    /// notes earlier outputs left on count too when they do not fit `produced`, as does each
    /// note on beyond the 256 notes the endpoint keeps track of.
    pub dropped_events: u64,
    /// The endpoint's whole latency in frames: the chain's latency the helper last reported,
    /// plus `latency * quantum`.
    pub latency: u32,
    /// While quanta the callback submitted are not all completed, the time since the helper's
    /// worker last completed one, or since the endpoint started, or was recovered, when the
    /// worker has completed none since; zero while none are outstanding or the endpoint is not
    /// running. It grows while a plugin hangs in its processing. How long a stall counts as a
    /// hang is the application's choice, after which it recovers the endpoint with
    /// [`crate::Chain::recover_realtime`]. A crashed helper shows as its exit through
    /// [`crate::Chain::helper_monitor`].
    pub stalled: Duration,
}

/// The kinds a failure is stored as, by index.
const KINDS: [FailureKind; 17] = [
    FailureKind::InvalidInput,
    FailureKind::Unsupported,
    FailureKind::NotFound,
    FailureKind::Load,
    FailureKind::Configuration,
    FailureKind::NotPrepared,
    FailureKind::Closed,
    FailureKind::RestartRequired,
    FailureKind::EditorOpen,
    FailureKind::StateMismatch,
    FailureKind::State,
    FailureKind::Processing,
    FailureKind::Editor,
    FailureKind::Host,
    FailureKind::Protocol,
    FailureKind::Crashed,
    FailureKind::TimedOut,
];

/// What the control thread and the callback share, all in atomics.
#[derive(Default)]
pub(crate) struct State {
    running: AtomicBool,
    /// The chain was prepared again or dropped; the endpoint plays its fallback for good.
    closed: AtomicBool,
    generation: AtomicU64,
    /// The generation a resync stopped: the callback submits nothing more of it.
    halted: AtomicU64,
    /// Odd while a callback runs.
    inside: AtomicU64,
    resync: AtomicBool,
    callbacks: AtomicU64,
    processed: AtomicU64,
    missed: AtomicU64,
    overflows: AtomicU64,
    size_mismatches: AtomicU64,
    failures: AtomicU64,
    /// The kind's index plus one above the slot plus one, or 0.
    last_failure: AtomicU64,
    dropped_events: AtomicU64,
    /// The chain's latency the helper last reported.
    latency: AtomicU32,
    /// The dry path's delay for the current generation, in frames.
    dry_delay: AtomicU64,
    dry: mailbox::Mailbox<dry::DryDelay>,
    /// A recovered chain's ring, which the callback takes when the generation that recovery
    /// started begins, handing back the ring it replaces.
    ring: mailbox::Mailbox<Ring>,
}

impl State {
    fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn fail(&self, slot: Option<usize>, kind: FailureKind) {
        Self::count(&self.failures);
        let kind = KINDS.iter().position(|&known| known == kind).unwrap_or(14) as u64 + 1;
        let slot = slot.map_or(0, |slot| slot as u64 + 1);
        self.last_failure
            .store(kind << 32 | slot & u64::from(u32::MAX), Ordering::Relaxed);
        self.resync.store(true, Ordering::Relaxed);
    }

    fn last_failure(&self) -> Option<RealtimeFailure> {
        let stored = self.last_failure.load(Ordering::Relaxed);
        let kind = *KINDS.get(((stored >> 32) as usize).checked_sub(1)?)?;
        let slot = (stored & u64::from(u32::MAX)).checked_sub(1);
        Some(RealtimeFailure {
            slot: slot.map(|slot| slot as usize),
            kind,
        })
    }

    pub(crate) fn status(&self, late: u64, stalled: Duration, added: u32) -> RealtimeStatus {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        RealtimeStatus {
            running: self.running.load(Ordering::Acquire),
            resync: self.resync.load(Ordering::Relaxed),
            callbacks: load(&self.callbacks),
            processed: load(&self.processed),
            missed: load(&self.missed),
            overflows: load(&self.overflows),
            size_mismatches: load(&self.size_mismatches),
            failures: load(&self.failures),
            last_failure: self.last_failure(),
            late,
            dropped_events: load(&self.dropped_events),
            latency: self.latency.load(Ordering::Relaxed).saturating_add(added),
            stalled,
        }
    }

    /// Whether the endpoint runs.
    pub(crate) fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Starts generation `generation` with the chain's current latency, once the helper runs or
    /// adopted it.
    pub(crate) fn start(&self, generation: u64, latency: u32, dry_delay: u64) {
        self.latency.store(latency, Ordering::Relaxed);
        self.dry_delay.store(dry_delay, Ordering::Relaxed);
        self.resync.store(false, Ordering::Relaxed);
        self.generation.store(generation, Ordering::Release);
        self.running.store(true, Ordering::Release);
    }

    /// Callback: begins a call. Either [`State::halt`] sees the call begin and waits for it to
    /// end, or the call sees the halt.
    fn enter(&self) {
        self.inside.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::SeqCst);
    }

    /// Callback: ends a call, after which the control thread sees what it wrote to the ring.
    fn leave(&self) {
        self.inside.fetch_add(1, Ordering::Release);
    }

    /// Whether a resync stopped the callback's submissions to `generation`.
    fn halted(&self, generation: u64) -> bool {
        self.halted.load(Ordering::Relaxed) == generation
    }

    /// Stops the callback's submissions to the current generation for a resync, and returns once
    /// no call submits any more. A call that began before the halt is waited for. A call that
    /// read the current generation and sees the halt only after the next start cleared `resync`
    /// still finds that generation halted; it submits again once it follows the new one.
    pub(crate) fn halt(&self) {
        self.halted.store(self.generation(), Ordering::Relaxed);
        self.resync.store(true, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        let inside = self.inside.load(Ordering::Acquire);
        // A call never waits, so this ends within one call.
        while inside % 2 == 1 && self.inside.load(Ordering::Acquire) == inside {
            std::thread::yield_now();
        }
    }

    /// Stops the callback's submissions before the helper stops.
    pub(crate) fn stop(&self) {
        self.running.store(false, Ordering::Release);
        self.reclaim();
    }

    /// Makes the callback play its fallback without submitting, as after an overflow, until the
    /// next start.
    pub(crate) fn suspend(&self) {
        self.resync.store(true, Ordering::Relaxed);
    }

    /// Frees what the callback replaced.
    pub(crate) fn reclaim(&self) {
        self.dry.reclaim();
        self.ring.reclaim();
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.stop();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) fn send_dry(&self, delay: Box<dry::DryDelay>) {
        self.dry.send(delay);
    }

    /// Hands the callback the ring of the generation started next.
    pub(crate) fn send_ring(&self, ring: Box<Ring>) {
        self.ring.send(ring);
    }
}

pub(crate) use dry::DryDelay;

#[cfg(test)]
mod tests;
