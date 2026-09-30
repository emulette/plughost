use super::*;
use plughost_core::FactoryPresetId;

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn factory_programs_reach_the_processor_and_survive_immediate_state_save() {
    let _serial = serial();
    let mut original = plugin("plughost-test-delay");
    let presets = original.factory_presets().unwrap();
    assert_eq!(
        presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Unity", "Quarter", "Half"]
    );
    assert_eq!(presets[1].group.as_deref(), Some("Factory delay"));
    assert_eq!(
        presets[1].id,
        FactoryPresetId::Vst3 {
            unit_id: 7,
            list_id: 100,
            program_index: 1
        }
    );
    assert_eq!(
        original.select_factory_preset(&presets[1].id),
        Err(Error::Vst3(Vst3Error::NotPrepared))
    );
    original
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    original.set_parameter(0, 0.75).unwrap();
    original.select_factory_preset(&presets[1].id).unwrap();
    assert_eq!(original.parameter_value(0), 0.25);
    let saved = original
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    assert_eq!(
        f64::from_le_bytes(saved.component.clone().try_into().unwrap()),
        0.25
    );
    let mut restored = plugin("plughost-test-delay");
    restored
        .restore_state(&saved, plughost_core::StatePurpose::Project)
        .unwrap();
    restored
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    let input = vec![1.0f32; 512];
    let mut output = vec![0.0f32; 512];
    let mut right = output.clone();
    restored
        .process(
            &plughost_core::BlockContext::new(512),
            &[&input, &input],
            &mut [&mut output, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert!(output[..480].iter().all(|&value| value == 0.0));
    assert!(output[480..].iter().all(|&value| value == 0.25));
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn invalid_factory_program_identity_does_not_edit_the_current_state() {
    let _serial = serial();
    let mut plugin = plugin("plughost-test-delay");
    plugin
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    plugin.set_parameter(0, 0.75).unwrap();
    let saved = plugin
        .save_state(plughost_core::StatePurpose::Project)
        .unwrap();
    for id in [
        FactoryPresetId::AudioUnit { number: 1 },
        FactoryPresetId::Vst3 {
            unit_id: 0,
            list_id: 100,
            program_index: 1,
        },
        FactoryPresetId::Vst3 {
            unit_id: 7,
            list_id: -1,
            program_index: 1,
        },
        FactoryPresetId::Vst3 {
            unit_id: 7,
            list_id: 100,
            program_index: -1,
        },
        FactoryPresetId::Vst3 {
            unit_id: 7,
            list_id: 100,
            program_index: 3,
        },
    ] {
        assert_eq!(
            plugin.select_factory_preset(&id),
            Err(Error::Vst3(Vst3Error::InvalidFactoryPreset))
        );
        assert_eq!(
            plugin
                .save_state(plughost_core::StatePurpose::Project)
                .unwrap(),
            saved
        );
    }
}
