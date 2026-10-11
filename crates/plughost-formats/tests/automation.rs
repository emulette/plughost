//! Native point delivery, including queue reuse after control-only flushes.
#![cfg(all(feature = "clap", feature = "vst3"))]

use plughost_core::{
    BlockContext, FailureKind, HostIdentity, Layout, ParameterChange, PluginFormat, PluginRef,
    ProcessConfig, ProcessMode, SampleFormat,
};
use plughost_formats::HostedPlugin;
use std::path::Path;
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

fn routing(format: PluginFormat) -> PluginRef {
    let (extension, class) = match format {
        PluginFormat::Vst3 => ("vst3", "706C7567686F7374526F7574696E6701"),
        PluginFormat::Clap => ("clap", "com.studio.plughost.test-routing"),
        _ => unreachable!(),
    };
    PluginRef {
        format,
        bundle: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-plugins/plughost-test-routing.{extension}"
        ))),
        class_id: class.into(),
    }
}

fn prepared(reference: &PluginRef) -> Box<dyn HostedPlugin> {
    let mut plugin = plughost_formats::load(reference, &HostIdentity::default()).unwrap();
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
    plugin
}

fn point(id: u64, offset: usize, value: f64) -> ParameterChange {
    ParameterChange { id, offset, value }
}

#[test]
#[ignore = "needs routing VST3 and CLAP fixtures"]
fn rejected_points_and_control_flushes_do_not_leave_stale_native_queues() {
    let _serial = SERIAL.lock().unwrap();
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut plugin = prepared(&routing(format));
        let processor = plugin.processor();
        let input = [1.0f32; 32];
        let mut left = [7.0f32; 32];
        let mut right = left;
        let invalid = [point(0, 0, 0.5), point(u64::MAX, 31, 0.0)];
        let error = processor
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &invalid,
                &[],
                &mut Vec::new(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), FailureKind::InvalidInput);
        assert_eq!(left, [7.0; 32]);
        assert_eq!(right, left);
        processor
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(left, input, "invalid automation reached DSP");
        assert_eq!(right, input);
        for (id, value) in [(0, 0.25), (1, 1.0)] {
            plugin.set_parameter(id, value).unwrap();
            plugin.timing().unwrap(); // Flushes without advancing the audio position.
        }
        for edits in [
            [point(0, 0, 0.5), point(1, 0, 0.0)],
            [point(1, 0, 0.0), point(0, 0, 0.5)],
        ] {
            processor
                .process_audio_f32(
                    &BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &edits,
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
            assert_eq!(left, [0.5; 32]);
            assert_eq!(right, left);
        }
    }
}

#[test]
#[ignore = "needs routing fixtures and Apple AUDelay on macOS"]
#[allow(clippy::print_stdout)]
fn native_automation_benchmark() {
    let _serial = SERIAL.lock().unwrap();
    let references = [
        routing(PluginFormat::Vst3),
        routing(PluginFormat::Clap),
        #[cfg(all(feature = "au", target_os = "macos"))]
        PluginRef {
            format: PluginFormat::AudioUnit,
            bundle: None,
            class_id: "6175667864656C796170706C".into(),
        },
    ];
    for reference in references {
        let au = reference.format == PluginFormat::AudioUnit;
        let plugin = prepared(&reference);
        let processor = plugin.processor();
        let mut edits = Vec::new();
        let expected: Vec<f32> = (0..512)
            .map(|frame| {
                let gain = (frame % 5) as f64 * 0.25;
                let bypass = frame % 4 == 0;
                if au {
                    edits.push(point(0, frame, 0.0)); // Entirely dry: exact input samples.
                } else {
                    for _ in 0..4 {
                        edits.push(point(0, frame, gain));
                        edits.push(point(1, frame, if bypass { 1.0 } else { 0.0 }));
                    }
                }
                if au || bypass { 1.0 } else { gain as f32 }
            })
            .collect();
        let input = [1.0f32; 512];
        let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
        let mut produced = Vec::new();
        let mut block = || {
            processor
                .process_audio_f32(
                    &BlockContext::new(512),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &edits,
                    &[],
                    &mut produced,
                )
                .unwrap();
            assert_eq!(left.as_slice(), expected);
            assert_eq!(right, left);
            assert!(produced.is_empty());
        };
        block();
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            block();
        }
        println!(
            "{:?}, 1,000 blocks, {} points/block: {:?}",
            reference.format,
            edits.len(),
            start.elapsed()
        );
    }
}
