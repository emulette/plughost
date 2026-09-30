use plughost::*;

mod support;

use support::{block, delay as reference, prepared_delay as chain};

fn value(parameters: &[(ParameterInfo, f64)], id: u64) -> f64 {
    parameters.iter().find(|(info, _)| info.id == id).unwrap().1
}

#[test]
#[ignore = "needs helper and test plugins"]
fn native_and_helper_clap_state_contexts_reach_the_plugin() {
    let mut native =
        plughost_formats::load(&reference(PluginFormat::Clap), &HostIdentity::default()).unwrap();
    let mut chain = chain(PluginFormat::Clap);
    for (purpose, code) in [
        (StatePurpose::Project, 3.0),
        (StatePurpose::Preset, 1.0),
        (StatePurpose::Duplicate, 2.0),
    ] {
        native.set_parameter(0, 0.25).unwrap();
        chain.set_parameter(0, 0, 0.25).unwrap();
        let direct = native.save_state(purpose).unwrap();
        let isolated = chain.save_state(0, purpose).unwrap();
        assert_eq!(direct, isolated);
        assert_eq!(value(&native.parameters(), 11), code / 3.0);
        assert_eq!(value(&chain.parameters(0).unwrap(), 11), code / 3.0);
        native.restore_state(&direct, purpose).unwrap();
        chain.restore_state(0, &isolated, purpose).unwrap();
        assert_eq!(value(&native.parameters(), 12), code / 3.0);
        assert_eq!(value(&chain.parameters(0).unwrap(), 12), code / 3.0);
        assert_eq!(&block(&mut chain, 512, 1.0)[480..], &[0.25; 32]);
    }
    assert_eq!(
        chain.export_preset(0).unwrap_err().kind(),
        FailureKind::Unsupported
    );
    assert_eq!(
        chain
            .import_preset(0, b"no common CLAP file")
            .unwrap_err()
            .kind(),
        FailureKind::Unsupported
    );
    assert_eq!(
        chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap()
            .component,
        0.25f64.to_le_bytes()
    );
}

#[test]
#[ignore = "needs helper and VST3 test plugin"]
fn vst3_preset_file_roundtrip_validates_class_and_preserves_output_on_failure() {
    use plughost_formats::vst3::Preset;
    let mut chain = chain(PluginFormat::Vst3);
    chain.set_parameter(0, 0, 0.25).unwrap();
    let mut preset = Preset::from_bytes(&chain.export_preset(0).unwrap()).unwrap();
    preset.info = "<MetaInfo><Attribute id=\"Name\" value=\"테스트\"/></MetaInfo>"
        .as_bytes()
        .to_vec();
    let bytes = preset.to_bytes().unwrap();
    let info = chain.inspect_preset(0, &bytes).unwrap();
    assert_eq!(info.class_id, preset.class_id);
    assert_eq!(
        info.metadata,
        PresetMetadata::Vst3 {
            info: preset.info.clone()
        }
    );
    chain = chain.recover(&[]).unwrap();
    chain.import_preset(0, &bytes).unwrap();
    assert_eq!(value(&chain.parameters(0).unwrap(), 1), 0.0);
    assert_eq!(&block(&mut chain, 512, 1.0)[480..], &[0.25; 32]);
    let original = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    chain
        .restore_state(0, &original, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(value(&chain.parameters(0).unwrap(), 1), 1.0);
    let mut wrong = preset.clone();
    wrong.class_id = "00000000000000000000000000000000".into();
    assert_eq!(
        chain
            .import_preset(0, &wrong.to_bytes().unwrap())
            .unwrap_err()
            .kind(),
        FailureKind::StateMismatch
    );
    let mut corrupt = bytes.clone();
    corrupt[4] = 2;
    assert!(chain.import_preset(0, &corrupt).is_err());
    for controller in [false, true] {
        let mut rejected = preset.clone();
        if controller {
            rejected.controller = (-1.0f64).to_le_bytes().to_vec();
        } else {
            rejected.component = (-1.0f64).to_le_bytes().to_vec();
        }
        assert_eq!(
            chain
                .import_preset(0, &rejected.to_bytes().unwrap())
                .unwrap_err()
                .kind(),
            FailureKind::State
        );
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            original
        );
    }
    {
        let purpose = StatePurpose::Duplicate;
        assert_eq!(
            chain.save_state(0, purpose).unwrap_err().kind(),
            FailureKind::Unsupported
        );
        assert_eq!(
            chain
                .restore_state(0, &original, purpose)
                .unwrap_err()
                .kind(),
            FailureKind::Unsupported
        );
    }
    assert_eq!(value(&chain.parameters(0).unwrap(), 1), 1.0);
    assert_eq!(&block(&mut chain, 512, 1.0)[480..], &[0.25; 32]);
    let oversized = vec![0; plughost_core::MAX_PRESET_BYTES + 1];
    assert_eq!(
        chain.import_preset(0, &oversized).unwrap_err().kind(),
        FailureKind::InvalidInput
    );
    assert_eq!(
        chain.inspect_preset(0, &oversized).unwrap_err().kind(),
        FailureKind::InvalidInput
    );
    assert_eq!(
        chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap(),
        original
    );
    assert_eq!(
        chain.import_preset(3, &bytes).unwrap_err().kind(),
        FailureKind::InvalidInput
    );
}
