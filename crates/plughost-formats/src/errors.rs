#[cfg(all(feature = "au", target_os = "macos"))]
use crate::au::AuError;
#[cfg(feature = "clap")]
use crate::clap::ClapError;
#[cfg(feature = "vst3")]
use crate::vst3::Vst3Error;
use plughost_core::{Failure, FailureKind, InputError, PluginFormat, RenderError};
use std::fmt;

const UNSUPPORTED_FORMAT: &str = "this build does not support the plugin's format";
const NO_BUNDLE: &str = "VST3 and CLAP plugins need a bundle path";
const CONTINUOUS_PARAMETER: &str = "continuous parameters do not expose a discrete choice list";
const PRESET_UNSUPPORTED: &str = "this format has no supported preset file codec";
const FACTORY_PRESETS_UNSUPPORTED: &str = "this format has no supported factory program interface";
const PRESET_DISCOVERY_UNSUPPORTED: &str = "this format has no supported discovery preset loader";

// Messages shared by the format-specific errors.
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) const NOT_PREPARED: &str = "the plugin is not prepared for processing";
#[cfg(any(feature = "vst3", all(feature = "au", target_os = "macos")))]
pub(crate) const CLOSED: &str = "the plugin was closed";
#[cfg(any(feature = "vst3", feature = "clap"))]
pub(crate) const ACTIVATE: &str = "the plugin failed to activate";
#[cfg(any(feature = "vst3", feature = "clap"))]
pub(crate) const PROCESS: &str = "the plugin's process call failed";
#[cfg(any(feature = "vst3", feature = "clap"))]
pub(crate) const NO_EDITOR: &str = "the plugin has no editor for this platform";
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) const STATE_TOO_LARGE: &str = "the plugin's saved state exceeds the maximum size";
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) const STATE_PURPOSE_UNSUPPORTED: &str =
    "the plugin does not support the requested state context";
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) const PARAMETER_CONVERSION: &str =
    "the plugin does not provide this parameter conversion";
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) const PARAMETER_RESULT: &str =
    "the plugin returned an invalid parameter conversion result";

/// The message for a saved state over `MAX_STATE_BYTES`.
#[cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
pub(crate) fn state_too_large(f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
        f,
        "{STATE_TOO_LARGE} ({} MiB)",
        plughost_core::MAX_STATE_BYTES >> 20
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    ContinuousParameter,
    PresetUnsupported,
    FactoryPresetsUnsupported,
    PresetDiscoveryUnsupported,
    Input(InputError),
    #[cfg(feature = "vst3")]
    Vst3(Vst3Error),
    #[cfg(all(feature = "au", target_os = "macos"))]
    Au(AuError),
    #[cfg(feature = "clap")]
    Clap(ClapError),
    UnsupportedFormat(PluginFormat),
    NoBundle,
    Render(RenderError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ContinuousParameter => f.write_str(CONTINUOUS_PARAMETER),
            Error::PresetUnsupported => f.write_str(PRESET_UNSUPPORTED),
            Error::FactoryPresetsUnsupported => f.write_str(FACTORY_PRESETS_UNSUPPORTED),
            Error::PresetDiscoveryUnsupported => f.write_str(PRESET_DISCOVERY_UNSUPPORTED),
            Error::Input(error) => error.fmt(f),
            #[cfg(feature = "vst3")]
            Error::Vst3(error) => error.fmt(f),
            #[cfg(all(feature = "au", target_os = "macos"))]
            Error::Au(error) => error.fmt(f),
            #[cfg(feature = "clap")]
            Error::Clap(error) => error.fmt(f),
            Error::UnsupportedFormat(format) => write!(f, "{UNSUPPORTED_FORMAT} ({format:?})"),
            Error::NoBundle => f.write_str(NO_BUNDLE),
            Error::Render(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<RenderError> for Error {
    fn from(error: RenderError) -> Error {
        Error::Render(error)
    }
}

#[cfg(feature = "vst3")]
impl From<Vst3Error> for Error {
    fn from(error: Vst3Error) -> Error {
        Error::Vst3(error)
    }
}

#[cfg(all(feature = "au", target_os = "macos"))]
impl From<AuError> for Error {
    fn from(error: AuError) -> Error {
        Error::Au(error)
    }
}

#[cfg(feature = "clap")]
impl From<ClapError> for Error {
    fn from(error: ClapError) -> Error {
        Error::Clap(error)
    }
}

impl Error {
    /// Caller input rejected by the owning format before it changed the plugin.
    pub fn input_error(&self) -> Option<InputError> {
        match self {
            Self::Input(error) => Some(*error),
            #[cfg(feature = "vst3")]
            Self::Vst3(Vst3Error::Input(error)) => Some(*error),
            #[cfg(feature = "clap")]
            Self::Clap(ClapError::Input(error)) => Some(*error),
            #[cfg(all(feature = "au", target_os = "macos"))]
            Self::Au(AuError::Input(error)) => Some(*error),
            _ => None,
        }
    }

    pub fn kind(&self) -> FailureKind {
        match self {
            Self::Input(_) | Self::NoBundle => FailureKind::InvalidInput,
            Self::UnsupportedFormat(_)
            | Self::PresetUnsupported
            | Self::ContinuousParameter
            | Self::FactoryPresetsUnsupported
            | Self::PresetDiscoveryUnsupported => FailureKind::Unsupported,
            Self::Render(error) => error.kind(),
            #[cfg(feature = "vst3")]
            Self::Vst3(error) => error.kind(),
            #[cfg(feature = "clap")]
            Self::Clap(error) => error.kind(),
            #[cfg(all(feature = "au", target_os = "macos"))]
            Self::Au(error) => error.kind(),
        }
    }

    /// A plugin produced more events than one block allows; the block's events are lost, so
    /// the caller resynchronizes with a reset.
    pub fn is_output_event_overflow(&self) -> bool {
        match self {
            #[cfg(feature = "vst3")]
            Self::Vst3(Vst3Error::OutputEventCapacity) => true,
            #[cfg(feature = "clap")]
            Self::Clap(ClapError::OutputEventCapacity) => true,
            #[cfg(all(feature = "au", target_os = "macos"))]
            Self::Au(AuError::OutputEventCapacity) => true,
            _ => false,
        }
    }

    pub fn failure(&self) -> Failure {
        Failure::new(self.kind(), self.to_string())
    }
}
