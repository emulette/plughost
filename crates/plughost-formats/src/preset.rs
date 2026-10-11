//! Native preset containers. File I/O belongs to the caller; CLAP has no common state file codec.
use crate::Error;
use plughost_core::{InputError, MAX_PRESET_BYTES, PluginFormat, PluginState, PresetInfo};

/// Validated native preset identity and opaque processor/controller payloads.
pub struct DecodedPreset {
    /// Plugin class identity and format-specific container metadata.
    pub info: PresetInfo,
    /// Processor state, or the format's single state blob.
    pub component: Vec<u8>,
    /// Optional VST3 controller state; empty for formats with one state blob.
    pub controller: Vec<u8>,
}

/// Whether this build can encode and decode the format's native preset files.
pub fn supports(format: PluginFormat) -> bool {
    match format {
        #[cfg(feature = "vst3")]
        PluginFormat::Vst3 => true,
        #[cfg(all(feature = "au", target_os = "macos"))]
        PluginFormat::AudioUnit => true,
        _ => false,
    }
}

/// Validates a bounded native preset container without loading a plugin.
pub fn decode(format: PluginFormat, bytes: &[u8]) -> Result<DecodedPreset, Error> {
    if bytes.len() > MAX_PRESET_BYTES {
        return Err(Error::Input(InputError::PresetSize));
    }
    match format {
        #[cfg(feature = "vst3")]
        PluginFormat::Vst3 => {
            let preset = crate::vst3::Preset::from_bytes(bytes)?;
            Ok(DecodedPreset {
                info: PresetInfo::new(
                    preset.class_id,
                    plughost_core::PresetMetadata::Vst3 { info: preset.info },
                ),
                component: preset.component,
                controller: preset.controller,
            })
        }
        #[cfg(all(feature = "au", target_os = "macos"))]
        PluginFormat::AudioUnit => Ok(DecodedPreset {
            info: crate::au::inspect_preset(bytes)?,
            component: crate::au::from_preset(bytes)?,
            controller: Vec::new(),
        }),
        _ => Err(Error::PresetUnsupported),
    }
}

/// Encodes current native state. VST3 XML annotations belong to the caller, not plugin state;
/// use `vst3::Preset` when writing caller-owned annotations.
pub fn encode(state: &PluginState) -> Result<Vec<u8>, Error> {
    match state.format {
        #[cfg(feature = "vst3")]
        PluginFormat::Vst3 => bounded(crate::vst3::Preset::from_state(state).to_bytes()?),
        #[cfg(all(feature = "au", target_os = "macos"))]
        PluginFormat::AudioUnit => bounded(crate::au::to_preset(&state.component)?),
        _ => Err(Error::PresetUnsupported),
    }
}

#[cfg(any(feature = "vst3", all(feature = "au", target_os = "macos")))]
fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    if bytes.len() > MAX_PRESET_BYTES {
        return Err(Error::Input(InputError::PresetSize));
    }
    Ok(bytes)
}
