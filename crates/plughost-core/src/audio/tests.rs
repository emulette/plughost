use super::*;
use crate::{EventInputRoute, EventSource, SlotEventConfig};

fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}

fn config() -> RoutedChainConfig {
    RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F64,
        mode: ProcessMode::Offline,
        inputs: vec![Layout::Stereo, Layout::Mono],
        slots: vec![
            SlotAudioConfig {
                events: Default::default(),
                configuration: Some(3),
                inputs: vec![
                    AudioInputRoute {
                        bus: bus(8, Layout::Stereo),
                        source: AudioSource::External { bus: 0 },
                        adaptation: ChannelAdaptation::Exact,
                    },
                    AudioInputRoute {
                        bus: bus(9, Layout::Mono),
                        source: AudioSource::External { bus: 1 },
                        adaptation: ChannelAdaptation::Exact,
                    },
                ],
                outputs: vec![bus(10, Layout::Stereo), bus(11, Layout::Mono)],
            },
            SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![
                    AudioInputRoute {
                        bus: bus(0, Layout::Stereo),
                        source: AudioSource::Previous { bus: 11 },
                        adaptation: ChannelAdaptation::MonoToStereo,
                    },
                    AudioInputRoute {
                        bus: bus(1, Layout::Mono),
                        source: AudioSource::External { bus: 1 },
                        adaptation: ChannelAdaptation::Exact,
                    },
                ],
                outputs: vec![bus(20, Layout::Stereo), bus(21, Layout::Mono)],
            },
        ],
    }
}

#[test]
fn routed_chain_checks_sources_and_keeps_native_configuration_and_precision() {
    let config = config();
    assert_eq!(config.validate(2), Ok(()));
    assert_eq!(config.input_channels(), 3);
    assert_eq!(config.output_channels(), 3);
    let native = config.slot_config(0).unwrap();
    assert_eq!(native.configuration, Some(3));
    assert_eq!(native.sample_format, SampleFormat::F64);
    assert_eq!(native.input_channels(), 3);
    assert_eq!(native.output_channels(), 3);
    assert_eq!(native.validate(), Ok(()));
    assert_eq!(config.slot_config(2), Err(InputError::Slot));
}

#[test]
fn invalid_routes_are_rejected_without_inventing_sources_or_adaptations() {
    let valid = config();
    let mut invalid = valid.clone();
    invalid.slots[0].inputs[0].source = AudioSource::Previous { bus: 10 };
    assert!(matches!(
        invalid.validate(2),
        Err(InputError::AudioRoute { slot: 0, bus: 8 })
    ));
    invalid = valid.clone();
    invalid.slots[1].inputs[0].source = AudioSource::Previous { bus: 99 };
    assert!(matches!(
        invalid.validate(2),
        Err(InputError::AudioRoute { slot: 1, bus: 0 })
    ));
    invalid = valid.clone();
    invalid.slots[0].outputs[1].active = false;
    assert!(matches!(
        invalid.validate(2),
        Err(InputError::AudioRoute { .. })
    ));
    invalid = valid.clone();
    invalid.slots[0].inputs[0].source = AudioSource::External { bus: 2 };
    assert!(matches!(
        invalid.validate(2),
        Err(InputError::AudioRoute { .. })
    ));
    invalid = valid.clone();
    invalid.slots[1].inputs[0].adaptation = ChannelAdaptation::Exact;
    assert!(matches!(
        invalid.validate(2),
        Err(InputError::ChannelAdaptation { .. })
    ));
    assert_eq!(valid.validate(1), Err(InputError::AudioSlotCount));
}

fn event_config() -> RoutedChainConfig {
    let mut config = config();
    config.event_inputs = 2;
    config.slots[0].events = SlotEventConfig {
        inputs: vec![EventInputRoute {
            port: 41,
            source: EventSource::External { port: 0 },
        }],
        outputs: vec![51, 52],
    };
    config.slots[1].events = SlotEventConfig {
        inputs: vec![EventInputRoute {
            port: 61,
            source: EventSource::Previous { port: 51 },
        }],
        outputs: vec![71],
    };
    config
}

#[test]
fn event_routes_reject_missing_sources_duplicate_routes_and_duplicate_outputs() {
    let valid = event_config();
    assert_eq!(valid.validate(2), Ok(()));
    for (slot, source) in [
        (0, EventSource::Previous { port: 51 }),
        (0, EventSource::External { port: 2 }),
        (1, EventSource::Previous { port: 99 }),
    ] {
        let mut invalid = valid.clone();
        invalid.slots[slot].events.inputs[0].source = source;
        assert_eq!(
            invalid.validate(2),
            Err(InputError::EventRoute {
                slot,
                port: if slot == 0 { 41 } else { 61 },
            })
        );
    }
    let mut duplicate = valid.clone();
    duplicate.slots[1]
        .events
        .inputs
        .push(valid.slots[1].events.inputs[0]);
    assert_eq!(
        duplicate.validate(2),
        Err(InputError::EventRoute { slot: 1, port: 61 })
    );
    duplicate = valid;
    duplicate.slots[0].events.outputs.push(51);
    assert_eq!(
        duplicate.validate(2),
        Err(InputError::DuplicateEventPort { id: 51 })
    );
}

#[test]
fn sixteen_event_ports_and_merged_routes_fit_but_seventeen_do_not() {
    let mut valid = event_config();
    valid.event_inputs = 16;
    valid.slots[0].events.inputs = (0..16)
        .map(|port| EventInputRoute {
            port: 41,
            source: EventSource::External { port },
        })
        .collect();
    valid.slots[0].events.outputs = (51..67).collect();
    assert_eq!(valid.validate(2), Ok(()));
    assert_eq!(valid.slot_config(0).unwrap().events.inputs, [41]);
    for field in [0, 1, 2] {
        let mut invalid = valid.clone();
        match field {
            0 => invalid.event_inputs = 17,
            1 => invalid.slots[0].events.inputs.push(EventInputRoute {
                port: 42,
                source: EventSource::External { port: 0 },
            }),
            _ => invalid.slots[0].events.outputs.push(67),
        }
        assert_eq!(invalid.validate(2), Err(InputError::EventPortCount));
    }
}

#[test]
fn silent_and_disabled_buses_do_not_require_a_connected_source() {
    let mut config = config();
    let route = &mut config.slots[0].inputs[0];
    route.source = AudioSource::Silence;
    assert_eq!(config.validate(2), Ok(()));
    let route = &mut config.slots[0].inputs[0];
    route.source = AudioSource::Previous { bus: u64::MAX };
    route.bus.active = false;
    assert_eq!(config.validate(2), Ok(()));
    assert_eq!(config.slot_config(0).unwrap().input_channels(), 1);
}

#[test]
fn bus_validation_rejects_duplicate_ids_and_inactive_only_layouts_before_preparation() {
    let mut native = config().slot_config(0).unwrap();
    native.inputs.push(native.inputs[0]);
    assert_eq!(
        native.validate(),
        Err(InputError::DuplicateAudioBus { id: 8 })
    );
    native.inputs.pop();
    native.inputs[0].layout = Layout::None;
    assert_eq!(native.validate(), Err(InputError::AudioBusLayout { id: 8 }));
    native.inputs[0].active = false;
    assert_eq!(native.validate(), Ok(()));
    native.max_block_size = 0;
    assert_eq!(native.validate(), Err(InputError::BlockSize));
}

#[test]
fn audio_bus_limits_are_applied_before_route_expansion() {
    let mut outputs = config();
    outputs.slots[0].outputs = (0..=MAX_AUDIO_BUSES)
        .map(|id| bus(id as u64, Layout::Surround71))
        .collect();
    assert_eq!(outputs.validate(2), Err(InputError::AudioBusCount));
    let mut inputs = config();
    inputs.inputs = vec![Layout::Stereo; MAX_AUDIO_BUSES + 1];
    assert_eq!(inputs.validate(2), Err(InputError::AudioBusCount));
}
