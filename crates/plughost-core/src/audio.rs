//! Portable audio bus configuration and explicit, serial chain routing.

use serde::{Deserialize, Serialize};

use crate::{InputError, Layout, ProcessMode, SampleFormat, Support};

/// Bus enumeration and configuration are bounded before allocation or native iteration.
pub const MAX_AUDIO_BUSES: usize = 64;
/// Maximum active channel count in one direction, including all buses.
pub const MAX_AUDIO_CHANNELS: usize = 512;
/// Maximum sample storage in one prepared chain's external-input alignment delays (256 MiB).
/// Preparation or staged state replacement fails before committing if the budget is exceeded.
/// During replacement both the old bank and its prepared replacement may coexist.
pub const MAX_ALIGNMENT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioDirection {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioBusRole {
    Main,
    Auxiliary,
}

/// A native bus snapshot. An unknown layout is not inferred from its channel count.
/// IDs are scoped to a direction and the selected native configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioBusInfo {
    pub id: u64,
    pub index: u32,
    pub name: String,
    pub direction: AudioDirection,
    pub role: AudioBusRole,
    pub layout: Option<Layout>,
    pub channels: u32,
    pub active: Option<bool>,
    pub f32: Support,
    pub f64: Support,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfiguration {
    pub id: u64,
    pub name: String,
    pub inputs: Vec<AudioBusInfo>,
    pub outputs: Vec<AudioBusInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioBusConfig {
    pub id: u64,
    pub layout: Layout,
    pub active: bool,
}

/// Requested native buses. Buses omitted from a direction are inactive. Native layout/precision
/// negotiation follows validation; successful preparation returns the complete negotiated map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: f64,
    pub max_block_size: usize,
    pub sample_format: SampleFormat,
    pub mode: ProcessMode,
    pub configuration: Option<u64>,
    pub inputs: Vec<AudioBusConfig>,
    pub outputs: Vec<AudioBusConfig>,
    /// Native event ports to activate; ports not named are left inactive where the format allows.
    pub events: crate::EventConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BypassState {
    pub support: Support,
    pub enabled: Option<bool>,
}

/// Explicit channel adaptation. Surround layouts are never inferred, reordered or downmixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelAdaptation {
    Exact,
    /// Duplicate the mono sample into left and right with unity gain.
    MonoToStereo,
    /// Average left and right with gain 0.5 per channel.
    StereoToMono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSource {
    External { bus: usize },
    Previous { bus: u64 },
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInputRoute {
    pub bus: AudioBusConfig,
    pub source: AudioSource,
    pub adaptation: ChannelAdaptation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotAudioConfig {
    pub configuration: Option<u64>,
    pub inputs: Vec<AudioInputRoute>,
    pub outputs: Vec<AudioBusConfig>,
    pub events: crate::SlotEventConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutedChainConfig {
    pub sample_rate: f64,
    pub max_block_size: usize,
    pub sample_format: SampleFormat,
    pub mode: ProcessMode,
    /// External bus layouts in caller buffer order.
    pub inputs: Vec<Layout>,
    /// The chain's external event inputs, which events name by index.
    pub event_inputs: usize,
    pub slots: Vec<SlotAudioConfig>,
}

impl AudioConfig {
    pub fn validate(&self) -> Result<(), InputError> {
        crate::config::validate_processing(self.sample_rate, self.max_block_size)?;
        validate_buses(&self.inputs)?;
        validate_buses(&self.outputs)
    }

    pub fn input_channels(&self) -> usize {
        active_channels(&self.inputs)
    }

    pub fn output_channels(&self) -> usize {
        active_channels(&self.outputs)
    }
}

impl RoutedChainConfig {
    /// Validates routing without querying or mutating any plugin. Native bus IDs/layouts are
    /// checked when each slot negotiates its requested configuration.
    pub fn validate(&self, slots: usize) -> Result<(), InputError> {
        if slots == 0 {
            return Err(InputError::EmptyChain);
        }
        if self.slots.len() != slots {
            return Err(InputError::AudioSlotCount);
        }
        crate::config::validate_processing(self.sample_rate, self.max_block_size)?;
        if self.inputs.len() > MAX_AUDIO_BUSES {
            return Err(InputError::AudioBusCount);
        }
        if self.input_channels() > MAX_AUDIO_CHANNELS {
            return Err(InputError::AudioChannelCount);
        }
        if self.event_inputs > crate::MAX_EVENT_PORTS {
            return Err(InputError::EventPortCount);
        }
        for (slot, config) in self.slots.iter().enumerate() {
            let previous = slot
                .checked_sub(1)
                .map(|previous| &self.slots[previous].events);
            config.events.validate(slot, self.event_inputs, previous)?;
            if config.inputs.len() > MAX_AUDIO_BUSES {
                return Err(InputError::AudioBusCount);
            }
            let inputs: Vec<AudioBusConfig> = config.inputs.iter().map(|route| route.bus).collect();
            validate_buses(&inputs)?;
            validate_buses(&config.outputs)?;
            for route in &config.inputs {
                // A disabled bus consumes no audio; its saved source choice is not resolved.
                if !route.bus.active {
                    continue;
                }
                let source = match route.source {
                    AudioSource::External { bus } => self.inputs.get(bus).copied(),
                    AudioSource::Previous { bus } => slot
                        .checked_sub(1)
                        .and_then(|previous| {
                            self.slots[previous]
                                .outputs
                                .iter()
                                .find(|output| output.id == bus && output.active)
                        })
                        .map(|output| output.layout),
                    // Silence has the target's layout and requires no channel conversion.
                    AudioSource::Silence => Some(route.bus.layout),
                }
                .ok_or(InputError::AudioRoute {
                    slot,
                    bus: route.bus.id,
                })?;
                route.adaptation.validate(source, route.bus.layout)?;
            }
        }
        Ok(())
    }

    pub fn input_channels(&self) -> usize {
        self.inputs.iter().map(|layout| layout.channels()).sum()
    }

    pub fn output_channels(&self) -> usize {
        self.slots
            .last()
            .map_or(0, |slot| active_channels(&slot.outputs))
    }

    /// The chain's event outputs: the last slot's collected output ports.
    pub fn event_outputs(&self) -> usize {
        self.slots
            .last()
            .map_or(0, |slot| slot.events.outputs.len())
    }

    /// The selected slot's native request, retaining the chain's shared processing precision.
    pub fn slot_config(&self, slot: usize) -> Result<AudioConfig, InputError> {
        let config = self.slots.get(slot).ok_or(InputError::Slot)?;
        Ok(AudioConfig {
            sample_rate: self.sample_rate,
            max_block_size: self.max_block_size,
            sample_format: self.sample_format,
            mode: self.mode,
            configuration: config.configuration,
            inputs: config.inputs.iter().map(|route| route.bus).collect(),
            outputs: config.outputs.clone(),
            events: config.events.native(),
        })
    }
}

impl ChannelAdaptation {
    pub fn validate(self, source: Layout, target: Layout) -> Result<(), InputError> {
        let valid = match self {
            Self::Exact => source == target,
            Self::MonoToStereo => source == Layout::Mono && target == Layout::Stereo,
            Self::StereoToMono => source == Layout::Stereo && target == Layout::Mono,
        };
        if !valid {
            return Err(InputError::ChannelAdaptation { source, target });
        }
        Ok(())
    }
}

fn active_channels(buses: &[AudioBusConfig]) -> usize {
    buses
        .iter()
        .filter(|bus| bus.active)
        .map(|bus| bus.layout.channels())
        .sum()
}

fn validate_buses(buses: &[AudioBusConfig]) -> Result<(), InputError> {
    if buses.len() > MAX_AUDIO_BUSES {
        return Err(InputError::AudioBusCount);
    }
    for (index, bus) in buses.iter().enumerate() {
        if buses[..index].iter().any(|previous| previous.id == bus.id) {
            return Err(InputError::DuplicateAudioBus { id: bus.id });
        }
        if bus.active && bus.layout == Layout::None {
            return Err(InputError::AudioBusLayout { id: bus.id });
        }
    }
    if active_channels(buses) > MAX_AUDIO_CHANNELS {
        return Err(InputError::AudioChannelCount);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
