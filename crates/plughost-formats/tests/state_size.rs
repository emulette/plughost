use std::path::{Path, PathBuf};

use plughost_core::PluginRef;
use plughost_core::{
    HostIdentity, InputError, Layout, MAX_STATE_BYTES, PluginFormat, ProcessConfig, SampleFormat,
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
        let bundle = target().join(format!("plughost-test-large-state.{extension}"));
        let reference = match format {
            PluginFormat::Vst3 => PluginRef::vst3(bundle, class_id),
            _ => PluginRef::clap(bundle, class_id),
        };
        let mut plugin = plughost_formats::load(&reference, &HostIdentity::default()).unwrap();
        plugin
            .prepare(&ProcessConfig::new(
                48_000.0,
                512,
                SampleFormat::F32,
                Layout::Stereo,
                Layout::Stereo,
            ))
            .unwrap();
        let saved = plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap();
        let state = plughost_core::PluginState::new(
            plugin.info(),
            saved.component().to_vec(),
            Some(vec![0; MAX_STATE_BYTES - saved.component().len() + 1]),
        );
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
        assert_eq!(&saved.component()[..8], &0.5f64.to_le_bytes());
    }
}
