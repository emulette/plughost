//! The callback's side of recovery, without a helper: the callback takes a recovered chain's ring
//! when the generation recovery started begins, and hands back the ring it replaces, which only
//! the control thread frees.

use std::sync::Arc;

use plughost_core::ipc::shared::SlotConfig;
use plughost_core::ipc::shared::realtime::{Ring, RingDescriptor};
use plughost_core::{
    AudioBusConfig, BlockContext, Events, FailureKind, Layout, ProcessMode, RoutedChainConfig,
    SampleFormat, SlotAudioConfig,
};

use super::{
    DryDelay, Fallback, FallbackReason, RealtimeConfig, RealtimeEndpoint, RealtimeFailure,
    RealtimeOutput, State,
};

const QUANTUM: usize = 64;

fn ring(first: u64) -> Ring {
    Ring::new(RingDescriptor {
        control: first,
        first_lane: first + 1,
        lanes: 3,
        config: SlotConfig {
            sample_format: SampleFormat::F32,
            max_frames: QUANTUM,
            input_channels: 2,
            output_channels: 2,
        },
    })
    .unwrap()
}

/// A stereo effect's configuration.
fn chain() -> RoutedChainConfig {
    RoutedChainConfig::new(
        48_000.0,
        QUANTUM,
        SampleFormat::F32,
        vec![Layout::Stereo],
        vec![SlotAudioConfig::new(
            Vec::new(),
            vec![AudioBusConfig::new(0, Layout::Stereo, true)],
        )],
    )
    .with_mode(ProcessMode::Realtime)
    .with_event_inputs(1)
}

fn call(endpoint: &mut RealtimeEndpoint) -> RealtimeOutput {
    let input = [0.5f32; QUANTUM];
    let mut left = [0.0f32; QUANTUM];
    let mut right = [0.0f32; QUANTUM];
    endpoint
        .process_audio_f32(
            &BlockContext::new(QUANTUM),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap()
}

#[test]
fn the_callback_takes_a_recovered_ring_and_the_control_thread_frees_the_old_one() {
    let priming = RealtimeOutput::Fallback(FallbackReason::Priming);
    let state = Arc::new(State::default());
    let old = ring(1);
    let old_control = old.control();
    let config = RealtimeConfig::new(QUANTUM);
    let mut endpoint = RealtimeEndpoint::new(old, Arc::clone(&state), &config, &chain(), None);
    state.start(1, 0, 0);
    assert_eq!(call(&mut endpoint), priming);
    assert_eq!(old_control.progress().0, 1);

    // Recovery stops the submissions, then starts the next generation with a new ring.
    state.suspend();
    assert_eq!(
        call(&mut endpoint),
        RealtimeOutput::Fallback(FallbackReason::Resync)
    );
    let new = ring(10);
    let new_control = new.control();
    state.send_ring(Box::new(new));
    state.start(2, 0, 0);
    assert_eq!(call(&mut endpoint), priming);
    assert_eq!(
        call(&mut endpoint),
        RealtimeOutput::Fallback(FallbackReason::Missed)
    );
    assert_eq!(new_control.progress().0, 2);
    assert_eq!(old_control.progress().0, 1);

    // The callback handed the old ring back without freeing it; the control thread frees it.
    assert_eq!(Arc::strong_count(&old_control), 2);
    state.reclaim();
    assert_eq!(Arc::strong_count(&old_control), 1);

    // A start without a ring keeps the callback's, numbering quanta from 1 again.
    state.stop();
    state.start(3, 0, 0);
    assert_eq!(call(&mut endpoint), priming);
    assert_eq!(new_control.progress().0, 1);
    assert_eq!(Arc::strong_count(&new_control), 2);
}

#[test]
fn a_dry_callback_longer_than_the_line_plays_the_history_it_holds_and_silence_before() {
    let state = Arc::new(State::default());
    let config = RealtimeConfig::new(QUANTUM).with_fallback(Fallback::Dry);
    // One quantum of added latency over a chain latency of 0: the line holds two quanta.
    let line = Box::new(DryDelay::new(2, 2 * QUANTUM));
    let mut endpoint =
        RealtimeEndpoint::new(ring(1), Arc::clone(&state), &config, &chain(), Some(line));
    state.start(1, 0, QUANTUM as u64);

    let frames = 4 * QUANTUM;
    let input: Vec<f32> = (0..frames).map(|frame| frame as f32).collect();
    let mut left = vec![1.0f32; frames];
    let mut right = vec![1.0f32; frames];
    let played = endpoint
        .process_audio_f32(
            &BlockContext::new(frames),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();

    assert_eq!(played, RealtimeOutput::Fallback(FallbackReason::Resync));
    assert_eq!(state.status(0, Default::default(), 0).size_mismatches, 1);
    // The line holds the last two quanta of input. Delayed by one quantum, the older of them
    // plays in the callback's last quantum; what would play before it is no longer held.
    let held = frames - QUANTUM;
    for channel in [&left, &right] {
        assert!(channel[..held].iter().all(|&sample| sample == 0.0));
        assert!(
            channel[held..]
                .iter()
                .zip(held - QUANTUM..)
                .all(|(&sample, source)| sample == source as f32)
        );
    }
}

#[test]
fn stop_and_suspend_halt_the_generation_a_late_call_may_still_follow() {
    let state = State::default();
    state.start(1, 0, 0);
    state.stop();
    assert!(state.halted(1));
    state.start(2, 0, 0);
    assert!(!state.halted(2));
    state.suspend();
    assert!(state.halted(2));
}

#[test]
fn a_failure_in_a_slot_the_chain_does_not_have_is_a_protocol_failure() {
    let state = Arc::new(State::default());
    let mut ring = ring(1);
    // The worker's report on the first quantum, which the callback reads one quantum later.
    ring.fail(1, 1, None, Some(99), FailureKind::Processing)
        .unwrap();
    let config = RealtimeConfig::new(QUANTUM);
    let mut endpoint = RealtimeEndpoint::new(ring, Arc::clone(&state), &config, &chain(), None);
    state.start(1, 0, 0);
    assert_eq!(
        call(&mut endpoint),
        RealtimeOutput::Fallback(FallbackReason::Priming)
    );
    assert_eq!(
        call(&mut endpoint),
        RealtimeOutput::Fallback(FallbackReason::Resync)
    );
    let status = state.status(0, Default::default(), 0);
    assert_eq!(status.failures, 1);
    assert_eq!(
        status.last_failure,
        Some(RealtimeFailure {
            slot: None,
            kind: FailureKind::Protocol,
        })
    );
}
