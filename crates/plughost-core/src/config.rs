use crate::errors::InputError;
use serde::{Deserialize, Serialize};

/// Channel layout of an audio bus. Each layout has one channel order, listed on its variant.
/// Formats whose native order differs are mapped explicitly; a layout a format cannot express
/// is refused rather than approximated.
///
/// Ls and Rs are the surrounds of layouts without side channels. Layouts that also have side
/// surrounds (Lss, Rss) list Ls and Rs as the rear surrounds before them. Height channels are
/// top front (Ltf, Rtf), top middle (Ltm, Rtm) and top rear (Ltr, Rtr); Lw and Rw are the wides.
/// A new variant also belongs in [`Layout::ALL`], which formats search to recognize layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Layout {
    /// No bus, for example the input of an instrument.
    None,
    /// One center channel.
    Mono,
    /// Left, right.
    Stereo,
    /// L R C LFE Ls Rs
    Surround51,
    /// L R C LFE Ls Rs Lss Rss (7.1 surround with side channels)
    Surround71,
    /// L R C
    Lcr,
    /// L R Ls Rs
    Quad,
    /// L R C Ls Rs
    Surround50,
    /// L R C Ls Rs Lss Rss
    Surround70,
    /// L R C LFE Ls Rs Ltm Rtm
    Surround512,
    /// L R C LFE Ls Rs Ltf Rtf Ltr Rtr
    Surround514,
    /// L R C LFE Ls Rs Lss Rss Ltm Rtm
    Surround712,
    /// L R C LFE Ls Rs Lss Rss Ltf Rtf Ltr Rtr
    Surround714,
    /// L R C LFE Ls Rs Lss Rss Ltf Rtf Ltr Rtr Ltm Rtm Lw Rw
    Surround916,
    /// First-order ambisonics: ACN channel order, SN3D normalization.
    Ambisonics1,
    /// Second-order ambisonics: ACN channel order, SN3D normalization.
    Ambisonics2,
    /// Third-order ambisonics: ACN channel order, SN3D normalization.
    Ambisonics3,
    /// Fourth-order ambisonics: ACN channel order, SN3D normalization.
    Ambisonics4,
}

impl Layout {
    /// Every layout, in declaration order.
    pub const ALL: [Layout; 18] = [
        Layout::None,
        Layout::Mono,
        Layout::Stereo,
        Layout::Surround51,
        Layout::Surround71,
        Layout::Lcr,
        Layout::Quad,
        Layout::Surround50,
        Layout::Surround70,
        Layout::Surround512,
        Layout::Surround514,
        Layout::Surround712,
        Layout::Surround714,
        Layout::Surround916,
        Layout::Ambisonics1,
        Layout::Ambisonics2,
        Layout::Ambisonics3,
        Layout::Ambisonics4,
    ];

    /// Number of planar channels in this layout.
    pub fn channels(self) -> usize {
        match self {
            Layout::None => 0,
            Layout::Mono => 1,
            Layout::Stereo => 2,
            Layout::Lcr => 3,
            Layout::Quad | Layout::Ambisonics1 => 4,
            Layout::Surround50 => 5,
            Layout::Surround51 => 6,
            Layout::Surround70 => 7,
            Layout::Surround71 | Layout::Surround512 => 8,
            Layout::Ambisonics2 => 9,
            Layout::Surround514 | Layout::Surround712 => 10,
            Layout::Surround714 => 12,
            Layout::Surround916 | Layout::Ambisonics3 => 16,
            Layout::Ambisonics4 => 25,
        }
    }
}

/// Floating-point precision of the prepared audio buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleFormat {
    /// IEEE 754 single-precision samples.
    F32,
    /// IEEE 754 double-precision samples.
    F64,
}

/// The processing mode reported to the plugin. Offline rendering is not paced to wall-clock time;
/// `Realtime` only changes what the plugin is told, for plugins that misbehave in offline mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessMode {
    /// Tell the plugin it may use its offline rendering mode. This is the default.
    #[default]
    Offline,
    /// Tell the plugin it is processing in real time; this does not provide real-time scheduling.
    Realtime,
}

/// Processing configuration for a plugin’s main input and output buses.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessConfig {
    /// Samples per second; must be finite and greater than zero.
    pub sample_rate: f64,
    /// Maximum frames per processing call, from 1 through `i32::MAX`.
    pub max_block_size: usize,
    /// Sample precision used by every channel.
    pub sample_format: SampleFormat,
    /// Requested main input bus layout; `None` for an instrument without input.
    pub input: Layout,
    /// Requested main output bus layout.
    pub output: Layout,
    /// Processing mode reported to the plugin.
    pub mode: ProcessMode,
}

impl ProcessConfig {
    /// Validates scalar values before a format changes the current processing setup.
    /// Layout and sample-format support are negotiated with the plugin afterwards.
    pub fn validate(&self) -> Result<(), InputError> {
        validate_processing(self.sample_rate, self.max_block_size)
    }
}

pub(crate) fn validate_processing(
    sample_rate: f64,
    max_block_size: usize,
) -> Result<(), InputError> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(InputError::SampleRate);
    }
    // VST3 uses signed 32-bit block lengths; CLAP and AU use unsigned 32-bit lengths.
    if max_block_size == 0 || max_block_size > i32::MAX as usize {
        return Err(InputError::BlockSize);
    }
    Ok(())
}
