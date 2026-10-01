use plughost::{
    AudioBusConfig, AudioInputRoute, AudioSource, BlockContext, ChannelAdaptation, Error,
    FailureKind, InputError, Layout, PluginFormat, ProcessMode, RoutedChainConfig, SampleFormat,
    SlotAudioConfig,
};

mod support;

use support::{delay, delay_variant, routing, spawn};

fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}

fn route(id: u64, layout: Layout, source: AudioSource) -> AudioInputRoute {
    AudioInputRoute {
        bus: bus(id, layout),
        source,
        adaptation: ChannelAdaptation::Exact,
    }
}

fn ids(format: PluginFormat) -> (u64, u64, u64, u64) {
    if format == PluginFormat::Clap {
        (20, 21, 30, 31)
    } else {
        (0, 1, 0, 1)
    }
}

fn config(
    inputs: Vec<Layout>,
    slots: Vec<SlotAudioConfig>,
    sample_format: SampleFormat,
) -> RoutedChainConfig {
    RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format,
        mode: ProcessMode::Offline,
        inputs,
        slots,
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn reversed_bus_requests_route_previous_auxiliary_by_id_in_native_channel_order() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (aux_in, main_in, aux_out, main_out) = ids(format);
        let mut chain = spawn(&[routing(format), routing(format)]);
        let first = SlotAudioConfig {
            events: Default::default(),
            configuration: (format == PluginFormat::Clap).then_some(101),
            // Deliberately opposite native bus index order.
            inputs: vec![
                route(main_in, Layout::Stereo, AudioSource::External { bus: 0 }),
                route(aux_in, Layout::Mono, AudioSource::External { bus: 1 }),
            ],
            outputs: vec![bus(main_out, Layout::Stereo), bus(aux_out, Layout::Stereo)],
        };
        let mut second = first.clone();
        second.inputs[0].source = AudioSource::Previous { bus: main_out };
        second.inputs[1].source = AudioSource::Previous { bus: aux_out };
        second.inputs[1].adaptation = ChannelAdaptation::StereoToMono;
        let request = config(
            vec![Layout::Stereo, Layout::Mono],
            vec![first, second],
            SampleFormat::F64,
        );
        chain.prepare_audio(&request).unwrap();
        let left = [0.75f64; 4];
        let right = [0.5f64; 4];
        let key = [0.25f64; 4];
        let mut output = vec![vec![-1.0; 4]; 4];
        let mut buffers: Vec<&mut [f64]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        chain
            .process_audio_f64(
                &BlockContext::new(4),
                &[&left, &right, &key],
                &mut buffers,
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        // Last slot's monitor bus (native index 0) precedes its main bus (native index 1).
        assert_eq!(
            output,
            vec![
                vec![0.5625; 4],
                vec![0.5; 4],
                vec![0.28125; 4],
                vec![0.1875; 4]
            ],
            "{format:?}"
        );
    }
}

#[test]
#[ignore = "needs helper and routing fixtures (.ps1 or .sh build scripts)"]
fn disabled_auxiliary_routes_are_ignored_and_surround_channel_order_is_exact() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        for (layout, configuration) in [(Layout::Surround51, 102), (Layout::Surround71, 103)] {
            let (aux_in, main_in, aux_out, main_out) = ids(format);
            let mut chain = spawn(&[routing(format)]);
            let mut inactive = route(
                aux_in,
                Layout::Mono,
                AudioSource::Previous { bus: u64::MAX },
            );
            inactive.bus.active = false;
            // A disabled bus retains a meaningless saved adaptation without reading any source.
            inactive.adaptation = ChannelAdaptation::MonoToStereo;
            let mut discarded = bus(aux_out, Layout::Stereo);
            discarded.active = false;
            chain
                .prepare_audio(&config(
                    vec![layout],
                    vec![SlotAudioConfig {
                        events: Default::default(),
                        configuration: (format == PluginFormat::Clap).then_some(configuration),
                        inputs: vec![
                            route(main_in, layout, AudioSource::External { bus: 0 }),
                            inactive,
                        ],
                        outputs: vec![bus(main_out, layout), discarded],
                    }],
                    SampleFormat::F64,
                ))
                .unwrap();
            let input: Vec<Vec<f64>> = (0..layout.channels())
                .map(|index| vec![index as f64 + 0.125; 5])
                .collect();
            let inputs: Vec<&[f64]> = input.iter().map(Vec::as_slice).collect();
            let mut output = vec![vec![-1.0; 5]; layout.channels()];
            let mut outputs: Vec<&mut [f64]> = output.iter_mut().map(Vec::as_mut_slice).collect();
            chain
                .process_audio_f64(
                    &BlockContext::new(5),
                    &inputs,
                    &mut outputs,
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
            assert_eq!(output, input, "{format:?} {layout:?}");
        }
    }
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn late_native_reprepare_failure_preserves_every_old_slot_and_pending_audio() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (aux_in, main_in, aux_out, main_out) = ids(format);
        let mut chain = spawn(&[delay(format), routing(format)]);
        let mut unused = route(aux_in, Layout::Mono, AudioSource::Silence);
        unused.bus.active = false;
        let mut discarded = bus(aux_out, Layout::Stereo);
        discarded.active = false;
        let valid = config(
            vec![Layout::Stereo],
            vec![
                SlotAudioConfig {
                    events: Default::default(),
                    configuration: None,
                    inputs: vec![route(0, Layout::Stereo, AudioSource::External { bus: 0 })],
                    outputs: vec![bus(0, Layout::Stereo)],
                },
                SlotAudioConfig {
                    events: Default::default(),
                    configuration: (format == PluginFormat::Clap).then_some(101),
                    inputs: vec![
                        route(main_in, Layout::Stereo, AudioSource::Previous { bus: 0 }),
                        unused,
                    ],
                    outputs: vec![bus(main_out, Layout::Stereo), discarded],
                },
            ],
            SampleFormat::F32,
        );
        chain.prepare_audio(&valid).unwrap();
        chain.set_parameter(1, 0, 0.5).unwrap();
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        left[0] = 1.0;
        right[0] = 2.0;
        let mut first_left = [42.0f32; 64];
        let mut first_right = [42.0f32; 64];
        chain
            .process_audio_f32(
                &BlockContext::new(64),
                &[&left, &right],
                &mut [&mut first_left, &mut first_right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(first_left, [0.0; 64]);
        assert_eq!(first_right, [0.0; 64]);

        let mut malformed = valid.clone();
        malformed.slots[1].inputs[0].source = AudioSource::Previous { bus: 999 };
        assert!(matches!(
            chain.prepare_audio(&malformed),
            Err(Error::Input {
                error: InputError::AudioRoute { .. },
                ..
            })
        ));
        let mut refused = valid.clone();
        // Shared storage is rejected before allocating or replacing any native candidate.
        refused.max_block_size = 1 << 25;
        assert_eq!(
            chain.prepare_audio(&refused).unwrap_err().kind(),
            FailureKind::Configuration
        );
        refused.max_block_size = valid.max_block_size;
        // Slot 0 accepts this rate; the routing fixture in slot 1 mutates candidate gain and refuses it.
        refused.sample_rate = 12345.0;
        assert!(
            matches!(chain.prepare_audio(&refused), Err(Error::Operation { slot: Some(1), failure }) if failure.kind == FailureKind::Configuration)
        );

        let silence = [0.0f32; 512];
        let mut after_left = [42.0f32; 512];
        let mut after_right = [42.0f32; 512];
        chain
            .process_audio_f32(
                &BlockContext::new(512),
                &[&silence, &silence],
                &mut [&mut after_left, &mut after_right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        let mut expected_left = [0.0f32; 512];
        let mut expected_right = [0.0f32; 512];
        expected_left[416] = 0.5;
        expected_right[416] = 1.0;
        assert_eq!(after_left, expected_left, "{format:?}");
        assert_eq!(after_right, expected_right, "{format:?}");
    }
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn unused_upstream_delay_is_not_trimmed_from_an_external_main_path() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (aux_in, main_in, _, main_out) = ids(format);
        let mut chain = spawn(&[delay(format), routing(format)]);
        chain
            .prepare_audio(&config(
                vec![Layout::Stereo],
                vec![
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: None,
                        inputs: vec![route(0, Layout::Stereo, AudioSource::External { bus: 0 })],
                        outputs: vec![bus(0, Layout::Stereo)],
                    },
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: (format == PluginFormat::Clap).then_some(101),
                        inputs: vec![
                            route(main_in, Layout::Stereo, AudioSource::External { bus: 0 }),
                            route(aux_in, Layout::Mono, AudioSource::Silence),
                        ],
                        outputs: vec![bus(main_out, Layout::Stereo)],
                    },
                ],
                SampleFormat::F32,
            ))
            .unwrap();
        assert_eq!(chain.latency(), 0, "{format:?}");
        assert_eq!(chain.tail(), plughost::Tail::Samples(0));
        let left: Vec<f32> = (0..64).map(|frame| frame as f32 / 64.0).collect();
        let right: Vec<f32> = left.iter().map(|value| -*value).collect();
        let rendered = plughost::render(
            &mut chain,
            &[&left, &right],
            left.len(),
            &[],
            &plughost::RenderOptions::new(plughost::TailPolicy::Reported, 0.0),
        )
        .unwrap();
        assert_eq!(rendered.channels, vec![left, right]);
    }
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn external_sidechain_is_delayed_to_the_main_paths_arrival_across_blocks() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (aux_in, main_in, _, main_out) = ids(format);
        let mut chain = spawn(&[delay(format), routing(format)]);
        chain
            .prepare_audio(&config(
                vec![Layout::Stereo, Layout::Mono],
                vec![
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: None,
                        inputs: vec![route(0, Layout::Stereo, AudioSource::External { bus: 0 })],
                        outputs: vec![bus(0, Layout::Stereo)],
                    },
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: (format == PluginFormat::Clap).then_some(101),
                        inputs: vec![
                            route(main_in, Layout::Stereo, AudioSource::Previous { bus: 0 }),
                            route(aux_in, Layout::Mono, AudioSource::External { bus: 1 }),
                        ],
                        outputs: vec![bus(main_out, Layout::Stereo)],
                    },
                ],
                SampleFormat::F32,
            ))
            .unwrap();
        assert_eq!(chain.latency(), 480);
        let mut main = [0.0f32; 512];
        let mut key = [0.0f32; 512];
        main[0] = 1.0;
        key[0] = 0.5;
        let mut actual = [Vec::new(), Vec::new()];
        let mut start = 0;
        for frames in [127, 129, 256] {
            let end = start + frames;
            let mut left = vec![42.0f32; frames];
            let mut right = vec![42.0f32; frames];
            chain
                .process_audio_f32(
                    &BlockContext::new(frames),
                    &[&main[start..end], &main[start..end], &key[start..end]],
                    &mut [&mut left, &mut right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
            actual[0].extend(left);
            actual[1].extend(right);
            start = end;
        }
        let mut expected = vec![0.0f32; 512];
        expected[480] = 0.5;
        assert_eq!(actual, [expected.clone(), expected], "{format:?}");
    }
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn restoring_downstream_state_preserves_pending_external_sidechain_alignment() {
    check_restored_alignment(1);
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn restoring_upstream_state_clears_its_dependent_external_sidechain_history() {
    check_restored_alignment(0);
}

fn check_restored_alignment(replaced: usize) {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (aux_in, main_in, aux_out, main_out) = ids(format);
        let mut chain = spawn(&[delay(format), routing(format)]);
        chain
            .prepare_audio(&config(
                vec![Layout::Stereo, Layout::Mono],
                vec![
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: None,
                        inputs: vec![route(0, Layout::Stereo, AudioSource::External { bus: 0 })],
                        outputs: vec![bus(0, Layout::Stereo)],
                    },
                    SlotAudioConfig {
                        events: Default::default(),
                        configuration: (format == PluginFormat::Clap).then_some(101),
                        inputs: vec![
                            route(main_in, Layout::Stereo, AudioSource::Previous { bus: 0 }),
                            route(aux_in, Layout::Mono, AudioSource::External { bus: 1 }),
                        ],
                        outputs: vec![bus(main_out, Layout::Stereo), bus(aux_out, Layout::Stereo)],
                    },
                ],
                SampleFormat::F32,
            ))
            .unwrap();
        let state = chain
            .save_state(replaced, plughost::StatePurpose::Project)
            .unwrap();
        let mut main = [0.0f32; 64];
        let mut key = [0.0f32; 64];
        main[0] = 1.0;
        key[0] = 0.5;
        let mut before = vec![vec![42.0f32; 64]; 4];
        chain
            .process_audio_f32(
                &BlockContext::new(64),
                &[&main, &main, &key],
                &mut before.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        chain
            .restore_state(replaced, &state, plughost::StatePurpose::Project)
            .unwrap();
        let silence = [0.0f32; 512];
        let mut after = vec![vec![42.0f32; 512]; 4];
        chain
            .process_audio_f32(
                &BlockContext::new(512),
                &[&silence, &silence, &silence],
                &mut after.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        let mut expected = vec![vec![0.0f32; 512]; 4];
        if replaced == 1 {
            expected[0][416] = 1.0;
            expected[1][416] = 0.5;
            expected[2][416] = 0.5;
            expected[3][416] = 0.5;
        }
        assert_eq!(after, expected, "{format:?}, replaced slot {replaced}");
    }
}

#[test]
#[ignore = "needs helper, routing and latency-overflow fixtures (.ps1 or .sh build scripts)"]
fn excessive_alignment_is_rejected_before_replacing_a_working_chain() {
    let overflow = delay_variant(PluginFormat::Vst3, "latency-overflow", 7);
    let mut chain = spawn(&[overflow, routing(PluginFormat::Vst3)]);
    let original = config(
        vec![Layout::Stereo, Layout::Mono],
        vec![
            SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![route(0, Layout::Stereo, AudioSource::External { bus: 0 })],
                outputs: vec![bus(0, Layout::Stereo)],
            },
            SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![
                    route(1, Layout::Stereo, AudioSource::External { bus: 0 }),
                    route(0, Layout::Mono, AudioSource::External { bus: 1 }),
                ],
                outputs: vec![bus(1, Layout::Stereo)],
            },
        ],
        SampleFormat::F64,
    );
    chain.prepare_audio(&original).unwrap();
    let mut connected = original.clone();
    connected.slots[1].inputs[0].source = AudioSource::Previous { bus: 0 };
    let error = chain.prepare_audio(&connected).unwrap_err();
    assert_eq!(error.kind(), FailureKind::Configuration);
    assert_eq!(chain.latency(), 0);
    let value = 1.0 + f64::EPSILON;
    let input = [value; 4];
    let key = [0.0; 4];
    let (mut left, mut right) = ([99.0; 4], [99.0; 4]);
    chain
        .process_audio_f64(
            &BlockContext::new(4),
            &[&input, &input, &key],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left, input);
    assert_eq!(right, input);
}

#[test]
#[ignore = "needs helper and delay fixtures (.ps1 or .sh build scripts)"]
fn event_budget_rejection_preserves_pending_native_audio() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[delay(format)]);
        let config = chain
            .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        let mut impulse = [0.0f32; 64];
        impulse[0] = 1.0;
        chain
            .process_audio_f32(
                &BlockContext::new(64),
                &[&impulse, &impulse],
                &mut [&mut [0.0; 64], &mut [0.0; 64]],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        let automation = vec![
            plughost::AutomationEvent {
                slot: 0,
                change: plughost::ParameterChange {
                    id: 0,
                    offset: 0,
                    value: 0.25
                },
            };
            plughost::MAX_BLOCK_EVENTS + 1
        ];
        let (mut rejected_left, mut rejected_right) = ([99.0; 8], [99.0; 8]);
        let error = chain
            .process_audio_f32(
                &BlockContext::new(8),
                &[&[0.0; 8], &[0.0; 8]],
                &mut [&mut rejected_left, &mut rejected_right],
                &automation,
                &[],
                &mut Vec::new(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            Error::Input {
                error: InputError::EventCapacity,
                ..
            }
        ));
        assert_eq!(rejected_left, [99.0; 8]);
        assert_eq!(rejected_right, [99.0; 8]);
        let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
        chain
            .process_audio_f32(
                &BlockContext::new(512),
                &[&[0.0; 512], &[0.0; 512]],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        let mut expected = [0.0; 512];
        expected[416] = 1.0;
        assert_eq!(left, expected);
        assert_eq!(right, expected);
    }
}
