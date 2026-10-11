use serde::{Deserialize, Serialize};

use crate::plugin::PluginFormat;

/// A plugin's saved state with the identity of the class it belongs to. Where it is stored is up
/// to the application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginState {
    /// Native plugin format owning the state payload.
    pub format: PluginFormat,
    /// Native class ID identifying the plugin that may restore this state.
    pub class_id: String,
    /// Plugin display name captured with the state.
    pub name: String,
    /// Plugin vendor captured with the state.
    pub vendor: String,
    /// Plugin version captured with the state.
    pub version: String,
    /// The processor's state. Formats with a single state blob store it here.
    pub component: Vec<u8>,
    /// The VST3 edit controller's own state; empty for other formats.
    pub controller: Vec<u8>,
}

/// Maximum aggregate processor and controller payload for one project state (128 MiB).
/// This does not include the plugin's identity strings or serialization framing.
pub const MAX_STATE_BYTES: usize = 128 << 20;

impl PluginState {
    /// Checks the opaque payload before it is cloned or handed to a native plugin.
    pub fn validate(&self) -> Result<(), crate::InputError> {
        if self.component.len().saturating_add(self.controller.len()) > MAX_STATE_BYTES {
            return Err(crate::InputError::StateSize);
        }
        Ok(())
    }
}

/// Why a state is being saved or applied. Unsupported native contexts are explicit errors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StatePurpose {
    /// Save or restore persistent project state. This is the default.
    #[default]
    Project,
    /// Save or restore a reusable preset.
    Preset,
    /// Transfer state to a duplicate instance.
    Duplicate,
}

/// Bound for encoded preset files, separate from opaque project state and the IPC frame limit.
pub const MAX_PRESET_BYTES: usize = 64 << 20;

/// Native file metadata, kept separate from a plugin's opaque state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PresetMetadata {
    /// The VST3 Info chunk is opaque XML bytes, preserved without interpreting vendor attributes.
    Vst3 {
        /// Uninterpreted XML bytes from the native `Info` chunk.
        info: Vec<u8>,
    },
    /// Metadata from an Audio Unit property-list preset.
    AudioUnit {
        /// Native preset name, if supplied.
        name: Option<String>,
    },
}

/// Plugin identity and metadata read from a native preset container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PresetInfo {
    /// Native class ID identifying the plugin that may restore this state.
    pub class_id: String,
    /// Format-specific container metadata, separate from DSP state.
    pub metadata: PresetMetadata,
}

impl PresetInfo {
    /// Constructs decoded preset identity and native metadata.
    pub fn new(class_id: impl Into<String>, metadata: PresetMetadata) -> PresetInfo {
        PresetInfo {
            class_id: class_id.into(),
            metadata,
        }
    }
}
