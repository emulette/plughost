use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use plughost_core::{
    BlockContext, HostIdentity, Layout, ParameterEvent, ProcessConfig, ProcessMode, SampleFormat,
};
use plughost_formats::HostedPlugin;

fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

fn bundle(extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins")
        .join(format!("plughost-test-delay.{extension}"))
}

fn config() -> ProcessConfig {
    ProcessConfig {
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
}

#[cfg(feature = "clap")]
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn clap_flush_reports_native_gestures_dirty_and_parameter_rescans() {
    let _serial = serial();
    let mut plugin = plughost_formats::clap::Plugin::new(
        &bundle("clap"),
        "com.studio.plughost.test-delay.79000001",
        &HostIdentity::default(),
    )
    .unwrap();
    assert!(plugin.take_parameter_events().events.is_empty());
    plugin.set_parameter(0, 0.25).unwrap();
    plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(
        plugin.take_parameter_events().events,
        vec![
            ParameterEvent::BeginEdit { id: 0 },
            ParameterEvent::Value {
                id: 0,
                normalized: 0.25
            },
            ParameterEvent::EndEdit { id: 0 },
            ParameterEvent::Dirty { dirty: true },
        ]
    );
    plugin.set_parameter(1, 1.0).unwrap();
    plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    let batch = plugin.take_parameter_events();
    assert!(batch.events.contains(&ParameterEvent::MetadataChanged));
    assert!(batch.events.contains(&ParameterEvent::ValuesChanged));
    assert_eq!(plugin.parameter_details(0).unwrap().plain_at_one, 2.0);
    plugin.set_parameter(0, 0.375).unwrap();
    plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert!(
        plugin
            .take_parameter_events()
            .events
            .contains(&ParameterEvent::Value {
                id: 0,
                normalized: 0.375
            })
    );
    assert!(plugin.take_parameter_events().events.is_empty());
}

#[cfg(feature = "clap")]
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn clap_process_events_preserve_order_and_bound_overflow() {
    let _serial = serial();
    let mut plugin = plughost_formats::clap::Plugin::new(
        &bundle("clap"),
        "com.studio.plughost.test-delay.79000001",
        &HostIdentity::default(),
    )
    .unwrap();
    plugin.prepare(&config()).unwrap();
    plugin.take_parameter_events();
    let input = [0.0f32; 4];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    for _ in 0..100 {
        plugin.set_parameter(0, 0.5).unwrap();
        plugin
            .process(
                &BlockContext::new(4),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
    }
    let batch = plugin.take_parameter_events();
    assert_eq!(batch.events.len(), plughost_core::PARAMETER_EVENT_CAPACITY);
    assert_eq!(batch.dropped, 44);
    assert!(batch.resync_required);
    assert_eq!(
        &batch.events[batch.events.len() - 3..],
        &[
            ParameterEvent::BeginEdit { id: 0 },
            ParameterEvent::Value {
                id: 0,
                normalized: 0.5
            },
            ParameterEvent::EndEdit { id: 0 },
        ]
    );
    assert!(plugin.take_parameter_events().events.is_empty());
}

#[cfg(feature = "vst3")]
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn vst3_callbacks_and_processor_output_reach_the_consumer() {
    let _serial = serial();
    let module = plughost_formats::vst3::Module::load(&bundle("vst3")).unwrap();
    let mut plugin = plughost_formats::vst3::Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &HostIdentity::default(),
    )
    .unwrap();
    plugin.prepare(&config()).unwrap();
    plugin.take_parameter_events();
    plugin.set_parameter(7, 1.0).unwrap();
    let batch = plugin.take_parameter_events();
    assert_eq!(
        &batch.events[..6],
        &[
            ParameterEvent::BeginEdit { id: 0 },
            ParameterEvent::Value {
                id: 0,
                normalized: 0.25
            },
            ParameterEvent::EndEdit { id: 0 },
            ParameterEvent::MetadataChanged,
            ParameterEvent::ValuesChanged,
            ParameterEvent::Dirty { dirty: true },
        ]
    );
    let input = [0.0f32; 4];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    plugin
        .process(
            &BlockContext::new(4),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(
        plugin
            .take_parameter_events()
            .events
            .contains(&ParameterEvent::Value {
                id: 0,
                normalized: 0.375
            })
    );
    assert_eq!(plugin.parameter_value(0), 0.375);
}
