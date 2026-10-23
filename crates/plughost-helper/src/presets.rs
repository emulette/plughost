//! Isolated provider factory operations, dispatched only in scan mode.
use crate::Responder;
use plughost_core::ipc::Response;
use plughost_core::{HostIdentity, PresetDiscoveryTarget};
use std::io;
use std::path::Path;
pub fn providers(bundle: &Path, host: &HostIdentity, responder: &Responder) -> io::Result<()> {
    match entry(bundle).and_then(|entry| plughost_formats::clap::preset_providers(&entry, host)) {
        Ok(providers) => responder.send(&Response::PresetProviders(providers)),
        Err(error) => responder.fail(None, error.failure()),
    }
}
pub fn discover(
    bundle: &Path,
    host: &HostIdentity,
    target: &PresetDiscoveryTarget,
    responder: &Responder,
) -> io::Result<()> {
    // The target is checked before the module's code runs.
    if let Err(error) = target.validate() {
        return responder.fail(None, plughost_formats::Error::from(error).failure());
    }
    match entry(bundle)
        .and_then(|entry| plughost_formats::clap::discover_presets(&entry, host, target))
    {
        Ok(presets) => responder.send(&Response::DiscoveredPresets(presets)),
        Err(error) => responder.fail(None, error.failure()),
    }
}

fn entry(bundle: &Path) -> Result<plughost_formats::clap::Entry, plughost_formats::Error> {
    // SAFETY: plugin code runs only in this helper process, so whatever it does stays in it.
    unsafe { plughost_formats::clap::Entry::load(bundle) }
}
