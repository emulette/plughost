//! CLAP hosting against the `plughost-test-delay` plugin, whose delay is 10 ms at every sample
//! rate, and the `plughost-test-synth` instrument. Build them with
//! `scripts/build-test-plugins.sh`, then run `cargo test -p plughost-formats -- --ignored`.
//!
//! The tests share one module and run one at a time, each from a single thread, as a host's owning
//! thread would.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use plughost_core::{
    Event, Layout, PluginKind, ProcessConfig, ProcessMode, SampleFormat, StatePurpose,
};
use plughost_formats::clap::{self, ClapError, Plugin};
use plughost_formats::{BlockProcessor, Error, HostedPlugin};

const CLASS: &str = "com.studio.plughost.test-delay.79000001";
const SYNTH: &str = "com.studio.plughost.test-synth";
const GAIN: u64 = 0;
/// Set while inactive, the test delay doubles its gain range.
const RANGE: u64 = 1;
/// The tail the test delay reports, in samples.
const TAIL: u64 = 9;
/// Setting it makes the test delay ask for a restart and add 32 samples of latency per step.
const RESTART_MODE: u64 = 10;
/// Set to the purpose of the last state the test delay saved, when it supports state contexts.
const SAVED_CONTEXT: u64 = 11;
const BLOCK: usize = 512;
const RATES: [f64; 6] = [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0];

fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn bundle() -> PathBuf {
    test_plugin("plughost-test-delay")
}

fn test_plugin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins")
        .join(format!("{name}.clap"))
}

fn plugin() -> Plugin {
    Plugin::new(&bundle(), CLASS, &plughost_core::HostIdentity::default())
        .unwrap_or_else(|error| panic!("{}: {error}", bundle().display()))
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn unhandled_gui_requests_are_rejected_through_native_callbacks() {
    let _serial = serial();
    let mut plugin = plugin();
    // This fixture has no GUI; it records the host callback results as read-only parameters.
    assert_eq!(
        plugin.capabilities().unwrap().embedded_editor,
        plughost_core::Support::Unsupported
    );
    let parameters = plugin.parameters();
    for id in [5, 6] {
        let (info, value) = parameters.iter().find(|(p, _)| p.id == id).unwrap();
        assert!(info.flags.read_only);
        assert_eq!(*value, 0.0);
    }
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn native_logs_can_be_drained_without_an_application_callback() {
    let _serial = serial();
    let mut plugin = plugin();
    let initial = plugin.take_diagnostics();
    assert!(!initial.records.is_empty());
    assert!(initial.records.iter().all(|record| record.slot.is_none()
        && record.plugin.as_ref().is_some_and(|p| p.class_id == CLASS)));
    plugin.prepare(&config(48_000.0)).unwrap();
    let input = [0.0f32; BLOCK];
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    plugin
        .process(
            &plughost_core::BlockContext::new(left.len()),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(!plugin.take_diagnostics().records.is_empty());
    assert_eq!(
        plugin.take_diagnostics(),
        plughost_core::DiagnosticBatch::default()
    );
}

fn synth() -> Plugin {
    let mut synth = Plugin::new(
        &test_plugin("plughost-test-synth"),
        SYNTH,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    synth
        .prepare(&ProcessConfig {
            input: Layout::None,
            ..config(48_000.0)
        })
        .unwrap();
    synth
}

/// One block of the synth's first output channel.
fn play(synth: &mut Plugin, events: &[Event]) -> Vec<f32> {
    let mut output = vec![vec![1.0f32; BLOCK]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    synth
        .process(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |channel| channel.len())),
            &[],
            &mut outputs,
            &[],
            events,
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(output[0], output[1]);
    output.swap_remove(0)
}

fn config(sample_rate: f64) -> ProcessConfig {
    ProcessConfig {
        sample_rate,
        max_block_size: BLOCK,
        sample_format: SampleFormat::F32,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn invalid_parameter_edits_preserve_pending_value_and_state() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.prepare(&config(48_000.0)).unwrap();
    plugin.set_parameter(GAIN, 0.25).unwrap();
    for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(matches!(
            plugin.set_parameter(GAIN, value),
            Err(Error::Clap(ClapError::Input(
                plughost_core::InputError::ParameterValue
            )))
        ));
    }
    assert!(matches!(
        plugin.set_parameter(u64::MAX, 0.5),
        Err(Error::Clap(ClapError::Input(
            plughost_core::InputError::UnknownParameter { .. }
        )))
    ));
    let read_only = plugin
        .parameters()
        .into_iter()
        .find(|(parameter, _)| parameter.flags.read_only)
        .unwrap()
        .0;
    assert!(matches!(
        plugin.set_parameter(read_only.id, 0.5),
        Err(Error::Clap(ClapError::Input(plughost_core::InputError::ReadOnlyParameter { id }))) if id == read_only.id
    ));
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(
        plugin
            .parameters()
            .into_iter()
            .find(|(p, _)| p.id == GAIN)
            .unwrap()
            .1,
        0.25
    );
    assert_eq!(
        plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap(),
        state
    );
    let output = run(&mut plugin, &vec![vec![1.0; BLOCK * 2]; 2]);
    assert_eq!(output[0][BLOCK], 0.25);
}

fn signal(frames: usize) -> Vec<Vec<f32>> {
    (0..2)
        .map(|ch| {
            (0..frames)
                .map(|i| 0.5 * ((i as f32) * 0.013 * (ch + 1) as f32).sin())
                .collect()
        })
        .collect()
}

/// Processes `input` block by block, without latency compensation.
fn run(plugin: &mut Plugin, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    run_processor(plugin.processor().as_ref(), input)
}

fn run_processor(processor: &dyn BlockProcessor, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut output = vec![vec![0.0f32; frames]; 2];
    for start in (0..frames).step_by(BLOCK) {
        let end = (start + BLOCK).min(frames);
        let inputs: Vec<&[f32]> = input.iter().map(|c| &c[start..end]).collect();
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(|c| &mut c[start..end]).collect();
        processor
            .process_audio_f32(
                &plughost_core::BlockContext::new(
                    outputs.first().map_or(0, |channel| channel.len()),
                ),
                &inputs,
                &mut outputs,
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
    }
    output
}

/// The largest difference between `output` and `input` delayed by `latency` and scaled by `gain`.
fn delay_error(output: &[Vec<f32>], input: &[Vec<f32>], latency: usize, gain: f32) -> f32 {
    output
        .iter()
        .zip(input)
        .flat_map(|(out, inp)| {
            out[latency..]
                .iter()
                .zip(inp)
                .map(move |(o, i)| (o - gain * i).abs())
        })
        .fold(0.0, f32::max)
}

fn parameter_value(plugin: &mut Plugin, id: u64) -> f64 {
    plugin
        .parameters()
        .into_iter()
        .find(|(parameter, _)| parameter.id == id)
        .map(|(_, value)| value)
        .unwrap()
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn classes_are_listed_with_their_plugin_ids() {
    let _serial = serial();
    let classes = clap::classes(&bundle()).unwrap();
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].class_id, CLASS);
    assert_eq!(classes[0].name, "plughost test delay");
    assert_eq!(classes[0].kind, PluginKind::Effect);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn latency_is_read_for_the_prepared_sample_rate() {
    let _serial = serial();
    let mut plugin = plugin();
    for rate in RATES {
        plugin.prepare(&config(rate)).unwrap();
        let latency = (rate / 100.0) as usize;
        assert_eq!(
            plugin.timing().map(|timing| timing.latency),
            Ok(latency as u32),
            "{rate} Hz"
        );
        let input = signal(latency + 4 * BLOCK);
        let output = run(&mut plugin, &input);
        assert!(
            delay_error(&output, &input, latency, 1.0) < 1e-6,
            "{rate} Hz"
        );
    }
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn values_read_right_after_an_edit_include_it() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.set_parameter(GAIN, 0.25).unwrap();
    assert_eq!(parameter_value(&mut plugin, GAIN), 0.25);
    plugin.prepare(&config(48_000.0)).unwrap();
    plugin.set_parameter(GAIN, 0.5).unwrap();
    assert_eq!(parameter_value(&mut plugin, GAIN), 0.5);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn edits_reach_the_next_block() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.prepare(&config(48_000.0)).unwrap();
    plugin.set_parameter(GAIN, 0.5).unwrap();
    let input = signal(480 + 4 * BLOCK);
    let output = run(&mut plugin, &input);
    assert!(delay_error(&output, &input, 480, 0.5) < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn edit_saved_right_after_it_is_made_survives_in_a_fresh_instance() {
    let _serial = serial();
    let mut original = plugin();
    original.prepare(&config(48_000.0)).unwrap();
    // As an editor does: the edit is made and saved with no audio in between.
    original.set_parameter(GAIN, 0.25).unwrap();
    let state = original
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();

    let mut fresh = plugin();
    fresh
        .restore_state(&state, plughost_core::StatePurpose::Project)
        .unwrap();
    assert!((parameter_value(&mut fresh, GAIN) - 0.25).abs() < 1e-9);
    fresh.prepare(&config(48_000.0)).unwrap();
    let input = signal(480 + 4 * BLOCK);
    let output = run(&mut fresh, &input);
    assert!(delay_error(&output, &input, 480, 0.25) < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn reset_keeps_the_state_and_clears_internal_audio() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.prepare(&config(48_000.0)).unwrap();
    plugin.set_parameter(GAIN, 0.5).unwrap();
    run(&mut plugin, &signal(4 * BLOCK));
    plugin.reset().unwrap();
    assert!((parameter_value(&mut plugin, GAIN) - 0.5).abs() < 1e-9);
    let silence = vec![vec![0.0f32; 2 * BLOCK]; 2];
    assert_eq!(run(&mut plugin, &silence), silence);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh or .ps1"]
fn reset_without_state_extension_stops_notes_and_keeps_controls() {
    let _serial = serial();
    let mut synth = synth();
    play(
        &mut synth,
        &[
            Event::control_change(0, 0, 7, 64),
            Event::note_on(0, 0, 69, 127),
        ],
    );
    synth.reset().unwrap();
    assert!(play(&mut synth, &[]).iter().all(|&sample| sample == 0.0));
    assert_eq!(
        play(&mut synth, &[Event::note_on(0, 0, 69, 127)])[0],
        64.0 / 127.0
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn a_plugin_without_state_contexts_saves_project_state_and_refuses_the_others() {
    let _serial = serial();
    let mut plugin = Plugin::new(
        &test_plugin("plughost-test-noop-reset"),
        "com.studio.plughost.test-delay.79000006",
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    plugin.set_parameter(GAIN, 0.5).unwrap();
    let original = plugin.save_state(StatePurpose::Project).unwrap();
    plugin
        .restore_state(&original, StatePurpose::Project)
        .unwrap();
    assert_eq!(parameter_value(&mut plugin, SAVED_CONTEXT), 0.0);
    for purpose in [StatePurpose::Preset, StatePurpose::Duplicate] {
        assert_eq!(
            plugin.save_state(purpose),
            Err(Error::Clap(ClapError::StatePurposeUnsupported))
        );
        assert_eq!(
            plugin.restore_state(&original, purpose),
            Err(Error::Clap(ClapError::StatePurposeUnsupported))
        );
    }
    assert_eq!(plugin.save_state(StatePurpose::Project), Ok(original));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn layouts_the_plugin_refuses_are_errors() {
    let _serial = serial();
    let mono = ProcessConfig {
        input: Layout::Mono,
        output: Layout::Mono,
        ..config(48_000.0)
    };
    assert_eq!(
        plugin().prepare(&mono),
        Err(Error::Clap(ClapError::LayoutRefused {
            requested: Layout::Mono,
            plugin_channels: 2
        }))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn instruments_are_listed_as_instruments() {
    let _serial = serial();
    let classes = clap::classes(&test_plugin("plughost-test-synth")).unwrap();
    assert_eq!(classes[0].kind, PluginKind::Instrument);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn notes_start_and_stop_at_their_sample_offsets() {
    let _serial = serial();
    let mut synth = synth();
    let output = play(
        &mut synth,
        &[
            Event::note_on(100, 0, 69, 127),
            Event::note_off(300, 0, 69, 0),
        ],
    );
    assert!(output[..100].iter().all(|&s| s == 0.0));
    // The note's first sample is its velocity.
    assert_eq!(output[100], 1.0);
    assert!(output[101..300].iter().any(|&s| s != 0.0));
    assert!(output[300..].iter().all(|&s| s == 0.0));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn other_midi_messages_reach_a_port_that_takes_midi() {
    let _serial = serial();
    let mut synth = synth();
    let output = play(
        &mut synth,
        &[
            Event::control_change(0, 0, 7, 64),
            Event::note_on(0, 0, 69, 127),
        ],
    );
    assert_eq!(output[0], 64.0 / 127.0);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn edits_use_the_current_parameter_range() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.set_parameter(RANGE, 1.0).unwrap();
    // Saving delivers the edit; the inactive plugin doubles the gain range and rescans.
    plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    plugin.set_parameter(GAIN, 0.5).unwrap();
    plugin.prepare(&config(48_000.0)).unwrap();
    let input = signal(480 + 4 * BLOCK);
    let output = run(&mut plugin, &input);
    assert!(delay_error(&output, &input, 480, 1.0) < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn stepped_host_edits_reach_the_plugin_as_whole_values() {
    let _serial = serial();
    let mut plugin = plugin();
    plugin.prepare(&config(48_000.0)).unwrap();
    // The tail parameter is stepped over [0, 32]; 0.3 of its range is 9.6.
    plugin.set_parameter(TAIL, 0.3).unwrap();
    run(&mut plugin, &signal(BLOCK));
    assert_eq!(
        plugin.timing().map(|timing| timing.tail),
        Ok(plughost_core::render::Tail::Samples(10))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn a_restart_requested_while_processing_keeps_the_block_and_fails_the_next() {
    let _serial = serial();
    let mut plugin = plugin();
    let config = config(48_000.0);
    plugin.prepare(&config).unwrap();
    let processor = plugin.processor();
    let input = [0.5f32; 32];
    let process = |automation: &[plughost_core::ParameterChange]| {
        let (mut left, mut right) = ([0.0f32; 32], [0.0f32; 32]);
        processor
            .process_audio_f32(
                &plughost_core::BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                automation,
                &[],
                &mut Vec::new(),
            )
            .map(|()| left)
    };
    // The plugin asks for a restart from the block that sets its restart mode.
    let automation = [plughost_core::ParameterChange {
        id: RESTART_MODE,
        offset: 0,
        value: 0.5,
    }];
    assert!(process(&automation).is_ok());
    assert!(plugin.processor().restart_required());
    assert_eq!(process(&[]), Err(Error::Clap(ClapError::RestartRequired)));
    plugin.prepare(&config).unwrap();
    assert_eq!(plugin.timing().map(|timing| timing.latency).unwrap(), 512);
    assert!(!plugin.processor().restart_required());
    assert!(process(&[]).is_ok());
    // In its last mode the plugin asks again while it activates; that request stands.
    let automation = [plughost_core::ParameterChange {
        value: 1.0,
        ..automation[0]
    }];
    assert!(process(&automation).is_ok());
    assert_eq!(process(&[]), Err(Error::Clap(ClapError::RestartRequired)));
    plugin.prepare(&config).unwrap();
    assert_eq!(plugin.timing().map(|timing| timing.latency).unwrap(), 544);
    assert!(plugin.processor().restart_required());
    assert_eq!(process(&[]), Err(Error::Clap(ClapError::RestartRequired)));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn a_timer_removed_by_another_timer_is_not_called() {
    let _serial = serial();
    // The fixture's first timer removes its second one; it reports calls the removed one gets.
    const REMOVED_TIMER_CALLS: u64 = 15;
    let bundle = test_plugin("plughost-test-timers");
    let mut plugin = Plugin::new(
        &bundle,
        "com.studio.plughost.test-delay.7900000d",
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(2));
        plugin.idle();
    }
    assert_eq!(parameter_value(&mut plugin, REMOVED_TIMER_CALLS), 0.0);
}
