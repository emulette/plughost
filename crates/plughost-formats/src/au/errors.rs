use std::fmt;

use plughost_core::{FailureKind, InputError, Layout, SampleFormat};

use crate::errors::{
    CLOSED, NOT_PREPARED, PARAMETER_CONVERSION, PARAMETER_RESULT, STATE_PURPOSE_UNSUPPORTED,
    state_too_large,
};
use plughost_core::messages::{
    BUFFERS, OUTPUT_EVENT_CAPACITY, STATE_MISMATCH, UNCONVERTIBLE_OUTPUT_EVENTS, UNKNOWN_EVENT_PORT,
};

const AUDIO_BUS_METADATA: &str = "the Audio Unit returned invalid audio bus metadata";
const AUDIO_CONFIGURATION: &str = "the Audio Unit does not support the requested bus configuration";
const BYPASS_UNSUPPORTED: &str = "the Audio Unit does not support the requested bypass state";
const FACTORY_PRESET: &str = "the Audio Unit factory preset is unavailable";
const FACTORY_PRESET_METADATA: &str =
    "the Audio Unit returned invalid or excessive factory preset metadata";
const FACTORY_PRESET_SELECTION: &str = "the Audio Unit did not select the requested factory preset";
const CLASS_ID: &str = "not an Audio Unit class ID (24 hexadecimal digits)";
const INSTANTIATE: &str = "the Audio Unit could not be instantiated";
const SAMPLE_FORMAT_UNSUPPORTED: &str = "Audio Units process 32-bit float samples only";
const NO_BUS: &str = "the Audio Unit has no bus for the requested layout";
const LAYOUT_REFUSED: &str = "the Audio Unit refused the requested layout";
const ALLOCATE: &str = "the Audio Unit could not allocate render resources";
const PROCESSING_STORAGE: &str = "could not reserve the prepared Audio Unit audio buffers";
const RENDER: &str = "the Audio Unit's render call failed";
const RENDER_BLOCKS: &str = "the Audio Unit returned no render or parameter scheduling block";
const STATE: &str = "the Audio Unit state could not be encoded or decoded";
const INVALIDATED: &str =
    "the Audio Unit extension connection was invalidated; create a new instance";
const NO_EDITOR: &str = "the Audio Unit has no editor view";

pub(crate) fn unconvertible_output_events(count: u64) -> String {
    format!("{UNCONVERTIBLE_OUTPUT_EVENTS} {count}")
}

/// Audio Unit loading, configuration, rendering, state, and editor failures.
#[derive(Clone, Debug, PartialEq)]
pub enum AuError {
    /// Caller input violated a processing or control contract.
    Input(InputError),
    /// The requested native event port does not exist or is not enabled.
    UnknownEventPort {
        /// Requested native event port ID.
        id: u64,
    },
    /// The Audio Unit returned invalid audio bus metadata.
    AudioBusMetadata,
    /// The Audio Unit does not support the requested bus configuration.
    AudioConfiguration,
    /// The Audio Unit does not support the requested bypass state.
    BypassUnsupported,
    /// The encoded native state exceeds `MAX_STATE_BYTES`.
    StateTooLarge,
    /// The Audio Unit factory preset is unavailable.
    FactoryPreset,
    /// The Audio Unit returned invalid or excessive factory preset metadata.
    FactoryPresetMetadata,
    /// The Audio Unit did not select the requested factory preset.
    FactoryPresetSelection,
    /// The plugin does not support the requested state context.
    StatePurposeUnsupported,
    /// The plugin does not support the requested parameter conversion.
    ParameterConversion,
    /// The plugin returned a non-finite or otherwise invalid parameter conversion.
    ParameterResult,
    /// The supplied Audio Unit class ID is not 24 hexadecimal digits.
    ClassId(String),
    /// The Audio Unit could not be instantiated. The payload contains the native diagnostic, or
    /// the requested class ID when no registered Audio Unit has it.
    Instantiate(String),
    /// Audio Units process 32-bit float samples only.
    SampleFormatUnsupported(SampleFormat),
    /// No main audio bus can provide the requested layout.
    NoBus(Layout),
    /// The Audio Unit refused the requested layout.
    LayoutRefused(Layout, String),
    /// The Audio Unit could not allocate render resources. The payload contains the native diagnostic.
    Allocate(String),
    /// The plugin is not prepared for processing.
    NotPrepared,
    /// The buffers do not match the prepared layout or block size.
    Buffers,
    /// Could not reserve the prepared Audio Unit audio buffers.
    ProcessingStorage,
    /// The native render call returned the supplied OSStatus error code.
    Render(i32),
    /// The Audio Unit returned no render or parameter scheduling block.
    RenderBlocks,
    /// Native output exceeded the per-block event or system-exclusive budget; reset before processing again.
    OutputEventCapacity,
    /// The Audio Unit state could not be encoded or decoded.
    State,
    /// Saved state belongs to a different plugin format or class.
    StateMismatch,
    /// The owning plugin handle has been dropped; its processor is no longer usable.
    Closed,
    /// The Audio Unit extension connection was invalidated; create a new instance.
    Invalidated,
    /// The Audio Unit has no editor view.
    NoEditor,
}

impl fmt::Display for AuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuError::AudioBusMetadata => f.write_str(AUDIO_BUS_METADATA),
            AuError::AudioConfiguration => f.write_str(AUDIO_CONFIGURATION),
            AuError::BypassUnsupported => f.write_str(BYPASS_UNSUPPORTED),
            AuError::StateTooLarge => state_too_large(f),
            AuError::FactoryPreset => f.write_str(FACTORY_PRESET),
            AuError::FactoryPresetMetadata => f.write_str(FACTORY_PRESET_METADATA),
            AuError::FactoryPresetSelection => f.write_str(FACTORY_PRESET_SELECTION),
            AuError::ParameterConversion => f.write_str(PARAMETER_CONVERSION),
            AuError::ParameterResult => f.write_str(PARAMETER_RESULT),
            AuError::StatePurposeUnsupported => f.write_str(STATE_PURPOSE_UNSUPPORTED),
            AuError::Input(error) => error.fmt(f),
            AuError::ClassId(id) => write!(f, "{CLASS_ID}: {id}"),
            AuError::Instantiate(reason) => write!(f, "{INSTANTIATE}: {reason}"),
            AuError::SampleFormatUnsupported(format) => {
                write!(f, "{SAMPLE_FORMAT_UNSUPPORTED} ({format:?})")
            }
            AuError::NoBus(layout) => write!(f, "{NO_BUS} ({layout:?})"),
            AuError::LayoutRefused(layout, reason) => {
                write!(f, "{LAYOUT_REFUSED} ({layout:?}): {reason}")
            }
            AuError::Allocate(reason) => write!(f, "{ALLOCATE}: {reason}"),
            AuError::NotPrepared => f.write_str(NOT_PREPARED),
            AuError::Invalidated => f.write_str(INVALIDATED),
            AuError::ProcessingStorage => f.write_str(PROCESSING_STORAGE),
            AuError::Buffers => f.write_str(BUFFERS),
            AuError::UnknownEventPort { id } => write!(f, "{UNKNOWN_EVENT_PORT} ({id})"),
            AuError::Render(status) => write!(f, "{RENDER} (OSStatus {status})"),
            AuError::RenderBlocks => f.write_str(RENDER_BLOCKS),
            AuError::OutputEventCapacity => f.write_str(OUTPUT_EVENT_CAPACITY),
            AuError::State => f.write_str(STATE),
            AuError::StateMismatch => f.write_str(STATE_MISMATCH),
            AuError::Closed => f.write_str(CLOSED),
            AuError::NoEditor => f.write_str(NO_EDITOR),
        }
    }
}

impl std::error::Error for AuError {}

impl AuError {
    /// Classifies this failure for scan policy and helper diagnostics.
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::AudioBusMetadata | Self::RenderBlocks => FailureKind::Host,
            Self::AudioConfiguration | Self::ProcessingStorage => FailureKind::Configuration,
            Self::BypassUnsupported => FailureKind::Unsupported,
            Self::StateTooLarge => FailureKind::State,
            Self::FactoryPreset => FailureKind::InvalidInput,
            Self::FactoryPresetMetadata => FailureKind::Host,
            Self::FactoryPresetSelection => FailureKind::State,
            Self::StatePurposeUnsupported => FailureKind::Unsupported,
            Self::ParameterConversion => FailureKind::Unsupported,
            Self::ParameterResult => FailureKind::Host,
            Self::Input(_) | Self::ClassId(_) | Self::Buffers | Self::UnknownEventPort { .. } => {
                FailureKind::InvalidInput
            }
            Self::Instantiate(_) => FailureKind::Load,
            Self::SampleFormatUnsupported(_) | Self::NoEditor => FailureKind::Unsupported,
            Self::NoBus(_) | Self::LayoutRefused(_, _) | Self::Allocate(_) => {
                FailureKind::Configuration
            }
            Self::NotPrepared => FailureKind::NotPrepared,
            Self::Closed => FailureKind::Closed,
            Self::Invalidated => FailureKind::RestartRequired,
            Self::StateMismatch => FailureKind::StateMismatch,
            Self::State => FailureKind::State,
            Self::Render(_) | Self::OutputEventCapacity => FailureKind::Processing,
        }
    }
}
