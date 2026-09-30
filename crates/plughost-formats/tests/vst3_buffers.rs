//! Allocation regression against the real routing fixture.
#![cfg(feature = "vst3")]

mod processing;
mod support;

use std::path::Path;

use plughost_core::{
    AudioBusConfig, AudioConfig, BlockContext, HostIdentity, Layout, ProcessMode, SampleFormat,
};
use plughost_formats::vst3::{Module, Plugin};
use plughost_formats::{BlockProcessor, HostedPlugin};
use processing::ProcessSample;

#[test]
#[ignore = "needs routing test plugin"]
fn prepared_native_audio_reuses_storage_across_blocks_and_reconfiguration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins/plughost-test-routing.vst3");
    let module = Module::load(&path).unwrap();
    let mut plugin = Plugin::new(
        &module,
        &module.classes()[0].class_id,
        &HostIdentity::default(),
    )
    .unwrap();
    // A moved processor uses the same prepared storage. Every call supplies fresh output storage.
    for (format, multiple) in [
        (SampleFormat::F32, false),
        (SampleFormat::F64, true),
        (SampleFormat::F32, true),
        (SampleFormat::F64, false),
    ] {
        let bus = |id, layout, active| AudioBusConfig { id, layout, active };
        plugin
            .prepare_audio(&AudioConfig {
                events: Default::default(),
                sample_rate: 48_000.0,
                max_block_size: 512,
                sample_format: format,
                mode: ProcessMode::Offline,
                configuration: None,
                inputs: vec![bus(0, Layout::Mono, multiple), bus(1, Layout::Stereo, true)],
                outputs: vec![
                    bus(0, Layout::Stereo, multiple),
                    bus(1, Layout::Stereo, true),
                ],
            })
            .unwrap();
        let processor = plugin.processor();
        std::thread::spawn(move || match format {
            SampleFormat::F32 => blocks(processor.as_ref(), multiple, 1.0f32),
            SampleFormat::F64 => blocks(processor.as_ref(), multiple, 1.0 + f64::EPSILON),
        })
        .join()
        .unwrap();
    }
}

fn blocks<S: ProcessSample>(processor: &dyn BlockProcessor, multiple: bool, value: S) {
    // COM interface tables and thread-local runtime state may initialize on the first call.
    for (index, frames) in [512, 512, 1, 127, 0, 511, 512].into_iter().enumerate() {
        let key = vec![S::default(); frames];
        let input = vec![value; frames];
        let inputs = if multiple {
            vec![key.as_slice(), &input, &input]
        } else {
            vec![input.as_slice(); 2]
        };
        let mut output = vec![vec![S::default(); frames]; if multiple { 4 } else { 2 }];
        let mut outputs: Vec<_> = output.iter_mut().map(Vec::as_mut_slice).collect();
        let changes = vec![
            plughost_core::ParameterChange {
                id: 0,
                offset: frames.saturating_sub(1),
                value: 1.0,
            };
            if frames == 0 {
                0
            } else {
                plughost_core::MAX_BLOCK_EVENTS
            }
        ];
        let (result, requests) = support::heap_requests(|| {
            S::process(
                processor,
                &BlockContext::new(frames),
                &inputs,
                &mut outputs,
                &changes,
                &[],
            )
        });
        result.unwrap();
        if index != 0 {
            assert_eq!(requests, 0);
        }
        for channel in &output[if multiple { 2 } else { 0 }..] {
            assert!(
                channel
                    .iter()
                    .all(|sample| sample.to_f64() == value.to_f64())
            );
        }
        if multiple {
            assert!(
                output[0]
                    .iter()
                    .all(|sample| sample.to_f64() == value.to_f64())
            );
            assert!(output[1].iter().all(|sample| sample.to_f64() == 0.0));
        }
        assert!(input.iter().all(|sample| sample.to_f64() == value.to_f64()));
    }
}
