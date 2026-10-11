//! Native CLAP lifecycle regressions. Build the delay fixture variants first.

use std::path::Path;

use plughost_core::{
    BlockContext, HostIdentity, Layout, ProcessConfig, ProcessMode, SampleFormat, Support,
};
use plughost_formats::HostedPlugin;
use plughost_formats::clap::{ClapError, Plugin};

fn plugin(variant: &str, suffix: u32) -> Plugin {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins")
        .join(format!("plughost-test-{variant}.clap"));
    Plugin::new(
        &bundle,
        &format!("com.studio.plughost.test-delay.{:x}", 0x79000000 + suffix),
        &HostIdentity::default(),
    )
    .unwrap()
}

fn prepare(plugin: &mut Plugin) {
    plugin
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            input: Layout::Stereo,
            output: Layout::Stereo,
        })
        .unwrap();
}

fn process(plugin: &mut Plugin) -> Result<[Vec<f32>; 2], ClapError> {
    let input = [1.0f32; 512];
    let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
    plugin.process(
        &BlockContext::new(512),
        &[&input, &input],
        &mut [&mut left, &mut right],
        &[],
        &[],
        &mut Vec::new(),
    )?;
    Ok([left, right])
}

fn value(plugin: &mut Plugin, id: u64) -> f64 {
    plugin
        .parameters()
        .into_iter()
        .find(|(info, _)| info.id == id)
        .unwrap()
        .1
}

#[test]
#[ignore = "needs start-failure CLAP fixture"]
fn failed_start_keeps_host_edits_for_the_next_block() {
    let mut plugin = plugin("start-failure", 0x10);
    prepare(&mut plugin);
    plugin.set_parameter(0, 0.25).unwrap();
    assert!(matches!(process(&mut plugin), Err(ClapError::Process(_))));
    let output = process(&mut plugin).unwrap();
    for channel in output {
        assert_eq!(&channel[..480], &[0.0; 480]);
        assert_eq!(&channel[480..], &[0.25; 32]);
    }
    assert_eq!(value(&mut plugin, 8), 512.0 / 1_000_000.0);
}

#[test]
#[ignore = "needs flush-request CLAP fixture"]
fn processing_consumes_a_flush_request_without_an_extra_idle_flush() {
    let mut unprocessed = plugin("flush-request", 0x11);
    prepare(&mut unprocessed);
    assert_eq!(value(&mut unprocessed, 15), 0.0);
    unprocessed.idle();
    assert_eq!(value(&mut unprocessed, 15), 0.001);

    let mut processed = plugin("flush-request", 0x11);
    prepare(&mut processed);
    process(&mut processed).unwrap();
    processed.idle();
    assert_eq!(value(&mut processed, 15), 0.0);
}

#[test]
#[ignore = "needs mpe-ports CLAP fixture"]
fn mpe_only_ports_advertise_note_input_and_output() {
    let mut plugin = plugin("mpe-ports", 0x12);
    let ports = plugin.event_ports().unwrap();
    assert_eq!(ports.len(), 2);
    for port in ports {
        assert_eq!(port.midi, Support::Supported);
        assert_eq!(port.mpe, Support::Supported);
    }
    let caps = plugin.capabilities().unwrap();
    assert_eq!(caps.note_input, Support::Supported);
    assert_eq!(caps.note_output, Support::Supported);
}
