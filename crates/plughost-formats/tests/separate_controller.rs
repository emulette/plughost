//! State and messages must cross two distinct native VST3 objects.
#![cfg(feature = "vst3")]

use plughost_core::{
    BlockContext, HostIdentity, Layout, ParameterChange, PluginFormat, PluginRef, ProcessConfig,
    ProcessMode, SampleFormat, StatePurpose,
};
use plughost_formats::HostedPlugin;
use std::path::Path;

fn value(plugin: &mut dyn HostedPlugin, id: u64) -> f64 {
    plugin
        .parameters()
        .into_iter()
        .find(|(p, _)| p.id == id)
        .unwrap()
        .1
}

#[test]
#[ignore = "needs routing VST3 fixture"]
fn component_state_and_connection_messages_reach_a_separate_controller() {
    let reference = PluginRef {
        format: PluginFormat::Vst3,
        bundle: Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-plugins/plughost-test-routing.vst3"),
        ),
        class_id: "706C7567686F7374526F7574696E6701".into(),
    };
    let mut plugin = plughost_formats::load(&reference, &HostIdentity::default()).unwrap();
    // The controller starts with opposite defaults until setComponentState copies the DSP state.
    assert_eq!(value(&mut *plugin, 0), 1.0);
    assert_eq!(value(&mut *plugin, 1), 0.0);
    plugin
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    let processor = plugin.processor();
    let input = [1.0f32; 8];
    let (mut left, mut right) = ([0.0f32; 8], [0.0f32; 8]);
    let mut block = |changes: &[ParameterChange]| {
        processor
            .process_audio_f32(
                &BlockContext::new(8),
                &[&input, &input],
                &mut [&mut left, &mut right],
                changes,
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(right, left);
        left
    };
    plugin.set_parameter(0, 0.25).unwrap();
    assert_eq!(
        block(&[ParameterChange {
            id: 0,
            offset: 3,
            value: 0.75
        }]),
        [0.25, 0.25, 0.25, 0.75, 0.75, 0.75, 0.75, 0.75]
    );
    // The block contains the initial anchor, pending edit, and explicit point.
    // The controller reads this count by sending an IMessage through IConnectionPoint.
    assert_eq!(value(&mut *plugin, 2), 0.03);
    assert_eq!(value(&mut *plugin, 0), 0.75);
    let state = plugin.save_state(StatePurpose::Project).unwrap();
    assert_eq!(&state.component[..8], &0.75f64.to_le_bytes());
    assert_eq!(&state.controller[..8], &0.75f64.to_le_bytes());
    plugin.set_parameter(0, 0.5).unwrap();
    assert_eq!(block(&[]), [0.5; 8]);
    plugin.restore_state(&state, StatePurpose::Project).unwrap();
    assert_eq!(value(&mut *plugin, 0), 0.75);
    assert_eq!(block(&[]), [0.75; 8]);
}
