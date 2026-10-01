//! Event ports, routing between slots, MIDI and system exclusive output, and the output budget,
//! through real helper processes. Build the helper and the test plugins first (see `support`).

use plughost::{
    AudioBusConfig, AudioDirection, AudioInputRoute, AudioSource, BlockContext, Chain,
    ChannelAdaptation, Error, Event, EventData, EventInputRoute, EventPortInfo, EventSource,
    FailureKind, Layout, PluginFormat, PluginRef, ProcessMode, RenderOptions, RoutedChainConfig,
    SampleFormat, SlotAudioConfig, SlotEventConfig, Support, TailPolicy, render,
};

mod support;

use support::{delay, routing, spawn, synth};

const KEY_CONTROLLER: u8 = 20;
const FLOOD: [u8; 4] = [0xF0, 0x7D, 0x7F, 0xF7];

fn stereo(id: u64) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout: Layout::Stereo,
        active: true,
    }
}

fn ports(chain: &mut Chain, slot: usize, direction: AudioDirection) -> Vec<u64> {
    chain
        .event_ports(slot)
        .unwrap()
        .into_iter()
        .filter(|port| port.direction == direction)
        .map(|port| port.id)
        .collect()
}

/// Routes `slot`'s first two event inputs from the chain's two external event inputs.
fn two_inputs(chain: &mut Chain, config: &mut RoutedChainConfig, slot: usize) {
    config.event_inputs = 2;
    config.slots[slot].events.inputs = ports(chain, slot, AudioDirection::Input)
        .into_iter()
        .take(2)
        .enumerate()
        .map(|(port, id)| EventInputRoute {
            port: id,
            source: EventSource::External { port },
        })
        .collect();
}

/// The events in their MIDI 1.0 form, as a MIDI consumer sees them. The routing fixture's VST3
/// side sends its notes as VST3 note events, which arrive as notes; its CLAP side sends MIDI.
fn as_midi(format: PluginFormat, events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .map(|event| match (&event.data, event.data.to_midi()) {
            (EventData::NoteOn(_) | EventData::NoteOff(_), Some(bytes)) => {
                assert_eq!(format, PluginFormat::Vst3);
                Event::midi(event.offset, bytes).on_port(event.port)
            }
            (EventData::Midi(_), _)
                if event.message().is_some_and(|message| {
                    matches!(
                        message,
                        plughost::Message::NoteOn { .. } | plughost::Message::NoteOff { .. }
                    )
                }) =>
            {
                assert_eq!(format, PluginFormat::Clap);
                event.clone()
            }
            _ => event.clone(),
        })
        .collect()
}

/// Processes one block of silence, returning the last slot's left channel and the chain's output
/// events.
fn run(
    chain: &mut Chain,
    config: &RoutedChainConfig,
    frames: usize,
    events: &[Event],
) -> Result<(Vec<f32>, Vec<Event>), Error> {
    let input = vec![0.0; frames];
    let inputs = vec![input.as_slice(); config.inputs.len() * 2];
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    let mut produced = Vec::new();
    chain.process_audio_f32(
        &BlockContext::new(frames),
        &inputs,
        &mut [&mut left, &mut right],
        &[],
        events,
        &mut produced,
    )?;
    Ok((left, produced))
}

fn instrument_chain() -> (Chain, RoutedChainConfig) {
    let mut chain = spawn(&[synth(PluginFormat::Clap)]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    (chain, config)
}

fn routing_chain(format: PluginFormat) -> (Chain, RoutedChainConfig) {
    let mut chain = spawn(&[routing(format)]);
    let mut config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    two_inputs(&mut chain, &mut config, 0);
    chain.prepare_audio(&config).unwrap();
    (chain, config)
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn event_ports_describe_native_buses_in_order() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[routing(format)]);
        let ports: Vec<(AudioDirection, u32, String)> = chain
            .event_ports(0)
            .unwrap()
            .into_iter()
            .map(|port: EventPortInfo| (port.direction, port.index, port.name))
            .collect();
        assert_eq!(
            ports,
            [
                (AudioDirection::Input, 0, "Notes".to_owned()),
                (AudioDirection::Input, 1, "Octave notes".to_owned()),
                (AudioDirection::Input, 2, "Expressions".to_owned()),
                (AudioDirection::Output, 0, "Notes out".to_owned()),
            ],
            "{format:?}"
        );
    }
}

#[test]
#[ignore = "needs helper, routing and synth fixtures (.ps1 or .sh build scripts)"]
fn event_ports_tell_where_note_expressions_and_mpe_arrive() {
    let inputs = |plugin: PluginRef| -> Vec<(Support, Support)> {
        spawn(&[plugin])
            .event_ports(0)
            .unwrap()
            .into_iter()
            .filter(|port| port.direction == AudioDirection::Input)
            .map(|port| (port.note_expression, port.mpe))
            .collect()
    };
    use Support::{Supported, Unknown, Unsupported};
    // The CLAP routing fixture's first input takes plain MIDI, its second MIDI with MPE, and its
    // third CLAP notes.
    assert_eq!(
        inputs(routing(PluginFormat::Clap)),
        [
            (Unsupported, Unsupported),
            (Unsupported, Supported),
            (Supported, Unsupported)
        ]
    );
    // Its VST3 controller lists no note expressions, and VST3 declares no MPE.
    assert_eq!(
        inputs(routing(PluginFormat::Vst3)),
        [(Unsupported, Unknown); 3]
    );
    assert_eq!(
        inputs(synth(PluginFormat::Clap)),
        [(Supported, Unsupported); 2]
    );
    // The VST3 synth's controller lists tuning for both buses.
    assert_eq!(inputs(synth(PluginFormat::Vst3)), [(Supported, Unknown); 2]);
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn midi_effect_output_plays_an_instrument_before_an_audio_effect() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[
            routing(format),
            synth(PluginFormat::Clap),
            delay(PluginFormat::Vst3),
        ]);
        let effect_output = ports(&mut chain, 0, AudioDirection::Output)[0];
        let config = RoutedChainConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            inputs: Vec::new(),
            event_inputs: 1,
            slots: vec![
                // The MIDI effect's audio buses stay inactive.
                SlotAudioConfig {
                    configuration: None,
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    events: SlotEventConfig {
                        inputs: vec![EventInputRoute {
                            port: ports(&mut chain, 0, AudioDirection::Input)[0],
                            source: EventSource::External { port: 0 },
                        }],
                        outputs: vec![effect_output],
                    },
                },
                SlotAudioConfig {
                    configuration: None,
                    inputs: Vec::new(),
                    outputs: vec![stereo(0)],
                    events: SlotEventConfig {
                        inputs: vec![EventInputRoute {
                            port: 0,
                            source: EventSource::Previous {
                                port: effect_output,
                            },
                        }],
                        outputs: Vec::new(),
                    },
                },
                SlotAudioConfig {
                    configuration: None,
                    inputs: vec![AudioInputRoute {
                        bus: stereo(0),
                        source: AudioSource::Previous { bus: 0 },
                        adaptation: ChannelAdaptation::Exact,
                    }],
                    outputs: vec![stereo(0)],
                    events: SlotEventConfig::default(),
                },
            ],
        };
        chain.prepare_audio(&config).unwrap();
        // The effect moves key 57 up an octave, so the instrument plays A4 from frame 1000.
        let events = [
            Event::note_on(1000, 0, 57, 127),
            Event::note_off(2000, 0, 57, 0),
        ];
        let options = RenderOptions::new(TailPolicy::Reported, 0.0);
        let rendered = render::<f32, _>(&mut chain, &[], 4800, &events, &options).unwrap();
        let output = &rendered.channels[0];
        assert!(output[..1000].iter().all(|&s| s == 0.0), "{format:?}");
        assert_eq!(output[1000], 1.0, "{format:?}");
        // A4 completes a cycle in 48000 / 440 frames; A3 would be at the trough there.
        assert!((output[1000 + 109] - 1.0).abs() < 0.05, "{format:?}");
        assert!(output[2000..].iter().all(|&s| s == 0.0), "{format:?}");
        assert!(rendered.events.is_empty());
    }
}

#[test]
#[ignore = "needs helper and synth fixtures (.ps1 or .sh build scripts)"]
fn the_same_note_on_two_ports_sounds_twice() {
    let (mut chain, mut config) = instrument_chain();
    two_inputs(&mut chain, &mut config, 0);
    chain.prepare_audio(&config).unwrap();
    let (output, _) = run(
        &mut chain,
        &config,
        64,
        &[
            Event::note_on(0, 0, 69, 127),
            Event::note_on(0, 0, 69, 127).on_port(1),
            Event::note_off(32, 0, 69, 0),
        ],
    )
    .unwrap();
    // The second port's voice is inverted: together they cancel, and alone it remains.
    assert!(output[..32].iter().all(|&s| s == 0.0));
    assert_eq!(output[32..].len(), 32);
    assert!(output[32..].iter().all(|&s| s != 0.0));
    let (output, _) = run(
        &mut chain,
        &config,
        64,
        &[Event::note_off(0, 0, 69, 0).on_port(1)],
    )
    .unwrap();
    assert!(output.iter().all(|&s| s == 0.0));
}

#[test]
#[ignore = "needs helper and synth fixtures (.ps1 or .sh build scripts)"]
fn system_exclusive_sustain_and_reset_reach_the_instrument() {
    let (mut chain, config) = instrument_chain();
    chain.prepare_audio(&config).unwrap();
    let (output, _) = run(
        &mut chain,
        &config,
        64,
        &[
            Event::sysex(0, vec![0xF0, 0x7D, 0x01, 64, 0xF7]),
            Event::control_change(0, 0, 64, 127),
            Event::note_on(10, 0, 69, 127),
            Event::note_off(20, 0, 69, 0),
            Event::control_change(40, 0, 64, 0),
        ],
    )
    .unwrap();
    assert_eq!(output[10], 64.0 / 127.0);
    // The pedal holds the note past its note off until the pedal is released.
    assert!(output[10..40].iter().all(|&s| s != 0.0));
    assert!(output[40..].iter().all(|&s| s == 0.0));
    run(&mut chain, &config, 64, &[Event::note_on(0, 0, 69, 127)]).unwrap();
    chain.reset().unwrap();
    let (output, _) = run(&mut chain, &config, 64, &[]).unwrap();
    assert!(output.iter().all(|&s| s == 0.0));
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn last_slot_events_leave_the_chain_in_offset_order() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (mut chain, config) = routing_chain(format);
        let sysex = vec![0xF0, 0x7D, 0x01, 0x02, 0xF7];
        let (_, produced) = run(
            &mut chain,
            &config,
            64,
            &[
                Event::note_on(5, 0, 60, 100),
                Event::note_on(7, 3, 60, 90).on_port(1),
                Event::sysex(9, sysex.clone()),
                Event::note_off(11, 3, 60, 0).on_port(1),
            ],
        )
        .unwrap();
        assert_eq!(
            as_midi(format, &produced),
            [
                Event::note_on(5, 0, 72, 100),
                Event::control_change(5, 0, KEY_CONTROLLER, 60),
                Event::note_on(7, 3, 84, 90),
                Event::control_change(7, 3, KEY_CONTROLLER, 60),
                Event::sysex(9, sysex),
                Event::note_off(11, 3, 84, 0),
            ],
            "{format:?}"
        );
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn events_a_plugin_sends_past_the_block_land_on_its_last_frame() {
    // The fixture sends a note off on key 0 at the block's length; CLAP also reports its end.
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (mut chain, config) = routing_chain(format);
        chain.take_diagnostics().unwrap();
        let (_, produced) = run(&mut chain, &config, 64, &[Event::note_off(10, 0, 0, 0)]).unwrap();
        assert_eq!(
            as_midi(format, &produced),
            [Event::note_off(63, 0, 12, 0)],
            "{format:?}"
        );
        let (_, produced) = run(&mut chain, &config, 64, &[]).unwrap();
        assert_eq!(produced, [], "{format:?}");
        let diagnostics = chain.take_diagnostics().unwrap();
        assert!(
            diagnostics.records.is_empty(),
            "{format:?}: {diagnostics:?}"
        );
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn exceeding_the_output_budget_stops_processing_until_reset() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (mut chain, config) = routing_chain(format);
        let error = run(&mut chain, &config, 64, &[Event::sysex(0, FLOOD.to_vec())]).unwrap_err();
        assert_eq!(error.kind(), FailureKind::Processing, "{format:?}");
        assert!(matches!(error, Error::Operation { slot: Some(0), .. }));
        // Events already delivered to the plugin may be lost, so the stream is out of sync.
        let error = run(&mut chain, &config, 64, &[]).unwrap_err();
        assert!(matches!(error, Error::Operation { slot: None, .. }));
        assert_eq!(error.kind(), FailureKind::Processing);
        chain.reset().unwrap();
        let (_, produced) = run(&mut chain, &config, 64, &[Event::note_on(0, 0, 60, 100)]).unwrap();
        assert_eq!(
            as_midi(format, &produced[..1]),
            [Event::note_on(0, 0, 72, 100)],
            "{format:?}"
        );
    }
}
