//! Exact block automation through real native fixtures and the isolated chain.
use plughost::*;
mod support;

use support::{routing as reference, spawn};

fn point(slot: usize, id: u64, offset: usize, value: f64) -> AutomationEvent {
    AutomationEvent {
        slot,
        change: ParameterChange { id, offset, value },
    }
}

#[test]
#[ignore = "needs helper and routing VST3/CLAP fixtures"]
fn overlapping_slot_ramps_discrete_points_and_duplicate_edits_preserve_samples() {
    let ramps = [
        AutomationRamp {
            slot: 0,
            id: 0,
            start: 0,
            end: 8191,
            from: 0.0,
            to: 1.0,
        },
        AutomationRamp {
            slot: 1,
            id: 0,
            start: 0,
            end: 8191,
            from: 1.0,
            to: 0.0,
        },
    ];
    let points = [
        point(1, 1, 3000, 0.51),
        point(1, 1, 5000, 0.49),
        point(0, 0, 8192, 0.25),
        point(0, 0, 8192, 0.75),
        point(1, 0, 8192, 0.2),
        point(1, 0, 8192, 0.8),
    ];
    let input = vec![1.0f32; 8200];
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        for block in [127, 4096] {
            let mut chain = spawn(&[reference(format), reference(format)]);
            let config = chain
                .main_bus_config(48_000.0, block, Layout::Stereo, &[Layout::Stereo; 2])
                .unwrap();
            chain.prepare_audio(&config).unwrap();
            let result = render_with_schedule(
                &mut chain,
                &[&input, &input],
                input.len(),
                &[],
                &RenderOptions {
                    tail: TailPolicy::Reported,
                    max_tail_seconds: 0.0,
                },
                &RenderSchedule {
                    automation: &points,
                    ramps: &ramps,
                    ..Default::default()
                },
            )
            .unwrap();
            for (frame, &sample) in result.channels[0].iter().enumerate() {
                let gain = frame as f64 / 8191.0;
                let expected = if frame >= 8192 {
                    0.6
                } else if (3000..5000).contains(&frame) {
                    gain
                } else {
                    gain * (1.0 - gain)
                };
                assert!(
                    (f64::from(sample) - expected).abs() < 1e-6,
                    "{format:?}, block {block}, sample {frame}: {sample} != {expected}"
                );
            }
            assert_eq!(result.channels[0], result.channels[1]);
            assert_eq!(
                chain
                    .parameters(0)
                    .unwrap()
                    .iter()
                    .find(|(p, _)| p.id == 0)
                    .unwrap()
                    .1,
                0.75
            );
            assert_eq!(
                chain
                    .parameters(1)
                    .unwrap()
                    .iter()
                    .find(|(p, _)| p.id == 0)
                    .unwrap()
                    .1,
                0.8
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs the helper and Apple AUDelay"]
fn apple_ramp_matches_single_sample_submission() {
    let reference = PluginRef {
        format: PluginFormat::AudioUnit,
        bundle: None,
        class_id: "6175667864656C796170706C".into(),
    };
    let ramp = [AutomationRamp {
        slot: 0,
        id: 0,
        start: 0,
        end: 256,
        from: 0.25,
        to: 0.75,
    }];
    let input = vec![0.25f32; 257];
    let mut expected: Option<Vec<Vec<f32>>> = None;
    for block in [1, 128] {
        let mut chain = spawn(std::slice::from_ref(&reference));
        let config = chain
            .main_bus_config(48_000.0, block, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        chain.set_parameter(0, 1, 0.5).unwrap();
        chain.set_parameter(0, 0, 0.25).unwrap();
        // Apply control edits and refresh native tail metadata before opening the render.
        let silence = vec![0.0f32; block];
        let (mut left, mut right) = (silence.clone(), silence.clone());
        chain
            .process_audio_f32(
                &BlockContext::new(block),
                &[&silence, &silence],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        let result = render_with_schedule(
            &mut chain,
            &[&input, &input],
            input.len(),
            &[],
            &RenderOptions {
                tail: TailPolicy::Reported,
                max_tail_seconds: 0.0,
            },
            &RenderSchedule {
                ramps: &ramp,
                ..Default::default()
            },
        )
        .unwrap();
        if let Some(expected) = &expected {
            for (actual, expected) in result
                .channels
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
            {
                assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
            }
        } else {
            expected = Some(result.channels);
        }
    }
}
