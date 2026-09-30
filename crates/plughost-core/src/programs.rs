use serde::{Deserialize, Serialize};

/// Maximum native factory program catalog returned by one plugin instance.
pub const MAX_FACTORY_PRESETS: usize = 4096;

/// Native factory program identity, scoped to one plugin class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct FactoryPreset {
    pub id: FactoryPresetId,
    pub name: String,
    /// Native program-list name when the format groups factory programs.
    pub group: Option<String>,
}
