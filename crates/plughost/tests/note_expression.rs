//! Notes with IDs and the expression of single notes, in CLAP and VST3, through real helper
//! processes: the synth fixture plays overlapping notes on one key and their tuning and pressure
//! from the samples they arrive at, and the routing fixture passes them on to the next slot and
//! the application. Build the helper and the test plugins first (see `support`).

use std::f64::consts::TAU;

use plughost::{
    AudioDirection, BlockContext, Chain, Event, EventData, EventInputRoute, EventSource,
    ExpressionKind, Layout, Note, NoteExpression, PluginFormat, RoutedChainConfig, SlotEventConfig,
};

mod support;

use support::{routing, spawn, synth};

const FORMATS: [PluginFormat; 2] = [PluginFormat::Vst3, PluginFormat::Clap];
const SAMPLE_RATE: f64 = 48_000.0;
const FRAMES: usize = 512;
const KEY: u8 = 69;

fn note_on(offset: usize, id: u32, velocity: f64) -> Event {
    Event::new(
        offset,
        EventData::NoteOn(Note::new(0, KEY, velocity).with_id(id)),
    )
}

fn note_off(offset: usize, id: u32) -> Event {
    Event::new(
        offset,
        EventData::NoteOff(Note::new(0, KEY, 0.0).with_id(id)),
    )
}

fn expression(offset: usize, id: u32, kind: ExpressionKind, value: f64) -> Event {
    let expression = NoteExpression::new(0, KEY, kind, value).with_id(id);
    Event::new(offset, EventData::Expression(expression))
}

/// What the synth fixture plays for `events` on its first port: each note a voice of
/// `velocity × (1 + pressure) × cos(phase)` at its key plus its tuning, from the sample of its
/// note on to the sample of its note off.
fn expected(events: &[Event]) -> Vec<f32> {
    struct Voice {
        id: u32,
        velocity: f64,
        tuning: f64,
        pressure: f64,
        phase: f64,
    }
    let mut voices: Vec<Voice> = Vec::new();
    let mut pending = events.iter().peekable();
    (0..FRAMES)
        .map(|frame| {
            while let Some(event) = pending.next_if(|event| event.offset <= frame) {
                match &event.data {
                    EventData::NoteOn(note) => voices.push(Voice {
                        id: note.id.unwrap(),
                        velocity: note.velocity,
                        tuning: 0.0,
                        pressure: 0.0,
                        phase: 0.0,
                    }),
                    EventData::NoteOff(note) => voices.retain(|voice| Some(voice.id) != note.id),
                    EventData::Expression(expression) => {
                        for voice in voices.iter_mut().filter(|v| Some(v.id) == expression.id) {
                            match expression.kind {
                                ExpressionKind::Tuning => voice.tuning = expression.value,
                                ExpressionKind::Pressure => voice.pressure = expression.value,
                                kind => panic!("the synth does not play {kind:?}"),
                            }
                        }
                    }
                    data => panic!("unexpected {data:?}"),
                }
            }
            let mut sample = 0.0;
            for voice in &mut voices {
                sample += voice.velocity * (1.0 + voice.pressure) * (voice.phase * TAU).cos();
                let pitch = f64::from(KEY) + voice.tuning;
                let frequency = 440.0 * 2f64.powf((pitch - 69.0) / 12.0);
                voice.phase = (voice.phase + frequency / SAMPLE_RATE).fract();
            }
            sample as f32
        })
        .collect()
}

/// Processes one block of `events` and returns the left channel and the chain's output events.
fn run(chain: &mut Chain, config: &RoutedChainConfig, events: &[Event]) -> (Vec<f32>, Vec<Event>) {
    let outputs = config.output_channels();
    let mut output = vec![vec![0.0; FRAMES]; outputs];
    let mut views: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let mut produced = Vec::new();
    chain
        .process_audio_f32(
            &BlockContext::new(FRAMES),
            &[],
            &mut views,
            &[],
            events,
            &mut produced,
        )
        .unwrap();
    (output.into_iter().next().unwrap_or_default(), produced)
}

fn assert_plays(output: &[f32], events: &[Event], context: &str) {
    let expected = expected(events);
    for (frame, (actual, expected)) in output.iter().zip(&expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-5,
            "{context}: frame {frame} is {actual}, expected {expected}"
        );
    }
}

/// The routing fixture's events: the chain's external event input to its expression port, and
/// its output collected.
fn routing_slot(chain: &mut Chain, slot: usize) -> (SlotEventConfig, u64) {
    let ports = chain.event_ports(slot).unwrap();
    let id = |direction, index| {
        ports
            .iter()
            .find(|port| port.direction == direction && port.index == index)
            .unwrap()
            .id
    };
    let output = id(AudioDirection::Output, 0);
    let events = SlotEventConfig {
        inputs: vec![EventInputRoute {
            port: id(AudioDirection::Input, 2),
            source: EventSource::External { port: 0 },
        }],
        outputs: vec![output],
    };
    (events, output)
}

#[test]
#[ignore = "needs helper and synth fixtures (.ps1 or .sh build scripts)"]
fn overlapping_notes_on_one_key_take_their_own_tuning_and_pressure() {
    let events = [
        note_on(0, 1, 0.5),
        // A second note on the same key, told apart by its ID.
        note_on(64, 2, 0.25),
        // An octave up for the second note alone.
        expression(128, 2, ExpressionKind::Tuning, 12.0),
        expression(192, 1, ExpressionKind::Pressure, 0.5),
        // The first note ends; the second keeps sounding with its tuning.
        note_off(256, 1),
        note_off(320, 2),
    ];
    for format in FORMATS {
        let mut chain = spawn(&[synth(format)]);
        let config = chain
            .main_bus_config(SAMPLE_RATE, FRAMES, Layout::None, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        let (output, _) = run(&mut chain, &config, &events);
        assert_eq!(output[0], 0.5, "{format:?}");
        assert_plays(&output, &events, &format!("{format:?}"));
        assert!(output[320..].iter().all(|&s| s == 0.0), "{format:?}");
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn notes_and_expressions_a_plugin_sends_reach_the_application() {
    let events = [
        note_on(4, 7, 0.5),
        expression(10, 7, ExpressionKind::Tuning, 15.0),
        expression(12, 7, ExpressionKind::Pressure, 0.5),
        Event::new(20, EventData::NoteOff(Note::new(0, KEY, 0.25).with_id(7))),
    ];
    for format in FORMATS {
        let mut chain = spawn(&[routing(format)]);
        let mut config = chain
            .main_bus_config(SAMPLE_RATE, FRAMES, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        config.slots[0].events = routing_slot(&mut chain, 0).0;
        chain.prepare_audio(&config).unwrap();
        let input = vec![0.0; FRAMES];
        let mut output = vec![vec![0.0; FRAMES]; 2];
        let mut views: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        let mut produced = Vec::new();
        chain
            .process_audio_f32(
                &BlockContext::new(FRAMES),
                &[&input, &input],
                &mut views,
                &[],
                &events,
                &mut produced,
            )
            .unwrap();
        assert_eq!(produced, events, "{format:?}");
    }
}

#[test]
#[ignore = "needs helper, routing and synth fixtures (.ps1 or .sh build scripts)"]
fn expressions_a_plugin_sends_play_the_next_slot_at_their_offsets() {
    let events = [
        note_on(0, 3, 0.5),
        expression(100, 3, ExpressionKind::Tuning, 12.0),
        expression(200, 3, ExpressionKind::Pressure, 0.5),
        note_off(300, 3),
    ];
    for source in FORMATS {
        for instrument in FORMATS {
            let mut chain = spawn(&[routing(source), synth(instrument)]);
            // The routing fixture's audio buses stay inactive; its output events reach the
            // synth's first input.
            let mut config = chain
                .main_bus_config(
                    SAMPLE_RATE,
                    FRAMES,
                    Layout::None,
                    &[Layout::None, Layout::Stereo],
                )
                .unwrap();
            config.slots[0].events = routing_slot(&mut chain, 0).0;
            chain.prepare_audio(&config).unwrap();
            let (played, _) = run(&mut chain, &config, &events);
            assert_plays(&played, &events, &format!("{source:?} to {instrument:?}"));
        }
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn notes_reach_a_midi_port_in_their_midi_form() {
    // The CLAP routing fixture's first input takes MIDI alone; it moves notes up an octave.
    let mut chain = spawn(&[routing(PluginFormat::Clap)]);
    let config = chain
        .main_bus_config(SAMPLE_RATE, FRAMES, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let input = vec![0.0; FRAMES];
    let mut output = vec![vec![0.0; FRAMES]; 2];
    let mut views: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let mut produced = Vec::new();
    chain
        .process_audio_f32(
            &BlockContext::new(FRAMES),
            &[&input, &input],
            &mut views,
            &[],
            &[
                Event::new(5, EventData::NoteOn(Note::new(0, 60, 0.5).with_id(9))),
                // Tuning has no MIDI 1.0 form; it does not reach the port.
                expression(6, 9, ExpressionKind::Tuning, 1.0),
                Event::new(7, EventData::NoteOff(Note::new(0, 60, 0.0).with_id(9))),
            ],
            &mut produced,
        )
        .unwrap();
    assert_eq!(
        produced,
        [
            Event::note_on(5, 0, 72, 64),
            Event::control_change(5, 0, 20, 60),
            Event::note_off(7, 0, 72, 0),
        ]
    );
}
