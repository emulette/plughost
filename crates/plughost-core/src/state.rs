use serde::{Deserialize, Serialize};

use crate::plugin::{PluginId, PluginInfo};

mod encoding;

/// A plugin's saved state with the identity of the class it belongs to. Its contents are opaque:
/// store it with [`PluginState::to_bytes`] and read it back with [`PluginState::from_bytes`],
/// which later plughost versions keep reading. Where it is stored is up to the
/// application.
///
/// The name, vendor and version are those of the plugin that saved it, for display: a restore
/// checks only the [`PluginId`], so a state saved by another version of the plugin restores, and
/// the plugin decides what to make of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginState {
    pub(crate) id: PluginId,
    pub(crate) name: String,
    pub(crate) vendor: String,
    pub(crate) version: String,
    /// The processor's state, or the single state blob of formats that have one.
    pub(crate) component: Vec<u8>,
    /// The VST3 edit controller's own state, when it has one; `None` for other formats.
    pub(crate) controller: Option<Vec<u8>>,
}

/// Maximum aggregate processor and controller payload for one project state (128 MiB).
/// This does not include the plugin's identity strings or serialization framing.
pub const MAX_STATE_BYTES: usize = 128 << 20;

impl PluginState {
    /// A state of the loaded plugin `info` describes, which names its identity, with a
    /// `controller` state only for VST3 and a payload of at most [`MAX_STATE_BYTES`]. Internal to
    /// the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(info: &PluginInfo, component: Vec<u8>, controller: Option<Vec<u8>>) -> PluginState {
        PluginState {
            id: info.id.clone(),
            name: info.name.clone(),
            vendor: info.vendor.clone(),
            version: info.version.clone(),
            component,
            controller,
        }
    }

    /// The plugin class that may restore this state.
    pub fn id(&self) -> &PluginId {
        &self.id
    }

    /// The display name of the plugin that saved the state.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The vendor of the plugin that saved the state, or empty when it gave none.
    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    /// The version of the plugin that saved the state, or empty when it gave none.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The processor's state, or the format's single state blob. Internal to the plughost crates;
    /// exempt from semver.
    #[doc(hidden)]
    pub fn component(&self) -> &[u8] {
        &self.component
    }

    /// The VST3 edit controller's state, if it saved one. Internal to the plughost crates; exempt
    /// from semver.
    #[doc(hidden)]
    pub fn controller(&self) -> Option<&[u8]> {
        self.controller.as_deref()
    }

    /// Checks the opaque payload before it is cloned or handed to a native plugin.
    pub fn validate(&self) -> Result<(), crate::InputError> {
        let controller = self.controller.as_ref().map_or(0, Vec::len);
        if self.component.len().saturating_add(controller) > MAX_STATE_BYTES {
            return Err(crate::InputError::StateSize);
        }
        Ok(())
    }

    /// The state in plughost's persisted form: a versioned container of tagged sections for the
    /// identity and each native payload, independent of the helper protocol. Store these bytes;
    /// [`PluginState::from_bytes`] reads them in this and later plughost versions.
    pub fn to_bytes(&self) -> Vec<u8> {
        encoding::encode(self)
    }

    /// Reads bytes [`PluginState::to_bytes`] wrote, in this or an earlier plughost version from
    /// 0.2.0 on,
    /// without a plugin, so a state of a plugin that is not installed can be kept and written
    /// again unchanged. Bytes that are not such a state fail with [`crate::InputError::StateBytes`],
    /// and a payload over [`MAX_STATE_BYTES`] with [`crate::InputError::StateSize`], before
    /// anything of that size is allocated.
    pub fn from_bytes(bytes: &[u8]) -> Result<PluginState, crate::InputError> {
        encoding::decode(bytes)
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
