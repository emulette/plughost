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
