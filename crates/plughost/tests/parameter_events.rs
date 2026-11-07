use plughost::*;

mod support;

use support::{block, prepared_delay as chain};

#[test]
#[ignore = "needs helper and test plugins"]
fn native_edit_notifications_survive_helper_transport_and_overflow() {
    let mut clap = chain(PluginFormat::Clap);
    let _ = clap.take_parameter_events(0).unwrap();
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
        Some(0.5)
    );
}

#[test]
#[ignore = "needs helper and test plugins"]
fn dynamic_flags_refresh_process_validation_without_consuming_app_events() {
    let mut vst3 = chain(PluginFormat::Vst3);
    let _ = vst3.take_parameter_events(0).unwrap();
    assert!(!vst3.parameter_details(0, 0).unwrap().info.flags.read_only);
    vst3.set_parameter(0, 7, 1.0).unwrap();
    let input = [0.0f32; 4];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    let automation = [AutomationEvent::new(0, ParameterChange::new(0, 0, 0.75))];
    assert!(matches!(
        <Chain as plughost_core::render::Process<f32>>::validate_automation(&mut vst3, &automation),
        Err(Error::Input {
            slot: Some(0),
            error: InputError::ReadOnlyParameter { id: 0 }
        })
    ));
    let error = vst3
        .process_audio_f32(
            &BlockContext::new(4),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &automation,
            &Events::new(),
            &mut Events::for_block(),
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
        &Events::new(),
        &mut Events::for_block(),
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
            &Events::new(),
            &mut Events::for_block(),
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
        &Events::new(),
        &mut Events::for_block(),
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
    let _ = chain.take_parameter_events(0).unwrap();
    chain
        .restore_state(0, &state, plughost::StatePurpose::Project)
        .unwrap();
    let batch = chain.take_parameter_events(0).unwrap();
    assert!(batch.resync_required);
    assert_eq!(batch.dropped, 0);
    assert!(batch.events.contains(&ParameterEvent::MetadataChanged));
    assert!(batch.events.contains(&ParameterEvent::ValuesChanged));
}

#[test]
#[ignore = "needs helper and the meters CLAP fixture"]
fn a_flood_of_meter_values_does_not_make_the_helper_read_the_parameters_again() {
    // 200 meters report their values every block; small blocks come many times between two
    // helper ticks. The fixture counts the parameter value reads it serves.
    const VALUE_READS: u64 = 15;
    let mut chain = support::spawn(&[support::fixture(
        PluginFormat::Clap,
        "plughost-test-meters",
        "com.studio.plughost.test-delay.7900001d",
    )]);
    support::prepare(&mut chain);
    let reads = |chain: &mut Chain| {
        let (_, value) = chain
            .parameters(0)
            .unwrap()
            .into_iter()
            .find(|(info, _)| info.id == VALUE_READS)
            .unwrap();
        chain
            .parameter_to_plain(0, VALUE_READS, value.unwrap())
            .unwrap()
            .round() as u64
    };
    let one_read = {
        let before = reads(&mut chain);
        reads(&mut chain) - before
    };
    let before = reads(&mut chain);
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(300) {
        block(&mut chain, 64, 1.0);
    }
    // Only this read; the helper read none for the values the meters reported.
    assert_eq!(reads(&mut chain) - before, one_read);
}
