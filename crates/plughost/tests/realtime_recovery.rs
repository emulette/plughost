//! Recovering a realtime endpoint in a new helper after its helper crashed or hung, through real
//! helper processes and a synthetic audio callback that keeps the same endpoint throughout. The
//! manual driver leaves the helper far more time than it needs for each quantum. Build the helper
//! and the test plugins first (see `support`).

use std::thread;
use std::time::{Duration, Instant};

use plughost::{
    BlockContext, Chain, Error, FallbackReason, InputError, PluginFormat, PluginRef, PluginState,
    RealtimeConfig, RealtimeEndpoint, RealtimeOutput, RealtimeStatus, Timeouts,
};

mod support;

use support::{delay, delay_variant, prepare, prepared_delay, routing, spawn, spawn_with};

const QUANTUM: usize = 128;
/// The time the manual driver leaves between callbacks.
const PACE: Duration = Duration::from_millis(10);
/// The process call the `stall-in-process` fixture holds up, counted from 0.
const STALL_AT: usize = 8;
/// The delay fixture's gain parameter.
const GAIN: u64 = 0;
/// How long the application in these tests waits for a stalled helper before recovering.
const HANG: Duration = Duration::from_millis(100);

type Stereo = [Vec<f32>; 2];

const PRIMING: RealtimeOutput = RealtimeOutput::Fallback(FallbackReason::Priming);
const RESYNC: RealtimeOutput = RealtimeOutput::Fallback(FallbackReason::Resync);

fn silence() -> Stereo {
    [vec![0.0; QUANTUM], vec![0.0; QUANTUM]]
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

/// One callback of the manual driver, with callback `index`'s input.
fn callback(endpoint: &mut RealtimeEndpoint, index: usize) -> (RealtimeOutput, Stereo) {
    thread::sleep(PACE);
    let input = sine(index);
    let mut output = silence();
    let [left, right] = &mut output;
    let played = endpoint
        .process_audio_f32(
            &BlockContext::new(QUANTUM),
            &[&input[0], &input[1]],
            &mut [left, right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    (played, output)
}

/// The output of a chain recovered from `states` for `callbacks` sine quanta from callback
/// `first`'s on, processed offline, one block each.
fn offline(
    plugin: &PluginRef,
    states: &[PluginState],
    first: usize,
    callbacks: usize,
) -> Vec<Stereo> {
    let mut base = spawn(std::slice::from_ref(plugin));
    prepare(&mut base);
    let mut chain = base.recover(states).unwrap();
    (first..first + callbacks)
        .map(|index| {
            let input = sine(index);
            let mut output = silence();
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

fn terminate(pid: u32) {
    #[cfg(target_os = "macos")]
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!process.is_null());
        assert_ne!(TerminateProcess(process, 17), 0);
        CloseHandle(process);
    }
}

/// Polls the chain's endpoint status until `done` holds, for at most 10 seconds.
fn wait_for(chain: &Chain, done: impl Fn(&RealtimeStatus) -> bool) {
    let start = Instant::now();
    while !done(&chain.realtime_status().unwrap()) {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(PACE);
    }
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn clap_endpoint_recovers_from_a_crashed_helper() {
    crash_recovery(delay(PluginFormat::Clap));
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn vst3_endpoint_recovers_from_a_crashed_helper() {
    crash_recovery(delay(PluginFormat::Vst3));
}

/// The callback goes on in its own thread while its helper is killed and the endpoint recovered
/// from a snapshot taken while it ran. The same endpoint then plays the recovered chain's output
/// after priming, exactly as offline processing of the same input does.
fn crash_recovery(plugin: PluginRef) {
    const PROCESSED: usize = 8;
    let mut chain = spawn(std::slice::from_ref(&plugin));
    prepare(&mut chain);
    chain.set_parameter(0, GAIN, 0.25).unwrap();
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    chain.start_realtime().unwrap();
    for index in 0..4 {
        callback(&mut endpoint, index);
    }
    let snapshot = chain.snapshot().unwrap();
    assert_eq!((snapshot.states.len(), snapshot.bypass.len()), (1, 1));
    // The snapshot does not hold up the endpoint, and an edit after it is not recovered.
    chain.set_parameter(0, GAIN, 0.75).unwrap();
    for index in 4..6 {
        assert_eq!(callback(&mut endpoint, index).0, RealtimeOutput::Processed);
    }

    let monitor = chain.helper_monitor();
    terminate(monitor.process_id());
    assert!(monitor.wait_for_exit(Duration::from_secs(10)).is_some());
    let mut index = 6;
    while !chain.realtime_status().unwrap().resync {
        assert!(index < 20, "the endpoint did not resync");
        callback(&mut endpoint, index);
        index += 1;
    }
    let status = chain.realtime_status().unwrap();
    assert!(
        status.running && status.stalled > Duration::ZERO,
        "{status:?}"
    );
    assert_eq!(status.overflows, 1);

    let first = index;
    let (recovered, played) = thread::scope(|scope| {
        let driver = scope.spawn(|| {
            let mut played = Vec::new();
            let mut index = first;
            let mut processed = 0;
            while processed < PROCESSED {
                assert!(index < first + 3000, "the endpoint did not recover");
                let (output, samples) = callback(&mut endpoint, index);
                if played.iter().any(|&(_, output, _)| output == PRIMING) {
                    processed += 1;
                }
                played.push((index, output, samples));
                index += 1;
            }
            played
        });
        let recovered = chain.recover_realtime(&snapshot).unwrap();
        (recovered, driver.join().unwrap())
    });
    let mut recovered = recovered;
    // The endpoint moved: the old chain no longer has it.
    assert_eq!(chain.realtime_status(), None);
    assert!(matches!(
        chain.recover_realtime(&snapshot),
        Err(Error::NotPrepared)
    ));
    drop(chain);

    // Until the new generation starts, the endpoint plays its fallback.
    let start = played
        .iter()
        .position(|&(_, output, _)| output == PRIMING)
        .unwrap();
    assert!(
        played[..start]
            .iter()
            .all(|&(_, output, _)| output == RESYNC)
    );
    assert_eq!(played[start].2, silence());
    let expected = offline(&plugin, &snapshot.states, played[start].0, PROCESSED);
    for (offset, (index, output, samples)) in played[start + 1..].iter().enumerate() {
        assert_eq!(*output, RealtimeOutput::Processed, "callback {index}");
        assert_eq!(*samples, expected[offset], "callback {index}");
    }
    // The comparison covers the delayed signal, not only the delay line's initial silence.
    assert!(
        expected
            .iter()
            .flatten()
            .flatten()
            .any(|&sample| sample != 0.0)
    );

    thread::sleep(PACE);
    let status = recovered.realtime_status().unwrap();
    assert!(status.running && !status.resync, "{status:?}");
    assert_eq!(status.stalled, Duration::ZERO);
    assert_eq!(status.callbacks, played.last().unwrap().0 as u64 + 1);
    recovered.stop_realtime().unwrap();
    assert_eq!(
        callback(&mut endpoint, 0).0,
        RealtimeOutput::Fallback(FallbackReason::Stopped)
    );
}

#[test]
#[ignore = "needs helper and the stall-in-process fixture (.ps1 or .sh build scripts)"]
fn a_hung_helper_shows_as_stalled_and_is_recovered_while_it_still_runs() {
    let mut stall = support::stall::Stall::new();
    let plugin = delay_variant(PluginFormat::Clap, "stall-in-process", 0x18);
    let mut chain = spawn_with(
        std::slice::from_ref(&plugin),
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    assert_eq!(chain.realtime_status().unwrap().stalled, Duration::ZERO);
    chain.start_realtime().unwrap();
    for index in 0..STALL_AT {
        callback(&mut endpoint, index);
    }
    let snapshot = chain.snapshot().unwrap();
    // The helper completed every quantum submitted, so nothing is outstanding.
    thread::sleep(PACE);
    assert_eq!(chain.realtime_status().unwrap().stalled, Duration::ZERO);

    // The helper holds up the next quantum; the time since its last completion grows.
    callback(&mut endpoint, STALL_AT);
    stall.wait_until_entered();
    let stalled = chain.realtime_status().unwrap().stalled;
    assert!(stalled > Duration::ZERO);
    wait_for(&chain, |status| status.stalled >= HANG);
    assert!(chain.realtime_status().unwrap().stalled > stalled);
    let mut index = STALL_AT + 1;
    while !chain.realtime_status().unwrap().resync {
        assert!(index < 20, "the endpoint did not resync");
        callback(&mut endpoint, index);
        index += 1;
    }
    let before = chain.realtime_status().unwrap();
    assert!(before.stalled >= HANG);

    // Recovery does not need the hung helper, which goes on stalling.
    let old = chain.helper_monitor();
    let mut recovered = chain.recover_realtime(&snapshot).unwrap();
    assert_eq!(old.exit_status(), None);
    let first = index;
    let expected = offline(&plugin, &snapshot.states, first, 4);
    let check = |index: usize, endpoint: &mut RealtimeEndpoint| {
        let (output, samples) = callback(endpoint, index);
        if index == first {
            assert_eq!(output, PRIMING);
        } else {
            assert_eq!(output, RealtimeOutput::Processed, "callback {index}");
            assert_eq!(samples, expected[index - first - 1], "callback {index}");
        }
    };
    for index in first..first + 3 {
        check(index, &mut endpoint);
    }
    // Released, the old helper completes the quanta it held in its own shared memory only.
    drop(stall);
    thread::sleep(HANG);
    for index in first + 3..first + 5 {
        check(index, &mut endpoint);
    }
    let status = recovered.realtime_status().unwrap();
    assert!(!status.resync && status.failures == 0, "{status:?}");
    assert_eq!(status.missed, before.missed);
    recovered.stop_realtime().unwrap();
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_failed_recovery_leaves_the_endpoint_with_its_chain() {
    let mut chain = prepared_delay(PluginFormat::Clap);
    let snapshot = chain.snapshot().unwrap();
    // Recovery needs the chain's endpoint.
    assert!(matches!(
        chain.recover_realtime(&snapshot),
        Err(Error::NotPrepared)
    ));
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    chain.start_realtime().unwrap();
    for index in 0..3 {
        callback(&mut endpoint, index);
    }
    let mut wrong = snapshot.clone();
    wrong.states.push(snapshot.states[0].clone());
    assert!(matches!(
        chain.recover_realtime(&wrong),
        Err(Error::Input {
            slot: None,
            error: InputError::StateCount
        })
    ));
    // The endpoint plays its fallback with its chain, which restarts it as after a resync.
    assert_eq!(callback(&mut endpoint, 3).0, RESYNC);
    let status = chain.realtime_status().unwrap();
    assert!(status.running && status.resync, "{status:?}");
    chain.stop_realtime().unwrap();
    chain.reset().unwrap();
    chain.start_realtime().unwrap();
    assert_eq!(callback(&mut endpoint, 4).0, PRIMING);
    assert_eq!(callback(&mut endpoint, 5).0, RealtimeOutput::Processed);

    // The application may try again.
    let mut recovered = chain.recover_realtime(&snapshot).unwrap();
    assert_eq!(callback(&mut endpoint, 6).0, PRIMING);
    assert_eq!(callback(&mut endpoint, 7).0, RealtimeOutput::Processed);
    recovered.stop_realtime().unwrap();
}

#[test]
#[ignore = "needs helper and the routing fixture (.ps1 or .sh build scripts)"]
fn recovery_applies_the_snapshot_bypass() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[routing(format)]);
        prepare(&mut chain);
        let _endpoint = chain
            .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
            .unwrap();
        chain.start_realtime().unwrap();
        chain.set_bypass(0, true).unwrap();
        let snapshot = chain.snapshot().unwrap();
        assert_eq!(snapshot.bypass, [Some(true)], "{format:?}");
        let mut recovered = chain.recover_realtime(&snapshot).unwrap();
        assert_eq!(
            recovered.bypass(0).unwrap().enabled,
            Some(true),
            "{format:?}"
        );
    }
}
