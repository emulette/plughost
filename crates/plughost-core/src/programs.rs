use serde::{Deserialize, Serialize};

/// Maximum native factory program catalog returned by one plugin instance.
pub const MAX_FACTORY_PRESETS: usize = 4096;

/// Native factory program identity, scoped to one plugin class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FactoryPresetId {
    Vst3 {
        unit_id: i32,
        list_id: i32,
        program_index: i32,
    },
    AudioUnit {
        number: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FactoryPreset {
    pub id: FactoryPresetId,
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
