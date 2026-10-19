//! The chain's control of its realtime endpoint: creating it for the prepared configuration, and
//! starting, resynchronizing and stopping the helper's worker. While the endpoint runs, the chain
//! replaces one slot's instance between quanta and refuses what would reconfigure or reset the
//! chain under it.

use std::sync::Arc;
use std::time::Duration;

use plughost_core::InputError;
use plughost_core::ipc::shared::realtime::{Control, MAX_LANES, Ring, RingDescriptor};
use plughost_core::ipc::shared::{SlotConfig, clock};

use super::*;
use crate::realtime::{
    DryDelay, Fallback, RealtimeConfig, RealtimeEndpoint, RealtimeStatus, State,
};

/// The control thread's handle on the chain's endpoint.
pub(super) struct Realtime {
    pub(super) state: Arc<State>,
    /// The control mapping of the chain's ring.
    pub(super) control: Arc<Control>,
    pub(super) config: RealtimeConfig,
    /// The helper's worker runs.
    pub(super) running: bool,
    /// The dry path's storage capacity the callback has, in frames.
    dry_capacity: usize,
    /// When the current generation started, in [`clock`] ticks.
    started: u64,
}

impl Realtime {
    /// Frames the endpoint adds to the chain's latency.
    fn added(&self) -> u32 {
        (self.config.latency * self.config.quantum) as u32
    }

    /// The time since the worker last completed a quantum, or since the generation started,
    /// while a submitted quantum is outstanding. The worker stamps its heartbeat only when it
    /// completes one.
    fn stalled(&self, running: bool) -> Duration {
        let (submitted, completed) = self.control.progress();
        if !running || submitted <= completed {
            return Duration::ZERO;
        }
        let heartbeat = self.control.worker().1;
        clock::duration(clock::now().saturating_sub(heartbeat.max(self.started)))
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        self.state.close();
    }
}

impl Chain {
    /// Creates the realtime endpoint for the prepared chain, replacing one created before, which
    /// then plays its fallback for good, as does an endpoint after the chain that has it is
    /// prepared again or dropped; [`Chain::recover_realtime`] moves it to another chain. Each
    /// callback is one quantum of `config.quantum` frames, at most the prepared maximum block
    /// size, in the prepared precision. Move the returned endpoint to the audio callback's thread;
    /// it plays its fallback until [`Chain::start_realtime`].
    pub fn realtime_endpoint(
        &mut self,
        config: &RealtimeConfig,
    ) -> Result<RealtimeEndpoint, Error> {
        self.refuse_while_realtime()?;
        let chain = self.config.as_ref().ok_or(Error::NotPrepared)?;
        let invalid = |error| Error::Input { slot: None, error };
        if config.quantum == 0 || config.quantum > chain.max_block_size {
            return Err(invalid(InputError::RealtimeQuantum));
        }
        if !(1..=MAX_LANES - 2).contains(&config.latency) {
            return Err(invalid(InputError::RealtimeLatency));
        }
        let dry = config.fallback == Fallback::Dry;
        if dry && chain.input_channels() != chain.output_channels() {
            return Err(invalid(InputError::RealtimeDry));
        }
        let ring = self.install_ring(config)?;
        let chain = self.config.as_ref().ok_or(Error::NotPrepared)?;
        let state = Arc::new(State::default());
        let added = config.latency * config.quantum;
        let dry_capacity = self.latency as usize + added + config.quantum;
        let dry = dry.then(|| {
            let mut delay = DryDelay::new(chain.input_channels(), dry_capacity);
            delay.set_delay(self.latency as usize + added);
            Box::new(delay)
        });
        let control = ring.control();
        let endpoint = RealtimeEndpoint::new(ring, Arc::clone(&state), config, chain, dry);
        self.realtime = Some(Realtime {
            control,
            state,
            config: *config,
            running: false,
            dry_capacity,
            started: 0,
        });
        Ok(endpoint)
    }

    /// Creates a ring of `config`'s shape for the prepared chain and installs it in the helper.
    pub(super) fn install_ring(&mut self, config: &RealtimeConfig) -> Result<Ring, Error> {
        let chain = self.config.as_ref().ok_or(Error::NotPrepared)?;
        let lanes = config.latency + 2;
        let first = self.generation.checked_add(1).ok_or(Error::Protocol)?;
        self.generation = first + lanes as u64;
        let descriptor = RingDescriptor {
            control: first,
            first_lane: first + 1,
            lanes,
            config: SlotConfig {
                sample_format: chain.sample_format,
                max_frames: config.quantum,
                input_channels: chain.input_channels(),
                output_channels: chain.output_channels(),
            },
        };
        let ring = Ring::new(descriptor).map_err(shared::storage_error)?;
        let response = self.helper.install_ring(
            &ring,
            |tokens| Request::RealtimeInstall {
                ring: descriptor,
                tokens,
            },
            self.timeouts.control,
        )?;
        expect_done(response)?;
        Ok(ring)
    }

    /// Starts the realtime endpoint: its callbacks submit their quanta from now on, numbered
    /// from 1, and play the chain's output `latency` callbacks later. Returns the endpoint's
    /// whole latency in frames, the chain's plus `latency * quantum`. After a resync, call
    /// [`Chain::resync_realtime`]; stopping the endpoint, [`Chain::reset`]ting the chain and
    /// starting it again works too.
    pub fn start_realtime(&mut self) -> Result<u32, Error> {
        self.start_generation(None)
    }

    /// Starts the endpoint's next generation once the helper's worker runs, handing the callback
    /// `ring` for it.
    pub(super) fn start_generation(&mut self, ring: Option<Box<Ring>>) -> Result<u32, Error> {
        let realtime = self.realtime.as_ref().ok_or(Error::NotPrepared)?;
        if realtime.running {
            return Ok(self.latency.saturating_add(realtime.added()));
        }
        let config = self.config.as_ref().ok_or(Error::NotPrepared)?;
        let generation = realtime.state.generation() + 1;
        let quantum_seconds =
            (realtime.config.latency * realtime.config.quantum) as f64 / config.sample_rate;
        let deadline = clock::ticks(Duration::from_secs_f64(quantum_seconds));
        let added = realtime.added();
        let dry_delay = self.dry_delay()?;
        let realtime = self.realtime.as_ref().ok_or(Error::NotPrepared)?;
        realtime.control.start(generation, deadline);
        let response = self.request(Request::RealtimeStart, self.timeouts.control)?;
        expect_done(response)?;
        let latency = self.latency;
        let realtime = self.realtime.as_mut().ok_or(Error::NotPrepared)?;
        if let Some(ring) = ring {
            realtime.state.send_ring(ring);
        }
        realtime.started = clock::now();
        realtime.state.start(generation, latency, dry_delay as u64);
        realtime.running = true;
        Ok(latency.saturating_add(added))
    }

    /// Resynchronizes the running realtime endpoint without stopping it, and returns its whole
    /// latency in frames, as [`Chain::start_realtime`] does. Call it from the control thread once
    /// [`RealtimeStatus::resync`] is set; nothing resynchronizes on its own. The callbacks stop
    /// submitting and play the fallback ([`crate::FallbackReason::Resync`]) while every plugin is
    /// reset as by [`Chain::reset`], which ends the notes the plugins hold and applies a latency
    /// they announced. The helper's worker goes on and numbers the new generation's quanta from
    /// 1; its first output plays as is `latency` callbacks later, as after a start, and the
    /// first callback ends the notes earlier outputs delivered through `produced`.
    ///
    /// A plugin whose reset does not return within the control timeout, as when it holds up
    /// the worker, fails it with [`Error::TimedOut`], and the endpoint plays its fallback until
    /// the application recovers. A stopped endpoint has nothing to resynchronize: the call
    /// returns its latency, as [`Chain::start_realtime`] would, and changes nothing.
    pub fn resync_realtime(&mut self) -> Result<u32, Error> {
        let realtime = self.realtime.as_ref().ok_or(Error::NotPrepared)?;
        if !realtime.running {
            return Ok(self.latency.saturating_add(realtime.added()));
        }
        realtime.state.halt();
        let generation = realtime.state.generation() + 1;
        // No callback submits now, so the control thread may clear the submitted sequence; the
        // worker adopts the generation before the helper resets the plugins.
        realtime.control.resync(generation);
        match self.request(Request::RealtimeResync, self.timeouts.control)? {
            Response::Timing { latency, tail, .. } => {
                self.latency = latency;
                self.tail = tail;
            }
            _ => return Err(self.protocol_error()),
        }
        let dry_delay = self.dry_delay()?;
        let latency = self.latency;
        let realtime = self.realtime.as_mut().ok_or(Error::NotPrepared)?;
        realtime.started = clock::now();
        realtime.state.start(generation, latency, dry_delay as u64);
        Ok(latency.saturating_add(realtime.added()))
    }

    /// The dry path's delay for a generation at the chain's current latency. The callback gets
    /// storage enough for it, which it takes as it follows the generation.
    fn dry_delay(&mut self) -> Result<usize, Error> {
        let config = self.config.as_ref().ok_or(Error::NotPrepared)?;
        let realtime = self.realtime.as_mut().ok_or(Error::NotPrepared)?;
        let delay = self.latency as usize + realtime.added() as usize;
        let capacity = delay + realtime.config.quantum;
        if realtime.config.fallback == Fallback::Dry && capacity > realtime.dry_capacity {
            realtime
                .state
                .send_dry(Box::new(DryDelay::new(config.input_channels(), capacity)));
            realtime.dry_capacity = capacity;
        }
        Ok(delay)
    }

    /// Stops the realtime endpoint between quanta; its callbacks play the fallback until it is
    /// started again.
    pub fn stop_realtime(&mut self) -> Result<(), Error> {
        let Some(realtime) = &mut self.realtime else {
            return Ok(());
        };
        if !realtime.running {
            return Ok(());
        }
        realtime.state.stop();
        realtime.running = false;
        let response = self.request(Request::RealtimeStop, self.timeouts.control)?;
        expect_done(response)
    }

    /// What the realtime endpoint did, or `None` without one. Call it from a control thread: it
    /// also releases the ring the endpoint stopped using after a recovery.
    pub fn realtime_status(&self) -> Option<RealtimeStatus> {
        let realtime = self.realtime.as_ref()?;
        realtime.state.reclaim();
        let running = realtime.state.running();
        Some(realtime.state.status(
            realtime.control.worker().0,
            realtime.stalled(running),
            realtime.added(),
        ))
    }

    pub(super) fn realtime_running(&self) -> bool {
        self.realtime
            .as_ref()
            .is_some_and(|realtime| realtime.running)
    }

    /// Refuses what would reconfigure or reset the chain under a running endpoint.
    pub(super) fn refuse_while_realtime(&self) -> Result<(), Error> {
        if self.realtime_running() {
            return Err(Error::RealtimeRunning);
        }
        Ok(())
    }
}
