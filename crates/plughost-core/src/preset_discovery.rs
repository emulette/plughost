//! CLAP provider discovery data. No common CLAP preset file encoding is implied.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PRESET_DISCOVERY_TEXT_BYTES: usize = 16384;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetLocation {
    Plugin,
    File { path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDiscoveryTarget {
    pub provider_id: String,
    pub location: PresetLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFileType {
    pub name: String,
    pub description: Option<String>,
    pub extension: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetLocationInfo {
    pub name: String,
    pub flags: u32,
    pub location: PresetLocation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetSoundpack {
    pub id: String,
    pub name: String,
    pub flags: u32,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub vendor: Option<String>,
    pub image_path: Option<String>,
    pub release_timestamp: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetProviderInfo {
    pub id: String,
    pub name: String,
    pub vendor: Option<String>,
    pub file_types: Vec<PresetFileType>,
    pub locations: Vec<PresetLocationInfo>,
    pub soundpacks: Vec<PresetSoundpack>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetPluginId {
    pub abi: String,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPreset {
    /// The plugin location, or the file that holds this preset; load it with this location.
    pub location: PresetLocation,
    pub name: Option<String>,
    pub load_key: Option<String>,
    pub plugin_ids: Vec<PresetPluginId>,
    pub soundpack_id: Option<String>,
    pub flags: Option<u32>,
    pub creators: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<u64>,
    pub modification_time: Option<u64>,
    pub features: Vec<String>,
    pub extra_info: Vec<(String, String)>,
}

/// Presets found at one provider location. A file in a crawled directory that the provider
/// refused is listed with its failure instead of failing the other files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDiscovery {
    pub presets: Vec<DiscoveredPreset>,
    pub failed_files: Vec<PresetFileFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFileFailure {
    pub path: PathBuf,
    pub failure: crate::Failure,
}
