//! Audio Unit hosting against the Apple units every Mac has: AUDelay and DLSMusicDevice.

#![cfg(target_os = "macos")]

use plughost_core::{Layout, MidiEvent, PluginKind, ProcessConfig, ProcessMode, SampleFormat};
use plughost_formats::HostedPlugin;
use plughost_formats::au::{self, Plugin};

/// 'aufx' 'dely' 'appl'
const AU_DELAY: &str = "6175667864656C796170706C";
/// 'aumu' 'dls ' 'appl'
const DLS_MUSIC_DEVICE: &str = "61756D75646C7320";
/// AUDelay's parameters: wet/dry mix (percent) and delay time (seconds).
const WET_DRY_MIX: u64 = 0;
const DELAY_TIME: u64 = 1;

fn config(input: Layout) -> ProcessConfig {
    ProcessConfig {
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        input,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
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

fn run(plugin: &mut Plugin, input: &[Vec<f32>], blocks: usize) -> Vec<Vec<f32>> {
    let mut output = vec![Vec::new(); 2];
    for block in 0..blocks {
        let slices: Vec<&[f32]> = input
            .iter()
            .map(|c| &c[block * 512..(block + 1) * 512])
            .collect();
        let mut out = vec![vec![0.0f32; 512]; 2];
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
        plugin
            .process(
                &plughost_core::BlockContext::new(outs.first().map_or(0, |channel| channel.len())),
                &slices,
                &mut outs,
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        for (all, part) in output.iter_mut().zip(out) {
            all.extend(part);
        }
    }
    output
}

fn value(plugin: &mut Plugin, id: u64) -> f64 {
    plugin
        .parameters()
        .into_iter()
        .find(|(p, _)| p.id == id)
        .unwrap()
        .1
}

#[test]
fn apple_units_are_listed_from_the_registry() {
    let components = au::components();
    let delay = components.iter().find(|c| c.class_id == AU_DELAY).unwrap();
    assert_eq!(
        (delay.vendor.as_str(), delay.name.as_str()),
        ("Apple", "AUDelay")
    );
    assert_eq!(delay.kind, PluginKind::Effect);
    let dls = components
        .iter()
        .find(|c| c.class_id.starts_with(DLS_MUSIC_DEVICE))
        .unwrap();
    assert_eq!(dls.kind, PluginKind::Instrument);
}

#[test]
fn standard_units_and_unavailable_factory_ids_are_explicit() {
    let mut plugin = Plugin::new(AU_DELAY).unwrap();
    let details = plugin.parameter_details(DELAY_TIME).unwrap();
    assert_eq!(
        details.native_unit,
        Some(objc2_audio_toolbox::AudioUnitParameterUnit::Seconds.0)
    );
    let before = value(&mut plugin, DELAY_TIME);
    assert!(
        plugin
            .select_factory_preset(&plughost_core::FactoryPresetId::AudioUnit { number: i64::MAX })
            .is_err()
    );
    assert_eq!(value(&mut plugin, DELAY_TIME), before);
    assert!(
        plugin
            .factory_presets()
            .unwrap()
            .iter()
            .all(|preset| matches!(preset.id, plughost_core::FactoryPresetId::AudioUnit { .. }))
    );
    assert!(
        !plugin
            .take_parameter_events()
            .events
            .iter()
            .any(|event| matches!(event, plughost_core::ParameterEvent::Dirty { .. }))
    );
}

#[test]
fn an_effect_processes_and_its_state_survives_a_fresh_instance() {
    let mut plugin = Plugin::new(AU_DELAY).unwrap();
    plugin.prepare(&config(Layout::Stereo)).unwrap();
    // Fully dry: the delay passes its input through.
    plugin.set_parameter(WET_DRY_MIX, 0.0).unwrap();
    let input = signal(512 * 8);
    let output = run(&mut plugin, &input, 8);
    let difference = output
        .iter()
        .flatten()
        .zip(input.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(difference < 1e-4, "{difference}");

    plugin.set_parameter(DELAY_TIME, 0.25).unwrap();
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    let edited = value(&mut plugin, DELAY_TIME);
    assert!((edited - 0.25).abs() < 1e-4);
    let preset = au::to_preset(&state.component).unwrap();
    assert!(preset.starts_with(b"<?xml"));

    let mut fresh = Plugin::new(AU_DELAY).unwrap();
    let mut restored = state.clone();
    restored.component = au::from_preset(&preset).unwrap();
    fresh
        .restore_state(&restored, plughost_core::StatePurpose::Project)
        .unwrap();
    assert!((value(&mut fresh, DELAY_TIME) - edited).abs() < 1e-4);
}

#[test]
fn reset_clears_the_delay_line() {
    let mut plugin = Plugin::new(AU_DELAY).unwrap();
    plugin.prepare(&config(Layout::Stereo)).unwrap();
    plugin.set_parameter(WET_DRY_MIX, 1.0).unwrap();
    run(&mut plugin, &signal(512 * 8), 8);
    plugin.reset().unwrap();
    let silence = vec![vec![0.0f32; 512 * 4]; 2];
    let peak = run(&mut plugin, &silence, 4)
        .iter()
        .flatten()
        .fold(0.0f32, |p, s| p.max(s.abs()));
    assert!(peak < 1e-6, "{peak}");
}

#[test]
fn an_instrument_plays_notes_from_their_offset() {
    let class = au::components()
        .into_iter()
        .find(|c| c.class_id.starts_with(DLS_MUSIC_DEVICE))
        .unwrap()
        .class_id;
    let mut plugin = Plugin::new(&class).unwrap();
    plugin.prepare(&config(Layout::None)).unwrap();
    let mut block = |events: &[MidiEvent]| {
        let mut output = vec![vec![1.0f32; 512]; 2];
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        plugin
            .process(
                &plughost_core::BlockContext::new(
                    outputs.first().map_or(0, |channel| channel.len()),
                ),
                &[],
                &mut outputs,
                &[],
                events,
                &mut Vec::new(),
            )
            .unwrap();
        output
    };
    let peak = |output: &[Vec<f32>], range: std::ops::Range<usize>| {
        output
            .iter()
            .flat_map(|c| &c[range.clone()])
            .fold(0.0f32, |p, s| p.max(s.abs()))
    };
    assert!(peak(&block(&[]), 0..512) < 1e-6);
    let played = block(&[MidiEvent::note_on(100, 0, 60, 127)]);
    // DLSMusicDevice's attack starts a few samples after the note.
    assert!(peak(&played, 0..100) < 1e-6);
    assert!(peak(&played, 100..132) > 1e-6);
    assert!(peak(&played, 100..512) > 1e-3);
}

#[test]
fn surround_layouts_pass_each_channel_through() {
    for layout in Layout::ALL
        .into_iter()
        .filter(|layout| layout.channels() > 2)
    {
        let mut plugin = Plugin::new(AU_DELAY).unwrap();
        plugin
            .prepare(&ProcessConfig {
                input: layout,
                output: layout,
                ..config(layout)
            })
            .unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        plugin.set_parameter(WET_DRY_MIX, 0.0).unwrap();
        // A different constant per channel shows whether channels stay in place.
        let input: Vec<Vec<f32>> = (0..layout.channels())
            .map(|ch| vec![0.1 * (ch + 1) as f32; 512])
            .collect();
        let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
        let mut output = vec![vec![0.0f32; 512]; layout.channels()];
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        plugin
            .process(
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
        for (channel, (out, expected)) in output.iter().zip(&input).enumerate() {
            assert!(
                (out[511] - expected[511]).abs() < 1e-4,
                "{layout:?} channel {channel}"
            );
        }
    }
}

fn audio_config(layout: Layout) -> plughost_core::AudioConfig {
    let bus = plughost_core::AudioBusConfig {
        id: 0,
        layout,
        active: true,
    };
    plughost_core::AudioConfig {
        events: Default::default(),
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        mode: ProcessMode::Offline,
        configuration: None,
        inputs: vec![bus],
        outputs: vec![bus],
    }
}

#[test]
fn explicit_bus_api_preserves_speaker_order_and_rejects_f64_without_unpreparing() {
    for layout in Layout::ALL
        .into_iter()
        .filter(|&layout| layout != Layout::None)
    {
        let mut plugin = Plugin::new(AU_DELAY).unwrap();
        let config = audio_config(layout);
        let buses = plugin.prepare_audio(&config).unwrap();
        assert_eq!(plugin.audio_buses().unwrap(), buses);
        assert!(
            buses
                .iter()
                .filter(|bus| bus.active == Some(true))
                .all(|bus| {
                    bus.id == 0
                        && bus.layout == Some(layout)
                        && bus.channels as usize == layout.channels()
                        && bus.f64 == plughost_core::Support::Unsupported
                })
        );
        assert!(plugin.audio_configurations().unwrap().is_empty());
        plugin.set_parameter(WET_DRY_MIX, 0.0).unwrap();
        assert!(
            plugin
                .prepare_audio(&plughost_core::AudioConfig {
                    sample_format: SampleFormat::F64,
                    ..config
                })
                .is_err()
        );
        let input: Vec<Vec<f32>> = (0..layout.channels())
            .map(|channel| {
                let mut samples = vec![0.0; 512];
                samples[channel * 16] = (channel + 1) as f32 / 10.0;
                samples
            })
            .collect();
        let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
        let mut output = vec![vec![0.0; 512]; layout.channels()];
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        plugin
            .processor()
            .process_audio_f32(
                &plughost_core::BlockContext::new(512),
                &inputs,
                &mut outputs,
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        for (actual, expected) in output.iter().flatten().zip(input.iter().flatten()) {
            assert!((actual - expected).abs() < 1e-4);
        }
    }
}

#[test]
fn native_bypass_keeps_parameter_state_and_survives_reset() {
    let mut plugin = Plugin::new(AU_DELAY).unwrap();
    plugin.prepare_audio(&audio_config(Layout::Stereo)).unwrap();
    plugin.set_parameter(WET_DRY_MIX, 0.75).unwrap();
    // The AUv2 bridge updates its parameter tree through the run loop. Saving settles
    // the edit, so bypass is compared against the edited value, not a stale snapshot.
    plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    let wet = value(&mut plugin, WET_DRY_MIX);
    assert!((wet - 0.75).abs() < 1e-4);
    assert_ne!(
        plugin.bypass().unwrap().support,
        plughost_core::Support::Unsupported
    );
    plugin.set_bypass(true).unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
    assert_eq!(value(&mut plugin, WET_DRY_MIX), wet);
    plugin.reset().unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
    assert!((value(&mut plugin, WET_DRY_MIX) - wet).abs() < 1e-4);
    let input = signal(512);
    let output = run(&mut plugin, &input, 1);
    for (actual, expected) in output.iter().flatten().zip(input.iter().flatten()) {
        assert!((actual - expected).abs() < 1e-4);
    }
    plugin.set_bypass(false).unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(false));
    assert!((value(&mut plugin, WET_DRY_MIX) - wet).abs() < 1e-4);
}
