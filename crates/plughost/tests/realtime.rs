//! Isolated realtime processing through real helper processes, driven by a synthetic audio
//! callback: each callback hands one quantum to the helper and plays the output of the quantum
//! submitted `latency` callbacks before, or the fallback. The manual driver leaves the helper far
//! more time than it needs for each quantum; the paced smoke test calls back at the audio rate
//! and skips ahead when it falls behind, as audio devices do, without asserting how many quanta
//! missed. Build the helper and the test plugins first (see `support`).

use std::thread;
use std::time::{Duration, Instant};

use plughost::{
    AutomationEvent, BlockContext, Chain, Error, FailureKind, Fallback, FallbackReason,
    ParameterChange, PluginFormat, PluginRef, RealtimeConfig, RealtimeEndpoint, RealtimeOutput,
    StatePurpose, Timeouts,
};

mod support;

use support::{delay, delay_variant, prepare, routing, spawn, spawn_with, stereo_config};

const QUANTUM: usize = 128;
/// The delay fixture's latency, by which it passes its input through.
const LATENCY: usize = 480;
/// The time the manual driver leaves between callbacks.
const PACE: Duration = Duration::from_millis(10);
/// The process call the `stall-in-process` fixture holds up, counted from 0.
const STALL_AT: u64 = 8;

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

/// Serves one callback in the endpoint's quantum, or in the input's length.
fn serve(endpoint: &mut RealtimeEndpoint, input: &Stereo) -> (RealtimeOutput, Stereo) {
    automate(endpoint, input, &[])
}

/// Serves one callback with `automation`.
fn automate(
    endpoint: &mut RealtimeEndpoint,
    input: &Stereo,
    automation: &[AutomationEvent],
) -> (RealtimeOutput, Stereo) {
    let frames = input[0].len();
    let mut output = silence(frames);
    let [left, right] = &mut output;
    let played = endpoint
        .process_audio_f32(
            &BlockContext::new(frames),
            &[&input[0], &input[1]],
            &mut [left, right],
            automation,
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    (played, output)
}

/// One callback of the manual driver.
fn callback(endpoint: &mut RealtimeEndpoint, input: &Stereo) -> (RealtimeOutput, Stereo) {
    thread::sleep(PACE);
    serve(endpoint, input)
}

/// The plugin's output for each of `callbacks` sine quanta processed offline, one block each.
fn offline(plugin: &PluginRef, callbacks: usize) -> Vec<Stereo> {
    let mut chain = spawn(std::slice::from_ref(plugin));
    prepare(&mut chain);
    (0..callbacks)
        .map(|index| {
            let input = sine(index);
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

/// A started endpoint for the prepared chain.
fn started(chain: &mut Chain, config: &RealtimeConfig) -> RealtimeEndpoint {
    let endpoint = chain.realtime_endpoint(config).unwrap();
    chain.start_realtime().unwrap();
    endpoint
}

#[test]
#[ignore = "needs helper and the delay and routing fixtures (.ps1 or .sh build scripts)"]
fn clap_realtime_output_is_the_offline_output_latency_quanta_later() {
    offline_equivalence(delay(PluginFormat::Clap), LATENCY);
    offline_equivalence(routing(PluginFormat::Clap), 0);
}

#[test]
#[ignore = "needs helper and the delay and routing fixtures (.ps1 or .sh build scripts)"]
fn vst3_realtime_output_is_the_offline_output_latency_quanta_later() {
    offline_equivalence(delay(PluginFormat::Vst3), LATENCY);
    offline_equivalence(routing(PluginFormat::Vst3), 0);
}

/// Without misses, the endpoint plays the plugin's offline output `latency` quanta later, from the
/// first output after priming on. The routing fixture has no latency, so its first output is not
/// silent.
fn offline_equivalence(plugin: PluginRef, plugin_latency: usize) {
    const CALLBACKS: usize = 24;
    let expected = offline(&plugin, CALLBACKS);
    let mut chain = spawn(&[plugin]);
    prepare(&mut chain);
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM).with_latency(2))
        .unwrap();
    let whole = (plugin_latency + 2 * QUANTUM) as u32;
    assert_eq!(chain.start_realtime().unwrap(), whole);
    for index in 0..CALLBACKS {
        let (played, output) = callback(&mut endpoint, &sine(index));
        if index < 2 {
            assert_eq!(played, RealtimeOutput::Fallback(FallbackReason::Priming));
            assert_eq!(output, silence(QUANTUM));
            assert_eq!(endpoint.last_completion(), None);
            continue;
        }
        assert_eq!(played, RealtimeOutput::Processed, "callback {index}");
        // The helper completed the quantum well before the callback that played it.
        let completion = endpoint.last_completion().unwrap();
        assert!(
            completion > Duration::ZERO && completion < PACE,
            "{completion:?}"
        );
        assert_eq!(output, expected[index - 2], "callback {index}");
    }
    let status = chain.realtime_status().unwrap();
    assert!(status.running && !status.resync);
    assert_eq!(
        (
            status.callbacks,
            status.processed,
            status.missed,
            status.late
        ),
        (CALLBACKS as u64, CALLBACKS as u64 - 2, 0, 0)
    );
    assert_eq!(status.latency, whole);
    chain.stop_realtime().unwrap();
    assert_eq!(
        callback(&mut endpoint, &sine(0)).0,
        RealtimeOutput::Fallback(FallbackReason::Stopped)
    );
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn the_dry_fallback_lines_up_with_the_processed_output() {
    const START: usize = 8;
    const STOP: usize = 18;
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let config = RealtimeConfig::new(QUANTUM).with_fallback(Fallback::Dry);
    let mut endpoint = chain.realtime_endpoint(&config).unwrap();
    // Impulses whose delayed copies fall on neither edge of a callback, so a fallback has no step
    // to ramp out of, at levels every crossfade gain keeps exact. The plugin hears only what was
    // submitted, so the callbacks whose delayed input the first output covers are silent.
    let impulses = |index: usize| {
        let mut input = silence(QUANTUM);
        if index < 3 || (START..STOP).contains(&index) {
            input[index % 2][37] = 0.5;
            input[(index + 1) % 2][90] = -0.25;
        }
        input
    };
    let mut played = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for index in 0..24 {
        match index {
            START => {
                chain.start_realtime().unwrap();
            }
            STOP => chain.stop_realtime().unwrap(),
            _ => {}
        }
        let input = impulses(index);
        let (output, samples) = callback(&mut endpoint, &input);
        played.push(output);
        inputs.push(input);
        outputs.push(samples);
    }
    let stopped = RealtimeOutput::Fallback(FallbackReason::Stopped);
    assert!(played[..START].iter().all(|&output| output == stopped));
    assert_eq!(
        played[START],
        RealtimeOutput::Fallback(FallbackReason::Priming)
    );
    assert!(
        played[START + 1..STOP]
            .iter()
            .all(|&output| output == RealtimeOutput::Processed)
    );
    assert!(played[STOP..].iter().all(|&output| output == stopped));
    // Whichever source a callback played, it played the input delayed by the whole latency.
    let delay = LATENCY + QUANTUM;
    let nonzero = |quanta: &[Stereo], channel: usize, shift: usize| -> Vec<(usize, f32)> {
        quanta
            .iter()
            .flat_map(|quantum| quantum[channel].iter().copied())
            .enumerate()
            .filter(|&(_, sample)| sample != 0.0)
            .map(|(frame, sample)| (frame + shift, sample))
            .filter(|&(frame, _)| frame < quanta.len() * QUANTUM)
            .collect()
    };
    for channel in 0..2 {
        assert_eq!(
            nonzero(&outputs, channel, 0),
            nonzero(&inputs, channel, delay),
            "channel {channel}"
        );
    }
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn reconfiguring_the_chain_under_a_running_endpoint_is_refused() {
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let config = stereo_config(&mut chain);
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    let gain = chain.parameters(0).unwrap()[0].0.id;
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let running = |result: Result<(), Error>| matches!(result, Err(Error::RealtimeRunning));
    assert!(running(chain.reset()));
    assert!(running(chain.reprepare()));
    assert!(running(chain.prepare_audio(&config).map(|_| ())));
    let input = sine(0);
    let mut output = silence(QUANTUM);
    let [left, right] = &mut output;
    assert!(running(chain.process_audio_f32(
        &BlockContext::new(QUANTUM),
        &[&input[0], &input[1]],
        &mut [left, right],
        &[],
        &[],
        &mut Vec::new(),
    )));
    assert!(running(
        chain
            .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
            .map(|_| ())
    ));
    // Parameter edits and replacements of a slot's instance reach the running chain between
    // quanta.
    chain.set_parameter(0, gain, 0.25).unwrap();
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    chain.reprepare_slot(0).unwrap();
    for index in 0..4 {
        callback(&mut endpoint, &sine(index));
    }
    assert_eq!(chain.realtime_status().unwrap().processed, 3);

    chain.stop_realtime().unwrap();
    chain.reset().unwrap();
    chain.prepare_audio(&config).unwrap();
    // Preparing the chain again retired the endpoint for good.
    assert!(matches!(chain.start_realtime(), Err(Error::NotPrepared)));
    assert_eq!(chain.realtime_status(), None);
    assert_eq!(
        callback(&mut endpoint, &sine(4)).0,
        RealtimeOutput::Fallback(FallbackReason::Stopped)
    );
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_restart_request_waits_while_the_endpoint_runs() {
    /// The CLAP delay fixture's parameter whose change makes it request a restart.
    const RESTART_MODE: u64 = 10;
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let automation = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: RESTART_MODE,
            offset: 0,
            value: 0.5,
        },
    }];
    thread::sleep(PACE);
    automate(&mut endpoint, &sine(0), &automation);
    // The plugin goes on with its current activation, and the request is reported.
    for index in 1..6 {
        assert_eq!(
            callback(&mut endpoint, &sine(index)).0,
            RealtimeOutput::Processed,
            "callback {index}"
        );
    }
    let status = chain.realtime_status().unwrap();
    assert!(!status.resync && status.failures == 0, "{status:?}");
    assert!(
        chain.take_changes().unwrap().snapshot[0]
            .unwrap()
            .restart_required
    );

    // Once the endpoint stops, the chain refuses to process until it is prepared again.
    chain.stop_realtime().unwrap();
    let input = sine(6);
    let mut output = silence(QUANTUM);
    let [left, right] = &mut output;
    let mut process = |chain: &mut Chain| {
        chain.process_audio_f32(
            &BlockContext::new(QUANTUM),
            &[&input[0], &input[1]],
            &mut [&mut *left, &mut *right],
            &[],
            &[],
            &mut Vec::new(),
        )
    };
    assert_eq!(
        process(&mut chain).unwrap_err().kind(),
        FailureKind::RestartRequired
    );
    chain.reprepare().unwrap();
    process(&mut chain).unwrap();
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_callback_of_another_size_resyncs_until_the_endpoint_restarts() {
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    for index in 0..3 {
        callback(&mut endpoint, &sine(index));
    }
    let short = sine(3).map(|mut channel| {
        channel.truncate(QUANTUM / 2);
        channel
    });
    let resync = RealtimeOutput::Fallback(FallbackReason::Resync);
    assert_eq!(callback(&mut endpoint, &short).0, resync);
    assert_eq!(callback(&mut endpoint, &sine(4)).0, resync);
    let status = chain.realtime_status().unwrap();
    assert!(status.running && status.resync);
    assert_eq!((status.size_mismatches, status.processed), (1, 2));

    chain.stop_realtime().unwrap();
    chain.reset().unwrap();
    chain.start_realtime().unwrap();
    assert_eq!(
        callback(&mut endpoint, &sine(5)).0,
        RealtimeOutput::Fallback(FallbackReason::Priming)
    );
    assert_eq!(
        callback(&mut endpoint, &sine(6)).0,
        RealtimeOutput::Processed
    );
    let status = chain.realtime_status().unwrap();
    assert!(!status.resync);
    assert_eq!(status.processed, 3);
}

#[test]
#[ignore = "needs helper and the stall-in-process fixture (.ps1 or .sh build scripts)"]
fn a_stalled_plugin_misses_overflows_and_recovers_after_a_restart() {
    let mut stall = support::stall::Stall::new();
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Clap, "stall-in-process", 0x18)],
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    let mut last = silence(QUANTUM);
    for index in 0..=STALL_AT as usize {
        last = callback(&mut endpoint, &sine(index)).1;
    }
    // The helper holds up the quantum just submitted; the next two callbacks miss its output and
    // the next one's, and the one after finds no free lane.
    stall.wait_until_entered();
    let (played, output) = callback(&mut endpoint, &sine(9));
    assert_eq!(played, RealtimeOutput::Fallback(FallbackReason::Missed));
    for channel in 0..2 {
        // The fallback ramps from the last sample played down to silence.
        let held = f64::from(last[channel][QUANTUM - 1]);
        assert_ne!(held, 0.0);
        assert_eq!(
            output[channel][0],
            (held * (1.0 - 1.0 / QUANTUM as f64)) as f32
        );
        assert_eq!(output[channel][QUANTUM - 1], 0.0);
    }
    assert_eq!(
        callback(&mut endpoint, &sine(10)).0,
        RealtimeOutput::Fallback(FallbackReason::Missed)
    );
    assert_eq!(
        callback(&mut endpoint, &sine(11)).0,
        RealtimeOutput::Fallback(FallbackReason::Resync)
    );
    drop(stall);
    // The helper completes the three quanta it holds, each past its deadline.
    let started_waiting = Instant::now();
    while chain.realtime_status().unwrap().late < 3 {
        assert!(started_waiting.elapsed() < Duration::from_secs(10));
        thread::sleep(PACE);
    }
    let status = chain.realtime_status().unwrap();
    assert!(status.resync);
    assert_eq!(
        (
            status.processed,
            status.missed,
            status.overflows,
            status.late
        ),
        (STALL_AT, 2, 1, 3)
    );

    chain.stop_realtime().unwrap();
    chain.reset().unwrap();
    chain.start_realtime().unwrap();
    for index in 12..16 {
        let (played, output) = callback(&mut endpoint, &sine(index));
        let wanted = if index == 12 {
            RealtimeOutput::Fallback(FallbackReason::Priming)
        } else {
            RealtimeOutput::Processed
        };
        assert_eq!(played, wanted, "callback {index}");
        // The reset plugin's delay line starts empty; the stalled generation's late outputs, still
        // in the lanes, are not played.
        assert_eq!(output, silence(QUANTUM), "callback {index}");
    }
    assert!(!chain.realtime_status().unwrap().resync);
}

#[test]
#[ignore = "needs helper and the stall-in-state fixture (.ps1 or .sh build scripts)"]
fn a_slow_state_save_does_not_hold_up_the_endpoint() {
    let mut stall = support::stall::Stall::new();
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Vst3, "stall-in-state", 0x19)],
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM));
    // The fixture holds up the first save after it processed.
    for index in 0..2 {
        callback(&mut endpoint, &sine(index));
    }
    let saving = thread::spawn(move || {
        let state = chain.save_state(0, StatePurpose::Project);
        (chain, state)
    });
    // The plugin goes on processing while its component saves its state.
    stall.wait_until_entered();
    for index in 2..10 {
        let played = callback(&mut endpoint, &sine(index)).0;
        assert_eq!(played, RealtimeOutput::Processed, "callback {index}");
    }
    drop(stall);
    let (chain, state) = saving.join().unwrap();
    state.unwrap();
    assert_eq!(chain.realtime_status().unwrap().missed, 0);
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn paced_callbacks_play_the_offline_output() {
    const CALLBACKS: usize = 750;
    let expected = offline(&delay(PluginFormat::Clap), CALLBACKS);
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let mut endpoint = started(&mut chain, &RealtimeConfig::new(QUANTUM).with_latency(2));
    let period = Duration::from_secs_f64(QUANTUM as f64 / 48_000.0);
    let start = Instant::now();
    let mut slot = 0;
    let mut previous = RealtimeOutput::Fallback(FallbackReason::Priming);
    for index in 0..CALLBACKS {
        let due = start + period * slot;
        let now = Instant::now();
        if due > now {
            thread::sleep(due - now);
        }
        let (played, output) = serve(&mut endpoint, &sine(index));
        // A driver that fell behind skips to its next slot instead of calling back in a burst.
        let passed = (start.elapsed().as_secs_f64() / period.as_secs_f64()).ceil() as u32;
        slot = (slot + 1).max(passed);
        // A callback after a missed output fades in; the others, the first after the start too,
        // play the offline output exactly.
        if played == RealtimeOutput::Processed
            && previous != RealtimeOutput::Fallback(FallbackReason::Missed)
        {
            assert!(output == expected[index - 2], "callback {index}");
        }
        previous = played;
    }
    assert!(chain.realtime_status().unwrap().processed > 0);
}
