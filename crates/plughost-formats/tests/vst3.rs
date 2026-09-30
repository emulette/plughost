//! Behavior against real VST3 modules: the VST3 SDK examples `again`, `again-simple`, `adelay`,
//! `note-expression-synth`, and `utf16-name` and the `plughost-test-delay` plugin. Build them with `scripts/build-test-plugins.sh`, then run
//! `cargo test -p plughost-formats -- --ignored`.
//!
//! VST3 modules keep process-wide state that is not safe to load or unload from several threads
//! at once, and the in-process API is used from one thread, so the tests run one at a time.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use plughost_core::render::{RenderOptions, TailPolicy, render};
use plughost_core::{
    Layout, MidiEvent, PluginKind, ProcessConfig, ProcessMode, Sample, SampleFormat,
};
use plughost_formats::vst3::{Module, Plugin, Preset, Vst3Error};
use plughost_formats::{Error, HostedPlugin};

const NEEDS_PLUGINS: &str = "needs scripts/build-test-plugins.sh";
#[path = "vst3/audio.rs"]
mod audio;
#[path = "vst3/factory_programs.rs"]
mod factory_programs;
#[path = "vst3/sdk_samples.rs"]
mod sdk_samples;
const AGAIN_CLASS: &str = "84E8DE5F92554F5396FAE4133C935A18";
const AGAIN_GAIN: u64 = 0;
const ADELAY_DELAY: u64 = 100;
const TEST_DELAY_LATENCY: u32 = 480;
const RATES: [f64; 6] = [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0];

fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn module(name: &str) -> Rc<Module> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins")
        .join(format!("{name}.vst3"));
    Module::load(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
#[ignore = "needs routing VST3 fixture"]
fn factory_receives_a_live_host_before_class_enumeration() {
    let _serial = serial();
    let module = module("plughost-test-routing");
    assert_eq!(module.classes()[0].vendor, "plughost");
    let instance = Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    drop(module);
    assert_eq!(instance.info().vendor, "plughost");
}

#[test]
#[ignore = "needs routing VST3 fixture"]
fn hosted_factory_uses_the_callers_identity() {
    let _serial = serial();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins/plughost-test-routing.vst3");
    let identity = plughost_core::HostIdentity {
        name: "Factory identity fixture".into(),
        ..plughost_core::HostIdentity::default()
    };
    let plugin = plughost_formats::load(
        &plughost_core::PluginRef {
            format: plughost_core::PluginFormat::Vst3,
            bundle: Some(path),
            class_id: "706C7567686F7374526F7574696E6701".into(),
        },
        &identity,
    )
    .unwrap();
    assert_eq!(plugin.info().vendor, identity.name);
}

fn plugin(name: &str) -> Plugin {
    let module = module(name);
    let class = module.classes()[0].class_id.clone();
    Plugin::new(&module, &class, &plughost_core::HostIdentity::default()).unwrap()
}

fn config(sample_rate: f64, sample_format: SampleFormat) -> ProcessConfig {
    ProcessConfig {
        sample_rate,
        max_block_size: 512,
        sample_format,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn invalid_prepare_keeps_the_existing_processor_usable() {
    let _serial = serial();
    let mut plugin = plugin("again");
    let valid = config(48_000.0, SampleFormat::F32);
    plugin.prepare(&valid).unwrap();
    plugin.set_parameter(AGAIN_GAIN, 0.5).unwrap();
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    for size in [0, i32::MAX as usize + 1, usize::MAX] {
        let invalid = ProcessConfig {
            max_block_size: size,
            ..valid
        };
        assert!(matches!(
            plugin.prepare(&invalid),
            Err(Error::Vst3(Vst3Error::Input(
                plughost_core::InputError::BlockSize
            )))
        ));
        assert_eq!(plugin.config(), Some(valid));
    }
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let invalid = ProcessConfig {
            sample_rate: rate,
            ..valid
        };
        assert!(matches!(
            plugin.prepare(&invalid),
            Err(Error::Vst3(Vst3Error::Input(
                plughost_core::InputError::SampleRate
            )))
        ));
        assert_eq!(plugin.config(), Some(valid));
    }
    assert_eq!(
        plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap(),
        state
    );
    let input = vec![1.0f32; 64];
    let mut output = vec![0.0f32; 64];
    let mut right = vec![0.0f32; 64];
    plugin
        .process(
            &plughost_core::BlockContext::new(output.len()),
            &[&input, &input],
            &mut [&mut output, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(output.iter().all(|value| (*value - 0.5).abs() < 1e-6));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn invalid_parameter_edits_preserve_pending_value_and_state() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    plugin.set_parameter(AGAIN_GAIN, 0.25).unwrap();
    for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(matches!(
            plugin.set_parameter(AGAIN_GAIN, value),
            Err(Error::Vst3(Vst3Error::Input(
                plughost_core::InputError::ParameterValue
            )))
        ));
    }
    for id in [u64::MAX, 0xFFFF] {
        assert!(matches!(
            plugin.set_parameter(id, 0.5),
            Err(Error::Vst3(Vst3Error::Input(
                plughost_core::InputError::UnknownParameter { .. }
            )))
        ));
    }
    let meter = plugin
        .parameters()
        .into_iter()
        .find(|p| p.0.flags.read_only)
        .unwrap();
    assert!(matches!(
        plugin.set_parameter(meter.0.id, 0.5),
        Err(Error::Vst3(Vst3Error::Input(
            plughost_core::InputError::ReadOnlyParameter { .. }
        )))
    ));
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(plugin.parameter_value(AGAIN_GAIN), 0.25);
    assert_eq!(
        plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap(),
        state
    );
    let output = run(&mut plugin, &vec![vec![1.0f32; 64]; 2]);
    assert!(output[0].iter().all(|value| (*value - 0.25).abs() < 1e-6));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn malformed_parameter_changes_do_not_touch_audio_or_pending_edits() {
    use plughost_core::ParameterChange;
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    plugin.set_parameter(AGAIN_GAIN, 0.5).unwrap();
    let input = [1.0f32; 64];
    let mut output = [42.0f32; 64];
    let mut right = [42.0f32; 64];
    let change = |offset, value| ParameterChange {
        id: AGAIN_GAIN,
        offset,
        value,
    };
    let invalid_value = Vst3Error::Input(plughost_core::InputError::ParameterValue);
    for (changes, error) in [
        (vec![change(0, f64::NAN)], invalid_value.clone()),
        (vec![change(0, f64::INFINITY)], invalid_value.clone()),
        (vec![change(0, -0.1)], invalid_value.clone()),
        (vec![change(0, 1.1)], invalid_value),
        (vec![change(32, 0.2), change(0, 0.3)], Vst3Error::Buffers),
    ] {
        assert_eq!(
            plugin.process(
                &plughost_core::BlockContext::new(output.len()),
                &[&input, &input],
                &mut [&mut output, &mut right],
                &changes,
                &[],
                &mut Vec::new(),
            ),
            Err(error)
        );
        assert_eq!(output, [42.0; 64]);
        assert_eq!(right, [42.0; 64]);
    }
    plugin
        .process(
            &plughost_core::BlockContext::new(output.len()),
            &[&input, &input],
            &mut [&mut output, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(output.iter().all(|value| (*value - 0.5).abs() < 1e-6));
}

fn signal<S: Sample>(frames: usize, from_f64: fn(f64) -> S) -> Vec<Vec<S>> {
    (0..2)
        .map(|ch| {
            (0..frames)
                .map(|i| from_f64(0.5 * ((i as f64) * 0.013 * (ch + 1) as f64).sin()))
                .collect()
        })
        .collect()
}

fn run<S: Sample>(plugin: &mut Plugin, input: &[Vec<S>]) -> Vec<Vec<S>> {
    let slices: Vec<&[S]> = input.iter().map(Vec::as_slice).collect();
    let options = RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    };
    render(plugin, &slices, input[0].len(), &[], &options)
        .unwrap()
        .channels
}

fn max_difference<S: Sample>(a: &[Vec<S>], b: &[Vec<S>], gain: f64) -> f64 {
    a.iter()
        .zip(b)
        .flat_map(|(x, y)| {
            x.iter()
                .zip(y)
                .map(move |(p, q)| (p.to_f64() - gain * q.to_f64()).abs())
        })
        .fold(0.0, f64::max)
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn classes_are_listed_with_their_sdk_class_ids() {
    let _serial = serial();
    let classes = module("again").classes();
    let names: Vec<&str> = classes.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["AGain VST3", "AGain SideChain VST3"],
        "{NEEDS_PLUGINS}"
    );
    assert_eq!(classes[0].class_id, AGAIN_CLASS);
    assert_eq!(classes[0].kind, PluginKind::Effect);
    assert!(classes[0].categories.iter().any(|c| c == "Fx"));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn processes_at_every_guaranteed_sample_rate() {
    let _serial = serial();
    let mut plugin = plugin("again");
    for rate in RATES {
        plugin.prepare(&config(rate, SampleFormat::F32)).unwrap();
        let input = signal(rate as usize / 10, |v| v as f32);
        let output = run(&mut plugin, &input);
        assert!(max_difference(&output, &input, 1.0) < 1e-6, "{rate} Hz");
    }
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn processes_64_bit_samples() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F64))
        .unwrap();
    let input = signal(4800, |v| v);
    let output = run(&mut plugin, &input);
    assert!(max_difference(&output, &input, 1.0) < 1e-12);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn latency_is_removed_when_rendering() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    assert_eq!(plugin.latency(), Ok(TEST_DELAY_LATENCY));
    let input = signal(4800, |v| v as f32);
    assert_eq!(run(&mut plugin, &input), input);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn edit_saved_right_after_it_is_made_survives_in_a_fresh_instance() {
    let _serial = serial();
    let input = signal(4800, |v| v as f32);
    let mut original = plugin("again");
    original
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    run(&mut original, &input);
    // As an editor does: the controller takes the value and reports it; no audio follows.
    original.set_parameter(AGAIN_GAIN, 0.25).unwrap();
    let state = original
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();

    let mut fresh = plugin("again");
    fresh
        .restore_state(&state, plughost_core::StatePurpose::Project)
        .unwrap();
    fresh.prepare(&config(48_000.0, SampleFormat::F32)).unwrap();
    assert!((fresh.parameter_value(AGAIN_GAIN) - 0.25).abs() < 1e-6);
    let output = run(&mut fresh, &input);
    assert!(max_difference(&output, &input, 0.25) < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn reset_keeps_the_state_and_clears_internal_audio() {
    let _serial = serial();
    let mut plugin = plugin("adelay");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    plugin.set_parameter(ADELAY_DELAY, 0.1).unwrap();
    run(&mut plugin, &signal(24_000, |v| v as f32));
    plugin.reset().unwrap();
    assert!((plugin.parameter_value(ADELAY_DELAY) - 0.1).abs() < 1e-6);
    let silence = vec![vec![0.0f32; 12_000]; 2];
    assert_eq!(run(&mut plugin, &silence), silence);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn vstpreset_files_restore_the_state() {
    let _serial = serial();
    let mut original = plugin("again");
    original
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    original.set_parameter(AGAIN_GAIN, 0.4).unwrap();
    let bytes = Preset::from_state(
        &original
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap(),
    )
    .to_bytes()
    .unwrap();

    let preset = Preset::from_bytes(&bytes).unwrap();
    assert_eq!(preset.class_id, AGAIN_CLASS);
    let mut fresh = plugin("again");
    fresh.restore_preset(&preset).unwrap();
    assert!((fresh.parameter_value(AGAIN_GAIN) - 0.4).abs() < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn layouts_the_plugin_refuses_are_errors() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    let mono = ProcessConfig {
        input: Layout::Mono,
        output: Layout::Mono,
        ..config(48_000.0, SampleFormat::F32)
    };
    assert_eq!(
        plugin.prepare(&mono),
        Err(Error::Vst3(Vst3Error::LayoutRefused {
            requested: Layout::Mono,
            plugin_channels: 2
        }))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn other_speakers_with_the_same_channel_count_are_refused() {
    let _serial = serial();
    // The test delay answers a 5.1 proposal with 6.0.
    let surround = ProcessConfig {
        input: Layout::Surround51,
        output: Layout::Surround51,
        ..config(48_000.0, SampleFormat::F32)
    };
    assert_eq!(
        plugin("plughost-test-delay").prepare(&surround),
        Err(Error::Vst3(Vst3Error::LayoutRefused {
            requested: Layout::Surround51,
            plugin_channels: 6
        }))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn a_main_input_the_caller_does_not_use_gets_silence() {
    let _serial = serial();
    // The test delay keeps its stereo input, as instruments that also process audio do.
    let mut plugin = plugin("plughost-test-delay");
    plugin
        .prepare(&ProcessConfig {
            input: Layout::None,
            ..config(48_000.0, SampleFormat::F32)
        })
        .unwrap();
    let mut output = vec![vec![1.0f32; 512]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process::<f32>(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |c| c.len())),
            &[],
            &mut outputs,
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(output.iter().flatten().all(|&s| s == 0.0));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn buffers_must_match_the_prepared_sample_format() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let input = signal(64, |v| v);
    let mut output = vec![vec![0.0f64; 64]; 2];
    let inputs: Vec<&[f64]> = input.iter().map(Vec::as_slice).collect();
    let mut outputs: Vec<&mut [f64]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    assert_eq!(
        plugin.process(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |channel| channel.len())),
            &inputs,
            &mut outputs,
            &[],
            &[],
            &mut Vec::new(),
        ),
        Err(Vst3Error::SampleFormatMismatch)
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn state_from_another_class_is_rejected() {
    let _serial = serial();
    let state = plugin("again")
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(
        plugin("plughost-test-delay").restore_state(&state, plughost_core::StatePurpose::Project),
        Err(Error::Vst3(Vst3Error::StateMismatch))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn edits_reach_a_processor_running_on_another_thread() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let processor = plugin.processor();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let (done, finished) = std::sync::mpsc::channel::<f32>();
    let worker = std::thread::spawn(move || {
        let input = signal(512, |v| v as f32);
        let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
        while wait.recv().is_ok() {
            let mut output = vec![vec![0.0f32; 512]; 2];
            let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
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
            let ratio = output[0][100] / input[0][100];
            done.send(ratio).unwrap();
        }
    });
    go.send(()).unwrap();
    assert!((finished.recv().unwrap() - 1.0).abs() < 1e-5);
    // The owning thread edits, as an editor does; the processing thread picks it up.
    plugin.set_parameter(AGAIN_GAIN, 0.5).unwrap();
    go.send(()).unwrap();
    assert!((finished.recv().unwrap() - 0.5).abs() < 1e-5);
    drop(go);
    worker.join().unwrap();
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn automation_reaches_the_controller_on_the_owning_thread_only() {
    use plughost_core::ParameterChange;
    let _serial = serial();
    // The test delay asks to be reloaded when its controller is called from another thread.
    let mut plugin = plugin("plughost-test-delay");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let processor = plugin.processor();
    std::thread::spawn(move || {
        let input = [0.0f32; 64];
        let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
        for value in [0.5, 0.25] {
            processor
                .process_audio_f32(
                    &plughost_core::BlockContext::new(64),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[ParameterChange {
                        id: 0,
                        offset: 0,
                        value,
                    }],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
        }
    })
    .join()
    .unwrap();
    assert_eq!(plugin.parameter_value(0), 0.25);
    assert!(!plugin.processor().restart_required());
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn processors_fail_once_the_plugin_is_dropped() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let processor = plugin.processor();
    drop(plugin);
    let input = signal(64, |v| v as f32);
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; 64]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    assert_eq!(
        processor.process_audio_f32(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |channel| channel.len())),
            &inputs,
            &mut outputs,
            &[],
            &[],
            &mut Vec::new(),
        ),
        Err(Error::Vst3(Vst3Error::Closed))
    );
}

/// One block of `again` with `events`; again applies note velocity and gain per block.
fn again_block(plugin: &mut Plugin, input: &[Vec<f32>], events: &[MidiEvent]) -> Vec<Vec<f32>> {
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; input[0].len()]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |channel| channel.len())),
            &inputs,
            &mut outputs,
            &[],
            events,
            &mut Vec::new(),
        )
        .unwrap();
    output
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn notes_reach_the_event_input() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let input = signal(512, |v| v as f32);
    // again lowers its gain by the velocity of the held note.
    let held = again_block(&mut plugin, &input, &[MidiEvent::note_on(0, 0, 60, 64)]);
    assert!(max_difference(&held, &input, 1.0 - 64.0 / 127.0) < 1e-6);
    let released = again_block(&mut plugin, &input, &[MidiEvent::note_off(0, 0, 60, 0)]);
    assert!(max_difference(&released, &input, 1.0) < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn midi_controllers_reach_the_parameters_the_controller_maps() {
    let _serial = serial();
    let mut plugin = plugin("again");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let input = signal(512, |v| v as f32);
    // again maps MIDI volume (controller 7) to its gain.
    let output = again_block(
        &mut plugin,
        &input,
        &[MidiEvent::control_change(0, 0, 7, 32)],
    );
    assert!(max_difference(&output, &input, 32.0 / 127.0) < 1e-6);
    assert!((plugin.parameter_value(AGAIN_GAIN) - 32.0 / 127.0).abs() < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn instruments_start_notes_at_their_sample_offsets() {
    let _serial = serial();
    let module = module("note-expression-synth");
    let class = module
        .classes()
        .into_iter()
        .find(|c| c.kind == PluginKind::Instrument)
        .unwrap();
    let mut plugin = Plugin::new(
        &module,
        &class.class_id,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    plugin
        .prepare(&ProcessConfig {
            input: Layout::None,
            ..config(48_000.0, SampleFormat::F32)
        })
        .unwrap();
    let mut output = vec![vec![1.0f32; 512]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process::<f32>(
            &plughost_core::BlockContext::new(outputs.first().map_or(0, |c| c.len())),
            &[],
            &mut outputs,
            &[],
            &[MidiEvent::note_on(100, 0, 60, 127)],
            &mut Vec::new(),
        )
        .unwrap();
    // The synth's sine starts at zero on the note's first sample.
    let first = output[0].iter().position(|s| *s != 0.0);
    assert_eq!(first, Some(101));
}

/// The test delay's parameters: gain, whether the last restored state was project state, and
/// the MIDI controller mapped to the gain (0: volume, 1: expression).
const TEST_DELAY_PROJECT_STATE: u64 = 1;
const TEST_DELAY_GAIN_CONTROLLER: u64 = 2;

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn restored_state_is_marked_as_project_or_preset() {
    let _serial = serial();
    let state = plugin("plughost-test-delay")
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    let mut target = plugin("plughost-test-delay");
    target
        .restore_state(&state, plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(target.parameter_value(TEST_DELAY_PROJECT_STATE), 1.0);
    target.restore_preset(&Preset::from_state(&state)).unwrap();
    assert_eq!(target.parameter_value(TEST_DELAY_PROJECT_STATE), 0.0);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn changed_midi_controller_assignments_are_followed() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    // The plugin maps its gain to controller 11 instead of 7 and reports the change.
    plugin
        .set_parameter(TEST_DELAY_GAIN_CONTROLLER, 1.0)
        .unwrap();
    plugin.idle();
    let input = vec![vec![0.5f32; 512]; 2];
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; 512]; 2];
    for events in [&[MidiEvent::control_change(0, 0, 11, 64)][..], &[]] {
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        plugin
            .process(
                &plughost_core::BlockContext::new(
                    outputs.first().map_or(0, |channel| channel.len()),
                ),
                &inputs,
                &mut outputs,
                &[],
                events,
                &mut Vec::new(),
            )
            .unwrap();
    }
    assert!((output[0][511] - 0.5 * 64.0 / 127.0).abs() < 1e-6);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn processor_context_requirements_are_read_after_setup() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    assert_eq!(
        plugin.process_context_requirements(),
        Err(Vst3Error::NotPrepared)
    );
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    assert_eq!(
        plugin.process_context_requirements().unwrap(),
        Some((1 << 1) | (1 << 2) | (1 << 6) | (1 << 7) | (1 << 10))
    );
}

#[test]
#[ignore = "needs scripts/build-test-plugins.sh"]
fn a_cyclic_unit_hierarchy_is_an_error_and_recovers() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    let good = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    // The fixture makes a unit its own parent after a state with a negative gain is restored.
    let mut broken = good.clone();
    broken.component = (-1.0f64).to_le_bytes().to_vec();
    assert!(
        plugin
            .restore_state(&broken, plughost_core::StatePurpose::Project)
            .is_err()
    );
    assert!(matches!(
        plugin.parameter_details(0),
        Err(Error::Vst3(Vst3Error::ParameterGroups))
    ));
    plugin
        .restore_state(&good, plughost_core::StatePurpose::Project)
        .unwrap();
    assert!(plugin.parameter_details(0).unwrap().groups.is_some());
}
