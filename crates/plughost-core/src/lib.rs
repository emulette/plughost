//! Shared types and offline render rules for plughost.

mod audio;
mod capabilities;
pub use audio::{
    AudioBusConfig, AudioBusInfo, AudioBusRole, AudioConfig, AudioConfiguration, AudioDirection,
    AudioInputRoute, AudioSource, BypassState, ChannelAdaptation, MAX_ALIGNMENT_BYTES,
    MAX_AUDIO_BUSES, MAX_AUDIO_CHANNELS, RoutedChainConfig, SlotAudioConfig,
};
mod config;
mod diagnostics;
mod errors;
mod event;
mod event_routing;
mod host;
#[doc(hidden)]
pub mod ipc;
mod parameter;
mod programs;
pub use programs::{FactoryPreset, FactoryPresetId, MAX_FACTORY_PRESETS};
mod parameter_events;
mod plugin;
pub mod render;
mod sample;
mod state;
mod transport;

pub use capabilities::{Capabilities, CapabilityReport, Support};
pub use config::{Layout, ProcessConfig, ProcessMode, SampleFormat};
pub use diagnostics::{
    DIAGNOSTIC_CAPACITY, DIAGNOSTIC_MESSAGE_BYTES, Diagnostic, DiagnosticBatch, DiagnosticBuffer,
    DiagnosticSeverity,
};
pub use errors::{Failure, FailureKind, InputError, RenderError};

/// Diagnostic messages shared by the plughost crates. Internal to plughost, plughost-formats, and
/// plughost-helper; exempt from semver.
#[doc(hidden)]
pub mod messages {
    pub use crate::errors::{
        BUFFERS, CHAIN_NOT_PREPARED, MAIN_BUS, OUTPUT_EVENT_CAPACITY, STATE_MISMATCH,
        UNCONVERTIBLE_OUTPUT_EVENTS, UNKNOWN_EVENT_PORT,
    };
}
pub use event::{
    MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, Message, MidiData, MidiEvent, events_fit, sysex_bytes,
    validate_event_budget, validate_event_count,
};
pub use event_routing::{
    EventConfig, EventInputRoute, EventPortInfo, EventSource, MAX_EVENT_PORTS, SlotEventConfig,
};
pub use host::HostIdentity;
pub use parameter::{AutomationEvent, ParameterChange, ParameterFlags, ParameterInfo, changes_fit};
pub use parameter::{PARAMETER_TEXT_BYTES, validate_normalized, validate_parameter_text};
pub use parameter_events::{
    PARAMETER_EVENT_CAPACITY, ParameterEvent, ParameterEventBatch, ParameterEventBuffer,
};
pub use plugin::{PluginFormat, PluginInfo, PluginKind, PluginRef};
pub use sample::Sample;
pub use state::{MAX_STATE_BYTES, PluginState};
pub use transport::{BarPosition, BlockContext, LoopRegion, TimeSignature, Transport};

mod changes;
pub use changes::{CHANGE_CAPACITY, ChangeBatch, ChangeBuffer, PluginTiming, SlotChange};

pub use state::{MAX_PRESET_BYTES, PresetInfo, PresetMetadata, StatePurpose};

pub use parameter::{
    PARAMETER_CHOICE_PAGE_SIZE, ParameterChoice, ParameterChoicePage, ParameterDetails,
    ParameterGroup, validate_choice_page,
};

mod preset_discovery;
pub use preset_discovery::*;
