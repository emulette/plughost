//! Control of a running realtime endpoint through real helper processes: replacing a slot's
//! instance between quanta (restored state, presets, factory presets, a slot prepared again),
//! resynchronizing without stopping, and ending the notes the application received once a new
//! generation starts. A synthetic audio callback drives the endpoint as in `realtime.rs`. Build
//! the helper and the test plugins first (see `support`).

use std::thread;
use std::time::{Duration, Instant};

use plughost::{
    AudioDirection, AutomationEvent, BlockContext, Chain, Error, Event, EventData, EventInputRoute,
    EventSource, HostIdentity, Layout, Note, ParameterChange, PluginFormat, PluginRef,
    PresetLocation, RealtimeConfig, RealtimeEndpoint, RealtimeOutput, StatePurpose, Timeouts,
};

mod support;

use support::{delay, delay_variant, fixture, prepare, routing, spawn, spawn_with};

const QUANTUM: usize = 128;
/// The delay fixture's latency, by which it passes its input through.
const LATENCY: usize = 480;
/// The time the manual driver leaves between callbacks.
const PACE: Duration = Duration::from_millis(10);
/// The process call the `stall-in-process` fixture holds up, counted from 0.
const STALL_AT: usize = 8;
/// The CLAP delay fixture's gain parameter.
const GAIN: u64 = 0;

const PROCESSED: RealtimeOutput = RealtimeOutput::Processed;
const PRIMING: RealtimeOutput = RealtimeOutput::Fallback(plughost::FallbackReason::Priming);
const RESYNC: RealtimeOutput = RealtimeOutput::Fallback(plughost::FallbackReason::Resync);

type Stereo = [Vec<f32>; 2];

fn silence(frames: usize) -> Stereo {
    [vec![0.0; frames], vec![0.0; frames]]
}

/// Callback `index`'s input: a different sine on each channel.
fn sine(index: usize) -> Stereo {
    let channel = |offset: f32| {
        (0..QUANTUM)
            .map(|frame| (((index * QUANTUM + frame) as f32) * 0.01 + offset).sin() * 0.5)
            .collect()
    };
    [channel(0.0), channel(1.0)]
}

/// Serves one callback with `events`, returning what it played and produced.
fn serve(
    endpoint: &mut RealtimeEndpoint,
    input: &Stereo,
    automation: &[AutomationEvent],
    events: &[Event],
) -> (RealtimeOutput, Stereo, Vec<Event>) {
    let mut output = silence(QUANTUM);
    let [left, right] = &mut output;
    let mut produced = Vec::with_capacity(plughost::MAX_BLOCK_EVENTS);
    let played = endpoint
        .process_audio_f32(
            &BlockContext::new(QUANTUM),
            &[&input[0], &input[1]],
            &mut [left, right],
            automation,
            events,
            &mut produced,
        )
        .unwrap();
    (played, output, produced)
}

/// One callback of the manual driver.
fn callback(endpoint: &mut RealtimeEndpoint, input: &Stereo) -> (RealtimeOutput, Stereo) {
    thread::sleep(PACE);
    let (played, output, _) = serve(endpoint, input, &[], &[]);
    (played, output)
}

/// What a prepared chain plays for `inputs`, one block each.
fn offline(chain: &mut Chain, inputs: impl IntoIterator<Item = Stereo>) -> Vec<Stereo> {
    inputs
        .into_iter()
        .map(|input| {
            let mut output = silence(QUANTUM);
            let [left, right] = &mut output;
            chain
                .process_audio_f32(
                    &BlockContext::new(QUANTUM),
                    &[&input[0], &input[1]],
                    &mut [left, right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
            output
        })
        .collect()
}

/// A prepared chain of `plugin`, with `state` restored when there is one.
fn prepared(plugin: &PluginRef, state: Option<&plughost::PluginState>) -> Chain {
    let mut chain = spawn(std::slice::from_ref(plugin));
    prepare(&mut chain);
    if let Some(state) = state {
        chain
            .restore_state(0, state, StatePurpose::Project)
            .unwrap();
    }
    chain
}

/// A started endpoint for the prepared chain.
fn started(chain: &mut Chain, config: &RealtimeConfig) -> RealtimeEndpoint {
    let endpoint = chain.realtime_endpoint(config).unwrap();
    chain.start_realtime().unwrap();
    endpoint
}

/// Whether each channel of `outputs`, end to end, is `inputs` end to end delayed by `delay`
/// frames, silent before.
fn delayed(outputs: &[Stereo], inputs: &[Stereo], delay: usize) -> bool {
    (0..2).all(|channel| {
        let output: Vec<f32> = outputs.iter().flat_map(|o| o[channel].clone()).collect();
        let input: Vec<f32> = inputs.iter().flat_map(|i| i[channel].clone()).collect();
        output.iter().enumerate().all(|(frame, &sample)| {
            sample == frame.checked_sub(delay).map_or(0.0, |source| input[source])
        })
    })
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_state_restored_while_running_plays_from_the_next_quantum() {
    const SWAP: usize = 6;
    const CALLBACKS: usize = 16;
    let plugin = delay(PluginFormat::Clap);
    let mut chain = prepared(&plugin, None);
    chain.set_parameter(0, GAIN, 0.25).unwrap();
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain.set_parameter(0, GAIN, 1.0).unwrap();
    let before = offline(&mut prepared(&plugin, None), (0..SWAP).map(sine));
    let after = offline(
        &mut prepared(&plugin, Some(&state)),
        (SWAP..CALLBACKS).map(sine),
    );

    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let mut outputs = Vec::new();
    for index in 0..CALLBACKS {
        if index == SWAP {
            // The worker completed the quanta submitted so far; the next one is the new
            // instance's first.
            thread::sleep(PACE);
            chain
                .restore_state(0, &state, StatePurpose::Project)
                .unwrap();
        }
        let (played, output) = callback(&mut endpoint, &sine(index));
        if index > 0 {
            assert_eq!(played, PROCESSED, "callback {index}");
            outputs.push(output);
        }
    }
    let expected: Vec<Stereo> = before.into_iter().chain(after).collect();
    for (index, output) in outputs.iter().enumerate() {
        assert!(*output == expected[index], "quantum {index}");
    }
    // The new instance starts with an empty delay line, at the restored gain.
    assert!(outputs[SWAP + 4][0].iter().any(|&sample| sample != 0.0));
    let status = chain.realtime_status().unwrap();
    assert!(!status.resync, "{status:?}");
    assert_eq!((status.missed, status.failures), (0, 0));
    assert_eq!(status.latency, (LATENCY + QUANTUM) as u32);
}

#[test]
#[ignore = "needs helper and the delay and presets fixtures (.ps1 or .sh build scripts)"]
fn presets_and_factory_presets_apply_while_running() {
    let ones = || [vec![1.0; QUANTUM], vec![1.0; QUANTUM]];
    // A VST3 factory program, then an imported preset, each from the next quantum on.
    let plugin = delay(PluginFormat::Vst3);
    let mut chain = prepared(&plugin, None);
    let unity = chain.export_preset(0).unwrap();
    let presets = chain.factory_presets(0).unwrap();
    let mut quarter = prepared(&plugin, None);
    quarter.select_factory_preset(0, &presets[1].id).unwrap();
    let quarter = offline(&mut quarter, (0..8).map(|_| ones()));
    let mut unity_chain = prepared(&plugin, None);
    unity_chain.import_preset(0, &unity).unwrap();
    let unity_output = offline(&mut unity_chain, (0..8).map(|_| ones()));

    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    callback(&mut endpoint, &ones());
    thread::sleep(PACE);
    chain.select_factory_preset(0, &presets[1].id).unwrap();
    let mut outputs = Vec::new();
    for _ in 0..9 {
        let (played, output) = callback(&mut endpoint, &ones());
        assert_eq!(played, PROCESSED);
        outputs.push(output);
    }
    assert!(outputs[1..] == quarter[..]);
    thread::sleep(PACE);
    chain.import_preset(0, &unity).unwrap();
    let mut outputs = Vec::new();
    for _ in 0..9 {
        let (played, output) = callback(&mut endpoint, &ones());
        assert_eq!(played, PROCESSED);
        outputs.push(output);
    }
    assert!(outputs[1..] == unity_output[..]);
    let status = chain.realtime_status().unwrap();
    assert!(!status.resync && status.missed == 0, "{status:?}");

    // A CLAP preset from the plugin's own location.
    let mut chain = spawn_with(
        &[fixture(
            PluginFormat::Clap,
            "plughost-test-presets",
            "com.studio.plughost.test-presets",
        )],
        &HostIdentity::default(),
        Timeouts::default(),
    );
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let play = |endpoint: &mut RealtimeEndpoint| {
        thread::sleep(PACE);
        let mut output = silence(QUANTUM);
        let [left, right] = &mut output;
        let played = endpoint
            .process_audio_f32(
                &BlockContext::new(QUANTUM),
                &[],
                &mut [left, right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        (played, output)
    };
    for index in 0..3 {
        let (played, output) = play(&mut endpoint);
        if index > 0 {
            assert_eq!(
                (played, output),
                (PROCESSED, [vec![1.0; QUANTUM], vec![1.0; QUANTUM]])
            );
        }
    }
    thread::sleep(PACE);
    chain
        .load_discovered_preset(0, &PresetLocation::Plugin, Some("quarter"))
        .unwrap();
    // The quantum submitted before the change plays first.
    assert_eq!(play(&mut endpoint).1[0], vec![1.0; QUANTUM]);
    for _ in 0..3 {
        assert_eq!(
            play(&mut endpoint),
            (PROCESSED, [vec![0.25; QUANTUM], vec![0.25; QUANTUM]])
        );
    }
}

#[test]
#[ignore = "needs helper and the latency-change fixture (.ps1 or .sh build scripts)"]
fn a_latency_change_starts_a_new_generation() {
    /// The latency the `latency-change` variant announces and takes once it has processed 2,400
    /// frames.
    const CHANGED: usize = 2 * LATENCY;
    let plugin = delay_variant(PluginFormat::Vst3, "latency-change", 0xE);
    let mut chain = prepared(&plugin, None);
    let fresh = chain.save_state(0, StatePurpose::Project).unwrap();
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    for index in 0..20 {
        callback(&mut endpoint, &sine(index));
    }
    // The resync's reset applies the announced latency, which the new generation reports.
    assert_eq!(chain.resync_realtime().unwrap(), (CHANGED + QUANTUM) as u32);
    assert_eq!(chain.latency(), CHANGED as u32);
    let mut old_inputs: Vec<Stereo> = (20..32).map(sine).collect();
    let mut old_outputs = Vec::new();
    for (index, input) in old_inputs.iter().enumerate() {
        let (played, output) = callback(&mut endpoint, input);
        assert_eq!(played, if index == 0 { PRIMING } else { PROCESSED });
        if index > 0 {
            old_outputs.push(output);
        }
    }
    assert!(delayed(&old_outputs, &old_inputs, CHANGED));
    let status = chain.realtime_status().unwrap();
    assert!(status.running && !status.resync, "{status:?}");
    assert_eq!(status.latency, (CHANGED + QUANTUM) as u32);

    // A fresh instance has the first latency again. The callbacks go on while the slot is
    // replaced; no output of the new instance plays in the old generation.
    let restoring = thread::spawn(move || {
        let restored = chain.restore_state(0, &fresh, StatePurpose::Project);
        (chain, restored)
    });
    let mut played = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let started_waiting = Instant::now();
    let mut index = 32;
    while !restoring.is_finished() || played.len() < 8 || played.last() != Some(&PROCESSED) {
        assert!(started_waiting.elapsed() < Duration::from_secs(30));
        let input = sine(index);
        let (output, samples) = callback(&mut endpoint, &input);
        played.push(output);
        inputs.push(input);
        outputs.push(samples);
        index += 1;
    }
    for _ in 0..8 {
        let input = sine(index);
        let (output, samples) = callback(&mut endpoint, &input);
        played.push(output);
        inputs.push(input);
        outputs.push(samples);
        index += 1;
    }
    let (chain, restored) = restoring.join().unwrap();
    restored.unwrap();
    assert_eq!(chain.latency(), LATENCY as u32);
    // The old generation plays until the replacement, then the fallback, then the new one.
    let priming = played.iter().position(|&output| output == PRIMING).unwrap();
    let first_fallback = played
        .iter()
        .position(|&output| output != PROCESSED)
        .unwrap();
    assert!(played[first_fallback..priming].iter().all(|&o| o == RESYNC));
    // What the old generation played is the old instance's output.
    old_outputs.extend_from_slice(&outputs[..first_fallback]);
    old_inputs.extend_from_slice(&inputs[..first_fallback.saturating_sub(1)]);
    assert!(delayed(&old_outputs, &old_inputs, CHANGED));
    assert!(played[priming + 1..].iter().all(|&o| o == PROCESSED));
    assert!(delayed(
        &outputs[priming + 1..],
        &inputs[priming..],
        LATENCY
    ));
    let status = chain.realtime_status().unwrap();
    assert!(
        !status.resync && status.missed == 0 && status.failures == 0,
        "{status:?}"
    );
    assert_eq!(status.latency, (LATENCY + QUANTUM) as u32);
}

#[test]
#[ignore = "needs helper and the delay and stall-in-process fixtures (.ps1 or .sh build scripts)"]
fn a_resync_after_an_overflow_plays_the_offline_output_without_a_stop() {
    let mut stall = support::stall::Stall::new();
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Clap, "stall-in-process", 0x18)],
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    assert!(matches!(chain.resync_realtime(), Err(Error::NotPrepared)));
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    for index in 0..=STALL_AT {
        callback(&mut endpoint, &sine(index));
    }
    stall.wait_until_entered();
    for index in STALL_AT + 1..STALL_AT + 4 {
        callback(&mut endpoint, &sine(index));
    }
    drop(stall);
    let started_waiting = Instant::now();
    while chain.realtime_status().unwrap().late < 3 {
        assert!(started_waiting.elapsed() < Duration::from_secs(10));
        thread::sleep(PACE);
    }
    let status = chain.realtime_status().unwrap();
    assert!(status.resync && status.overflows == 1, "{status:?}");

    assert_eq!(chain.resync_realtime().unwrap(), (LATENCY + QUANTUM) as u32);
    let first = STALL_AT + 4;
    let expected = offline(
        &mut prepared(&delay(PluginFormat::Clap), None),
        (first..first + 12).map(sine),
    );
    for (offset, index) in (first..first + 13).enumerate() {
        let (played, output) = callback(&mut endpoint, &sine(index));
        if offset == 0 {
            assert_eq!(played, PRIMING);
            continue;
        }
        assert_eq!(played, PROCESSED, "callback {index}");
        // The reset plugin plays as a fresh one, from the first quantum after the resync on.
        assert!(output == expected[offset - 1], "callback {index}");
    }
    let status = chain.realtime_status().unwrap();
    assert!(status.running && !status.resync, "{status:?}");
    assert_eq!(status.processed, (STALL_AT + 12) as u64);

    // A stopped endpoint has nothing to resynchronize.
    chain.stop_realtime().unwrap();
    assert_eq!(chain.resync_realtime().unwrap(), (LATENCY + QUANTUM) as u32);
    assert!(!chain.realtime_status().unwrap().running);
}

#[test]
#[ignore = "needs helper and the routing fixture (.ps1 or .sh build scripts)"]
fn notes_the_application_received_end_after_a_resync() {
    // The CLAP routing fixture moves MIDI notes on its first input up an octave, and passes
    // notes with IDs on its third input on as they are, to its one output.
    let mut chain = spawn(&[routing(PluginFormat::Clap)]);
    let mut config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    let ports = chain.event_ports(0).unwrap();
    let input = |index| {
        ports
            .iter()
            .find(|port| port.direction == AudioDirection::Input && port.index == index)
            .unwrap()
            .id
    };
    config.event_inputs = 2;
    config.slots[0].events.inputs = vec![
        EventInputRoute {
            port: input(0),
            source: EventSource::External { port: 0 },
        },
        EventInputRoute {
            port: input(2),
            source: EventSource::External { port: 1 },
        },
    ];
    chain.prepare_audio(&config).unwrap();
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let note = |id, key| Note::new(0, key, 0.5).with_id(id);
    let starts = [
        Event::note_on(0, 0, 60, 100),
        Event::note_on(0, 0, 62, 100),
        Event::new(10, EventData::NoteOn(note(5, 64))).on_port(1),
        Event::new(10, EventData::NoteOn(note(6, 65))).on_port(1),
    ];
    let ends = [
        Event::note_off(0, 0, 62, 0),
        Event::new(10, EventData::NoteOff(note(6, 65))).on_port(1),
    ];
    let silent = silence(QUANTUM);
    thread::sleep(PACE);
    serve(&mut endpoint, &silent, &[], &starts);
    thread::sleep(PACE);
    let (_, _, produced) = serve(&mut endpoint, &silent, &[], &ends);
    assert_eq!(produced.len(), 6, "{produced:?}");
    thread::sleep(PACE);
    let (_, _, produced) = serve(&mut endpoint, &silent, &[], &[]);
    assert_eq!(produced.len(), 2, "{produced:?}");

    chain.resync_realtime().unwrap();
    thread::sleep(PACE);
    let (played, _, produced) = serve(&mut endpoint, &silent, &[], &[]);
    assert_eq!(played, PRIMING);
    assert_eq!(
        produced,
        [
            Event::note_off(0, 0, 72, 0),
            Event::new(0, EventData::NoteOff(Note::new(0, 64, 0.0).with_id(5))),
        ]
    );
    thread::sleep(PACE);
    let (played, _, produced) = serve(&mut endpoint, &silent, &[], &[]);
    assert_eq!((played, produced), (PROCESSED, Vec::new()));
    assert_eq!(chain.realtime_status().unwrap().dropped_events, 0);
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_slot_prepared_again_drops_its_restart_request() {
    /// The CLAP delay fixture's parameter whose change makes it request a restart.
    const RESTART_MODE: u64 = 10;
    let restart = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: RESTART_MODE,
            offset: 0,
            value: 0.5,
        },
    }];
    let restart_required = |chain: &mut Chain| {
        chain.take_changes().unwrap().snapshot[0]
            .unwrap()
            .restart_required
    };

    // Offline, the request refuses blocks until the slot is prepared again.
    let plugin = delay(PluginFormat::Clap);
    let mut chain = spawn(std::slice::from_ref(&plugin));
    assert!(matches!(chain.reprepare_slot(0), Err(Error::NotPrepared)));
    prepare(&mut chain);
    chain.set_parameter(0, GAIN, 0.375).unwrap();
    let block = |chain: &mut Chain, automation: &[AutomationEvent]| {
        let input = sine(0);
        let mut output = silence(QUANTUM);
        let [left, right] = &mut output;
        chain.process_audio_f32(
            &BlockContext::new(QUANTUM),
            &[&input[0], &input[1]],
            &mut [left, right],
            automation,
            &[],
            &mut Vec::new(),
        )
    };
    assert_eq!(
        block(&mut chain, &restart).unwrap_err().kind(),
        plughost::FailureKind::RestartRequired
    );
    assert!(restart_required(&mut chain));
    chain.reprepare_slot(0).unwrap();
    assert!(!restart_required(&mut chain));
    block(&mut chain, &[]).unwrap();
    let gain = chain.parameters(0).unwrap()[0].1;
    assert_eq!(gain, 0.375);

    // While the endpoint runs, the stream goes on through the request and the replacement, and
    // a request of the new instance waits too.
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    thread::sleep(PACE);
    serve(&mut endpoint, &sine(0), &restart, &[]);
    for index in 1..4 {
        assert_eq!(callback(&mut endpoint, &sine(index)).0, PROCESSED);
    }
    assert!(restart_required(&mut chain));
    thread::sleep(PACE);
    chain.reprepare_slot(0).unwrap();
    assert!(!restart_required(&mut chain));
    thread::sleep(PACE);
    serve(&mut endpoint, &sine(4), &restart, &[]);
    for index in 5..9 {
        assert_eq!(callback(&mut endpoint, &sine(index)).0, PROCESSED);
    }
    assert!(restart_required(&mut chain));
    let status = chain.realtime_status().unwrap();
    assert!(!status.resync && status.failures == 0, "{status:?}");
    chain.stop_realtime().unwrap();
    chain.reprepare_slot(0).unwrap();
    block(&mut chain, &[]).unwrap();
}
