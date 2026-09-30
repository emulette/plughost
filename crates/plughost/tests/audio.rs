//! Routed audio is exercised through real native plugins and the supervised helper.
use plughost::*;
mod support;

use support::{routing, spawn};

fn chain(format: PluginFormat, slots: usize) -> Chain {
    spawn(&vec![routing(format); slots])
}
fn ids(format: PluginFormat) -> (u64, u64, u64, u64) {
    match format {
        PluginFormat::Vst3 => (0, 1, 0, 1),
        PluginFormat::Clap => (20, 21, 30, 31),
        _ => unreachable!(),
    }
}
fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}
fn route(
    id: u64,
    layout: Layout,
    source: AudioSource,
    adaptation: ChannelAdaptation,
) -> AudioInputRoute {
    AudioInputRoute {
        bus: bus(id, layout),
        source,
        adaptation,
    }
}
fn config(format: PluginFormat, precision: SampleFormat) -> RoutedChainConfig {
    let (key, main, auxiliary, out) = ids(format);
    RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 16,
        sample_format: precision,
        mode: ProcessMode::Offline,
        inputs: vec![Layout::Mono, Layout::Stereo],
        slots: vec![SlotAudioConfig {
            events: Default::default(),
            configuration: (format == PluginFormat::Clap).then_some(101),
            inputs: vec![
                route(
                    key,
                    Layout::Mono,
                    AudioSource::External { bus: 0 },
                    ChannelAdaptation::Exact,
                ),
                route(
                    main,
                    Layout::Stereo,
                    AudioSource::External { bus: 1 },
                    ChannelAdaptation::Exact,
                ),
            ],
            outputs: vec![bus(auxiliary, Layout::Stereo), bus(out, Layout::Stereo)],
        }],
    }
}
fn options() -> RenderOptions {
    RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    }
}
fn input(frames: usize) -> Vec<Vec<f64>> {
    vec![
        vec![0.25; frames],
        vec![0.123456789123; frames],
        vec![0.765432198765; frames],
    ]
}
fn run(chain: &mut Chain, input: &[Vec<f64>]) -> Vec<Vec<f64>> {
    render(
        chain,
        &input.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        input[0].len(),
        &[],
        &options(),
    )
    .unwrap()
    .channels
}
fn assert_audio(actual: &[Vec<f64>], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (channel, expected) in actual.iter().zip(expected) {
        assert!(
            channel
                .iter()
                .all(|sample| (sample - expected).abs() < 1e-14),
            "{channel:?} != {expected}"
        );
    }
}

#[test]
#[ignore = "needs helper and native routing fixtures"]
fn native_bus_identity_f64_sidechain_render_and_bypass() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = chain(format, 1);
        let buses = chain.audio_buses(0).unwrap();
        let main = buses
            .iter()
            .find(|bus| bus.direction == AudioDirection::Input && bus.role == AudioBusRole::Main)
            .unwrap();
        assert_eq!(main.index, 1);
        assert_eq!(main.id, ids(format).1);
        let configurations = chain.audio_configurations(0).unwrap();
        if format == PluginFormat::Clap {
            assert!(
                configurations
                    .iter()
                    .any(|configuration| configuration.id == 101)
            );
        }
        let negotiated = chain
            .prepare_audio(&config(format, SampleFormat::F64))
            .unwrap();
        assert_eq!(negotiated.len(), 1);
        assert_eq!(negotiated[0].len(), 4);
        let mut empty = [Vec::<f64>::new(), Vec::new(), Vec::new(), Vec::new()];
        chain
            .process_audio_f64(
                &BlockContext::new(0),
                &[&[], &[], &[]],
                &mut empty.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert!(empty.iter().all(Vec::is_empty));
        let audio = input(37);
        assert_audio(
            &run(&mut chain, &audio),
            &[audio[1][0], 0.25, audio[1][0] * 0.75, audio[2][0] * 0.75],
        );
        assert_eq!(chain.bypass(0).unwrap().enabled, Some(false));
        chain.set_bypass(0, true).unwrap();
        assert_eq!(chain.bypass(0).unwrap().enabled, Some(true));
        assert_audio(
            &run(&mut chain, &audio),
            &[audio[1][0], 0.25, audio[1][0], audio[2][0]],
        );
    }
}

#[test]
#[ignore = "needs helper and native routing fixtures"]
fn routed_state_reset_recovery_and_failed_reprepare_preserve_contract() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = chain(format, 1);
        let valid = config(format, SampleFormat::F64);
        chain.prepare_audio(&valid).unwrap();
        chain.set_parameter(0, 0, 0.5).unwrap();
        let state = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        let audio = input(17);
        let expected = [audio[1][0], 0.25, audio[1][0] * 0.375, audio[2][0] * 0.375];
        let mut rejected = valid.clone();
        rejected.sample_rate = 12_345.0;
        assert!(chain.prepare_audio(&rejected).is_err());
        assert_audio(&run(&mut chain, &audio), &expected);
        chain.reprepare().unwrap();
        assert_audio(&run(&mut chain, &audio), &expected);
        chain.reset().unwrap();
        assert_audio(&run(&mut chain, &audio), &expected);
        chain.set_parameter(0, 0, 1.0).unwrap();
        chain
            .restore_state(0, &state, plughost::StatePurpose::Project)
            .unwrap();
        assert_audio(&run(&mut chain, &audio), &expected);
        chain = chain.recover(&[]).unwrap();
        assert_audio(
            &run(&mut chain, &audio),
            &[audio[1][0], 0.25, audio[1][0] * 0.75, audio[2][0] * 0.75],
        );
        chain
            .restore_state(0, &state, plughost::StatePurpose::Project)
            .unwrap();
        assert_audio(&run(&mut chain, &audio), &expected);
    }
}

#[test]
#[ignore = "needs helper and native routing fixtures"]
fn precision_rejection_preserves_output_and_f32_legacy_entry_uses_routing() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = chain(format, 1);
        chain
            .prepare_audio(&config(format, SampleFormat::F64))
            .unwrap();
        let mut output = vec![vec![99.0f32; 4]; 4];
        let result = chain.process_audio_f32(
            &BlockContext {
                frames: 4,
                transport: None,
            },
            &[&[0.25; 4], &[0.5; 4], &[0.75; 4]],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &[],
            &mut Vec::new(),
        );
        assert!(matches!(
            result,
            Err(Error::Input {
                error: InputError::AudioSampleFormat,
                ..
            })
        ));
        assert_eq!(output, vec![vec![99.0; 4]; 4]);
        chain
            .prepare_audio(&config(format, SampleFormat::F32))
            .unwrap();
        chain
            .process_audio_f32(
                &BlockContext {
                    frames: 4,
                    transport: None,
                },
                &[&[0.25; 4], &[0.5; 4], &[0.75; 4]],
                &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(
            output,
            vec![vec![0.5; 4], vec![0.25; 4], vec![0.375; 4], vec![0.5625; 4]]
        );
        let wrong = render(
            &mut chain,
            &[&[0.0f64; 0], &[0.0f64; 0], &[0.0f64; 0]],
            0,
            &[],
            &options(),
        );
        assert!(matches!(
            wrong,
            Err(Error::Input {
                error: InputError::AudioSampleFormat,
                ..
            })
        ));
        let config = chain
            .main_bus_config(48_000.0, 16, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        chain.reprepare().unwrap();
        let legacy = render(
            &mut chain,
            &[&[0.5f32; 4], &[0.75f32; 4]],
            4,
            &[],
            &options(),
        )
        .unwrap();
        assert_eq!(legacy.channels, vec![vec![0.5; 4], vec![0.75; 4]]);
    }
}

#[test]
#[ignore = "needs helper and native routing fixtures"]
fn serial_previous_bus_and_explicit_mono_stereo_adaptation() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = chain(format, 2);
        let (key, main, _, out) = ids(format);
        let routed = RoutedChainConfig {
            event_inputs: 0,
            sample_rate: 48_000.0,
            max_block_size: 16,
            sample_format: SampleFormat::F64,
            mode: ProcessMode::Offline,
            inputs: vec![Layout::Mono, Layout::Stereo],
            slots: vec![
                SlotAudioConfig {
                    events: Default::default(),
                    configuration: (format == PluginFormat::Clap).then_some(100),
                    inputs: vec![route(
                        main,
                        Layout::Mono,
                        AudioSource::External { bus: 0 },
                        ChannelAdaptation::Exact,
                    )],
                    outputs: vec![bus(out, Layout::Mono)],
                },
                SlotAudioConfig {
                    events: Default::default(),
                    configuration: (format == PluginFormat::Clap).then_some(101),
                    inputs: vec![
                        route(
                            key,
                            Layout::Mono,
                            AudioSource::External { bus: 1 },
                            ChannelAdaptation::StereoToMono,
                        ),
                        route(
                            main,
                            Layout::Stereo,
                            AudioSource::Previous { bus: out },
                            ChannelAdaptation::MonoToStereo,
                        ),
                    ],
                    outputs: vec![bus(out, Layout::Stereo)],
                },
            ],
        };
        chain.prepare_audio(&routed).unwrap();
        let audio = vec![vec![0.123456789123; 33], vec![0.25; 33], vec![0.5; 33]];
        assert_audio(&run(&mut chain, &audio), &[audio[0][0] * 0.625; 2]);
    }
}
