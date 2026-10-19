//! The realtime endpoint's callback allocates and frees nothing in any path it takes, through real
//! helper processes: stopped, priming and processing with events and automation, the dry fallback
//! and its ramps, missed outputs and the crossfade back, a full ring and a callback of another
//! size, a resync and the note offs that follow it. Run it with the `realtime-audit` feature; on macOS, building with `RTSAN_ENABLE=1` also
//! makes RealtimeSanitizer abort on any allocation, lock or system call the callback makes. Build
//! the helper and the test plugins first (see `support`).
#![cfg(feature = "realtime-audit")]

use std::thread;
use std::time::Duration;

use plughost::{
    AutomationEvent, BlockContext, Event, Fallback, FallbackReason, Layout, MAX_BLOCK_EVENTS,
    ParameterChange, PluginFormat, RealtimeConfig, RealtimeEndpoint, RealtimeOutput, Timeouts,
};

mod support;

use support::{delay, delay_variant, prepare, routing, spawn, spawn_with, synth};

#[global_allocator]
static ALLOCATOR: plughost_core::audit::Allocator = plughost_core::audit::Allocator;

const QUANTUM: usize = 128;
/// The time left between callbacks, far more than the helper needs for a quantum.
const PACE: Duration = Duration::from_millis(10);
/// The process call the `stall-in-process` fixture holds up, counted from 0.
const STALL_AT: usize = 8;

/// One callback of `frames` frames on `inputs` input channels, after the pace.
fn call(
    endpoint: &mut RealtimeEndpoint,
    inputs: usize,
    frames: usize,
    automation: &[AutomationEvent],
    events: &[Event],
) -> RealtimeOutput {
    thread::sleep(PACE);
    let input = vec![vec![0.25f32; frames]; inputs];
    let input: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; frames]; 2];
    let mut output: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let mut produced = Vec::with_capacity(MAX_BLOCK_EVENTS);
    endpoint
        .process_audio_f32(
            &BlockContext::new(frames),
            &input,
            &mut output,
            automation,
            events,
            &mut produced,
        )
        .unwrap()
}

#[test]
#[ignore = "needs helper and the delay, synth, routing and stall-in-process fixtures (.ps1 or .sh build scripts)"]
fn the_realtime_callback_allocates_nothing() {
    let processed = RealtimeOutput::Processed;
    let fallback = RealtimeOutput::Fallback;

    // An instrument takes notes; a callback of another size resynchronizes it.
    let mut chain = spawn(&[synth(PluginFormat::Clap)]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    assert_eq!(
        call(&mut endpoint, 0, QUANTUM, &[], &[]),
        fallback(FallbackReason::Stopped)
    );
    chain.start_realtime().unwrap();
    let notes = [
        Event::note_on(0, 0, 60, 100),
        Event::note_on(64, 0, 64, 100),
    ];
    assert_eq!(
        call(&mut endpoint, 0, QUANTUM, &[], &notes),
        fallback(FallbackReason::Priming)
    );
    for _ in 0..4 {
        assert_eq!(call(&mut endpoint, 0, QUANTUM, &[], &notes), processed);
    }
    assert_eq!(
        call(&mut endpoint, 0, QUANTUM / 2, &[], &[]),
        fallback(FallbackReason::Resync)
    );
    chain.stop_realtime().unwrap();

    // An effect plays its delayed input while stopped and priming, with automation once it runs,
    // and ramps back to the input when it stops.
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let config = RealtimeConfig::new(QUANTUM).with_fallback(Fallback::Dry);
    let mut endpoint = chain.realtime_endpoint(&config).unwrap();
    call(&mut endpoint, 2, QUANTUM, &[], &[]);
    chain.start_realtime().unwrap();
    let automation = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: 0,
            offset: 32,
            value: 0.5,
        },
    }];
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &automation, &[]),
        fallback(FallbackReason::Priming)
    );
    for _ in 0..4 {
        assert_eq!(call(&mut endpoint, 2, QUANTUM, &automation, &[]), processed);
    }
    chain.stop_realtime().unwrap();
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &[]),
        fallback(FallbackReason::Stopped)
    );

    // A stalled effect misses an output, plays its delayed input, and crossfades back to the
    // output once the helper catches up.
    let mut stall = support::stall::Stall::new();
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Clap, "stall-in-process", 0x18)],
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    let mut endpoint = chain.realtime_endpoint(&config).unwrap();
    chain.start_realtime().unwrap();
    for _ in 0..=STALL_AT {
        call(&mut endpoint, 2, QUANTUM, &[], &[]);
    }
    stall.wait_until_entered();
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &[]),
        fallback(FallbackReason::Missed)
    );
    drop(stall);
    assert_eq!(call(&mut endpoint, 2, QUANTUM, &[], &[]), processed);
    chain.stop_realtime().unwrap();

    // A stalled plugin misses outputs and then fills the ring.
    let mut stall = support::stall::Stall::new();
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Clap, "stall-in-process", 0x18)],
        &stall.identity(),
        Timeouts::default(),
    );
    prepare(&mut chain);
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    chain.start_realtime().unwrap();
    for _ in 0..=STALL_AT {
        call(&mut endpoint, 2, QUANTUM, &[], &[]);
    }
    stall.wait_until_entered();
    for _ in 0..2 {
        assert_eq!(
            call(&mut endpoint, 2, QUANTUM, &[], &[]),
            fallback(FallbackReason::Missed)
        );
    }
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &[]),
        fallback(FallbackReason::Resync)
    );
    drop(stall);

    // Notes the chain sent end in the first callback after a resync.
    let mut chain = spawn(&[routing(PluginFormat::Clap)]);
    prepare(&mut chain);
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    chain.start_realtime().unwrap();
    let notes = [Event::note_on(0, 0, 60, 100), Event::note_on(0, 0, 62, 100)];
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &notes),
        fallback(FallbackReason::Priming)
    );
    assert_eq!(call(&mut endpoint, 2, QUANTUM, &[], &[]), processed);
    chain.resync_realtime().unwrap();
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &[]),
        fallback(FallbackReason::Priming)
    );
    assert_eq!(call(&mut endpoint, 2, QUANTUM, &[], &[]), processed);
    chain.stop_realtime().unwrap();

    assert_eq!(plughost_core::audit::processing_allocations(), (0, 0));
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn the_realtime_callback_allocates_nothing_across_a_recovery() {
    let processed = RealtimeOutput::Processed;
    let fallback = RealtimeOutput::Fallback;

    // The callback takes the recovered chain's ring and hands back its own.
    let mut chain = spawn(&[delay(PluginFormat::Clap)]);
    prepare(&mut chain);
    let mut endpoint = chain
        .realtime_endpoint(&RealtimeConfig::new(QUANTUM))
        .unwrap();
    chain.start_realtime().unwrap();
    call(&mut endpoint, 2, QUANTUM, &[], &[]);
    assert_eq!(call(&mut endpoint, 2, QUANTUM, &[], &[]), processed);
    let snapshot = chain.snapshot().unwrap();
    let mut recovered = chain.recover_realtime(&snapshot).unwrap();
    assert_eq!(
        call(&mut endpoint, 2, QUANTUM, &[], &[]),
        fallback(FallbackReason::Priming)
    );
    for _ in 0..4 {
        assert_eq!(call(&mut endpoint, 2, QUANTUM, &[], &[]), processed);
    }
    recovered.stop_realtime().unwrap();

    assert_eq!(plughost_core::audit::processing_allocations(), (0, 0));
}
