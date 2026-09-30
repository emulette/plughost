//! Isolated provider factory operations, dispatched only in scan mode.
use crate::Responder;
use plughost_core::ipc::Response;
use plughost_core::{HostIdentity, PresetDiscoveryTarget};
use std::io;
use std::path::Path;
pub fn providers(bundle: &Path, host: &HostIdentity, responder: &Responder) -> io::Result<()> {
    match plughost_formats::clap::preset_providers(bundle, host) {
        Ok(providers) => responder.send(&Response::PresetProviders(providers)),
        Err(error) => responder.fail(Some(0), plughost_formats::Error::from(error).failure()),
    }
}
pub fn discover(
    bundle: &Path,
    host: &HostIdentity,
    target: &PresetDiscoveryTarget,
    responder: &Responder,
) -> io::Result<()> {
    match plughost_formats::clap::discover_presets(bundle, host, target) {
        Ok(presets) => responder.send(&Response::DiscoveredPresets(presets)),
        Err(error) => responder.fail(Some(0), plughost_formats::Error::from(error).failure()),
    }
}
