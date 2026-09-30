use std::path::{Path, PathBuf};

use plughost_core::PluginRef;
use plughost_core::{
    HostIdentity, InputError, Layout, MAX_STATE_BYTES, PluginFormat, ProcessConfig, ProcessMode,
    SampleFormat,
};

fn target() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugins")
}

#[test]
#[ignore = "needs scripts/build-test-plugins (.ps1 or .sh)"]
fn oversized_direct_restore_is_rejected_before_discarding_pending_edits() {
    for (format, extension, class_id) in [
        (
            PluginFormat::Vst3,
            "vst3",
            "706C7567686F737444656C6179000008",
        ),
        (
            PluginFormat::Clap,
            "clap",
            "com.studio.plughost.test-delay.79000008",
        ),
    ] {
        let reference = PluginRef {
            format,
            bundle: Some(target().join(format!("plughost-test-large-state.{extension}"))),
            class_id: class_id.to_owned(),
        };
        let mut plugin = plughost_formats::load(&reference, &HostIdentity::default()).unwrap();
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
        let mut state = plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap();
        state
            .controller
            .resize(MAX_STATE_BYTES - state.component.len() + 1, 0);
        plugin.set_parameter(0, 0.5).unwrap();
        assert_eq!(
            plugin
                .restore_state(&state, plughost_core::StatePurpose::Project)
                .unwrap_err()
                .input_error(),
            Some(InputError::StateSize)
        );
        drop(state);
        let saved = plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap();
        assert_eq!(&saved.component[..8], &0.5f64.to_le_bytes());
    }
}
