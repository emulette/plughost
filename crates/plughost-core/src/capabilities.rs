use serde::{Deserialize, Serialize};

/// Evidence from a native query, not a guarantee that an operation will succeed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Support {
    /// The format has no safe query, or the query did not determine support.
    #[default]
    Unknown,
    /// The native query explicitly reported that the feature is unavailable.
    Unsupported,
    /// The native query reported support for the current configuration.
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
    /// Producing note on/off events; other event kinds require their own port capabilities.
    pub note_output: Support,
    /// Applying normalized parameter edits at sample offsets within a block.
    pub sample_accurate_automation: Support,
    /// Saving and restoring native plugin state.
    pub state: Support,
    /// Enumerating factory programs/presets. File-based preset loading is a separate operation.
    pub factory_presets: Support,
    /// Processing all exposed audio ports in this precision. Layout negotiation may still fail.
    pub f32: Support,
    /// Processing all exposed audio ports in double precision; layout negotiation can still fail.
    pub f64: Support,
}

/// Native plugin evidence and the isolated Chain API's implementation for the queried slot.
/// A feature needs both sides to support it. This does not describe the in-process API's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CapabilityReport {
    /// Features reported by the native plugin.
    pub plugin: Capabilities,
    /// Features implemented by the isolated host path.
    pub host: Capabilities,
}

impl CapabilityReport {
    /// Combines independent plugin evidence and host implementation support.
    pub fn new(plugin: Capabilities, host: Capabilities) -> CapabilityReport {
        CapabilityReport { plugin, host }
    }
}
