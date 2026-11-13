//! Host bypass: a slot's flag, apart from the plugin's native bypass, under which the processing
//! thread plays the slot's main input, delayed by the slot's latency, in place of its main
//! output while the plugin goes on processing. The flag belongs to the slot, so it stays through
//! preparation, state changes and slot changes; a slot it cannot play, by its main buses or by
//! the alignment budget, refuses it.
use super::*;
use crate::errors::{HOST_BYPASS_LAYOUT, HOST_BYPASS_STORAGE};

impl Host {
    /// Turns the slot's host bypass on or off. In a prepared chain it takes effect between
    /// blocks, crossfading under a running realtime worker; preparing the chain installs it
    /// otherwise.
    pub(super) fn set_host_bypass(&mut self, slot: usize, enabled: bool) -> Response {
        if enabled && self.audio_config.is_some() {
            let refused = if self.dry_paths[slot].is_none() {
                Some(HOST_BYPASS_LAYOUT)
            } else if !self.dry_lines[slot] {
                Some(HOST_BYPASS_STORAGE)
            } else {
                None
            };
            if let Some(message) = refused {
                return Response::Failed {
                    slot: Some(slot),
                    failure: Failure::new(FailureKind::Configuration, message),
                };
            }
        }
        self.host_bypass[slot] = enabled;
        if self.audio_config.is_some() {
            drop(self.processing.request(Command::HostBypass {
                slot,
                enabled,
                ramp: self.realtime_running,
            }));
        }
        Response::Done
    }
}

/// The dry paths of the slots `config` prepares with the buses in `reports`, or the failure of
/// the first slot whose host bypass is on (`bypassed`) without one.
pub(super) fn plan_dry(
    config: &RoutedChainConfig,
    reports: &[Vec<plughost_core::AudioBusInfo>],
    bypassed: &[bool],
) -> Result<Vec<Option<DryPath>>, Box<Response>> {
    let paths: Vec<Option<DryPath>> = config
        .slots
        .iter()
        .zip(reports)
        .map(|(setup, buses)| DryPath::new(setup, buses))
        .collect();
    match (0..paths.len()).find(|&slot| bypassed[slot] && paths[slot].is_none()) {
        Some(slot) => Err(Box::new(Response::Failed {
            slot: Some(slot),
            failure: Failure::new(FailureKind::Configuration, HOST_BYPASS_LAYOUT),
        })),
        None => Ok(paths),
    }
}
