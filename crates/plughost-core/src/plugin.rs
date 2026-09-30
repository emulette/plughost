use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst3,
    /// Audio Unit (macOS), v2 or v3.
    AudioUnit,
    Clap,
}

/// A plugin class to load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRef {
    pub format: PluginFormat,
    /// The bundle that contains the class. Audio Units have none; the system registry finds them.
    pub bundle: Option<PathBuf>,
    pub class_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginKind {
    Effect,
    Instrument,
}

/// A plugin class as its format describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub format: PluginFormat,
    /// The format's class identifier. For VST3 this is the 32-digit hexadecimal class ID in the
    /// same form `.vstpreset` files use. For Audio Units it is the component type, subtype, and
    /// manufacturer codes as 24 hexadecimal digits. For CLAP it is the plugin ID.
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    pub kind: PluginKind,
    /// Format sub-categories, for example `Fx` and `EQ`.
    pub categories: Vec<String>,
}
