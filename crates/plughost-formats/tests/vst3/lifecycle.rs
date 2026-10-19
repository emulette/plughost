use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
#[ignore = "needs context-required VST3 fixture"]
fn inactive_flush_provides_a_complete_zero_frame_context() {
    let _serial = serial();
    let module = module("plughost-test-context-required");
    let mut plugin = Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    plugin.set_parameter(0, 0.25).unwrap();
    plugin.flush().unwrap();
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(&state.component[..8], &0.25f64.to_le_bytes());
    assert!(plugin.config().is_none());
}

#[test]
#[ignore = "needs delay VST3 fixture"]
fn inactive_flush_delivers_processor_output_to_the_controller() {
    let _serial = serial();
    let module = module("plughost-test-delay");
    let mut plugin = Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    plugin.set_parameter(7, 1.0).unwrap();
    plugin.flush().unwrap();
    assert_eq!(plugin.parameter_value(0), 0.375);
    assert!(plugin.config().is_none());
}

/// The float-mode fixture's parameter counting lifecycle calls made with subnormal numbers not
/// flushed.
const FLOAT_MODE_UNFLUSHED: u64 = 10;

#[test]
#[ignore = "needs float-mode VST3 fixture"]
fn inactive_flush_runs_with_subnormal_numbers_flushed_to_zero() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-float-mode");
    plugin.set_parameter(0, 0.25).unwrap();
    plugin.flush().unwrap();
    assert_eq!(plugin.parameter_value(FLOAT_MODE_UNFLUSHED), 0.0);
}

#[test]
#[ignore = "needs float-mode VST3 fixture"]
fn reset_runs_with_subnormal_numbers_flushed_to_zero() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-float-mode");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    // Preparing does not flush, so reset is measured against the calls made until now.
    let prepared = plugin.parameter_value(FLOAT_MODE_UNFLUSHED);
    plugin.reset().unwrap();
    assert_eq!(plugin.parameter_value(FLOAT_MODE_UNFLUSHED), prepared);
}

#[test]
#[ignore = "needs mapping-probe VST3 fixture"]
fn midi_assignment_queries_allow_the_processing_thread_to_advance() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-mapping-probe");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    plugin
        .set_parameter(TEST_DELAY_GAIN_CONTROLLER, 1.0)
        .unwrap();
    let processor = plugin.processor();
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        let input = [0.0f32; 32];
        let (mut left, mut right) = ([0.0f32; 32], [0.0f32; 32]);
        while !stop.load(Ordering::Acquire) {
            processor
                .process_audio_f32(
                    &plughost_core::BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
            std::thread::yield_now();
        }
    });
    plugin.idle();
    done.store(true, Ordering::Release);
    worker.join().unwrap();
    assert_eq!(plugin.parameter_value(10), 1.0);
}
