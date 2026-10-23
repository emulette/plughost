#![cfg(feature = "clap")]
use plughost_core::*;
use plughost_formats::HostedPlugin;
use plughost_formats::clap::Plugin;
use std::path::Path;
fn plugin() -> Plugin {
    bundle("plughost-test-routing")
}
fn bundle(name: &str) -> Plugin {
    Plugin::new(
        // SAFETY: the tests load only the repository's fixture and SDK sample plugins.
        &unsafe {
            plughost_formats::clap::Entry::load(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../target/test-plugins/{name}.clap")),
            )
        }
        .unwrap(),
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap()
}
fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig::new(id, layout, true)
}
fn config(format: SampleFormat, layout: Layout, id: u64) -> AudioConfig {
    AudioConfig::new(
        48000.0,
        32,
        format,
        vec![bus(20, Layout::Mono), bus(21, layout)],
        vec![bus(30, Layout::Stereo), bus(31, layout)],
    )
    .with_configuration(id)
}
#[test]
#[ignore = "needs native routing fixture"]
fn native_port_ids_roles_and_configurations_are_preserved() {
    let mut plugin = plugin();
    let buses = plugin.audio_buses().unwrap();
    assert_eq!(
        buses.iter().map(|b| b.id).collect::<Vec<_>>(),
        [21, 20, 31, 30]
    );
    assert_eq!(buses[1].role, AudioBusRole::Auxiliary);
    assert_eq!(buses[0].role, AudioBusRole::Main);
    assert_eq!(buses[0].index, 0);
    assert_eq!(buses[1].layout, Some(Layout::Mono));
    assert_eq!(buses[0].layout, Some(Layout::Stereo));
    let configs = plugin.audio_configurations().unwrap();
    assert_eq!(
        configs.iter().map(|c| c.id).collect::<Vec<_>>(),
        [100, 101, 102, 103]
    );
    assert_eq!(configs[0].inputs[0].layout, Some(Layout::Mono));
    assert_eq!(configs[2].inputs[0].channels, 6);
    assert_eq!(configs[2].inputs[0].layout, None); // Config-info does not expose surround speaker maps.
    let buses = plugin
        .prepare_audio(&config(SampleFormat::F32, Layout::Surround51, 102))
        .unwrap();
    assert_eq!(buses[0].layout, Some(Layout::Surround51));
    assert_eq!(buses[2].layout, Some(Layout::Surround51));
    let mut input: Vec<Vec<f32>> = (1..=6).map(|i| vec![i as f32 / 10.0; 32]).collect();
    input.push(vec![0.0; 32]);
    let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0f32; 32]; 8];
    let mut outputs: Vec<_> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process(
            &BlockContext::new(32),
            &refs,
            &mut outputs,
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    for channel in 0..6 {
        assert_eq!(output[channel], input[channel]);
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
            &[&left, &right, &key],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(output[2], left);
    assert_eq!(output[3], key);
    assert_eq!(output[0], vec![exact * 0.75; 32]);
    assert_eq!(output[1], vec![0.875 * 0.75; 32]);
    assert_ne!(output[2][0], f64::from(exact as f32));
    assert_eq!(
        plugin.bypass().unwrap(),
        BypassState::new(Support::Supported, Some(false))
    );
    plugin.set_bypass(true).unwrap();
    assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
    plugin
        .process(
            &BlockContext::new(32),
            &[&left, &right, &key],
            &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(output[0], left);
    assert_eq!(output[1], right);
}
#[test]
#[ignore = "needs native routing fixture"]
fn inactive_ports_keep_native_indices_and_reset_retains_routing() {
    let mut plugin = plugin();
    let mut request = config(SampleFormat::F64, Layout::Stereo, 101);
    request.inputs.remove(0);
    request.outputs.remove(0);
    let buses = plugin.prepare_audio(&request).unwrap();
    assert_eq!(buses[1].active, Some(false));
    assert_eq!(buses[3].active, Some(false));
    assert_eq!(buses[0].index, 0);
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
                &Events::new(),
                &mut Events::for_block(),
            )
            .unwrap();
        assert_eq!(output, [left.clone(), right.clone()]);
    }
    plugin
        .prepare(&ProcessConfig::new(
            48000.0,
            32,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
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
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(left_out, vec![0.5; 32]);
    assert_eq!(right_out, vec![0.75; 32]);
}
#[test]
#[ignore = "needs native routing fixture"]
fn main_bus_preparation_follows_ports_left_out_without_port_activation() {
    // Without port activation every port stays natively active, so main-bus preparation has
    // nothing to activate again.
    let mut plugin = bundle("plughost-test-routing-fixed-ports");
    let mut request = config(SampleFormat::F32, Layout::Stereo, 101);
    request.inputs.remove(0);
    request.outputs.remove(0);
    plugin.prepare_audio(&request).unwrap();
    plugin
        .prepare(&ProcessConfig::new(
            48000.0,
            32,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
        .unwrap();
    let (mut left, mut right) = (vec![0.0f32; 32], vec![0.0f32; 32]);
    plugin
        .process(
            &BlockContext::new(32),
            &[&[0.5; 32], &[0.75; 32]],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!((left, right), (vec![0.5; 32], vec![0.75; 32]));
}
#[test]
#[ignore = "needs native routing fixture"]
fn invalid_requested_layout_and_precision_buffers_are_rejected() {
    let mut plugin = plugin();
    // The selected configuration defines the ports; its layout is not requested away.
    let mut wrong = config(SampleFormat::F64, Layout::Mono, 101);
    assert_eq!(
        plugin.prepare_audio(&wrong).unwrap_err().kind(),
        FailureKind::Configuration
    );
    wrong.inputs[1].layout = Layout::Stereo;
    wrong.outputs[1].layout = Layout::Stereo;
    plugin.prepare_audio(&wrong).unwrap();
    let mut output = vec![vec![0.0f32; 32]; 4];
    assert_eq!(
        plugin
            .process(
                &BlockContext::new(32),
                &[&[0.0f32; 32][..]; 3],
                &mut output.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[],
                &Events::new(),
                &mut Events::for_block(),
            )
            .unwrap_err()
            .input_error(),
        Some(InputError::AudioSampleFormat)
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
    for (i, channel) in input.iter_mut().enumerate().take(8) {
        channel.fill((i + 1) as f64 + 2.0f64.powi(-40));
    }
    let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let rendered = render(
        &mut plugin,
        &refs,
        32,
        &Events::new(),
        &RenderOptions::new(TailPolicy::Reported, 0.0),
    )
    .unwrap();
    assert_eq!(rendered.channels.len(), 10);
    assert_eq!(&rendered.channels[..8], &input[..8]);
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
        let request = {
            let mut value = config(SampleFormat::F64, layout, 0);
            value.configuration = None;
            value
        };
        let buses = plugin
            .prepare_audio(&request)
            .unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        assert_eq!(buses[0].layout, Some(layout));
        assert_eq!(buses[2].layout, Some(layout));
        assert_eq!(plugin.audio_buses().unwrap(), buses);
        let mut input = vec![vec![0.0f64; 32]; layout.channels() + 1];
        for (i, channel) in input.iter_mut().enumerate().take(layout.channels()) {
            channel.fill((i + 1) as f64 + 2.0f64.powi(-40));
        }
        let refs: Vec<_> = input.iter().map(Vec::as_slice).collect();
        let rendered = render(
            &mut plugin,
            &refs,
            32,
            &Events::new(),
            &RenderOptions::new(TailPolicy::Reported, 0.0),
        )
        .unwrap();
        assert_eq!(rendered.channels.len(), layout.channels() + 2);
        assert_eq!(
            &rendered.channels[..layout.channels()],
            &input[..layout.channels()],
            "{layout:?}"
        );
    }
    assert_eq!(
        plugin
            .prepare_audio(&{
                let mut value = config(SampleFormat::F64, Layout::Surround916, 0);
                value.configuration = None;
                value
            })
            .unwrap_err()
            .kind(),
        FailureKind::Configuration
    );
}

#[test]
#[ignore = "needs native routing fixture"]
fn a_preparation_that_must_fail_leaves_the_ports_unconfigured() {
    let mut plugin = plugin();
    let stereo = {
        let mut value = config(SampleFormat::F64, Layout::Stereo, 0);
        value.configuration = None;
        value
    };
    plugin.prepare_audio(&stereo).unwrap();
    // The main output asks for 9.1.6, which CLAP cannot express, so 7.1.4 is not asked either.
    let mut mixed = stereo.clone();
    mixed.inputs[1].layout = Layout::Surround714;
    mixed.outputs[1].layout = Layout::Surround916;
    assert_eq!(
        plugin.prepare_audio(&mixed).unwrap_err().kind(),
        FailureKind::Configuration
    );
    let buses = plugin.audio_buses().unwrap();
    assert_eq!(buses[0].layout, Some(Layout::Stereo));
    assert_eq!(buses[2].layout, Some(Layout::Stereo));
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
                &[&left[..frames], &right[..frames], &key[..frames]],
                &mut outputs,
                &[],
                &Events::new(),
                &mut Events::for_block(),
            )
            .unwrap();
        assert_eq!(&output[2][..frames], &left[..frames]);
        assert_eq!(&output[3][..frames], &key[..frames]);
        assert!(output[0][..frames].iter().all(|sample| *sample == 1.5));
        assert!(output[1][..frames].iter().all(|sample| *sample == 3.0));
    }
}

#[test]
#[ignore = "needs native routing fixture"]
fn counts_past_the_hosts_bounds_are_neither_followed_nor_stored() {
    // The bundle reports u32::MAX parameters, note outputs that take no dialect and port
    // configurations, and describes every one.
    let mut plugin = bundle("plughost-test-routing-counts");
    // At most 65,536 parameters are read.
    assert_eq!(plugin.parameters().len(), 1 << 16);
    let diagnostics = plugin.take_diagnostics().records;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].severity,
        DiagnosticSeverity::PluginMisbehaving
    );
    // A note output taking a dialect may be past the ones looked at.
    assert_eq!(plugin.capabilities().unwrap().note_output, Support::Unknown);
    assert_eq!(
        plugin.audio_configurations().unwrap_err().kind(),
        FailureKind::Host
    );
}
