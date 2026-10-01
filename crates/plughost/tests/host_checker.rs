//! The VST3 SDK's `host-checker` example watches the host's calls and reports what it logged
//! through read-only `ProcessWarn1`..`8` parameters, 24 log IDs per parameter. Its log table in
//! the SDK sources names each ID's severity; the host must cause no error-level entry.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use plughost::{
    AutomationEvent, BlockContext, Chain, Event, Layout, ParameterChange, ParameterEvent,
    PluginFormat, ProcessMode, StatePurpose, TimeSignature, Transport,
};

mod support;

use support::{fixture, spawn};

const CLASS: &str = "23FC190E02DD4499A8D2230E50617DA3";
const IDS_PER_PARAMETER: u32 = 24;

/// The severity of each log ID, in the order of the SDK's `LOG_EVENT_LIST`.
fn severities() -> Vec<String> {
    let sdk = std::env::var_os("VST3_SDK_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.vst3sdk/VST_SDK/vst3sdk"),
        PathBuf::from,
    );
    let header =
        std::fs::read_to_string(sdk.join("public.sdk/samples/vst/hostchecker/source/logevents.h"))
            .unwrap();
    // The list is one macro: its lines continue while they end in a backslash.
    let mut lines = header[header.find("#define LOG_EVENT_LIST").unwrap()..].lines();
    let mut list = String::new();
    for line in lines.by_ref() {
        list.push_str(line);
        if !line.trim_end().ends_with('\\') {
            break;
        }
    }
    list.split("LOG_DEF")
        .skip(2)
        .map(|entry| entry.split(',').nth(2).unwrap().trim().to_owned())
        .collect()
}

struct Log {
    warnings: BTreeMap<u64, u32>,
    ids: BTreeSet<u32>,
}

impl Log {
    fn drain(&mut self, chain: &mut Chain, phase: &str) {
        let batch = chain.take_parameter_events(0).unwrap();
        assert_eq!(batch.dropped, 0, "{phase}");
        for event in batch.events {
            if let ParameterEvent::Value { id, normalized } = event
                && let Some(&first) = self.warnings.get(&id)
            {
                let bits = (normalized * f64::from(1u32 << IDS_PER_PARAMETER)).round() as u32;
                self.ids.extend(
                    (0..IDS_PER_PARAMETER)
                        .filter(|bit| bits & (1 << bit) != 0)
                        .map(|bit| first + bit),
                );
            }
        }
    }
}

/// Plays `blocks` blocks with a running transport, a note in every eighth block, and automation.
fn play(chain: &mut Chain, blocks: usize, frames: usize, rate: f64, automated: u64) {
    let input = vec![0.1f32; frames];
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    for block in 0..blocks {
        let position = (block * frames) as i64;
        let context = BlockContext {
            frames,
            transport: Some(Transport {
                sample_position: position,
                beat_position: Some(position as f64 / rate * 2.0),
                bar_position: None,
                tempo: Some(120.0),
                time_signature: Some(TimeSignature {
                    numerator: 4,
                    denominator: 4,
                }),
                playing: true,
                loop_region: None,
            }),
        };
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: automated,
                offset: frames / 2,
                value: (block % 10) as f64 / 10.0,
            },
        }];
        let notes = if block % 8 == 0 {
            vec![
                Event::note_on(0, 0, 60, 100),
                Event::note_off(frames - 1, 0, 60, 0),
            ]
        } else {
            Vec::new()
        };
        chain
            .process_audio_f32(
                &context,
                &[&input, &input],
                &mut [&mut left, &mut right],
                &automation,
                &notes,
                &mut Vec::new(),
            )
            .unwrap();
    }
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and build-test-plugins.ps1 (or .sh)"]
fn the_host_checker_logs_no_host_errors() {
    let mut chain = spawn(&[fixture(PluginFormat::Vst3, "host-checker", CLASS)]);
    let parameters = chain.parameters(0).unwrap();
    let mut log = Log {
        warnings: parameters
            .iter()
            .filter_map(|(info, _)| {
                let number: u32 = info.title.strip_prefix("ProcessWarn")?.parse().ok()?;
                Some((info.id, (number - 1) * IDS_PER_PARAMETER))
            })
            .collect(),
        ids: BTreeSet::new(),
    };
    assert_eq!(log.warnings.len(), 8);
    let automated = parameters
        .iter()
        .find(|(info, _)| info.flags.automatable && !info.flags.read_only && !info.flags.bypass)
        .unwrap()
        .0
        .id;

    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    play(&mut chain, 100, 512, 48_000.0, automated);
    log.drain(&mut chain, "processing");
    play(&mut chain, 50, 100, 48_000.0, automated);
    log.drain(&mut chain, "shorter blocks");
    chain.set_parameter(0, automated, 0.3).unwrap();
    chain.reset().unwrap();
    play(&mut chain, 20, 512, 48_000.0, automated);
    log.drain(&mut chain, "reset");
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    play(&mut chain, 20, 512, 48_000.0, automated);
    log.drain(&mut chain, "state");
    // A prepared chain stages its replacement on a fresh instance.
    let mut config = chain
        .main_bus_config(96_000.0, 1024, Layout::Stereo, &[Layout::Stereo])
        .unwrap();
    config.mode = ProcessMode::Realtime;
    chain.prepare_audio(&config).unwrap();
    play(&mut chain, 50, 1024, 96_000.0, automated);
    log.drain(&mut chain, "staged preparation");

    let severities = severities();
    let errors: Vec<u32> = log
        .ids
        .iter()
        .copied()
        .filter(|&id| severities[id as usize] == "LOG_ERR")
        .collect();
    assert!(!log.ids.is_empty(), "the checker reported nothing");
    assert!(errors.is_empty(), "host errors by log ID: {errors:?}");
}
