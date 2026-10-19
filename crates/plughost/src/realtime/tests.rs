//! The callback's side of recovery, without a helper: the callback takes a recovered chain's ring
//! when the generation recovery started begins, and hands back the ring it replaces, which only
//! the control thread frees.

use std::sync::Arc;

use plughost_core::ipc::shared::SlotConfig;
use plughost_core::ipc::shared::realtime::{Ring, RingDescriptor};
use plughost_core::{
    AudioBusConfig, BlockContext, Layout, ProcessMode, RoutedChainConfig, SampleFormat,
    SlotAudioConfig,
};

use super::{FallbackReason, RealtimeConfig, RealtimeEndpoint, RealtimeOutput, State};

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
    RoutedChainConfig {
        sample_rate: 48_000.0,
        max_block_size: QUANTUM,
        sample_format: SampleFormat::F32,
        mode: ProcessMode::Realtime,
        inputs: vec![Layout::Stereo],
        event_inputs: 1,
        slots: vec![SlotAudioConfig {
            configuration: None,
            inputs: Vec::new(),
            outputs: vec![AudioBusConfig {
                id: 0,
                layout: Layout::Stereo,
                active: true,
            }],
            events: Default::default(),
        }],
    }
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
            &[],
            &mut Vec::new(),
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
