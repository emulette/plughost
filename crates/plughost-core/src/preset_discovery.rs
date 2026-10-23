//! CLAP provider discovery data. No common CLAP preset file encoding is implied.
use std::path::PathBuf;

/// Maximum UTF-8 bytes accepted for one provider text field.
pub const PRESET_DISCOVERY_TEXT_BYTES: usize = 16384;

bitflags::bitflags! {
    /// What a CLAP preset provider says of a location's, soundpack's or preset's content. Bits
    /// that later CLAP versions define are kept, though they have no name here.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct PresetFlags: u32 {
        /// Content that comes with the plugin.
        const FACTORY_CONTENT = 1 << 0;
        /// Content the user made.
        const USER_CONTENT = 1 << 1;
        /// Content for demonstration.
        const DEMO_CONTENT = 1 << 2;
        /// Content the user marked as a favorite.
        const FAVORITE = 1 << 3;
    }
}

/// Native CLAP provider location used both for discovery and preset loading.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetDiscoveryTarget {
    /// Native provider ID within its CLAP module.
    pub provider_id: String,
    /// Internal plugin storage or the provider’s filesystem location.
    pub location: PresetLocation,
}

impl PresetDiscoveryTarget {
    /// `location` searched with the provider `provider_id`.
    pub fn new(provider_id: impl Into<String>, location: PresetLocation) -> PresetDiscoveryTarget {
        PresetDiscoveryTarget {
            provider_id: provider_id.into(),
            location,
        }
    }

    /// Checks the target before any provider code runs: a provider ID and, for a file location,
    /// an absolute Unicode path, each non-empty, without NUL and within
    /// [`PRESET_DISCOVERY_TEXT_BYTES`].
    pub fn validate(&self) -> Result<(), crate::InputError> {
        let text = |value: &str| {
            !value.is_empty() && value.len() <= PRESET_DISCOVERY_TEXT_BYTES && !value.contains('\0')
        };
        let location = match &self.location {
            PresetLocation::Plugin => true,
            PresetLocation::File { path } => path.is_absolute() && path.to_str().is_some_and(text),
        };
        if text(&self.provider_id) && location {
            Ok(())
        } else {
            Err(crate::InputError::PresetDiscoveryTarget)
        }
    }
}

/// File type advertised by a CLAP preset provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetFileType {
    /// Native display name.
    pub name: String,
    /// Optional native descriptive text.
    pub description: Option<String>,
    /// File extension advertised by the provider, without a leading dot.
    pub extension: Option<String>,
}

impl PresetFileType {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(
        name: String,
        description: Option<String>,
        extension: Option<String>,
    ) -> PresetFileType {
        PresetFileType {
            name,
            description,
            extension,
        }
    }
}
/// Named discovery location advertised by a CLAP provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetLocationInfo {
    /// Native display name.
    pub name: String,
    /// What the provider says of the location's content.
    pub flags: PresetFlags,
    /// Internal plugin storage or the provider’s filesystem location.
    pub location: PresetLocation,
}

impl PresetLocationInfo {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(name: String, flags: PresetFlags, location: PresetLocation) -> PresetLocationInfo {
        PresetLocationInfo {
            name,
            flags,
            location,
        }
    }
}
/// Native metadata for a provider’s soundpack.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetSoundpack {
    /// Native identifier, scoped to the owning provider or ABI.
    pub id: String,
    /// Native display name.
    pub name: String,
    /// What the provider says of the soundpack's content.
    pub flags: PresetFlags,
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

impl PresetSoundpack {
    /// A soundpack with no optional metadata yet, which the fields then take. Internal to the
    /// plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(id: String, name: String, flags: PresetFlags) -> PresetSoundpack {
        PresetSoundpack {
            id,
            name,
            flags,
            description: None,
            homepage_url: None,
            vendor: None,
            image_path: None,
            release_timestamp: None,
        }
    }
}
/// A CLAP module’s preset provider and its advertised discovery metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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

impl PresetProviderInfo {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(
        id: String,
        name: String,
        vendor: Option<String>,
        file_types: Vec<PresetFileType>,
        locations: Vec<PresetLocationInfo>,
        soundpacks: Vec<PresetSoundpack>,
    ) -> PresetProviderInfo {
        PresetProviderInfo {
            id,
            name,
            vendor,
            file_types,
            locations,
            soundpacks,
        }
    }
}
/// Native plugin ABI and identity to which a discovered preset applies.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetPluginId {
    /// Provider-supplied ABI name, such as `clap`.
    pub abi: String,
    /// Native identifier, scoped to the owning provider or ABI.
    pub id: String,
}

impl PresetPluginId {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(abi: String, id: String) -> PresetPluginId {
        PresetPluginId { abi, id }
    }
}
/// One native preset discovered by a CLAP provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// What the provider says of the preset, or `None` if it did not say.
    pub flags: Option<PresetFlags>,
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

impl DiscoveredPreset {
    /// A preset at `location` with no metadata yet; the fields take what the provider reports.
    pub fn new(location: PresetLocation) -> DiscoveredPreset {
        DiscoveredPreset {
            location,
            name: None,
            load_key: None,
            plugin_ids: Vec::new(),
            soundpack_id: None,
            flags: None,
            creators: Vec::new(),
            description: None,
            creation_time: None,
            modification_time: None,
            features: Vec::new(),
            extra_info: Vec::new(),
        }
    }
}

/// Presets found at one provider location. A file in a crawled directory that the provider
/// refused is listed with its failure instead of failing the other files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetDiscovery {
    /// Successfully discovered presets, in observed order.
    pub presets: Vec<DiscoveredPreset>,
    /// Individual files refused by the provider during directory discovery.
    pub failed_files: Vec<PresetFileFailure>,
}

impl PresetDiscovery {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(
        presets: Vec<DiscoveredPreset>,
        failed_files: Vec<PresetFileFailure>,
    ) -> PresetDiscovery {
        PresetDiscovery {
            presets,
            failed_files,
        }
    }
}

/// One preset file’s discovery failure, without failing the surrounding directory.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresetFileFailure {
    /// Preset file that could not be inspected.
    pub path: PathBuf,
    /// Portable failure category and native diagnostic.
    pub failure: crate::Failure,
}

impl PresetFileFailure {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(path: PathBuf, failure: crate::Failure) -> PresetFileFailure {
        PresetFileFailure { path, failure }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_takes_a_provider_id_and_an_absolute_unicode_path_within_the_text_limit() {
        let file = |path: PathBuf| PresetLocation::File { path };
        let absolute = std::env::temp_dir().join("preset.phcp");
        for (provider, location) in [
            ("provider", PresetLocation::Plugin),
            ("provider", file(absolute.clone())),
        ] {
            assert_eq!(
                PresetDiscoveryTarget::new(provider, location).validate(),
                Ok(())
            );
        }
        let long = "x".repeat(PRESET_DISCOVERY_TEXT_BYTES + 1);
        for (provider, location) in [
            ("", PresetLocation::Plugin),
            ("pro\0vider", PresetLocation::Plugin),
            (long.as_str(), PresetLocation::Plugin),
            ("provider", file("relative.phcp".into())),
            ("provider", file(std::env::temp_dir().join("pre\0set.phcp"))),
        ] {
            assert_eq!(
                PresetDiscoveryTarget::new(provider, location).validate(),
                Err(crate::InputError::PresetDiscoveryTarget)
            );
        }
    }
}
