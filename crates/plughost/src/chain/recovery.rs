//! Snapshots of a chain's plugin settings, and recovering a realtime endpoint in a new helper
//! from one while the audio callback keeps the endpoint.

use plughost_core::StatePurpose;

use super::*;

/// What recovery restores of a chain's plugins, from [`Chain::snapshot`]: each slot's project
/// state and native bypass, in chain order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChainSnapshot {
    /// Each slot's state, saved with [`StatePurpose::Project`].
    pub states: Vec<PluginState>,
    /// Each slot's native bypass as [`Chain::bypass`] reported it, or `None` where unavailable.
    pub bypass: Vec<Option<bool>>,
}

impl Chain {
    /// Saves each slot's project state and native bypass for [`Chain::recover_realtime`], or for
    /// [`Chain::recover`] with its states. It works while the realtime endpoint runs, as
    /// [`Chain::save_state`] and [`Chain::bypass`] do; plughost never takes one by itself. Take
    /// snapshots while the helper is well: a plugin hanging in its processing may also hold up
    /// saving its state.
    pub fn snapshot(&mut self) -> Result<ChainSnapshot, Error> {
        let mut states = Vec::with_capacity(self.plugins.len());
        let mut bypass = Vec::with_capacity(self.plugins.len());
        for slot in 0..self.plugins.len() {
            states.push(self.save_state(slot, StatePurpose::Project)?);
            bypass.push(self.bypass(slot)?.enabled);
        }
        Ok(ChainSnapshot { states, bypass })
    }

    /// Recovers this chain's realtime endpoint in a new helper, typically after its helper
    /// crashed (see [`Chain::helper_monitor`]) or hung (see
    /// [`crate::RealtimeStatus::stalled`]), while the audio callback keeps the same
    /// [`crate::RealtimeEndpoint`]. The endpoint plays its fallback from the call on, as in a
    /// resync. A new chain is prepared from `snapshot.states` as by [`Chain::recover`], and each
    /// slot whose `snapshot.bypass` is `Some` gets that native bypass. The endpoint then moves to
    /// the new chain with its configuration and counters and starts again there: after `latency`
    /// priming callbacks it plays the new chain's output, as after [`Chain::start_realtime`].
    /// The new chain holds only what the snapshot holds; see [`Chain::recover`].
    ///
    /// This chain's helper is not contacted. A hung one may go on until this chain is dropped,
    /// but only with this chain's shared memory, never the new chain's. Switch to the returned
    /// chain and drop this one, which no longer has the endpoint; the shared memory the endpoint
    /// left is released by the new chain's [`Chain::realtime_status`] or
    /// [`Chain::stop_realtime`].
    ///
    /// Without an endpoint this returns [`Error::NotPrepared`]. On any other failure the endpoint
    /// stays with this chain, playing its fallback with `resync` set, and nothing is retried: the
    /// application recovers again, or stops the endpoint, resets this chain and starts it again
    /// when its helper is well.
    pub fn recover_realtime(&mut self, snapshot: &ChainSnapshot) -> Result<Chain, Error> {
        let realtime = self.realtime.as_ref().ok_or(Error::NotPrepared)?;
        realtime.state.suspend();
        let config = realtime.config;
        let mut chain = self.recover(&snapshot.states)?;
        for (slot, enabled) in snapshot.bypass.iter().enumerate() {
            if let Some(enabled) = *enabled {
                chain.set_bypass(slot, enabled)?;
            }
        }
        let ring = chain.install_ring(&config)?;
        let mut realtime = self.realtime.take().ok_or(Error::NotPrepared)?;
        let control = std::mem::replace(&mut realtime.control, ring.control());
        let running = std::mem::replace(&mut realtime.running, false);
        chain.realtime = Some(realtime);
        match chain.start_generation(Some(Box::new(ring))) {
            Ok(_) => Ok(chain),
            Err(error) => {
                // The callback has not been handed the ring; the endpoint returns to this chain
                // before the new one is dropped, so that dropping it does not close the endpoint.
                if let Some(mut realtime) = chain.realtime.take() {
                    realtime.control = control;
                    realtime.running = running;
                    self.realtime = Some(realtime);
                }
                Err(error)
            }
        }
    }
}
