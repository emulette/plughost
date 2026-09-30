//! Allocation regression against the explicitly selected Apple AUDelay, without registry scanning.
#![cfg(all(target_os = "macos", feature = "au"))]

mod support;

use plughost_core::{BlockContext, Layout, ProcessConfig, ProcessMode, SampleFormat};
use plughost_formats::HostedPlugin;
use plughost_formats::au::Plugin;

#[test]
fn prepared_apple_audio_reuses_storage_across_blocks_and_reconfiguration() {
    let mut plugin = Plugin::new("6175667864656C796170706C").unwrap();
    for (rate, maximum) in [(48_000.0, 512), (96_000.0, 128), (48_000.0, 512)] {
        plugin
            .prepare(&ProcessConfig {
                sample_rate: rate,
                max_block_size: maximum,
                sample_format: SampleFormat::F32,
                input: Layout::Stereo,
                output: Layout::Stereo,
                mode: ProcessMode::Offline,
            })
            .unwrap();
        plugin.set_parameter(0, 0.0).unwrap(); // AUDelay wet/dry: entirely dry.
        let processor = plugin.processor();
        std::thread::spawn(move || {
            // Native processing and per-thread runtime initialization are outside measurement.
            for (index, frames) in [maximum, maximum, 1, 0, maximum - 1, maximum]
                .into_iter()
                .enumerate()
            {
                let left: Vec<_> = (0..frames).map(|frame| frame as f32 / 1024.0).collect();
                let right: Vec<_> = left.iter().map(|sample| -*sample).collect();
                let mut outputs = [vec![99.0; frames], vec![99.0; frames]];
                let mut slices: Vec<_> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
                let changes = vec![
                    plughost_core::ParameterChange {
                        id: 0,
                        offset: 0,
                        value: 0.0,
                    };
                    if frames == 0 {
                        0
                    } else {
                        plughost_core::MAX_BLOCK_EVENTS
                    }
                ];
                let (result, requests) = support::heap_requests(|| {
                    processor.process_audio_f32(
                        &BlockContext::new(frames),
                        &[&left, &right],
                        &mut slices,
                        &changes,
                        &[],
                        &mut Vec::new(),
                    )
                });
                result.unwrap();
                if index != 0 {
                    assert_eq!(requests, 0);
                }
                assert_eq!(outputs[0], left);
                assert_eq!(outputs[1], right);
                assert!(
                    left.iter()
                        .enumerate()
                        .all(|(frame, sample)| *sample == frame as f32 / 1024.0)
                );
            }
        })
        .join()
        .unwrap();
    }
}
