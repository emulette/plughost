//! Preset discovery runs native provider code; callers should use the scan helper.
mod indexer;
mod receiver;
use super::ClapError;
use clack_extensions::preset_discovery::prelude::*;
use clack_host::prelude::{HostInfo, PluginEntry};
use indexer::Indexer;
use plughost_core::*;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

pub(super) fn text(value: &CStr) -> Result<String, ClapError> {
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| ClapError::PresetMetadata)
}
pub(super) fn optional(value: Option<&CStr>) -> Result<Option<String>, ClapError> {
    value.map(text).transpose()
}
fn entry(bundle: &Path, identity: &HostIdentity) -> Result<(PluginEntry, HostInfo), ClapError> {
    identity.validate().map_err(ClapError::Input)?;
    // SAFETY: loading native provider code is this operation's explicit purpose.
    let entry = unsafe { PluginEntry::load(bundle) }.map_err(|e| ClapError::Load(e.to_string()))?;
    let host = HostInfo::new(&identity.name, &identity.vendor, "", &identity.version)
        .map_err(|_| ClapError::PresetInput)?;
    Ok((entry, host))
}
/// Enumerates providers and their declarations without walking the filesystem.
pub fn preset_providers(
    bundle: &Path,
    identity: &HostIdentity,
) -> Result<Vec<PresetProviderInfo>, ClapError> {
    let (entry, host) = entry(bundle, identity)?;
    let factory = entry
        .get_factory::<PresetDiscoveryFactory>()
        .ok_or(ClapError::PresetUnsupported)?;
    let mut result = Vec::new();
    for index in 0..factory.provider_count() {
        let desc = factory
            .get_provider_descriptor(index)
            .ok_or(ClapError::PresetMetadata)?;
        let id = desc.id().ok_or(ClapError::PresetMetadata)?;
        let id_text = text(id)?;
        if result.iter().any(|p: &PresetProviderInfo| p.id == id_text) {
            return Err(ClapError::PresetMetadata);
        }
        let name = text(desc.name().ok_or(ClapError::PresetMetadata)?)?;
        let vendor = optional(desc.vendor())?;
        let mut provider = Provider::instantiate(Indexer::new(), &entry, id, &host)
            .map_err(|_| ClapError::PresetDiscovery)?;
        let declarations = provider.indexer_mut().take()?;
        result.push(PresetProviderInfo {
            id: id_text,
            name,
            vendor,
            file_types: declarations.file_types,
            locations: declarations.locations,
            soundpacks: declarations.soundpacks,
        });
    }
    Ok(result)
}
/// Queries one provider at a plugin location, a preset/container file, or a directory. A
/// directory is crawled like the preset roots of other formats: recursively, following links
/// without revisiting a link loop, skipping unreadable entries, in path order. Its files matching
/// the provider's declared file types are queried one by one; a file the provider refuses is
/// reported without failing the others.
pub fn discover_presets(
    bundle: &Path,
    identity: &HostIdentity,
    target: &PresetDiscoveryTarget,
) -> Result<PresetDiscovery, ClapError> {
    let id = input_string(&target.provider_id)?;
    location_path(&target.location)?;
    let (entry, host) = entry(bundle, identity)?;
    if entry.get_factory::<PresetDiscoveryFactory>().is_none() {
        return Err(ClapError::PresetUnsupported);
    }
    let mut provider = Provider::instantiate(Indexer::new(), &entry, &id, &host)
        .map_err(|_| ClapError::PresetDiscovery)?;
    if let Some(error) = &provider.indexer().error {
        return Err(error.clone());
    }
    let declarations = provider.indexer_mut().take()?;
    match &target.location {
        PresetLocation::File { path } if path.is_dir() => {
            let mut discovery = PresetDiscovery::default();
            for path in files(path, &declarations.file_types)? {
                let location = PresetLocation::File { path: path.clone() };
                match metadata(&mut provider, &location) {
                    Ok(presets) => discovery.presets.extend(presets),
                    Err(error) => discovery.failed_files.push(PresetFileFailure {
                        path,
                        failure: crate::Error::from(error).failure(),
                    }),
                }
            }
            Ok(discovery)
        }
        location => Ok(PresetDiscovery {
            presets: metadata(&mut provider, location)?,
            failed_files: Vec::new(),
        }),
    }
}

/// The files under `root` that match a declared file type. An empty extension matches any file.
fn files(root: &Path, types: &[PresetFileType]) -> Result<Vec<PathBuf>, ClapError> {
    std::fs::read_dir(root).map_err(|error| ClapError::PresetLocation(error.kind()))?;
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && types.iter().any(|kind| match kind.extension.as_deref() {
                    None | Some("") => true,
                    Some(wanted) => entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted)),
                })
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    files.sort();
    Ok(files)
}

/// The presets the provider reports at one plugin location or file.
fn metadata(
    provider: &mut Provider<Indexer>,
    location: &PresetLocation,
) -> Result<Vec<DiscoveredPreset>, ClapError> {
    let path = location_path(location)?;
    let native = native_location(&path);
    let mut receiver = receiver::Receiver::new(location.clone());
    let raw_receiver = receiver.raw();
    let (kind, path) = native.to_raw();
    // SAFETY: Provider owns its initialized instance, module and indexer for this entire call;
    // receiver and C strings are borrowed for the synchronous callback lifetime.
    let success = unsafe {
        (*provider.as_raw())
            .get_metadata
            .ok_or(ClapError::PresetUnsupported)?(
            provider.as_raw(), kind, path, &raw_receiver
        )
    };
    if let Some(error) = receiver.error {
        return Err(error);
    }
    if !success {
        return Err(ClapError::PresetDiscovery);
    }
    Ok(receiver.presets)
}
pub(crate) fn input_string(value: &str) -> Result<CString, ClapError> {
    if value.is_empty() || value.len() > PRESET_DISCOVERY_TEXT_BYTES {
        return Err(ClapError::PresetInput);
    }
    CString::new(value).map_err(|_| ClapError::PresetInput)
}
pub(crate) fn location_path(location: &PresetLocation) -> Result<Option<CString>, ClapError> {
    match location {
        PresetLocation::Plugin => Ok(None),
        PresetLocation::File { path } => {
            if !path.is_absolute() {
                return Err(ClapError::PresetInput);
            }
            input_string(path.to_str().ok_or(ClapError::PresetInput)?).map(Some)
        }
    }
}
pub(crate) fn native_location(path: &Option<CString>) -> Location<'_> {
    match path {
        None => Location::Plugin,
        Some(path) => Location::File { path },
    }
}
