use serde::{Deserialize, Serialize};

/// Evidence from a native query, not a guarantee that an operation will succeed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Support {
    /// The format has no safe query, or the query did not determine support.
    #[default]
    Unknown,
    Unsupported,
    Supported,
}

impl From<bool> for Support {
    fn from(supported: bool) -> Self {
        if supported {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}

/// A snapshot for the current platform and plugin configuration. Static scan metadata does not
/// establish these capabilities. Unknown is distinct from explicitly unsupported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Capabilities {
    /// A native editor embedded in the helper's parent window (not a floating-only editor).
    pub embedded_editor: Support,
    /// Enumerating detailed audio bus descriptions, not arbitrary layout acceptance.
    pub bus_discovery: Support,
    /// Note on/off input and output; does not imply support for all MIDI messages or dialects.
    pub note_input: Support,
    pub note_output: Support,
    pub sample_accurate_automation: Support,
    pub state: Support,
    /// Enumerating factory programs/presets. File-based preset loading is a separate operation.
    pub factory_presets: Support,
    /// Processing all exposed audio ports in this precision. Layout negotiation may still fail.
    pub f32: Support,
    pub f64: Support,
}

/// Native plugin evidence and the isolated Chain API's implementation for the queried slot.
/// A feature needs both sides to support it. This does not describe the in-process API's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CapabilityReport {
    pub plugin: Capabilities,
    pub host: Capabilities,
}

impl CapabilityReport {
    pub fn new(plugin: Capabilities, host: Capabilities) -> CapabilityReport {
        CapabilityReport { plugin, host }
    }
}
