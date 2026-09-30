//! One short-lived, supervised helper per native CLAP provider operation.
use crate::Error;
use crate::helper::{Helper, Mode};
use plughost_core::ipc::{Request, Response};
use plughost_core::{HostIdentity, PresetDiscovery, PresetDiscoveryTarget, PresetProviderInfo};
use std::path::Path;
use std::time::Duration;
/// Lists CLAP providers and declared preset locations. Provider code runs only in a scan helper.
pub fn preset_providers(
    helper: &Path,
    bundle: &Path,
    host: &HostIdentity,
    timeout: Duration,
) -> Result<Vec<PresetProviderInfo>, Error> {
    host.validate()
        .map_err(|error| Error::Input { slot: None, error })?;
    let mut helper = Helper::spawn(helper, Mode::Scan)?;
    match helper.request(
        Request::PresetProviders {
            bundle: bundle.to_owned(),
            host: host.clone(),
        },
        timeout,
    )? {
        Response::PresetProviders(providers) => Ok(providers),
        _ => Err(Error::Protocol),
    }
}
/// Queries metadata at one provider-declared location, preset/container file, or directory.
/// A directory is crawled for the provider's file types in one helper, and a file the provider
/// refuses is listed in `failed_files`. A crash or timeout ends only this short-lived helper and
/// fails the whole query; query the files of such a directory one by one to isolate the file.
pub fn discover_presets(
    helper: &Path,
    bundle: &Path,
    host: &HostIdentity,
    target: &PresetDiscoveryTarget,
    timeout: Duration,
) -> Result<PresetDiscovery, Error> {
    host.validate()
        .map_err(|error| Error::Input { slot: None, error })?;
    let mut helper = Helper::spawn(helper, Mode::Scan)?;
    match helper.request(
        Request::DiscoverPresets {
            bundle: bundle.to_owned(),
            host: host.clone(),
            target: target.clone(),
        },
        timeout,
    )? {
        Response::DiscoveredPresets(presets) => Ok(presets),
        _ => Err(Error::Protocol),
    }
}
