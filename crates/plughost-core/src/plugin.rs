use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Native plugin ABI used for discovery and loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PluginFormat {
    /// Steinberg VST 3.
    Vst3,
    /// Audio Unit (macOS), v2 or v3.
    AudioUnit,
    /// CLever Audio Plug-in (CLAP).
    Clap,
}

/// A plugin class to load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRef {
    /// Native plugin format.
    pub format: PluginFormat,
    /// The bundle that contains the class. Audio Units have none; the system registry finds them.
    pub bundle: Option<PathBuf>,
    /// Format-specific class identity; see `PluginInfo::class_id` for encoding.
    pub class_id: String,
}

/// Broad native classification used in discovery results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PluginKind {
    /// An audio or MIDI effect.
    Effect,
    /// A sound-generating instrument.
    Instrument,
}

/// A plugin class as its format describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PluginInfo {
    /// Native plugin format.
    pub format: PluginFormat,
    /// The format's class identifier. For VST3 this is the 32-digit hexadecimal class ID in the
    /// same form `.vstpreset` files use. For Audio Units it is the component type, subtype, and
    /// manufacturer codes as 24 hexadecimal digits. For CLAP it is the plugin ID.
    pub class_id: String,
    /// Native plugin display name.
    pub name: String,
    /// Native vendor name, or empty when unavailable.
    pub vendor: String,
    /// Native plugin version string, or empty when unavailable.
    pub version: String,
    /// Native SDK version string, or empty when unavailable.
    pub sdk_version: String,
    /// Native effect or instrument classification.
    pub kind: PluginKind,
    /// Format sub-categories, for example `Fx` and `EQ`.
    pub categories: Vec<String>,
}

impl PluginInfo {
    /// A class with no vendor, version, SDK version or categories yet.
    pub fn new(
        format: PluginFormat,
        class_id: impl Into<String>,
        name: impl Into<String>,
        kind: PluginKind,
    ) -> PluginInfo {
        PluginInfo {
            format,
            class_id: class_id.into(),
            name: name.into(),
            vendor: String::new(),
            version: String::new(),
            sdk_version: String::new(),
            kind,
            categories: Vec::new(),
        }
    }
}
