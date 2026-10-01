#![cfg(feature = "clap")]
use plughost_core::*;
use plughost_formats::clap::{ClapError, Plugin};
use plughost_formats::{Error, HostedPlugin};
use std::path::Path;
fn plugin() -> Plugin {
    Plugin::new(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-plugins/plughost-test-routing.clap"),
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap()
}
fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}
fn config(format: SampleFormat, layout: Layout, id: u64) -> AudioConfig {
    AudioConfig {
        events: Default::default(),
        sample_rate: 48000.0,
        max_block_size: 32,
        sample_format: format,
        mode: ProcessMode::Offline,
        configuration: Some(id),
        inputs: vec![bus(20, Layout::Mono), bus(21, layout)],
        outputs: vec![bus(30, Layout::Stereo), bus(31, layout)],
    }
}
#[test]
#[ignore = "needs native routing fixture"]
fn native_port_ids_roles_and_configurations_are_preserved() {
    let mut plugin = plugin();
    let buses = plugin.audio_buses().unwrap();
    assert_eq!(
        buses.iter().map(|b| b.id).collect::<Vec<_>>(),
        [20, 21, 30, 31]
    );
    assert_eq!(buses[0].role, AudioBusRole::Auxiliary);
    assert_eq!(buses[1].role, AudioBusRole::Main);
    assert_eq!(buses[1].index, 1);
    assert_eq!(buses[0].layout, Some(Layout::Mono));
    assert_eq!(buses[1].layout, Some(Layout::Stereo));
    let configs = plugin.audio_configurations().unwrap();
    assert_eq!(
        configs.iter().map(|c| c.id).collect::<Vec<_>>(),
        [100, 101, 102, 103]
    );
    assert_eq!(configs[0].inputs[1].layout, Some(Layout::Mono));
    assert_eq!(configs[2].inputs[1].channels, 6);
    assert_eq!(configs[2].inputs[1].layout, None); // Config-info does not expose surround speaker maps.
    let buses = plugin
        .prepare_audio(&config(SampleFormat::F32, Layout::Surround51, 102))
        .unwrap();
    assert_eq!(buses[1].layout, Some(Layout::Surround51));
    assert_eq!(buses[3].layout, Some(Layout::Surround51));
    let input: Vec<Vec<f32>> = (0..7).map(|i| vec![i as f32 / 10.0; 32]).collect();
    let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; 32]; 8];
    let mut outputs: Vec<_> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process(
            &BlockContext::new(32),
            &refs,
            &mut outputs,
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    for channel in 0..6 {
        assert_eq!(output[channel + 2], input[channel + 1]);
    }
}
#[test]
#[ignore = "needs native routing fixture"]
fn multibus_f64_preserves_precision_sidechain_auxiliary_and_native_bypass() {
    let mut plugin = plugin();
    plugin
        .prepare_audio(&config(SampleFormat::F64, Layout::Stereo, 101))
        .unwrap();
    let exact = 1.0 + 2.0f64.powi(-40);
    let key = vec![0.25f64; 32];
    let left = vec![exact; 32];
    let right = vec![0.875f64; 32];
    let mut output = vec![vec![0.0f64; 32]; 4];
    plugin
        .process(
            &BlockContext::new(32),
            &[&key, &left, &right],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(output[0], left);
    assert_eq!(output[1], key);
    assert_eq!(output[2], vec![exact * 0.75; 32]);
    assert_eq!(output[3], vec![0.875 * 0.75; 32]);
    assert_ne!(output[0][0], f64::from(exact as f32));
    assert_eq!(
        plugin.bypass().unwrap(),
        BypassState {
            support: Support::Supported,
            enabled: Some(false)
        }
    );
    plugin.set_bypass(true).unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
    plugin
        .process(
            &BlockContext::new(32),
            &[&key, &left, &right],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(output[2], left);
    assert_eq!(output[3], right);
}
#[test]
#[ignore = "needs native routing fixture"]
fn inactive_ports_keep_native_indices_and_reset_retains_routing() {
    let mut plugin = plugin();
    let mut request = config(SampleFormat::F64, Layout::Stereo, 101);
    request.inputs.remove(0);
    request.outputs.remove(0);
    let buses = plugin.prepare_audio(&request).unwrap();
    assert_eq!(buses[0].active, Some(false));
    assert_eq!(buses[2].active, Some(false));
    assert_eq!(buses[1].index, 1);
    let left = vec![0.375f64; 32];
    let right = vec![0.625f64; 32];
    let mut output = vec![vec![0.0f64; 32]; 2];
    for reset in [false, true] {
        if reset {
            plugin.reset().unwrap();
        }
        plugin
            .process(
                &BlockContext::new(32),
                &[&left, &right],
                &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(output, [left.clone(), right.clone()]);
    }
    plugin
        .prepare(&ProcessConfig {
            sample_rate: 48000.0,
            max_block_size: 32,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    assert!(
        plugin
            .audio_buses()
            .unwrap()
            .iter()
            .all(|b| b.active == Some(true))
    );
    let mut left_out = vec![0.0f32; 32];
    let mut right_out = vec![0.0f32; 32];
    plugin
        .process(
            &BlockContext::new(32),
            &[&[0.5; 32], &[0.75; 32]],
            &mut [&mut left_out, &mut right_out],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left_out, vec![0.5; 32]);
    assert_eq!(right_out, vec![0.75; 32]);
}
#[test]
#[ignore = "needs native routing fixture"]
fn invalid_requested_layout_and_precision_buffers_are_rejected() {
    let mut plugin = plugin();
    // The selected configuration defines the ports; its layout is not requested away.
    let mut wrong = config(SampleFormat::F64, Layout::Mono, 101);
    assert_eq!(
        plugin.prepare_audio(&wrong),
        Err(Error::Clap(ClapError::AudioConfiguration))
    );
    wrong.inputs[1].layout = Layout::Stereo;
    wrong.outputs[1].layout = Layout::Stereo;
    plugin.prepare_audio(&wrong).unwrap();
    let mut output = vec![vec![0.0f32; 32]; 4];
    assert_eq!(
        plugin.process(
            &BlockContext::new(32),
            &[&[0.0f32; 32][..]; 3],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &[],
            &mut Vec::new(),
        ),
        Err(ClapError::SampleFormatUnsupported(SampleFormat::F32))
    );
}

#[test]
#[ignore = "needs native routing fixture"]
fn native_render_keeps_all_active_channels_and_f64_surround_order() {
    use plughost_core::render::{Process, RenderOptions, TailPolicy, render};
    let mut plugin = plugin();
    let request = config(SampleFormat::F64, Layout::Surround71, 103);
    plugin.prepare_audio(&request).unwrap();
    assert_eq!(plugin.audio_config(), Some(request));
    assert_eq!(<Plugin as Process<f64>>::input_channels(&plugin), 9);
    assert_eq!(<Plugin as Process<f64>>::output_channels(&plugin), 10);
    let mut input = vec![vec![0.0f64; 32]; 9];
    for (i, channel) in input.iter_mut().enumerate().skip(1) {
        channel.fill(i as f64 + 2.0f64.powi(-40));
    }
    let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let rendered = render(
        &mut plugin,
        &refs,
        32,
        &[],
        &RenderOptions::new(TailPolicy::Reported, 0.0),
    )
    .unwrap();
    assert_eq!(rendered.channels.len(), 10);
    for i in 0..8 {
        assert_eq!(rendered.channels[i + 2], input[i + 1]);
    }
}

#[test]
#[ignore = "needs native routing fixture"]
fn configurable_ports_take_every_layout_clap_expresses() {
    use plughost_core::render::{RenderOptions, TailPolicy, render};
    let mut plugin = plugin();
    for layout in Layout::ALL
        .into_iter()
        .filter(|&layout| layout != Layout::None && layout != Layout::Surround916)
    {
        let request = AudioConfig {
            configuration: None,
            ..config(SampleFormat::F64, layout, 0)
        };
        let buses = plugin
            .prepare_audio(&request)
            .unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        assert_eq!(buses[1].layout, Some(layout));
        assert_eq!(buses[3].layout, Some(layout));
        assert_eq!(plugin.audio_buses().unwrap(), buses);
        let mut input = vec![vec![0.0f64; 32]; layout.channels() + 1];
        for (i, channel) in input.iter_mut().enumerate().skip(1) {
            channel.fill(i as f64 + 2.0f64.powi(-40));
        }
        let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
        let rendered = render(
            &mut plugin,
            &refs,
            32,
            &[],
            &RenderOptions::new(TailPolicy::Reported, 0.0),
        )
        .unwrap();
        assert_eq!(rendered.channels.len(), layout.channels() + 2);
        for i in 0..layout.channels() {
            assert_eq!(
                rendered.channels[i + 2],
                input[i + 1],
                "{layout:?} channel {i}"
            );
        }
    }
    assert_eq!(
        plugin.prepare_audio(&AudioConfig {
            configuration: None,
            ..config(SampleFormat::F64, Layout::Surround916, 0)
        }),
        Err(Error::Clap(ClapError::AudioConfiguration))
    );
}

#[test]
#[ignore = "needs native routing fixture"]
fn a_preparation_that_must_fail_leaves_the_ports_unconfigured() {
    let mut plugin = plugin();
    let stereo = AudioConfig {
        configuration: None,
        ..config(SampleFormat::F64, Layout::Stereo, 0)
    };
    plugin.prepare_audio(&stereo).unwrap();
    // The main output asks for 9.1.6, which CLAP cannot express, so 7.1.4 is not asked either.
    let mut mixed = stereo.clone();
    mixed.inputs[1].layout = Layout::Surround714;
    mixed.outputs[1].layout = Layout::Surround916;
    assert_eq!(
        plugin.prepare_audio(&mixed),
        Err(Error::Clap(ClapError::AudioConfiguration))
    );
    let buses = plugin.audio_buses().unwrap();
    assert_eq!(buses[1].layout, Some(Layout::Stereo));
    assert_eq!(buses[3].layout, Some(Layout::Stereo));
}

#[test]
#[ignore = "needs native routing fixture"]
fn full_bus_calls_of_varying_length_do_not_reuse_old_sidechain_audio() {
    let mut plugin = plugin();
    plugin
        .prepare_audio(&config(SampleFormat::F32, Layout::Stereo, 101))
        .unwrap();
    for frames in [32, 1, 17, 32] {
        let (key, left, right) = ([0.25f32; 32], [2.0; 32], [4.0; 32]);
        let mut output = [[99.0f32; 32]; 4];
        let mut outputs: Vec<_> = output
            .iter_mut()
            .map(|channel| &mut channel[..frames])
            .collect();
        plugin
            .process(
                &BlockContext::new(frames),
                &[&key[..frames], &left[..frames], &right[..frames]],
                &mut outputs,
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(&output[0][..frames], &left[..frames]);
        assert_eq!(&output[1][..frames], &key[..frames]);
        assert!(output[2][..frames].iter().all(|sample| *sample == 1.5));
        assert!(output[3][..frames].iter().all(|sample| *sample == 3.0));
    }
}
