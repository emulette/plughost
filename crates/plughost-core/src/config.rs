use crate::errors::InputError;
use serde::{Deserialize, Serialize};

/// Channel layout of a main audio bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layout {
    /// No bus, for example the input of an instrument.
    None,
    Mono,
    Stereo,
    /// L R C LFE Ls Rs
    Surround51,
    /// L R C LFE Ls Rs Lss Rss (7.1 surround with side channels)
    Surround71,
}

impl Layout {
    pub fn channels(self) -> usize {
        match self {
            Layout::None => 0,
            Layout::Mono => 1,
            Layout::Stereo => 2,
            Layout::Surround51 => 6,
            Layout::Surround71 => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleFormat {
    F32,
    F64,
}

/// The processing mode reported to the plugin. Offline rendering is not paced to wall-clock time;
/// `Realtime` only changes what the plugin is told, for plugins that misbehave in offline mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessMode {
    #[default]
    Offline,
    Realtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessConfig {
    pub sample_rate: f64,
    pub max_block_size: usize,
    pub sample_format: SampleFormat,
    pub input: Layout,
    pub output: Layout,
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
