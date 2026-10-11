use serde::{Deserialize, Serialize};

/// Maximum native factory program catalog returned by one plugin instance.
pub const MAX_FACTORY_PRESETS: usize = 4096;

/// Native factory program identity, scoped to one plugin class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FactoryPresetId {
    /// A program belonging to a VST3 unit and program list.
    Vst3 {
        /// Native unit ID owning the program list.
        unit_id: i32,
        /// Native program-list ID.
        list_id: i32,
        /// Zero-based native program index.
        program_index: i32,
    },
    /// An Audio Unit factory preset.
    AudioUnit {
        /// Native Audio Unit preset number.
        number: i64,
    },
}

/// A named native factory preset or program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FactoryPreset {
    /// Format-specific selection token, scoped to the plugin class.
    pub id: FactoryPresetId,
    /// Native preset or program name.
    pub name: String,
    /// Native program-list name when the format groups factory programs.
    pub group: Option<String>,
}

impl FactoryPreset {
    /// A program outside any group.
    pub fn new(id: FactoryPresetId, name: impl Into<String>) -> FactoryPreset {
        FactoryPreset {
            id,
            name: name.into(),
            group: None,
        }
    }
}
