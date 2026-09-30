//! AU bus negotiation. Channel counts are constraints, not speaker arrangements.
use std::ops::Range;

use objc2::rc::Retained;
use objc2::{AnyThread, msg_send};
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray, kAudioUnitType_Generator,
    kAudioUnitType_MIDIProcessor, kAudioUnitType_MusicDevice, kAudioUnitType_MusicEffect,
};
use objc2_avf_audio::{AVAudioChannelLayout, AVAudioFormat};
use objc2_core_audio_types::{
    kAudioChannelLayoutTag_DiscreteInOrder as DISCRETE_IN_ORDER,
    kAudioChannelLayoutTag_Mono as MONO, kAudioChannelLayoutTag_Stereo as STEREO,
    kAudioChannelLayoutTag_WAVE_5_1_A as SURROUND_51,
    kAudioChannelLayoutTag_WAVE_7_1 as SURROUND_71,
};
use objc2_foundation::NSError;
use plughost_core::{
    AudioBusConfig, AudioBusInfo, AudioBusRole, AudioConfig, AudioConfiguration, AudioDirection,
    Layout, ProcessConfig, Support,
};

use super::{AuError, Plugin, apply_state, describe, lock};

#[derive(Clone)]
pub(super) struct BusBuffer {
    pub index: usize,
    pub channels: Range<usize>,
}

impl Plugin {
    pub(crate) fn audio_buses(&self) -> Result<Vec<AudioBusInfo>, AuError> {
        let engine = lock(&self.engine);
        buses(
            engine.unit()?,
            engine.prepared.as_ref().map(|prepared| &prepared.config),
        )
    }

    /// AU reports channel-count constraints, not enumerated configuration identities.
    pub(crate) fn audio_configurations(&self) -> Result<Vec<AudioConfiguration>, AuError> {
        let engine = lock(&self.engine);
        engine.unit()?;
        Ok(Vec::new())
    }

    /// The unit's MIDI input, which instruments, music effects and MIDI processors have, and
    /// its MIDI outputs, one per cable. Output IDs are cable numbers.
    pub(crate) fn event_ports(&self) -> Result<Vec<plughost_core::EventPortInfo>, AuError> {
        let port = |index: u32, name: String, direction| plughost_core::EventPortInfo {
            id: u64::from(index),
            index,
            name,
            direction,
            midi: Support::Supported,
            sysex: Support::Supported,
        };
        let outputs = midi_output_names(lock(&self.engine).unit()?);
        Ok(self
            .midi_input()?
            .then(|| port(0, String::new(), AudioDirection::Input))
            .into_iter()
            .chain(
                outputs
                    .into_iter()
                    .zip(0..)
                    .map(|(name, index)| port(index, name, AudioDirection::Output)),
            )
            .collect())
    }

    pub(super) fn midi_input(&self) -> Result<bool, AuError> {
        let kind = super::super::components::parse_class_id(&self.info.class_id)?.componentType;
        Ok([
            kAudioUnitType_MusicDevice,
            kAudioUnitType_MusicEffect,
            kAudioUnitType_MIDIProcessor,
        ]
        .contains(&kind))
    }

    /// Negotiates every native bus and reserves host audio storage for the maximum block size.
    /// Failure after deallocation leaves this instance unprepared; callers needing atomic
    /// reconfiguration must stage a replacement instance.
    pub(crate) fn prepare_audio(
        &mut self,
        config: &AudioConfig,
    ) -> Result<Vec<AudioBusInfo>, AuError> {
        let midi_input = self.midi_input()?;
        let mut engine = lock(&self.engine);
        let outputs = midi_output_names(engine.unit()?).len() as u64;
        if let Some(&id) = config
            .events
            .outputs
            .iter()
            .filter(|&&id| id >= outputs)
            .chain(
                config
                    .events
                    .inputs
                    .iter()
                    .filter(|&&id| id != 0 || !midi_input),
            )
            .next()
        {
            return Err(AuError::UnknownEventPort { id });
        }
        engine.prepare_audio(config)?;
        if let (Some((state, purpose)), Some(unit)) =
            (self.restored_unprepared.take(), &engine.unit)
        {
            apply_state(unit, &state, purpose);
        }
        buses(engine.unit()?, Some(config))
    }
}

/// The names of the unit's MIDI outputs, one per cable.
pub(super) fn midi_output_names(unit: &AUAudioUnit) -> Vec<String> {
    let names = unsafe { unit.MIDIOutputNames() };
    (0..names.count())
        .map(|index| names.objectAtIndex(index).to_string())
        .collect()
}

fn format(bus: &AUAudioUnitBus) -> Result<Retained<AVAudioFormat>, AuError> {
    let format: Option<Retained<AVAudioFormat>> = unsafe { msg_send![bus, format] };
    format.ok_or(AuError::AudioBusMetadata)
}

fn layout(format: &AVAudioFormat, requested: Option<Layout>) -> Option<Layout> {
    match unsafe { format.channelLayout() } {
        Some(layout) => match unsafe { layout.layoutTag() } {
            MONO => Some(Layout::Mono),
            STEREO => Some(Layout::Stereo),
            SURROUND_51 => Some(Layout::Surround51),
            SURROUND_71 => Some(Layout::Surround71),
            // DiscreteInOrder explicitly preserves lane order, without assigning speakers.
            // Preserve the caller's negotiated labels; never infer them from channel count.
            tag if tag & 0xffff_0000 == DISCRETE_IN_ORDER => requested.filter(|layout| {
                layout.channels() == (tag & 0xffff) as usize
                    && layout.channels() == unsafe { format.channelCount() } as usize
            }),
            _ => None,
        },
        // AVAudioFormat explicitly permits omitted layouts for mono and stereo only.
        None => match unsafe { format.channelCount() } {
            1 => Some(Layout::Mono),
            2 => Some(Layout::Stereo),
            _ => None,
        },
    }
}

fn arrays(unit: &AUAudioUnit) -> [(AudioDirection, Retained<AUAudioUnitBusArray>); 2] {
    unsafe {
        [
            (AudioDirection::Input, unit.inputBusses()),
            (AudioDirection::Output, unit.outputBusses()),
        ]
    }
}

pub(super) fn buses(
    unit: &AUAudioUnit,
    config: Option<&AudioConfig>,
) -> Result<Vec<AudioBusInfo>, AuError> {
    let mut result = Vec::new();
    for (direction, array) in arrays(unit) {
        let count = unsafe { array.count() };
        if count > plughost_core::MAX_AUDIO_BUSES {
            return Err(AuError::AudioBusMetadata);
        }
        for index in 0..count {
            let bus = unsafe { array.objectAtIndexedSubscript(index) };
            let format = format(&bus)?;
            let channels = unsafe { format.channelCount() };
            if unsafe { bus.index() } != index
                || channels as usize > plughost_core::MAX_AUDIO_CHANNELS
            {
                return Err(AuError::AudioBusMetadata);
            }
            result.push(AudioBusInfo {
                id: index as u64,
                index: index as u32,
                name: unsafe { bus.name() }
                    .map(|name| name.to_string())
                    .unwrap_or_default(),
                direction,
                // AU's primary input/output is element zero; additional elements are auxiliaries.
                role: if index == 0 {
                    AudioBusRole::Main
                } else {
                    AudioBusRole::Auxiliary
                },
                layout: layout(
                    &format,
                    config.and_then(|config| {
                        let requested = match direction {
                            AudioDirection::Input => &config.inputs,
                            AudioDirection::Output => &config.outputs,
                        };
                        requested
                            .iter()
                            .find(|bus| bus.id == index as u64 && bus.active)
                            .map(|bus| bus.layout)
                    }),
                ),
                channels,
                active: Some(unsafe { bus.isEnabled() }),
                f32: if unsafe { format.isStandard() } {
                    Support::Supported
                } else {
                    Support::Unknown
                },
                f64: Support::Unsupported,
            });
        }
    }
    Ok(result)
}

pub(super) fn main_bus_config(
    unit: &AUAudioUnit,
    config: &ProcessConfig,
    midi_input: bool,
) -> Result<AudioConfig, AuError> {
    config.validate().map_err(AuError::Input)?;
    let buses = buses(unit, None)?;
    let configs = |direction, requested| {
        buses
            .iter()
            .filter(|bus| bus.direction == direction)
            .map(|bus| AudioBusConfig {
                id: bus.id,
                layout: if bus.index == 0 {
                    requested
                } else {
                    Layout::None
                },
                active: bus.index == 0 && requested != Layout::None,
            })
            .collect()
    };
    if config.input != Layout::None
        && !buses
            .iter()
            .any(|bus| bus.direction == AudioDirection::Input)
    {
        return Err(AuError::NoBus(config.input));
    }
    if config.output != Layout::None
        && !buses
            .iter()
            .any(|bus| bus.direction == AudioDirection::Output)
    {
        return Err(AuError::NoBus(config.output));
    }
    Ok(AudioConfig {
        sample_rate: config.sample_rate,
        max_block_size: config.max_block_size,
        sample_format: config.sample_format,
        mode: config.mode,
        configuration: None,
        inputs: configs(AudioDirection::Input, config.input),
        outputs: configs(AudioDirection::Output, config.output),
        events: plughost_core::EventConfig {
            inputs: midi_input.then_some(0).into_iter().collect(),
            outputs: (!midi_output_names(unit).is_empty())
                .then_some(0)
                .into_iter()
                .collect(),
        },
    })
}

fn tag(layout: Layout) -> Result<u32, AuError> {
    match layout {
        Layout::Mono => Ok(MONO),
        Layout::Stereo => Ok(STEREO),
        Layout::Surround51 => Ok(SURROUND_51),
        Layout::Surround71 => Ok(SURROUND_71),
        Layout::None => Err(AuError::AudioConfiguration),
    }
}

pub(super) fn validate_configuration(
    unit: &AUAudioUnit,
    config: &AudioConfig,
) -> Result<(), AuError> {
    if config.max_block_size > (u32::MAX as usize / size_of::<f32>())
        || !config.outputs.iter().any(|bus| bus.active)
        || config.configuration.is_some()
    {
        return Err(AuError::AudioConfiguration);
    }
    // Formats are negotiated with setFormat when configuring; channel capabilities are the only
    // constraint checked beforehand.
    for ((_, array), requested) in arrays(unit)
        .into_iter()
        .zip([&config.inputs, &config.outputs])
    {
        let count = unsafe { array.count() };
        if requested.iter().any(|bus| bus.id >= count as u64) {
            return Err(AuError::AudioConfiguration);
        }
    }
    if let Some(capabilities) = unsafe { unit.channelCapabilities() } {
        if capabilities.count() % 2 != 0 {
            return Err(AuError::AudioBusMetadata);
        }
        let counts = |buses: &[AudioBusConfig]| {
            let main = buses
                .iter()
                .find(|bus| bus.id == 0 && bus.active)
                .map_or(0, |bus| bus.layout.channels() as i64);
            let total = buses
                .iter()
                .filter(|bus| bus.active)
                .map(|bus| bus.layout.channels() as i64)
                .sum();
            (main, total)
        };
        let input = counts(&config.inputs);
        let output = counts(&config.outputs);
        // Instruments, generators and MIDI processors may leave their input off whatever channel
        // pairs they list (JUCE lists only the pairs with the input on).
        let input_off = input.1 == 0 && input_optional(unit);
        if !capabilities.is_empty()
            && !(0..capabilities.count()).step_by(2).any(|index| {
                matches_capability(
                    capabilities.objectAtIndex(index).longLongValue(),
                    capabilities.objectAtIndex(index + 1).longLongValue(),
                    input,
                    output,
                ) || input_off
                    && matches_capability(
                        -2,
                        capabilities.objectAtIndex(index + 1).longLongValue(),
                        input,
                        output,
                    )
            })
        {
            return Err(AuError::AudioConfiguration);
        }
    }
    Ok(())
}

/// Whether the unit's type makes audio input optional.
fn input_optional(unit: &AUAudioUnit) -> bool {
    let kind = unsafe { unit.componentDescription() }.componentType;
    [
        kAudioUnitType_MusicDevice,
        kAudioUnitType_Generator,
        kAudioUnitType_MIDIProcessor,
    ]
    .contains(&kind)
}

fn matches_capability(input: i64, output: i64, inputs: (i64, i64), outputs: (i64, i64)) -> bool {
    let side = |constraint: i64, counts: (i64, i64)| match constraint {
        -1 | -2 => true,
        value if value < -2 => value.checked_neg().is_some_and(|limit| counts.1 <= limit),
        0 => counts.1 == 0,
        value => counts.0 == value,
    };
    side(input, inputs)
        && side(output, outputs)
        && (!(input == -1 && output == -1) || inputs.0 == outputs.0)
}

pub(super) fn configure(
    unit: &AUAudioUnit,
    config: &AudioConfig,
) -> Result<(Vec<BusBuffer>, Vec<BusBuffer>), AuError> {
    let mut sides = [Vec::new(), Vec::new()];
    for (((_, array), requested), buffers) in arrays(unit)
        .into_iter()
        .zip([&config.inputs, &config.outputs])
        .zip(&mut sides)
    {
        let mut offset = 0;
        for index in 0..unsafe { array.count() } {
            let request = requested.iter().find(|bus| bus.id == index as u64);
            let active = request.is_some_and(|request| request.active);
            let bus = unsafe { array.objectAtIndexedSubscript(index) };
            if let Some(request) = request.filter(|request| request.active) {
                let format = unsafe {
                    AVAudioFormat::initStandardFormatWithSampleRate_channelLayout(
                        AVAudioFormat::alloc(),
                        config.sample_rate,
                        &AVAudioChannelLayout::layoutWithLayoutTag(tag(request.layout)?),
                    )
                };
                let set: Result<(), Retained<NSError>> =
                    unsafe { msg_send![&*bus, setFormat: &*format, error: _] };
                set.map_err(|error| AuError::LayoutRefused(request.layout, describe(&error)))?;
                let actual = self::format(&bus)?;
                if layout(&actual, Some(request.layout)) != Some(request.layout)
                    || unsafe { actual.sampleRate() } != config.sample_rate
                    || !unsafe { actual.isStandard() }
                {
                    return Err(AuError::AudioConfiguration);
                }
                let end = offset + request.layout.channels();
                buffers.push(BusBuffer {
                    index,
                    channels: offset..end,
                });
                offset = end;
            }
            unsafe { bus.setEnabled(active) };
            if unsafe { bus.isEnabled() } != active {
                return Err(AuError::AudioConfiguration);
            }
        }
    }
    let [inputs, outputs] = sides;
    Ok((inputs, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discrete_order_retains_only_explicit_matching_channel_labels() {
        let format = |tag| unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channelLayout(
                AVAudioFormat::alloc(),
                48_000.0,
                &AVAudioChannelLayout::layoutWithLayoutTag(tag),
            )
        };
        let discrete = format(DISCRETE_IN_ORDER | 6);
        assert_eq!(layout(&discrete, None), None);
        assert_eq!(
            layout(&discrete, Some(Layout::Surround51)),
            Some(Layout::Surround51)
        );
        assert_eq!(layout(&discrete, Some(Layout::Surround71)), None);
        let different_order = format(objc2_core_audio_types::kAudioChannelLayoutTag_MPEG_5_1_B);
        assert_eq!(layout(&different_order, Some(Layout::Surround51)), None);
        let unknown = format(objc2_core_audio_types::kAudioChannelLayoutTag_Unknown | 6);
        assert_eq!(layout(&unknown, Some(Layout::Surround51)), None);
    }

    #[test]
    fn channel_constraints_keep_equality_independence_and_total_limits_distinct() {
        assert!(matches_capability(-1, -1, (2, 3), (2, 2)));
        assert!(!matches_capability(-1, -1, (1, 1), (2, 2)));
        assert!(matches_capability(-1, -2, (1, 1), (8, 8)));
        assert!(matches_capability(-16, 2, (8, 16), (2, 2)));
        assert!(!matches_capability(-16, 2, (8, 17), (2, 2)));
        assert!(matches_capability(0, 2, (0, 0), (2, 2)));
        assert!(!matches_capability(0, 2, (0, 1), (2, 2)));
    }
}
