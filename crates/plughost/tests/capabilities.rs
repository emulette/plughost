//! Capabilities are observations from real loaded plugins, transported through the helper.
use plughost::{Error, InputError, Layout, PluginFormat, Support};

mod support;

use support::{delay, fixture, spawn};

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn native_support_is_distinct_from_chain_support_and_unknown() {
    let mut chain = spawn(&[delay(PluginFormat::Vst3), delay(PluginFormat::Clap)]);
    let vst3 = chain.capabilities(0).unwrap();
    assert_eq!(vst3.plugin.f64, Support::Supported);
    assert_eq!(vst3.host.f64, Support::Supported);
    assert_eq!(vst3.plugin.embedded_editor, Support::Unknown);
    assert_eq!(vst3.plugin.state, Support::Unknown);
    assert_eq!(vst3.plugin.factory_presets, Support::Supported);
    assert_eq!(vst3.host.factory_presets, Support::Supported);
    let clap = chain.capabilities(1).unwrap();
    assert_eq!(clap.plugin.embedded_editor, Support::Unsupported);
    assert_eq!(clap.plugin.state, Support::Supported);
    assert_eq!(clap.plugin.f64, Support::Unsupported);
    assert_eq!(clap.plugin.factory_presets, Support::Unknown);
    assert!(matches!(
        chain.capabilities(2),
        Err(Error::Input {
            slot: Some(2),
            error: InputError::Slot
        })
    ));
    assert!(!chain.editor_open(0).unwrap());
    assert!(!chain.editor_open(1).unwrap());
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo; 2])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    chain.set_parameter(1, 0, 0.25).unwrap();
    // A metadata query must not flush, reset, or consume the pending gain edit or stored delay.
    assert_eq!(chain.capabilities(1).unwrap(), clap);
    let state = chain
        .save_state(1, plughost::StatePurpose::Project)
        .unwrap();
    chain.reset().unwrap();
    chain
        .restore_state(1, &state, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(chain.capabilities(0).unwrap(), vst3);
    let input = [1.0; 512];
    let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
    chain
        .process_audio_f32(
            &plughost_core::BlockContext::new(left.len()),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left, [0.0; 512]); // two 480-sample delays
    chain.capabilities(0).unwrap();
    chain.capabilities(1).unwrap();
    chain
        .process_audio_f32(
            &plughost_core::BlockContext::new(left.len()),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left[..448], [0.0; 448]);
    assert_eq!(left[448..], [0.25; 64]);
    assert_eq!(left, right);
    chain = chain.recover(&[]).unwrap();
    assert_eq!(chain.capabilities(0).unwrap(), vst3);
    assert_eq!(chain.capabilities(1).unwrap(), clap);
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn every_slot_takes_note_input_through_event_routes() {
    let synth = fixture(
        PluginFormat::Clap,
        "plughost-test-synth",
        "com.studio.plughost.test-synth",
    );
    let mut chain = spawn(&[synth.clone(), synth]);
    let first = chain.capabilities(0).unwrap();
    let second = chain.capabilities(1).unwrap();
    assert_eq!(first.plugin, second.plugin);
    assert_eq!(first.plugin.note_input, Support::Supported);
    assert_eq!(first.host.note_input, Support::Supported);
    assert_eq!(second.host.note_input, Support::Supported);
    assert_eq!(first.plugin.state, Support::Unsupported);
    // The report lets a caller distinguish absent state support from an unqueried VST3 state API.
    assert_eq!(
        chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap_err()
            .kind(),
        plughost::FailureKind::Unsupported
    );
}
