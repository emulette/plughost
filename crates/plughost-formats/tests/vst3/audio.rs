use super::*;
use plughost_core::{AudioBusConfig, AudioBusRole, AudioConfig, AudioDirection, Support};

fn buses(sample_format: SampleFormat, main: Layout) -> AudioConfig {
    AudioConfig {
        events: Default::default(),
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format,
        mode: ProcessMode::Offline,
        configuration: None,
        inputs: vec![
            AudioBusConfig {
                id: 0,
                layout: Layout::Mono,
                active: true,
            },
            AudioBusConfig {
                id: 1,
                layout: main,
                active: true,
            },
        ],
        outputs: vec![
            AudioBusConfig {
                id: 0,
                layout: Layout::Stereo,
                active: true,
            },
            AudioBusConfig {
                id: 1,
                layout: main,
                active: true,
            },
        ],
    }
}

#[test]
#[ignore = "needs helper and routing test plugin"]
fn native_bus_roles_and_main_convenience_do_not_assume_bus_zero() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-routing");
    let discovered = plugin.audio_buses().unwrap();
    assert_eq!(discovered.len(), 4);
    assert_eq!(
        discovered
            .iter()
            .map(|bus| (bus.direction, bus.id, bus.role))
            .collect::<Vec<_>>(),
        [
            (AudioDirection::Input, 0, AudioBusRole::Auxiliary),
            (AudioDirection::Input, 1, AudioBusRole::Main),
            (AudioDirection::Output, 0, AudioBusRole::Auxiliary),
            (AudioDirection::Output, 1, AudioBusRole::Main),
        ]
    );
    assert!(
        discovered
            .iter()
            .all(|bus| bus.active.is_none() && bus.f64 == Support::Supported)
    );
    assert!(plugin.audio_configurations().unwrap().is_empty());
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let active = plugin.audio_buses().unwrap();
    assert!(
        active
            .iter()
            .all(|bus| bus.active == Some(bus.role == AudioBusRole::Main))
    );
    plugin.set_parameter(0, 0.5).unwrap();
    let input = [2.0f32; 4];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    plugin
        .process(
            &plughost_core::BlockContext::new(4),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left, [1.0; 4]);
    assert_eq!(right, [1.0; 4]);
}

#[test]
#[ignore = "needs routing test plugin"]
fn sidechain_auxiliary_outputs_and_point_automation_follow_native_bus_order() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-routing");
    let active = plugin
        .prepare_audio(&buses(SampleFormat::F32, Layout::Stereo))
        .unwrap();
    assert!(active.iter().all(|bus| bus.active == Some(true)));
    plugin.set_parameter(0, 0.5).unwrap();
    let (key, left, right) = ([0.25f32; 4], [2.0; 4], [4.0; 4]);
    let mut outputs = [[0.0f32; 4]; 4];
    let mut slices: Vec<_> = outputs
        .iter_mut()
        .map(|channel| channel.as_mut_slice())
        .collect();
    plugin
        .process(
            &plughost_core::BlockContext::new(4),
            &[&key, &left, &right],
            &mut slices,
            &[plughost_core::ParameterChange {
                id: 0,
                offset: 2,
                value: 0.25,
            }],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(outputs[0], [2.0; 4]);
    assert_eq!(outputs[1], [0.25; 4]);
    assert_eq!(outputs[2], [0.75, 0.75, 0.375, 0.375]);
    assert_eq!(outputs[3], [1.5, 1.5, 0.75, 0.75]);
}

#[test]
#[ignore = "needs routing test plugin"]
fn multibus_f64_surround_and_bypass_survive_state_and_reset() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-routing");
    let active = plugin
        .prepare_audio(&buses(SampleFormat::F64, Layout::Surround51))
        .unwrap();
    assert_eq!(
        active
            .iter()
            .filter(|bus| bus.role == AudioBusRole::Main)
            .map(|bus| bus.channels)
            .collect::<Vec<_>>(),
        [6, 6]
    );
    plugin.set_parameter(0, 0.5).unwrap();
    let mut inputs = vec![vec![0.0f64; 4]; 7];
    for (index, channel) in inputs.iter_mut().enumerate().skip(1) {
        channel.fill(index as f64 + 1.0e-10);
    }
    let input: Vec<_> = inputs.iter().map(Vec::as_slice).collect();
    let mut outputs = vec![vec![0.0f64; 4]; 8];
    let mut output: Vec<_> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .processor()
        .process_audio_f64(
            &plughost_core::BlockContext::new(4),
            &input,
            &mut output,
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(outputs[2][0], (1.0 + 1.0e-10) * 0.5);
    plugin.set_bypass(true).unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    plugin.reset().unwrap();
    assert_eq!(plugin.audio_buses().unwrap(), active);
    let mut output: Vec<_> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process(
            &plughost_core::BlockContext::new(4),
            &input,
            &mut output,
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(outputs[2], inputs[1]);
    assert_eq!(outputs[7], inputs[6]);
    let module = module("plughost-test-routing");
    let mut restored = Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &plughost_core::HostIdentity::default(),
    )
    .unwrap();
    restored
        .restore_state(&state, plughost_core::StatePurpose::Project)
        .unwrap();
    restored
        .prepare_audio(&buses(SampleFormat::F64, Layout::Surround51))
        .unwrap();
    assert_eq!(restored.bypass().unwrap().enabled, Some(true));
    restored.set_bypass(false).unwrap();
    assert_eq!(restored.parameter_value(0), 0.5);
    assert_eq!(
        restored.audio_config(),
        Some(buses(SampleFormat::F64, Layout::Surround51))
    );
    let rendered = render(
        &mut restored,
        &input,
        4,
        &[],
        &RenderOptions {
            tail: TailPolicy::Reported,
            max_tail_seconds: 0.0,
        },
    )
    .unwrap();
    assert_eq!(rendered.channels.len(), 8);
    assert_eq!(rendered.channels[2][0], inputs[1][0] * 0.5);
}

#[test]
#[ignore = "needs routing test plugin"]
fn invalid_native_audio_requests_and_unsupported_bypass_are_explicit() {
    let _serial = serial();
    let mut routing = plugin("plughost-test-routing");
    let mut invalid = buses(SampleFormat::F32, Layout::Stereo);
    invalid.inputs[0].id = 100;
    assert!(matches!(
        routing.prepare_audio(&invalid),
        Err(Error::Vst3(Vst3Error::UnknownAudioBus { id: 100 }))
    ));
    invalid = buses(SampleFormat::F32, Layout::Stereo);
    invalid.configuration = Some(0);
    assert!(matches!(
        routing.prepare_audio(&invalid),
        Err(Error::Vst3(Vst3Error::AudioConfigurationsUnsupported))
    ));
    invalid = buses(SampleFormat::F32, Layout::Stereo);
    invalid.sample_rate = 12345.0;
    assert!(matches!(
        routing.prepare_audio(&invalid),
        Err(Error::Vst3(Vst3Error::SetupRejected(_)))
    ));
    let mut delay = plugin("plughost-test-delay");
    assert_eq!(delay.bypass().unwrap().support, Support::Unsupported);
    assert_eq!(
        delay.set_bypass(true),
        Err(Error::Vst3(Vst3Error::BypassUnsupported))
    );
}

#[test]
#[ignore = "needs routing test plugin"]
fn seven_one_f64_render_preserves_every_native_speaker_position() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-routing");
    let snapshot = plugin
        .prepare_audio(&buses(SampleFormat::F64, Layout::Surround71))
        .unwrap();
    assert!(
        snapshot
            .iter()
            .filter(|bus| bus.role == AudioBusRole::Main)
            .all(|bus| bus.channels == 8 && bus.layout == Some(Layout::Surround71))
    );
    plugin.set_parameter(0, 0.5).unwrap();
    let mut input = vec![vec![0.25f64; 4]];
    for index in 0..Layout::Surround71.channels() {
        input.push(vec![(index + 1) as f64 + 1.0e-10; 4]);
    }
    let slices: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let rendered = render(
        &mut plugin,
        &slices,
        4,
        &[],
        &RenderOptions {
            tail: TailPolicy::Reported,
            max_tail_seconds: 0.0,
        },
    )
    .unwrap();
    assert_eq!(rendered.channels.len(), 10);
    assert_eq!(rendered.channels[0], input[1]);
    assert_eq!(rendered.channels[1], input[0]);
    for index in 0..Layout::Surround71.channels() {
        assert_eq!(
            rendered.channels[index + 2],
            vec![input[index + 1][0] * 0.375; 4],
            "channel {index}"
        );
    }
}

#[test]
#[ignore = "needs native delay fixture"]
fn custom_sample_implementations_are_rejected_before_native_pointer_casts() {
    #[derive(Clone, Copy, Default, Debug, PartialEq)]
    #[repr(transparent)]
    struct ClaimedF32(u32);
    impl Sample for ClaimedF32 {
        const FORMAT: SampleFormat = SampleFormat::F32;
        fn to_f64(self) -> f64 {
            f64::from(self.0)
        }
    }
    #[derive(Clone, Copy, Default, Debug, PartialEq)]
    #[repr(transparent)]
    struct ClaimedF64(u64);
    impl Sample for ClaimedF64 {
        const FORMAT: SampleFormat = SampleFormat::F64;
        fn to_f64(self) -> f64 {
            self.0 as f64
        }
    }
    fn rejected<S: Sample + PartialEq + std::fmt::Debug>(marker: S) {
        let mut plugin = plugin("plughost-test-delay");
        plugin.prepare(&config(48_000.0, S::FORMAT)).unwrap();
        plugin.set_parameter(0, 0.5).unwrap();
        let input = [marker; 4];
        let (mut left, mut right) = ([marker; 4], [marker; 4]);
        assert_eq!(
            plugin.process(
                &plughost_core::BlockContext::new(4),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            ),
            Err(Vst3Error::SampleFormatMismatch)
        );
        assert_eq!(left, [marker; 4]);
        assert_eq!(right, [marker; 4]);
        assert_eq!(plugin.parameter_value(5), 0.0);
    }
    let _serial = serial();
    rejected(ClaimedF32(3.0f32.to_bits()));
    rejected(ClaimedF64(3.0f64.to_bits()));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn arrangements_the_plugin_took_are_used_whatever_it_answered() {
    let _serial = serial();
    // This fixture answers kResultFalse after taking the requested stereo arrangement.
    let mut plugin = plugin("plughost-test-arrangement-false");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let input = vec![1.0f32; 512];
    let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    plugin
        .process(
            &plughost_core::BlockContext::new(512),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(plugin.config().unwrap().output, Layout::Stereo);
}

/// The points the routing fixture's last block brought for its gain.
fn gain_points(plugin: &mut Plugin) -> f64 {
    let value = plugin
        .parameters()
        .into_iter()
        .find(|(info, _)| info.id == 2)
        .unwrap()
        .1;
    (value * 100.0).round()
}

fn automate_gain(plugin: &mut Plugin, offset: usize) {
    let input = [0.5f32; 4];
    let mut outputs = [[0.0f32; 4]; 4];
    let mut slices: Vec<_> = outputs.iter_mut().map(|c| c.as_mut_slice()).collect();
    plugin
        .process(
            &plughost_core::BlockContext::new(4),
            &[&input, &input, &input],
            &mut slices,
            &[plughost_core::ParameterChange {
                id: 0,
                offset,
                value: 0.25,
            }],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
}

#[test]
#[ignore = "needs routing test plugin"]
fn a_first_point_after_preparation_or_restore_is_a_step_not_a_ramp() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-routing");
    let state = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    plugin
        .prepare_audio(&buses(SampleFormat::F32, Layout::Stereo))
        .unwrap();
    // A point after the block's start comes with one that holds the value until it, even
    // before the host sent the plugin any value.
    automate_gain(&mut plugin, 2);
    assert_eq!(gain_points(&mut plugin), 2.0);
    plugin
        .restore_state(&state, plughost_core::StatePurpose::Project)
        .unwrap();
    automate_gain(&mut plugin, 2);
    assert_eq!(gain_points(&mut plugin), 2.0);
}
