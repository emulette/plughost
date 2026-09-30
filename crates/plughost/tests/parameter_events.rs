use plughost::*;

mod support;

use support::{block, prepared_delay as chain};

#[test]
#[ignore = "needs helper and test plugins"]
fn native_edit_notifications_survive_helper_transport_and_overflow() {
    let mut clap = chain(PluginFormat::Clap);
    clap.take_parameter_events(0).unwrap();
    for _ in 0..100 {
        clap.set_parameter(0, 0, 0.5).unwrap();
        block(&mut clap, 4, 1.0);
    }
    let batch = clap.take_parameter_events(0).unwrap();
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
    assert!(clap.take_parameter_events(0).unwrap().events.is_empty());
    assert_eq!(
        clap.parameters(0)
            .unwrap()
            .iter()
            .find(|(info, _)| info.id == 0)
            .unwrap()
            .1,
        0.5
    );
}

#[test]
#[ignore = "needs helper and test plugins"]
fn dynamic_flags_refresh_process_validation_without_consuming_app_events() {
    let mut vst3 = chain(PluginFormat::Vst3);
    vst3.take_parameter_events(0).unwrap();
    assert!(!vst3.parameter_details(0, 0).unwrap().info.flags.read_only);
    vst3.set_parameter(0, 7, 1.0).unwrap();
    let input = [0.0f32; 4];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    let automation = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: 0,
            offset: 0,
            value: 0.75,
        },
    }];
    let error = vst3
        .process_audio_f32(
            &BlockContext::new(4),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &automation,
            &[],
            &mut Vec::new(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Input {
            slot: Some(0),
            error: InputError::ReadOnlyParameter { id: 0 }
        }
    ));
    let batch = vst3.take_parameter_events(0).unwrap();
    assert!(batch.events.contains(&ParameterEvent::MetadataChanged));
    assert!(
        batch
            .events
            .contains(&ParameterEvent::Dirty { dirty: true })
    );
    assert!(batch.events.windows(3).any(|events| events
        == [
            ParameterEvent::BeginEdit { id: 0 },
            ParameterEvent::Value {
                id: 0,
                normalized: 0.25
            },
            ParameterEvent::EndEdit { id: 0 }
        ]));
    assert!(vst3.parameter_details(0, 0).unwrap().info.flags.read_only);
    vst3.set_parameter(0, 7, 0.0).unwrap();
    vst3.process_audio_f32(
        &BlockContext::new(4),
        &[&input, &input],
        &mut [&mut left, &mut right],
        &automation,
        &[],
        &mut Vec::new(),
    )
    .unwrap();
    assert!(!vst3.parameter_details(0, 0).unwrap().info.flags.read_only);
    assert!(
        vst3.take_parameter_events(0)
            .unwrap()
            .events
            .contains(&ParameterEvent::MetadataChanged)
    );
    // Reset clears audio only; the controller still reports the gain as read-only.
    vst3.set_parameter(0, 7, 1.0).unwrap();
    vst3.reset().unwrap();
    assert!(vst3.take_parameter_events(0).unwrap().resync_required);
    assert!(vst3.parameter_details(0, 0).unwrap().info.flags.read_only);
    assert!(matches!(
        vst3.process_audio_f32(
            &BlockContext::new(4),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &automation,
            &[],
            &mut Vec::new(),
        ),
        Err(Error::Input {
            slot: Some(0),
            error: InputError::ReadOnlyParameter { id: 0 }
        })
    ));
    vst3.set_parameter(0, 7, 0.0).unwrap();
    vst3.process_audio_f32(
        &BlockContext::new(4),
        &[&input, &input],
        &mut [&mut left, &mut right],
        &automation,
        &[],
        &mut Vec::new(),
    )
    .unwrap();
}

#[test]
#[ignore = "needs helper and test plugins"]
fn replacing_slot_state_invalidates_the_consumer_parameter_view() {
    let mut chain = chain(PluginFormat::Clap);
    let state = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    chain.take_parameter_events(0).unwrap();
    chain
        .restore_state(0, &state, plughost::StatePurpose::Project)
        .unwrap();
    let batch = chain.take_parameter_events(0).unwrap();
    assert!(batch.resync_required);
    assert_eq!(batch.dropped, 0);
    assert!(batch.events.contains(&ParameterEvent::MetadataChanged));
    assert!(batch.events.contains(&ParameterEvent::ValuesChanged));
}
