use plughost::*;
mod support;

use support::{block, delay, prepare, spawn};

fn spawn_delay() -> Chain {
    spawn(&[delay(PluginFormat::Vst3)])
}

#[test]
#[ignore = "needs helper and test plugins"]
fn factory_program_selection_is_prepared_and_saved_before_any_audio_block() {
    let mut chain = spawn_delay();
    let presets = chain.factory_presets(0).unwrap();
    assert_eq!(
        presets
            .iter()
            .map(|preset| preset.name.as_str())
            .collect::<Vec<_>>(),
        ["Unity", "Quarter", "Half"]
    );
    assert_eq!(
        chain
            .select_factory_preset(0, &presets[1].id)
            .unwrap_err()
            .kind(),
        FailureKind::NotPrepared
    );
    prepare(&mut chain);
    chain.set_parameter(0, 0, 0.75).unwrap();
    chain.select_factory_preset(0, &presets[1].id).unwrap();
    let events = chain.take_parameter_events(0).unwrap();
    assert!(events.resync_required);
    assert!(events.events.contains(&ParameterEvent::MetadataChanged));
    assert!(events.events.contains(&ParameterEvent::ValuesChanged));
    assert!(!events.events.iter().any(|event| matches!(event,
        ParameterEvent::Value { id: 0, normalized } if *normalized == 0.75)));
    let saved = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(saved.component, 0.25f64.to_le_bytes());
    let parameters = chain.parameters(0).unwrap();
    assert_eq!(
        parameters.iter().find(|(info, _)| info.id == 0).unwrap().1,
        0.25
    );
    let mut restored = spawn_delay();
    restored
        .restore_state(0, &saved, plughost::StatePurpose::Project)
        .unwrap();
    prepare(&mut restored);
    let output = block(&mut restored, 512, 1.0);
    assert_eq!(&output[..480], &[0.0; 480]);
    assert_eq!(&output[480..], &[0.25; 32]);
    restored.reset().unwrap();
    assert!(restored.take_parameter_events(0).unwrap().resync_required);
}

#[test]
#[ignore = "needs helper and test plugins"]
fn failed_factory_program_selection_preserves_live_delay_history() {
    let mut chain = spawn_delay();
    prepare(&mut chain);
    chain.set_parameter(0, 0, 0.75).unwrap();
    let saved = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(&block(&mut chain, 512, 1.0)[480..], &[0.75; 32]);
    let invalid = FactoryPresetId::Vst3 {
        unit_id: 0,
        list_id: 100,
        program_index: 1,
    };
    assert_eq!(
        chain.select_factory_preset(0, &invalid).unwrap_err().kind(),
        FailureKind::InvalidInput
    );
    assert_eq!(
        chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap(),
        saved
    );
    assert_eq!(block(&mut chain, 64, 0.0), vec![0.75; 64]);
}
