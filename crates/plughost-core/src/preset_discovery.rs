//! CLAP provider discovery data. No common CLAP preset file encoding is implied.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum UTF-8 bytes accepted for one provider text field.
pub const PRESET_DISCOVERY_TEXT_BYTES: usize = 16384;

/// Native CLAP provider location used both for discovery and preset loading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetLocation {
    /// Presets stored internally by the plugin rather than in a filesystem location.
    Plugin,
    /// A preset file or directory exposed by a provider.
    File {
        /// Native preset file or directory path.
        path: PathBuf,
    },
}

/// One provider and location to inspect without loading an audio instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDiscoveryTarget {
    /// Native provider ID within its CLAP module.
    pub provider_id: String,
    /// Internal plugin storage or the provider’s filesystem location.
    pub location: PresetLocation,
}

/// File type advertised by a CLAP preset provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFileType {
    /// Native display name.
    pub name: String,
    /// Optional native descriptive text.
    pub description: Option<String>,
    /// File extension advertised by the provider, without a leading dot.
    pub extension: Option<String>,
}
/// Named discovery location advertised by a CLAP provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetLocationInfo {
    /// Native display name.
    pub name: String,
    /// Native CLAP preset-discovery flags, preserved as a bit set.
    pub flags: u32,
    /// Internal plugin storage or the provider’s filesystem location.
    pub location: PresetLocation,
}
/// Native metadata for a provider’s soundpack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetSoundpack {
    /// Native identifier, scoped to the owning provider or ABI.
    pub id: String,
    /// Native display name.
    pub name: String,
    /// Native CLAP preset-discovery flags, preserved as a bit set.
    pub flags: u32,
    /// Optional native descriptive text.
    pub description: Option<String>,
    /// Provider-supplied homepage URL; not fetched by discovery.
    pub homepage_url: Option<String>,
    /// Native vendor name, when supplied.
    pub vendor: Option<String>,
    /// Provider-supplied artwork path, when supplied.
    pub image_path: Option<String>,
    /// Release time in seconds since the Unix epoch, when supplied.
    pub release_timestamp: Option<u64>,
}
/// A CLAP module’s preset provider and its advertised discovery metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetProviderInfo {
    /// Native identifier, scoped to the owning provider or ABI.
    pub id: String,
    /// Native display name.
    pub name: String,
    /// Native vendor name, when supplied.
    pub vendor: Option<String>,
    /// Native file types accepted by this provider.
    pub file_types: Vec<PresetFileType>,
    /// Locations this provider offers for discovery.
    pub locations: Vec<PresetLocationInfo>,
    /// Soundpacks advertised by this provider.
    pub soundpacks: Vec<PresetSoundpack>,
}
/// Native plugin ABI and identity to which a discovered preset applies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetPluginId {
    /// Provider-supplied ABI name, such as `clap`.
    pub abi: String,
    /// Native identifier, scoped to the owning provider or ABI.
    pub id: String,
}
/// One native preset discovered by a CLAP provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPreset {
    /// The plugin location, or the file that holds this preset; load it with this location.
    pub location: PresetLocation,
    /// Preset display name, when supplied.
    pub name: Option<String>,
    /// Opaque provider key identifying a preset inside its location.
    pub load_key: Option<String>,
    /// Plugin identities to which the provider says this preset applies.
    pub plugin_ids: Vec<PresetPluginId>,
    /// Owning soundpack ID, when supplied.
    pub soundpack_id: Option<String>,
    /// Native flags, or `None` if the provider did not supply them.
    pub flags: Option<u32>,
    /// Creator names in the order reported by the provider.
    pub creators: Vec<String>,
    /// Optional native descriptive text.
    pub description: Option<String>,
    /// Creation time in seconds since the Unix epoch, when supplied.
    pub creation_time: Option<u64>,
    /// Modification time in seconds since the Unix epoch, when supplied.
    pub modification_time: Option<u64>,
    /// Native feature/category strings in provider order.
    pub features: Vec<String>,
    /// Additional native key/value metadata, preserved without interpretation.
    pub extra_info: Vec<(String, String)>,
}

/// Presets found at one provider location. A file in a crawled directory that the provider
/// refused is listed with its failure instead of failing the other files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDiscovery {
    /// Successfully discovered presets, in observed order.
    pub presets: Vec<DiscoveredPreset>,
    /// Individual files refused by the provider during directory discovery.
    pub failed_files: Vec<PresetFileFailure>,
}

/// One preset file’s discovery failure, without failing the surrounding directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFileFailure {
    /// Preset file that could not be inspected.
    pub path: PathBuf,
    /// Portable failure category and native diagnostic.
    pub failure: crate::Failure,
}
