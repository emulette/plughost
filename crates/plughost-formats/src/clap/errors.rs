use std::fmt;

use plughost_core::{FailureKind, InputError, Layout, SampleFormat};

use crate::errors::{
    ACTIVATE, NO_EDITOR, NOT_PREPARED, PARAMETER_CONVERSION, PARAMETER_RESULT, PROCESS,
    STATE_PURPOSE_UNSUPPORTED, state_too_large,
};
use plughost_core::messages::{
    BUFFERS, OUTPUT_EVENT_CAPACITY, STATE_MISMATCH, UNCONVERTIBLE_OUTPUT_EVENTS, UNKNOWN_EVENT_PORT,
};

const AUDIO_METADATA: &str = "the CLAP plugin returned invalid audio bus metadata";
const AUDIO_UNSUPPORTED: &str = "the CLAP plugin does not support this audio bus operation";
const AUDIO_CONFIGURATION: &str = "the CLAP plugin refused the requested audio bus configuration";
const BYPASS_UNSUPPORTED: &str = "the CLAP plugin does not provide a writable bypass parameter";
const PRESET_UNSUPPORTED: &str = "the CLAP module does not support the requested preset operation";
const PRESET_DISCOVERY: &str = "the CLAP provider failed to discover presets";
const PRESET_METADATA: &str = "the CLAP provider returned malformed preset metadata";
pub(super) const PRESET_LOAD: &str = "the CLAP plugin failed to load the preset";
const PRESET_INPUT: &str = "the CLAP preset location, provider ID, or load key is invalid";
const NOTE_PORT_METADATA: &str = "the CLAP plugin returned invalid note port metadata";
const PRESET_LOCATION: &str = "the CLAP preset location could not be read";
const BAR_POSITION_REQUIRED: &str = "CLAP beat transport requires an explicit bar position because it has no separate bar-valid flag";
const RESTART_REQUIRED: &str = "the plugin requested explicit re-preparation";
const EVENT_STORAGE: &str = "could not reserve the prepared CLAP input events";
const LOAD: &str = "the CLAP module could not be loaded";
const NO_FACTORY: &str = "the CLAP module has no plugin factory";
const CLASS_NOT_FOUND: &str = "the CLAP module has no plugin with ID";
const INSTANTIATE: &str = "the CLAP plugin could not be created";
const SAMPLE_FORMAT_UNSUPPORTED: &str =
    "the CLAP audio ports do not support the requested sample format";
const LAYOUT_REFUSED: &str = "the plugin's main audio port does not have the requested layout";
const REALTIME_ONLY: &str = "the plugin cannot render offline (hard real-time requirement)";
const PROCESSING_STORAGE: &str = "could not reserve the prepared CLAP audio buffers";
const STATE: &str = "the plugin failed to save or load its state";
const STATE_UNSUPPORTED: &str = "the plugin does not provide the CLAP state extension";
const EDITOR: &str = "the plugin's editor failed to open";

/// The diagnostic for output events with no MIDI form in one block.
pub(crate) fn unconvertible_output_events(count: u64) -> String {
    format!("{UNCONVERTIBLE_OUTPUT_EVENTS} {count}")
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClapError {
    Input(InputError),
    Render(plughost_core::RenderError),
    AudioMetadata,
    AudioUnsupported,
    AudioConfiguration,
    BypassUnsupported,
    PresetUnsupported,
    PresetDiscovery,
    PresetMetadata,
    PresetLoad,
    PresetInput,
    NotePortMetadata,
    UnknownEventPort {
        id: u64,
    },
    OutputEventCapacity,
    PresetLocation(std::io::ErrorKind),
    StatePurposeUnsupported,
    ParameterConversion,
    ParameterResult,
    Load(String),
    NoFactory,
    ClassNotFound(String),
    Instantiate(String),
    SampleFormatUnsupported(SampleFormat),
    LayoutRefused {
        requested: Layout,
        plugin_channels: u32,
    },
    RealtimeOnly,
    Activate(String),
    NotPrepared,
    RestartRequired,
    BarPositionRequired,
    Buffers,
    ProcessingStorage,
    EventStorage,
    Process(String),
    State,
    StateTooLarge,
    StateUnsupported,
    StateMismatch,
    NoEditor,
    Editor(String),
}

impl fmt::Display for ClapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AudioMetadata => f.write_str(AUDIO_METADATA),
            Self::AudioUnsupported => f.write_str(AUDIO_UNSUPPORTED),
            Self::AudioConfiguration => f.write_str(AUDIO_CONFIGURATION),
            Self::BypassUnsupported => f.write_str(BYPASS_UNSUPPORTED),
            Self::PresetUnsupported => f.write_str(PRESET_UNSUPPORTED),
            Self::PresetDiscovery => f.write_str(PRESET_DISCOVERY),
            Self::PresetMetadata => f.write_str(PRESET_METADATA),
            Self::PresetLoad => f.write_str(PRESET_LOAD),
            Self::PresetInput => f.write_str(PRESET_INPUT),
            Self::NotePortMetadata => f.write_str(NOTE_PORT_METADATA),
            Self::UnknownEventPort { id } => write!(f, "{UNKNOWN_EVENT_PORT} ({id})"),
            Self::OutputEventCapacity => f.write_str(OUTPUT_EVENT_CAPACITY),
            Self::PresetLocation(kind) => write!(f, "{PRESET_LOCATION} ({kind})"),
            ClapError::ParameterConversion => f.write_str(PARAMETER_CONVERSION),
            ClapError::ParameterResult => f.write_str(PARAMETER_RESULT),
            ClapError::StatePurposeUnsupported => f.write_str(STATE_PURPOSE_UNSUPPORTED),
            ClapError::Input(error) => error.fmt(f),
            Self::Render(error) => error.fmt(f),
            ClapError::Load(reason) => write!(f, "{LOAD}: {reason}"),
            ClapError::NoFactory => f.write_str(NO_FACTORY),
            ClapError::ClassNotFound(id) => write!(f, "{CLASS_NOT_FOUND} {id}"),
            ClapError::Instantiate(reason) => write!(f, "{INSTANTIATE}: {reason}"),
            ClapError::SampleFormatUnsupported(format) => {
                write!(f, "{SAMPLE_FORMAT_UNSUPPORTED} ({format:?})")
            }
            ClapError::LayoutRefused {
                requested,
                plugin_channels,
            } => write!(
                f,
                "{LAYOUT_REFUSED} ({requested:?}; the port has {plugin_channels} channels)"
            ),
            ClapError::RealtimeOnly => f.write_str(REALTIME_ONLY),
            ClapError::Activate(reason) => write!(f, "{ACTIVATE}: {reason}"),
            ClapError::BarPositionRequired => f.write_str(BAR_POSITION_REQUIRED),
            ClapError::RestartRequired => f.write_str(RESTART_REQUIRED),
            ClapError::NotPrepared => f.write_str(NOT_PREPARED),
            ClapError::EventStorage => f.write_str(EVENT_STORAGE),
            ClapError::ProcessingStorage => f.write_str(PROCESSING_STORAGE),
            ClapError::Buffers => f.write_str(BUFFERS),
            ClapError::Process(reason) => write!(f, "{PROCESS}: {reason}"),
            ClapError::State => f.write_str(STATE),
            ClapError::StateTooLarge => state_too_large(f),
            ClapError::StateUnsupported => f.write_str(STATE_UNSUPPORTED),
            ClapError::StateMismatch => f.write_str(STATE_MISMATCH),
            ClapError::NoEditor => f.write_str(NO_EDITOR),
            ClapError::Editor(reason) => write!(f, "{EDITOR}: {reason}"),
        }
    }
}

impl std::error::Error for ClapError {}

impl ClapError {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::AudioMetadata => FailureKind::Host,
            Self::AudioUnsupported | Self::BypassUnsupported => FailureKind::Unsupported,
            Self::AudioConfiguration | Self::ProcessingStorage | Self::EventStorage => {
                FailureKind::Configuration
            }
            Self::PresetUnsupported => FailureKind::Unsupported,
            Self::PresetDiscovery | Self::PresetMetadata | Self::PresetLoad => FailureKind::State,
            Self::PresetInput | Self::UnknownEventPort { .. } => FailureKind::InvalidInput,
            Self::NotePortMetadata => FailureKind::Host,
            Self::OutputEventCapacity => FailureKind::Processing,
            Self::PresetLocation(std::io::ErrorKind::NotFound) => FailureKind::NotFound,
            Self::PresetLocation(_) => FailureKind::Host,
            Self::StatePurposeUnsupported => FailureKind::Unsupported,
            Self::ParameterConversion => FailureKind::Unsupported,
            Self::ParameterResult => FailureKind::Host,
            Self::Input(_) | Self::Buffers => FailureKind::InvalidInput,
            Self::Render(error) => error.kind(),
            Self::ClassNotFound(_) => FailureKind::NotFound,
            Self::SampleFormatUnsupported(_)
            | Self::RealtimeOnly
            | Self::StateUnsupported
            | Self::NoEditor => FailureKind::Unsupported,
            Self::Load(_) | Self::NoFactory | Self::Instantiate(_) => FailureKind::Load,
            Self::LayoutRefused { .. } | Self::Activate(_) => FailureKind::Configuration,
            Self::BarPositionRequired => FailureKind::Unsupported,
            Self::RestartRequired => FailureKind::RestartRequired,
            Self::NotPrepared => FailureKind::NotPrepared,
            Self::StateMismatch => FailureKind::StateMismatch,
            Self::State | Self::StateTooLarge => FailureKind::State,
            Self::Process(_) => FailureKind::Processing,
            Self::Editor(_) => FailureKind::Editor,
        }
    }
}

pub(super) const PRESET_DISCOVERY_FAILED: &str = "preset discovery failed";
pub(super) const GUI_REQUEST_WITHOUT_EDITOR: &str =
    "the plugin asked to show or hide an editor that is not open";

pub(super) const PRESET_LOADED: &str = "the CLAP plugin reported a loaded preset";

impl From<plughost_core::RenderError> for ClapError {
    fn from(error: plughost_core::RenderError) -> Self {
        Self::Render(error)
    }
}
