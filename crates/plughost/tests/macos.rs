//! Apple AU regressions through the public application API and the signed helper.
#![cfg(target_os = "macos")]

use plughost::*;

mod support;

fn chain() -> Chain {
    support::spawn(&[PluginRef {
        format: PluginFormat::AudioUnit,
        bundle: None,
        class_id: "6175667864656C796170706C".into(),
    }])
}

fn render_markers(chain: &mut Chain, layout: Layout) {
    let input: Vec<Vec<f32>> = (0..layout.channels())
        .map(|channel| vec![(channel + 1) as f32 / 10.0; 1024])
        .collect();
    let output = render(
        chain,
        &input.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        1024,
        &[],
        &RenderOptions {
            tail: TailPolicy::Reported,
            max_tail_seconds: 0.0,
        },
    )
    .unwrap();
    for (actual, expected) in output.channels.iter().flatten().zip(input.iter().flatten()) {
        assert!((actual - expected).abs() < 1e-4);
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh"]
fn parameter_edits_refresh_tail_before_the_next_render() {
    for text_edit in [false, true] {
        let mut chain = chain();
        let config = chain
            .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        let previous_tail = chain.tail();
        if text_edit {
            assert_eq!(
                chain.parameter_text(0, 0, 0.0).unwrap_err().kind(),
                FailureKind::Unsupported
            );
            chain.set_parameter_text(0, 0, "0").unwrap();
        } else {
            chain.set_parameter(0, 0, 0.0).unwrap();
        }
        assert_ne!(chain.tail(), previous_tail);
        render_markers(&mut chain, Layout::Stereo);
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh"]
fn discrete_surround_routing_survives_reconfiguration_and_preset_restore() {
    let mut chain = chain();
    for layout in Layout::ALL
        .into_iter()
        .filter(|layout| layout.channels() > 2)
    {
        let bus = AudioBusConfig {
            id: 0,
            layout,
            active: true,
        };
        let config = RoutedChainConfig {
            event_inputs: 0,
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            inputs: vec![layout],
            slots: vec![SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![AudioInputRoute {
                    bus,
                    source: AudioSource::External { bus: 0 },
                    adaptation: ChannelAdaptation::Exact,
                }],
                outputs: vec![bus],
            }],
        };
        let negotiated = chain.prepare_audio(&config).unwrap();
        assert_eq!(chain.audio_buses(0).unwrap(), negotiated[0]);
        assert!(negotiated[0].iter().all(|bus| bus.layout == Some(layout)));
        chain.set_parameter(0, 0, 0.0).unwrap();
        render_markers(&mut chain, layout);
        let preset = chain.export_preset(0).unwrap();
        chain.set_parameter(0, 0, 1.0).unwrap();
        chain.import_preset(0, &preset).unwrap();
        render_markers(&mut chain, layout);
        chain.reset().unwrap();
        render_markers(&mut chain, layout);
        assert!(
            chain
                .prepare_audio(&RoutedChainConfig {
                    sample_format: SampleFormat::F64,
                    ..config
                })
                .is_err()
        );
        render_markers(&mut chain, layout);
    }
}
