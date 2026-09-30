use std::fmt;

use plughost_core::{FailureKind, InputError, Layout, RenderError, SampleFormat};

use crate::errors::{
    ACTIVATE, CLOSED, NO_EDITOR, NOT_PREPARED, PARAMETER_CONVERSION, PARAMETER_RESULT, PROCESS,
    STATE_PURPOSE_UNSUPPORTED, state_too_large,
};
use plughost_core::messages::{
    BUFFERS, MAIN_BUS, OUTPUT_EVENT_CAPACITY, STATE_MISMATCH, UNCONVERTIBLE_OUTPUT_EVENTS,
    UNKNOWN_EVENT_PORT,
};

const AUDIO_BUS_METADATA: &str = "the plugin returned invalid audio bus metadata";
const AUDIO_CONFIGURATIONS_UNSUPPORTED: &str = "VST3 does not enumerate named audio configurations";
const UNKNOWN_AUDIO_BUS: &str = "the plugin has no requested audio bus";
const AUDIO_BUS_ACTIVATION: &str = "the plugin refused activation for an audio bus";
const BYPASS_UNSUPPORTED: &str = "the plugin has no writable native bypass parameter";
const BYPASS_EDIT: &str = "the plugin rejected the bypass change";
const FACTORY_PRESETS_UNSUPPORTED: &str =
    "the plugin does not support native factory program selection";
const INVALID_FACTORY_PRESET: &str = "the factory program does not belong to this plugin";
const FACTORY_PRESET_METADATA: &str = "the plugin returned invalid factory program metadata";
const FACTORY_PRESET_SELECT: &str = "the plugin rejected the factory program";
const EVENT_STORAGE: &str = "could not reserve the prepared VST3 event storage";
const BUNDLE_OPEN: &str = "could not open the VST3 bundle";
const BUNDLE_LOAD: &str = "could not load the VST3 module binary";
const NO_ENTRY_POINT: &str = "the module does not export";
const ENTRY_FAILED: &str = "the module's entry function failed";
const NO_FACTORY: &str = "GetPluginFactory returned no factory";
const CLASS_NOT_FOUND: &str = "the module has no audio module class with ID";
const CREATE_COMPONENT: &str = "the factory could not create the component";
const INITIALIZE_COMPONENT: &str = "the component failed to initialize";
const NO_AUDIO_PROCESSOR: &str = "the component does not implement IAudioProcessor";
const NO_CONTROLLER_CLASS: &str = "the component names no edit controller class";
const CREATE_CONTROLLER: &str = "the factory could not create the edit controller";
const INITIALIZE_CONTROLLER: &str = "the edit controller failed to initialize";
const SAMPLE_FORMAT_UNSUPPORTED: &str = "the plugin cannot process samples of format";
const LAYOUT_REFUSED: &str = "the plugin refused the requested layout";
const SETUP_REJECTED: &str =
    "the plugin rejected the processing setup (sample rate, block size, sample format, or mode)";
const SAMPLE_FORMAT_MISMATCH: &str = "the buffers' sample format differs from the prepared format";
const PROCESSING_STORAGE: &str = "could not reserve the prepared VST3 audio buffers";
const RESTART_REQUIRED: &str =
    "the plugin changed its buses or asked to be reloaded; prepare it again";
const GET_STATE: &str = "the plugin failed to save its state";
const SET_STATE: &str = "the plugin failed to restore its state";
const PRESET: &str = "not a valid .vstpreset file";
const EDITOR_ATTACH: &str = "the plugin's editor failed to attach";
const PARAMETER_GROUPS: &str = "the plugin returned an invalid parameter group hierarchy";

/// The diagnostic for output events with no MIDI form since the last report.
pub(crate) fn unconvertible_output_events(count: u64) -> String {
    format!("{UNCONVERTIBLE_OUTPUT_EVENTS} {count}")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Vst3Error {
    AudioBusMetadata,
    AudioConfigurationsUnsupported,
    UnknownAudioBus {
        id: u64,
    },
    AudioBusActivation {
        id: u64,
        code: i32,
    },
    BypassUnsupported,
    BypassEdit(i32),
    FactoryPresetsUnsupported,
    InvalidFactoryPreset,
    FactoryPresetMetadata,
    FactoryPresetSelect(i32),
    ParameterGroups,
    Input(InputError),
    StatePurposeUnsupported,
    ParameterConversion,
    ParameterResult,
    BundleOpen,
    BundleLoad,
    NoEntryPoint(&'static str),
    EntryFailed,
    NoFactory,
    ClassNotFound(String),
    CreateComponent,
    InitializeComponent(i32),
    NoAudioProcessor,
    NoControllerClass,
    CreateController,
    InitializeController(i32),
    SampleFormatUnsupported(SampleFormat),
    NoBus(Layout),
    LayoutRefused {
        requested: Layout,
        plugin_channels: usize,
    },
    SetupRejected(i32),
    Activate(i32),
    NotPrepared,
    SampleFormatMismatch,
    Buffers,
    ProcessingStorage,
    EventStorage,
    OutputEventCapacity,
    UnknownEventPort {
        id: u64,
    },
    Process(i32),
    RestartRequired(i32),
    GetState(i32),
    StateTooLarge,
    SetState(i32),
    StateMismatch,
    Preset,
    Closed,
    NoEditor,
    EditorAttach(i32),
    Render(RenderError),
}

impl fmt::Display for Vst3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AudioBusMetadata => f.write_str(AUDIO_BUS_METADATA),
            Self::AudioConfigurationsUnsupported => f.write_str(AUDIO_CONFIGURATIONS_UNSUPPORTED),
            Self::UnknownAudioBus { id } => write!(f, "{UNKNOWN_AUDIO_BUS} ({id})"),
            Self::AudioBusActivation { id, code } => {
                write!(f, "{AUDIO_BUS_ACTIVATION} (bus {id}, tresult {code})")
            }
            Self::BypassUnsupported => f.write_str(BYPASS_UNSUPPORTED),
            Self::BypassEdit(code) => write!(f, "{BYPASS_EDIT} (tresult {code})"),
            Self::FactoryPresetsUnsupported => f.write_str(FACTORY_PRESETS_UNSUPPORTED),
            Self::InvalidFactoryPreset => f.write_str(INVALID_FACTORY_PRESET),
            Self::FactoryPresetMetadata => f.write_str(FACTORY_PRESET_METADATA),
            Self::FactoryPresetSelect(code) => {
                write!(f, "{FACTORY_PRESET_SELECT} (tresult {code})")
            }
            Vst3Error::ParameterConversion => f.write_str(PARAMETER_CONVERSION),
            Vst3Error::ParameterResult => f.write_str(PARAMETER_RESULT),
            Vst3Error::ParameterGroups => f.write_str(PARAMETER_GROUPS),
            Vst3Error::StatePurposeUnsupported => f.write_str(STATE_PURPOSE_UNSUPPORTED),
            Vst3Error::Input(error) => error.fmt(f),
            Vst3Error::BundleOpen => f.write_str(BUNDLE_OPEN),
            Vst3Error::BundleLoad => f.write_str(BUNDLE_LOAD),
            Vst3Error::NoEntryPoint(name) => write!(f, "{NO_ENTRY_POINT} {name}"),
            Vst3Error::EntryFailed => f.write_str(ENTRY_FAILED),
            Vst3Error::NoFactory => f.write_str(NO_FACTORY),
            Vst3Error::ClassNotFound(id) => write!(f, "{CLASS_NOT_FOUND} {id}"),
            Vst3Error::CreateComponent => f.write_str(CREATE_COMPONENT),
            Vst3Error::InitializeComponent(code) => {
                write!(f, "{INITIALIZE_COMPONENT} (tresult {code})")
            }
            Vst3Error::NoAudioProcessor => f.write_str(NO_AUDIO_PROCESSOR),
            Vst3Error::NoControllerClass => f.write_str(NO_CONTROLLER_CLASS),
            Vst3Error::CreateController => f.write_str(CREATE_CONTROLLER),
            Vst3Error::InitializeController(code) => {
                write!(f, "{INITIALIZE_CONTROLLER} (tresult {code})")
            }
            Vst3Error::SampleFormatUnsupported(format) => {
                write!(f, "{SAMPLE_FORMAT_UNSUPPORTED} {format:?}")
            }
            Vst3Error::NoBus(layout) => write!(f, "{MAIN_BUS} ({layout:?})"),
            Vst3Error::LayoutRefused {
                requested,
                plugin_channels,
            } => write!(
                f,
                "{LAYOUT_REFUSED} ({requested:?}; the plugin wants {plugin_channels} channels)"
            ),
            Vst3Error::SetupRejected(code) => write!(f, "{SETUP_REJECTED} (tresult {code})"),
            Vst3Error::Activate(code) => write!(f, "{ACTIVATE} (tresult {code})"),
            Vst3Error::NotPrepared => f.write_str(NOT_PREPARED),
            Vst3Error::SampleFormatMismatch => f.write_str(SAMPLE_FORMAT_MISMATCH),
            Vst3Error::EventStorage => f.write_str(EVENT_STORAGE),
            Vst3Error::OutputEventCapacity => f.write_str(OUTPUT_EVENT_CAPACITY),
            Vst3Error::UnknownEventPort { id } => write!(f, "{UNKNOWN_EVENT_PORT} ({id})"),
            Vst3Error::ProcessingStorage => f.write_str(PROCESSING_STORAGE),
            Vst3Error::Buffers => f.write_str(BUFFERS),
            Vst3Error::Process(code) => write!(f, "{PROCESS} (tresult {code})"),
            Vst3Error::RestartRequired(flags) => write!(f, "{RESTART_REQUIRED} (flags {flags:#x})"),
            Vst3Error::GetState(code) => write!(f, "{GET_STATE} (tresult {code})"),
            Vst3Error::StateTooLarge => state_too_large(f),
            Vst3Error::SetState(code) => write!(f, "{SET_STATE} (tresult {code})"),
            Vst3Error::StateMismatch => f.write_str(STATE_MISMATCH),
            Vst3Error::Preset => f.write_str(PRESET),
            Vst3Error::Closed => f.write_str(CLOSED),
            Vst3Error::NoEditor => f.write_str(NO_EDITOR),
            Vst3Error::EditorAttach(code) => write!(f, "{EDITOR_ATTACH} (tresult {code})"),
            Vst3Error::Render(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Vst3Error {}

impl Vst3Error {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::AudioBusMetadata => FailureKind::Host,
            Self::UnknownAudioBus { .. } | Self::UnknownEventPort { .. } => {
                FailureKind::InvalidInput
            }
            Self::OutputEventCapacity => FailureKind::Processing,
            Self::AudioConfigurationsUnsupported | Self::BypassUnsupported => {
                FailureKind::Unsupported
            }
            Self::AudioBusActivation { .. } => FailureKind::Configuration,
            Self::BypassEdit(_) => FailureKind::State,
            Self::FactoryPresetsUnsupported => FailureKind::Unsupported,
            Self::InvalidFactoryPreset => FailureKind::InvalidInput,
            Self::FactoryPresetMetadata => FailureKind::Host,
            Self::FactoryPresetSelect(_) => FailureKind::State,
            Self::ParameterGroups => FailureKind::Host,
            Self::StatePurposeUnsupported => FailureKind::Unsupported,
            Self::ParameterConversion => FailureKind::Unsupported,
            Self::ParameterResult => FailureKind::Host,
            Self::Input(_) | Self::SampleFormatMismatch | Self::Buffers | Self::Preset => {
                FailureKind::InvalidInput
            }
            Self::ClassNotFound(_) => FailureKind::NotFound,
            Self::SampleFormatUnsupported(_) | Self::NoEditor | Self::NoAudioProcessor => {
                FailureKind::Unsupported
            }
            Self::BundleOpen
            | Self::BundleLoad
            | Self::NoEntryPoint(_)
            | Self::EntryFailed
            | Self::NoFactory
            | Self::CreateComponent
            | Self::InitializeComponent(_)
            | Self::NoControllerClass
            | Self::CreateController
            | Self::InitializeController(_) => FailureKind::Load,
            Self::EventStorage
            | Self::ProcessingStorage
            | Self::NoBus(_)
            | Self::LayoutRefused { .. }
            | Self::SetupRejected(_)
            | Self::Activate(_) => FailureKind::Configuration,
            Self::NotPrepared => FailureKind::NotPrepared,
            Self::Closed => FailureKind::Closed,
            Self::RestartRequired(_) => FailureKind::RestartRequired,
            Self::StateMismatch => FailureKind::StateMismatch,
            Self::GetState(code) | Self::SetState(code)
                if *code == vst3::Steinberg::kNotImplemented
                    || *code == vst3::Steinberg::kNoInterface =>
            {
                FailureKind::Unsupported
            }
            Self::GetState(_) | Self::SetState(_) | Self::StateTooLarge => FailureKind::State,
            Self::Process(_) => FailureKind::Processing,
            Self::EditorAttach(_) => FailureKind::Editor,
            Self::Render(error) => error.kind(),
        }
    }
}

impl From<RenderError> for Vst3Error {
    fn from(error: RenderError) -> Vst3Error {
        Vst3Error::Render(error)
    }
}
