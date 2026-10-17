use serde::{Deserialize, Serialize};
use std::fmt;

/// Format-independent error categories. These describe the failure, not an automatic retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FailureKind {
    /// Caller input violated a host contract.
    InvalidInput,
    /// The format or plugin does not implement the requested operation.
    Unsupported,
    /// The requested plugin or native object was not found.
    NotFound,
    /// A module or plugin instance could not be loaded or initialized.
    Load,
    /// The requested processing configuration could not be applied.
    Configuration,
    /// Processing was requested before successful preparation.
    NotPrepared,
    /// The instance or its owning handle is closed.
    Closed,
    /// Processing cannot continue until the reported restart condition is handled.
    RestartRequired,
    /// The operation requires the editor to be closed.
    EditorOpen,
    /// The saved state belongs to another plugin identity.
    StateMismatch,
    /// Native state could not be saved, decoded, or restored.
    State,
    /// A plugin processing call or its output failed validation.
    Processing,
    /// A native editor operation failed.
    Editor,
    /// The host could not complete a platform or resource operation.
    Host,
    /// A helper response or shared-memory message violated the internal protocol.
    Protocol,
    /// The helper exited unexpectedly or was terminated.
    Crashed,
    /// An operation exceeded its configured deadline.
    TimedOut,
}

/// A portable category with the original diagnostic message, including native codes when provided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// Portable category for programmatic handling.
    pub kind: FailureKind,
    /// Human-readable diagnostic; do not parse it as a stable error code.
    pub message: String,
}

impl Failure {
    /// Constructs a category and diagnostic report.
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
const BLOCK_EVENTS: &str = "events must be notes, expressions in their ranges, MIDI 1.0 channel messages or complete \
     system exclusive messages, in offset order and inside the block";
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
/// Diagnostic for excessive native output; reset is required before further processing.
pub const OUTPUT_EVENT_CAPACITY: &str = "the plugin produced more events or system exclusive bytes in one block than MAX_BLOCK_EVENTS or MAX_BLOCK_SYSEX_BYTES allow; reset before processing again";
/// Diagnostic for a missing native event port.
pub const UNKNOWN_EVENT_PORT: &str = "the plugin has no event port with this ID";
/// Prefix identifying processing-path audit diagnostics.
pub const AUDIT: &str = "real-time audit:";
/// Diagnostic prefix for output event kinds with no portable representation.
pub const UNCONVERTIBLE_OUTPUT_EVENTS: &str = "output events with no portable form (chords, scales, choke, text expression, MIDI 2.0) were not delivered:";
/// Diagnostic for state belonging to another plugin class.
pub const STATE_MISMATCH: &str = "the state belongs to a different plugin class";
/// Diagnostic for mismatched audio buffers or block length.
pub const BUFFERS: &str = "the buffers do not match the prepared layout or block size";
/// Diagnostic for a chain without a prepared processing configuration.
pub const CHAIN_NOT_PREPARED: &str = "the chain is not prepared";

/// A request rejected before changing the plugin or its processing configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum InputError {
    /// Routing entries do not match the number of loaded plugin slots.
    AudioSlotCount,
    /// A direction exceeds `MAX_AUDIO_BUSES`.
    AudioBusCount,
    /// A direction exceeds `MAX_AUDIO_CHANNELS` active channels.
    AudioChannelCount,
    /// A native bus ID occurs more than once in one direction.
    DuplicateAudioBus {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// An active bus requested the empty layout.
    AudioBusLayout {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// An active input bus names a missing or inactive source.
    AudioRoute {
        /// Zero-based destination plugin slot.
        slot: usize,
        /// Native destination input bus ID.
        bus: u64,
    },
    /// The requested conversion does not match the two layouts.
    ChannelAdaptation {
        /// Layout supplied by the source.
        source: crate::Layout,
        /// Layout requested by the destination.
        target: crate::Layout,
    },
    /// Channel counts or per-channel frame lengths do not match the prepared buses.
    AudioBuffers,
    /// Sample precision differs from the prepared configuration.
    AudioSampleFormat,
    /// Aggregate component and controller state exceeds `MAX_STATE_BYTES`.
    StateSize,
    /// A discrete-choice page has an invalid start index or count.
    ParameterChoicePage,
    /// Native preset bytes exceed `MAX_PRESET_BYTES`.
    PresetSize,
    /// Transport contains a non-finite, unrepresentable, or inconsistent value.
    Transport,
    /// Automation is unordered, out of range, outside the block, or names a missing slot.
    Automation,
    /// The addressed parameter does not support sample-timed automation.
    NotAutomatable {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// Host text contains NUL, or its name is empty or exceeds 127 UTF-16 code units.
    HostIdentity,
    /// A chain was created or configured without a plugin.
    EmptyChain,
    /// Recovery states neither empty nor one per plugin.
    StateCount,
    /// The sample rate is not finite and greater than zero.
    SampleRate,
    /// Maximum block size is outside `1..=i32::MAX`.
    BlockSize,
    /// The legacy configuration does not provide one output layout per plugin.
    OutputCount,
    /// The plugin has no main audio bus with the requested layout.
    MainBus,
    /// The slot index does not address a loaded plugin.
    Slot,
    /// The parameter value is not finite and normalized to `0..=1`.
    ParameterValue,
    /// Display text is excessive, contains NUL, or is rejected by the plugin.
    ParameterText,
    /// The native-unit parameter value is non-finite or outside the native range.
    PlainParameterValue,
    /// The requested parameter ID does not exist.
    UnknownParameter {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// A host edit addressed a read-only parameter.
    ReadOnlyParameter {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// Combined automation/events or SysEx bytes exceed the per-block capacity.
    EventCapacity,
    /// An event routing direction exceeds `MAX_EVENT_PORTS`.
    EventPortCount,
    /// An output port ID occurs more than once in a slot.
    DuplicateEventPort {
        /// Native ID associated with the invalid request.
        id: u64,
    },
    /// An event input route names an unavailable source.
    EventRoute {
        /// Zero-based destination plugin slot.
        slot: usize,
        /// Native destination event input port ID.
        port: u64,
    },
    /// An event names an external port the chain does not expose.
    EventPort {
        /// Invalid zero-based external event port index.
        port: usize,
    },
    /// Events that are not valid, in offset order, or inside the block.
    Events,
    /// A bundle or preset path cannot be represented by the helper's Unicode path protocol.
    PathEncoding,
    /// A realtime quantum is zero frames or exceeds the prepared maximum block size.
    RealtimeQuantum,
    /// A realtime latency is not one, two or three quanta.
    RealtimeLatency,
    /// A dry fallback needs as many output channels as input channels.
    RealtimeDry,
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
            Self::Events => f.write_str(BLOCK_EVENTS),
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
            Self::PathEncoding => f.write_str(PATH_ENCODING),
            Self::RealtimeQuantum => f.write_str(REALTIME_QUANTUM),
            Self::RealtimeLatency => f.write_str(REALTIME_LATENCY),
            Self::RealtimeDry => f.write_str(REALTIME_DRY),
        }
    }
}

impl std::error::Error for InputError {}

const PATH_ENCODING: &str = "plugin bundle and preset paths must be valid Unicode";
const REALTIME_QUANTUM: &str =
    "a realtime quantum must be at least one frame and at most the prepared maximum block size";
const REALTIME_LATENCY: &str = "a realtime latency must be one, two or three quanta";
const REALTIME_DRY: &str = "a dry fallback needs as many output channels as input channels";

const INPUT_LENGTH: &str = "input channels do not all have the requested number of frames";
const INPUT_CHANNELS: &str = "input channel count does not match the processor";
const EVENTS: &str = "events must be valid, in offset order, inside the input";
const LATENCY_CHANGED: &str = "the processor's latency changed during rendering";
const LATENCY_NOT_APPLIED: &str = "a plugin announced a new latency that is not applied yet; reset \
     or prepare the chain again before rendering";
const TAIL_DURATION: &str = "maximum tail seconds must be finite and non-negative";
const SILENCE_THRESHOLD: &str = "silence threshold must be finite and non-negative";
const SILENCE_HOLD: &str = "silence hold seconds must be finite and non-negative";
const RENDER_LENGTH: &str =
    "render duration or sample buffer length exceeds the representable range";
const SESSION_CLOSED: &str = "the render session is no longer active";
pub(crate) const MESSAGE_TOO_LARGE: &str = "the message exceeds the maximum size";
pub(crate) const MALFORMED_MESSAGE: &str = "the message could not be decoded";

/// Errors from the offline render rules.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderError {
    /// The streaming render session has already finished, been cancelled or failed.
    SessionClosed,
    /// A shared processing or control input contract was violated.
    Input(InputError),
    /// Maximum tail duration is not finite and non-negative.
    TailDuration,
    /// Silence threshold is not finite and non-negative.
    SilenceThreshold,
    /// Silence hold duration is not finite and non-negative.
    SilenceHold,
    /// Render duration or sample storage exceeds the representable range.
    LengthOverflow,
    /// Input channels do not contain the requested equal frame count.
    InputLength,
    /// The supplied channel count differs from the prepared processor.
    InputChannels {
        /// Prepared channel count.
        expected: usize,
        /// Supplied channel count.
        actual: usize,
    },
    /// Events are invalid, unordered, or outside the render input. Events on ports the
    /// processor lacks fail its [`Process::validate_events`](crate::render::Process::validate_events).
    Events,
    /// The processor’s latency changed during a render.
    LatencyChanged {
        /// Latency at render start, in frames.
        before: u32,
        /// New latency, in frames.
        after: u32,
    },
    /// A plugin announced a new latency before the render started, and the processor has not
    /// applied it yet.
    LatencyNotApplied,
}

impl RenderError {
    /// Classifies a render error without deriving a retry policy.
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::SessionClosed => FailureKind::Closed,
            Self::Input(_)
            | Self::TailDuration
            | Self::SilenceThreshold
            | Self::SilenceHold
            | Self::LengthOverflow
            | Self::InputLength
            | Self::InputChannels { .. }
            | Self::Events => FailureKind::InvalidInput,
            Self::LatencyChanged { .. } => FailureKind::Configuration,
            Self::LatencyNotApplied => FailureKind::RestartRequired,
        }
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            RenderError::LatencyNotApplied => f.write_str(LATENCY_NOT_APPLIED),
        }
    }
}

impl std::error::Error for RenderError {}
