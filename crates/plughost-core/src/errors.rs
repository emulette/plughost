use serde::{Deserialize, Serialize};
use std::fmt;

/// Format-independent error categories. These describe the failure, not an automatic retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FailureKind {
    InvalidInput,
    Unsupported,
    NotFound,
    Load,
    Configuration,
    NotPrepared,
    Closed,
    RestartRequired,
    EditorOpen,
    StateMismatch,
    State,
    Processing,
    Editor,
    Host,
    Protocol,
    Crashed,
    TimedOut,
}

/// A portable category with the original diagnostic message, including native codes when provided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

const EVENT_CAPACITY: &str =
    "the block or pending edit queue exceeds MAX_BLOCK_EVENTS or MAX_BLOCK_SYSEX_BYTES";
const EVENT_PORT_COUNT: &str = "the event routing exceeds the maximum ports per slot or chain";
const DUPLICATE_EVENT_PORT: &str = "event output port ID occurs more than once in one slot";
const EVENT_ROUTE: &str = "the event route does not name an available source";
const EVENT_PORT: &str = "the event names an event port the chain does not have";
const AUDIO_SLOT_COUNT: &str = "the audio configuration needs one routing entry per plugin";
const AUDIO_BUS_COUNT: &str = "the audio configuration exceeds the maximum buses per direction";
const AUDIO_CHANNEL_COUNT: &str =
    "the audio configuration exceeds the maximum active channels per direction";
const DUPLICATE_AUDIO_BUS: &str = "audio bus ID occurs more than once in one direction";
const AUDIO_BUS_LAYOUT: &str = "an active audio bus must have a nonempty layout";
const AUDIO_ROUTE: &str = "the audio route does not name an available source";
const CHANNEL_ADAPTATION: &str =
    "the explicit channel adaptation does not match the source and target layouts";
const AUDIO_BUFFERS: &str =
    "audio buffers must match the bus channel count and contain equal frame counts";
const AUDIO_SAMPLE_FORMAT: &str =
    "audio sample precision does not match the prepared configuration";
const STATE_SIZE: &str = "plugin component and controller state together exceed the maximum size";
const PARAMETER_CHOICE_PAGE: &str =
    "parameter choice pages need a valid start index and a nonzero entry count within the maximum";
const PRESET_SIZE: &str = "the preset file exceeds the maximum size";
const AUTOMATION: &str =
    "automation must be ordered, normalized, inside the block and address a loaded slot";
const NOT_AUTOMATABLE: &str = "parameter cannot be automated";
const TRANSPORT: &str =
    "transport values must be finite, representable, and describe a valid tempo, meter and loop";
const SAMPLE_RATE: &str = "sample rate must be finite and greater than zero";
const BLOCK_SIZE: &str = "maximum block size must be between 1 and i32::MAX";
const OUTPUT_COUNT: &str = "the configuration needs one output layout per plugin";
const STATE_COUNT: &str = "recovery needs no states or one state per plugin";
const EMPTY_CHAIN: &str = "a chain must contain at least one plugin";
const INVALID_SLOT: &str = "the slot does not contain a loaded plugin";
const PARAMETER_VALUE: &str = "parameter value must be finite and normalized to 0..=1";
const PARAMETER_TEXT: &str =
    "parameter text must be accepted by the plugin, NUL-free and within the maximum UTF-8 bytes";
const PLAIN_PARAMETER_VALUE: &str =
    "plain parameter value must be finite and within the native parameter range";
const UNKNOWN_PARAMETER: &str = "the parameter does not exist";
const READ_ONLY_PARAMETER: &str = "the parameter is read-only";
const HOST_IDENTITY: &str =
    "host identity must be NUL-free, with a non-empty name of at most 127 UTF-16 code units";

/// Messages shared by several plughost crates.
pub const MAIN_BUS: &str = "the plugin has no main audio bus for the requested layout";
pub const OUTPUT_EVENT_CAPACITY: &str = "the plugin produced more events or system exclusive bytes in one block than MAX_BLOCK_EVENTS or MAX_BLOCK_SYSEX_BYTES allow; reset before processing again";
pub const UNKNOWN_EVENT_PORT: &str = "the plugin has no event port with this ID";
pub const UNCONVERTIBLE_OUTPUT_EVENTS: &str = "output events with no portable form (chords, scales, choke, text expression, MIDI 2.0) were not delivered:";
pub const STATE_MISMATCH: &str = "the state belongs to a different plugin class";
pub const BUFFERS: &str = "the buffers do not match the prepared layout or block size";
pub const CHAIN_NOT_PREPARED: &str = "the chain is not prepared";

/// A request rejected before changing the plugin or its processing configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum InputError {
    AudioSlotCount,
    AudioBusCount,
    AudioChannelCount,
    DuplicateAudioBus {
        id: u64,
    },
    AudioBusLayout {
        id: u64,
    },
    AudioRoute {
        slot: usize,
        bus: u64,
    },
    ChannelAdaptation {
        source: crate::Layout,
        target: crate::Layout,
    },
    AudioBuffers,
    AudioSampleFormat,
    StateSize,
    ParameterChoicePage,
    PresetSize,
    Transport,
    Automation,
    NotAutomatable {
        id: u64,
    },
    HostIdentity,
    EmptyChain,
    /// Recovery states neither empty nor one per plugin.
    StateCount,
    SampleRate,
    BlockSize,
    OutputCount,
    MainBus,
    Slot,
    ParameterValue,
    ParameterText,
    PlainParameterValue,
    UnknownParameter {
        id: u64,
    },
    ReadOnlyParameter {
        id: u64,
    },
    EventCapacity,
    EventPortCount,
    DuplicateEventPort {
        id: u64,
    },
    EventRoute {
        slot: usize,
        port: u64,
    },
    EventPort {
        port: usize,
    },
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventCapacity => f.write_str(EVENT_CAPACITY),
            Self::EventPortCount => {
                write!(f, "{EVENT_PORT_COUNT} ({})", crate::MAX_EVENT_PORTS)
            }
            Self::DuplicateEventPort { id } => write!(f, "{DUPLICATE_EVENT_PORT} ({id})"),
            Self::EventRoute { slot, port } => {
                write!(f, "{EVENT_ROUTE} (slot {slot}, input port {port})")
            }
            Self::EventPort { port } => write!(f, "{EVENT_PORT} ({port})"),
            Self::AudioSlotCount => f.write_str(AUDIO_SLOT_COUNT),
            Self::AudioBusCount => write!(f, "{AUDIO_BUS_COUNT} ({})", crate::MAX_AUDIO_BUSES),
            Self::AudioChannelCount => {
                write!(f, "{AUDIO_CHANNEL_COUNT} ({})", crate::MAX_AUDIO_CHANNELS)
            }
            Self::DuplicateAudioBus { id } => write!(f, "{DUPLICATE_AUDIO_BUS} ({id})"),
            Self::AudioBusLayout { id } => write!(f, "{AUDIO_BUS_LAYOUT} ({id})"),
            Self::AudioRoute { slot, bus } => {
                write!(f, "{AUDIO_ROUTE} (slot {slot}, input bus {bus})")
            }
            Self::ChannelAdaptation { source, target } => {
                write!(f, "{CHANNEL_ADAPTATION} ({source:?} to {target:?})")
            }
            Self::AudioBuffers => f.write_str(AUDIO_BUFFERS),
            Self::AudioSampleFormat => f.write_str(AUDIO_SAMPLE_FORMAT),
            Self::StateSize => write!(f, "{STATE_SIZE} ({} MiB)", crate::MAX_STATE_BYTES >> 20),
            Self::ParameterChoicePage => write!(
                f,
                "{PARAMETER_CHOICE_PAGE} ({})",
                crate::PARAMETER_CHOICE_PAGE_SIZE
            ),
            Self::PresetSize => write!(f, "{PRESET_SIZE} ({} MiB)", crate::MAX_PRESET_BYTES >> 20),
            Self::Automation => f.write_str(AUTOMATION),
            Self::NotAutomatable { id } => write!(f, "{NOT_AUTOMATABLE} ({id})"),
            Self::Transport => f.write_str(TRANSPORT),
            Self::HostIdentity => f.write_str(HOST_IDENTITY),
            Self::EmptyChain => f.write_str(EMPTY_CHAIN),
            Self::StateCount => f.write_str(STATE_COUNT),
            Self::SampleRate => f.write_str(SAMPLE_RATE),
            Self::BlockSize => f.write_str(BLOCK_SIZE),
            Self::OutputCount => f.write_str(OUTPUT_COUNT),
            Self::MainBus => f.write_str(MAIN_BUS),
            Self::Slot => f.write_str(INVALID_SLOT),
            Self::ParameterText => write!(f, "{PARAMETER_TEXT} ({})", crate::PARAMETER_TEXT_BYTES),
            Self::PlainParameterValue => f.write_str(PLAIN_PARAMETER_VALUE),
            Self::ParameterValue => f.write_str(PARAMETER_VALUE),
            Self::UnknownParameter { id } => write!(f, "{UNKNOWN_PARAMETER} ({id})"),
            Self::ReadOnlyParameter { id } => write!(f, "{READ_ONLY_PARAMETER} ({id})"),
        }
    }
}

impl std::error::Error for InputError {}

const INPUT_LENGTH: &str = "input channels do not all have the requested number of frames";
const INPUT_CHANNELS: &str = "input channel count does not match the processor";
const EVENTS: &str =
    "events must be valid MIDI messages on existing ports, in offset order, inside the input";
const LATENCY_CHANGED: &str = "the processor's latency changed during rendering";
const TAIL_DURATION: &str = "maximum tail seconds must be finite and non-negative";
const SILENCE_THRESHOLD: &str = "silence threshold must be finite and non-negative";
const SILENCE_HOLD: &str = "silence hold seconds must be finite and non-negative";
const RENDER_LENGTH: &str =
    "render duration or sample buffer length exceeds the representable range";
const TAIL_CHANGED: &str = "the processor tail changed during rendering; re-plan the render";
const SESSION_CLOSED: &str = "the render session is no longer active";
pub(crate) const MESSAGE_TOO_LARGE: &str = "the message exceeds the maximum size";
pub(crate) const MALFORMED_MESSAGE: &str = "the message could not be decoded";

/// Errors from the offline render rules.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderError {
    SessionClosed,
    TailChanged,
    Input(InputError),
    TailDuration,
    SilenceThreshold,
    SilenceHold,
    LengthOverflow,
    InputLength,
    InputChannels { expected: usize, actual: usize },
    Events,
    LatencyChanged { before: u32, after: u32 },
}

impl RenderError {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::TailChanged => FailureKind::Configuration,
            Self::SessionClosed => FailureKind::NotPrepared,
            Self::Input(_)
            | Self::TailDuration
            | Self::SilenceThreshold
            | Self::SilenceHold
            | Self::LengthOverflow
            | Self::InputLength
            | Self::InputChannels { .. }
            | Self::Events => FailureKind::InvalidInput,
            Self::LatencyChanged { .. } => FailureKind::Configuration,
        }
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::TailChanged => f.write_str(TAIL_CHANGED),
            RenderError::SessionClosed => f.write_str(SESSION_CLOSED),
            RenderError::Input(error) => error.fmt(f),
            RenderError::TailDuration => f.write_str(TAIL_DURATION),
            RenderError::SilenceThreshold => f.write_str(SILENCE_THRESHOLD),
            RenderError::SilenceHold => f.write_str(SILENCE_HOLD),
            RenderError::LengthOverflow => f.write_str(RENDER_LENGTH),
            RenderError::InputLength => f.write_str(INPUT_LENGTH),
            RenderError::InputChannels { expected, actual } => {
                write!(f, "{INPUT_CHANNELS} (expected {expected}, got {actual})")
            }
            RenderError::Events => f.write_str(EVENTS),
            RenderError::LatencyChanged { before, after } => {
                write!(f, "{LATENCY_CHANGED} ({before} to {after} samples)")
            }
        }
    }
}

impl std::error::Error for RenderError {}
