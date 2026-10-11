//! Whole-render validation must reject late invalid targets before earlier edits reach DSP.
use plughost::*;
use plughost_core::render::Process;

mod support;

fn point(slot: usize, id: u64, offset: usize) -> AutomationEvent {
    AutomationEvent {
        slot,
        change: ParameterChange {
            id,
            offset,
            value: 0.25,
        },
    }
}

#[test]
#[ignore = "needs helper and routing VST3/CLAP fixtures"]
fn late_invalid_render_targets_leave_earlier_plugin_state_untouched() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::routing(format)]);
        let config = chain
            .main_bus_config(48_000.0, 32, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        for (slot, id, expected) in [
            (0, u64::MAX, InputError::UnknownParameter { id: u64::MAX }),
            (1, 0, InputError::Slot),
        ] {
            let automation = [point(0, 0, 0), point(0, 0, 64), point(slot, id, 95)];
            let input = [1.0f32; 96];
            let result = render_with_schedule(
                &mut chain,
                &[&input, &input],
                input.len(),
                &[],
                &RenderOptions::new(TailPolicy::Reported, 0.0),
                &RenderSchedule {
                    automation: &automation,
                    ..Default::default()
                },
            );
            assert!(
                matches!(result, Err(Error::Input { slot: Some(s), error })
                if s == slot && error == expected),
                "{format:?}, {slot}, {id}"
            );
            let gain = chain
                .parameters(0)
                .unwrap()
                .into_iter()
                .find(|(p, _)| p.id == 0)
                .unwrap()
                .1;
            assert_eq!(gain, 1.0, "preflight advanced an earlier block");
        }
    }
}

#[test]
#[ignore = "needs helper and CLAP delay fixture"]
fn writable_nonautomatable_parameters_reject_block_and_render_automation() {
    let mut chain = support::spawn(&[support::delay(PluginFormat::Clap)]);
    let details = chain.parameter_details(0, 1).unwrap();
    assert!(!details.info.flags.read_only && !details.info.flags.automatable);
    chain.set_parameter(0, 1, 1.0).unwrap();
    assert_eq!(
        chain
            .parameters(0)
            .unwrap()
            .iter()
            .find(|(p, _)| p.id == 1)
            .unwrap()
            .1,
        1.0
    );
    let config = chain
        .main_bus_config(48_000.0, 16, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let input = [1.0f32; 96];
    let edits = [point(0, 0, 0), point(0, 1, 95)];
    let error = render_with_schedule(
        &mut chain,
        &[&input, &input],
        96,
        &[],
        &RenderOptions::new(TailPolicy::Reported, 0.0),
        &RenderSchedule {
            automation: &edits,
            ..Default::default()
        },
    )
    .unwrap_err();
    let rejected = |error| {
        assert!(matches!(
            error,
            Error::Input {
                slot: Some(0),
                error: InputError::NotAutomatable { id: 1 }
            }
        ))
    };
    rejected(error);
    let edits = [point(0, 0, 0), point(0, 1, 15)];
    let (mut left, mut right) = ([7.0; 16], [7.0; 16]);
    rejected(
        chain
            .process_audio_f32(
                &BlockContext::new(16),
                &[&input[..16], &input[..16]],
                &mut [&mut left, &mut right],
                &edits,
                &[],
                &mut Vec::new(),
            )
            .unwrap_err(),
    );
    assert_eq!(left, [7.0; 16]);
    assert_eq!(right, left);
    let parameters = chain.parameters(0).unwrap();
    let value = |id| parameters.iter().find(|(info, _)| info.id == id).unwrap().1;
    assert_eq!(value(0), 0.5, "the earlier gain edit must not be applied");
    assert_eq!(value(8), 0.0, "no native sample may be processed");
}

#[test]
#[ignore = "needs helper and native CLAP delay fixture"]
#[allow(clippy::print_stdout)]
fn render_validation_benchmark() {
    let references = vec![support::delay(PluginFormat::Clap); 8];
    let mut chain = support::spawn(&references);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo; 8])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let automation: Vec<_> = (0..4096).map(|i| point(i % 8, 0, i)).collect();
    <Chain as Process<f32>>::validate_automation(&mut chain, &automation).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        <Chain as Process<f32>>::validate_automation(&mut chain, &automation).unwrap();
    }
    println!(
        "1,000 preflights, 8 slots, 4,096 points: {:?}",
        start.elapsed()
    );
    for slot in 0..8 {
        assert_eq!(
            chain
                .parameters(slot)
                .unwrap()
                .iter()
                .find(|(p, _)| p.id == 0)
                .unwrap()
                .1,
            1.0
        );
    }
}
