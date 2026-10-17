//! The chain's control of its realtime endpoint: creating it for the prepared configuration, and
//! starting and stopping the helper's worker. While the endpoint runs, the chain refuses what
//! would change the plugins under it.

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
    state: Arc<State>,
    control: Arc<Control>,
    config: RealtimeConfig,
    running: bool,
    /// The dry path's storage capacity the callback has, in frames.
    dry_capacity: usize,
}

impl Realtime {
    /// Frames the endpoint adds to the chain's latency.
    fn added(&self) -> u32 {
        (self.config.latency * self.config.quantum) as u32
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        self.state.close();
    }
}

impl Chain {
    /// Creates the realtime endpoint for the prepared chain, replacing one created before, which
    /// then plays its fallback for good, as does an endpoint after the chain is prepared again or
    /// dropped. Each callback is one quantum of `config.quantum` frames, at most the prepared
    /// maximum block size, in the prepared precision. Move the returned endpoint to the audio
    /// callback's thread; it plays its fallback until [`Chain::start_realtime`].
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
        });
        Ok(endpoint)
    }

    /// Starts the realtime endpoint: its callbacks submit their quanta from now on, numbered
    /// from 1, and play the chain's output `latency` callbacks later. Returns the endpoint's
    /// whole latency in frames, the chain's plus `latency * quantum`. After a resync, stop the
    /// endpoint, [`Chain::reset`] the chain and start it again.
    pub fn start_realtime(&mut self) -> Result<u32, Error> {
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
        let dry_delay = self.latency as usize + added as usize;
        if realtime.config.fallback == Fallback::Dry
            && dry_delay + realtime.config.quantum > realtime.dry_capacity
        {
            let capacity = dry_delay + realtime.config.quantum;
            realtime
                .state
                .send_dry(Box::new(DryDelay::new(config.input_channels(), capacity)));
            if let Some(realtime) = &mut self.realtime {
                realtime.dry_capacity = capacity;
            }
        }
        let realtime = self.realtime.as_ref().ok_or(Error::NotPrepared)?;
        realtime.control.start(generation, deadline);
        let response = self.request(Request::RealtimeStart, self.timeouts.control)?;
        expect_done(response)?;
        let latency = self.latency;
        let realtime = self.realtime.as_mut().ok_or(Error::NotPrepared)?;
        realtime.state.start(generation, latency, dry_delay as u64);
        realtime.running = true;
        Ok(latency.saturating_add(added))
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

    /// What the realtime endpoint did, or `None` without one.
    pub fn realtime_status(&self) -> Option<RealtimeStatus> {
        let realtime = self.realtime.as_ref()?;
        Some(
            realtime
                .state
                .status(realtime.control.worker().0, realtime.added()),
        )
    }

    pub(super) fn realtime_running(&self) -> bool {
        self.realtime
            .as_ref()
            .is_some_and(|realtime| realtime.running)
    }

    /// Refuses what would change the plugins under a running endpoint.
    pub(super) fn refuse_while_realtime(&self) -> Result<(), Error> {
        if self.realtime_running() {
            return Err(Error::RealtimeRunning);
        }
        Ok(())
    }
}
