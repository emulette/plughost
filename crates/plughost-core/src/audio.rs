//! Portable audio bus configuration and explicit, serial chain routing.

use crate::{InputError, Layout, ProcessMode, SampleFormat, Support};

/// Bus enumeration and configuration are bounded before allocation or native iteration.
pub const MAX_AUDIO_BUSES: usize = 64;
/// Maximum active channel count in one direction, including all buses.
pub const MAX_AUDIO_CHANNELS: usize = 512;
/// Maximum sample storage in one prepared chain's external-input alignment delays (256 MiB).
/// Preparation or staged state replacement fails before committing if the budget is exceeded.
/// During replacement both the old bank and its prepared replacement may coexist.
pub const MAX_ALIGNMENT_BYTES: usize = 256 * 1024 * 1024;

/// Direction of an audio or event port relative to its plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioDirection {
    /// Data delivered to the plugin.
    Input,
    /// Data produced by the plugin.
    Output,
}

/// Native distinction between a main bus and an additional bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioBusRole {
    /// Primary audio signal.
    Main,
    /// Additional signal, such as a sidechain or instrument output.
    Auxiliary,
}

/// A native bus snapshot. An unknown layout is not inferred from its channel count.
/// IDs are scoped to a direction and the selected native configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AudioBusInfo {
    /// Native bus ID, scoped to its direction; not a position in the returned list.
    pub id: u64,
    /// Native bus index within its direction.
    pub index: u32,
    /// Native display name.
    pub name: String,
    /// Whether this bus receives or produces audio.
    pub direction: AudioDirection,
    /// Native main or auxiliary designation.
    pub role: AudioBusRole,
    /// Recognized portable layout, or `None` when the native arrangement is unknown.
    pub layout: Option<Layout>,
    /// Native channel count, including when the layout is unknown.
    pub channels: u32,
    /// Current activation state, or `None` if the format cannot report it.
    pub active: Option<bool>,
    /// Whether this bus supports single-precision processing.
    pub f32: Support,
    /// Whether this bus supports double-precision processing.
    pub f64: Support,
}

impl AudioBusInfo {
    /// A bus with an unknown layout, activation and precision support.
    pub fn new(
        id: u64,
        index: u32,
        name: impl Into<String>,
        direction: AudioDirection,
        role: AudioBusRole,
        channels: u32,
    ) -> AudioBusInfo {
        AudioBusInfo {
            id,
            index,
            name: name.into(),
            direction,
            role,
            layout: None,
            channels,
            active: None,
            f32: Support::Unknown,
            f64: Support::Unknown,
        }
    }
}

/// A named native bus configuration, such as a CLAP audio-port configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AudioConfiguration {
    /// Native configuration ID; not a position in the returned list.
    pub id: u64,
    /// Native display name.
    pub name: String,
    /// Input buses belonging to this configuration.
    pub inputs: Vec<AudioBusInfo>,
    /// Output buses belonging to this configuration.
    pub outputs: Vec<AudioBusInfo>,
}

impl AudioConfiguration {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(
        id: u64,
        name: String,
        inputs: Vec<AudioBusInfo>,
        outputs: Vec<AudioBusInfo>,
    ) -> AudioConfiguration {
        AudioConfiguration {
            id,
            name,
            inputs,
            outputs,
        }
    }
}

/// Requested layout and activation for one native audio bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AudioBusConfig {
    /// Native bus ID, scoped to its direction; not a position in the returned list.
    pub id: u64,
    /// Requested channel layout; an active bus cannot use `Layout::None`.
    pub layout: Layout,
    /// Whether to activate this bus for processing.
    pub active: bool,
}

impl AudioBusConfig {
    /// A request for native bus `id` with `layout`, active or not.
    pub const fn new(id: u64, layout: Layout, active: bool) -> AudioBusConfig {
        AudioBusConfig { id, layout, active }
    }
}

/// Requested native buses. Buses omitted from a direction are inactive. Native layout/precision
/// negotiation follows validation; successful preparation returns the complete negotiated map.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AudioConfig {
    /// Samples per second; must be finite and greater than zero.
    pub sample_rate: f64,
    /// Maximum frames per processing call, from 1 through `i32::MAX`.
    pub max_block_size: usize,
    /// Sample precision shared by all buses and slots.
    pub sample_format: SampleFormat,
    /// Processing mode reported to the plugin.
    pub mode: ProcessMode,
    /// Native named configuration to select, or `None` to retain the current selection.
    pub configuration: Option<u64>,
    /// Requested input buses; omitted buses are inactive.
    pub inputs: Vec<AudioBusConfig>,
    /// Requested output buses; omitted buses are inactive.
    pub outputs: Vec<AudioBusConfig>,
    /// Native event ports to activate; ports not named are left inactive where the format allows.
    pub events: crate::EventConfig,
}

/// Native bypass support and its last queried state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BypassState {
    /// Whether the plugin exposes writable native bypass.
    pub support: Support,
    /// Current bypass state, or `None` when unavailable.
    pub enabled: Option<bool>,
}

impl BypassState {
    /// Each field as given. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn new(support: Support, enabled: Option<bool>) -> BypassState {
        BypassState { support, enabled }
    }
}

/// Explicit channel adaptation. Surround layouts are never inferred, reordered or downmixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelAdaptation {
    /// Require identical source and destination layouts.
    Exact,
    /// Duplicate the mono sample into left and right with unity gain.
    MonoToStereo,
    /// Average left and right with gain 0.5 per channel.
    StereoToMono,
}

/// Source of one slot’s input bus in a serial chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioSource {
    /// Read an external bus supplied by the caller.
    External {
        /// Index into `RoutedChainConfig::inputs`.
        bus: usize,
    },
    /// Read an active output bus of the immediately preceding slot.
    Previous {
        /// Native output bus ID in the immediately preceding slot.
        bus: u64,
    },
    /// Fill the destination bus with zero samples.
    Silence,
}

/// An input bus request together with its source and explicit channel conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AudioInputRoute {
    /// Destination native input bus and requested layout.
    pub bus: AudioBusConfig,
    /// Signal source for the destination.
    pub source: AudioSource,
    /// Conversion to apply between source and destination layouts.
    pub adaptation: ChannelAdaptation,
}

impl AudioInputRoute {
    /// Input `bus` fed from `source`, with an exact layout match; see [`Self::with_adaptation`].
    pub const fn new(bus: AudioBusConfig, source: AudioSource) -> AudioInputRoute {
        AudioInputRoute {
            bus,
            source,
            adaptation: ChannelAdaptation::Exact,
        }
    }

    /// Converts the source's channels to the bus layout with `adaptation`.
    pub const fn with_adaptation(mut self, adaptation: ChannelAdaptation) -> AudioInputRoute {
        self.adaptation = adaptation;
        self
    }
}

/// Audio and event routing for a single plugin slot.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SlotAudioConfig {
    /// Native named configuration to select, or `None` to retain the current selection.
    pub configuration: Option<u64>,
    /// Input bus requests in caller order; native processing retains native bus order.
    pub inputs: Vec<AudioInputRoute>,
    /// Requested output buses; omitted buses are inactive.
    pub outputs: Vec<AudioBusConfig>,
    /// Native event ports and their serial-chain routing.
    pub events: crate::SlotEventConfig,
}

impl SlotAudioConfig {
    /// A slot's input routes and output buses, keeping its current named configuration and
    /// leaving its event ports inactive.
    pub fn new(inputs: Vec<AudioInputRoute>, outputs: Vec<AudioBusConfig>) -> SlotAudioConfig {
        SlotAudioConfig {
            configuration: None,
            inputs,
            outputs,
            events: crate::SlotEventConfig::default(),
        }
    }

    /// Selects the native named configuration `configuration` first.
    pub fn with_configuration(mut self, configuration: u64) -> SlotAudioConfig {
        self.configuration = Some(configuration);
        self
    }

    /// Activates and routes the slot's event ports as `events` says.
    pub fn with_events(mut self, events: crate::SlotEventConfig) -> SlotAudioConfig {
        self.events = events;
        self
    }
}

/// Explicit serial-chain configuration, validated before native preparation.
///
/// Each direction permits at most 64 audio buses and 512 active channels. External inputs
/// are flattened in caller order; a slot’s native bus IDs do not imply buffer positions.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RoutedChainConfig {
    /// Samples per second; must be finite and greater than zero.
    pub sample_rate: f64,
    /// Maximum frames per processing call, from 1 through `i32::MAX`.
    pub max_block_size: usize,
    /// Sample precision shared by all buses and slots.
    pub sample_format: SampleFormat,
    /// Processing mode reported to the plugin.
    pub mode: ProcessMode,
    /// External bus layouts in caller buffer order.
    pub inputs: Vec<Layout>,
    /// The chain's external event inputs, which events name by index.
    pub event_inputs: usize,
    /// Per-plugin requests, in the same order as the chain’s instances.
    pub slots: Vec<SlotAudioConfig>,
}

impl AudioConfig {
    /// One plugin's `inputs` and `outputs` offline, keeping its current named configuration and
    /// leaving its event ports inactive.
    pub fn new(
        sample_rate: f64,
        max_block_size: usize,
        sample_format: SampleFormat,
        inputs: Vec<AudioBusConfig>,
        outputs: Vec<AudioBusConfig>,
    ) -> AudioConfig {
        AudioConfig {
            sample_rate,
            max_block_size,
            sample_format,
            mode: ProcessMode::Offline,
            configuration: None,
            inputs,
            outputs,
            events: crate::EventConfig::default(),
        }
    }

    /// Reports `mode` to the plugin.
    pub fn with_mode(mut self, mode: ProcessMode) -> AudioConfig {
        self.mode = mode;
        self
    }

    /// Selects the native named configuration `configuration` first.
    pub fn with_configuration(mut self, configuration: u64) -> AudioConfig {
        self.configuration = Some(configuration);
        self
    }

    /// Activates the native event ports `events` names.
    pub fn with_events(mut self, events: crate::EventConfig) -> AudioConfig {
        self.events = events;
        self
    }

    /// Checks scalar processing values, unique bus IDs, layouts, and audio capacity limits.
    pub fn validate(&self) -> Result<(), InputError> {
        crate::config::validate_processing(self.sample_rate, self.max_block_size)?;
        validate_buses(&self.inputs)?;
        validate_buses(&self.outputs)
    }

    /// Total active input channels across the requested native buses.
    pub fn input_channels(&self) -> usize {
        active_channels(&self.inputs)
    }

    /// Total active output channels across the requested native buses.
    pub fn output_channels(&self) -> usize {
        active_channels(&self.outputs)
    }
}

impl RoutedChainConfig {
    /// A chain processing `inputs` through `slots` offline, without external event inputs; see
    /// [`Self::with_mode`] and [`Self::with_event_inputs`].
    pub fn new(
        sample_rate: f64,
        max_block_size: usize,
        sample_format: SampleFormat,
        inputs: Vec<Layout>,
        slots: Vec<SlotAudioConfig>,
    ) -> RoutedChainConfig {
        RoutedChainConfig {
            sample_rate,
            max_block_size,
            sample_format,
            mode: ProcessMode::Offline,
            inputs,
            event_inputs: 0,
            slots,
        }
    }

    /// Reports `mode` to the plugins.
    pub fn with_mode(mut self, mode: ProcessMode) -> RoutedChainConfig {
        self.mode = mode;
        self
    }

    /// Gives the chain `event_inputs` external event inputs.
    pub fn with_event_inputs(mut self, event_inputs: usize) -> RoutedChainConfig {
        self.event_inputs = event_inputs;
        self
    }

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

    /// Total external input channels in caller buffer order.
    pub fn input_channels(&self) -> usize {
        self.inputs.iter().map(|layout| layout.channels()).sum()
    }

    /// Total active output channels of the final slot.
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
    /// Checks whether this conversion supports the exact source and target layouts.
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
