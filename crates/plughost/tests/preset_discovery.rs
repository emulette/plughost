use plughost::*;
use std::path::PathBuf;
use std::time::Duration;

mod support;

use support::{fixture, helper, plugins, spawn_with};

const TIMEOUT: Duration = Duration::from_secs(1);

fn bundle() -> PathBuf {
    plugins().join("plughost-test-presets.clap")
}
fn query(location: PresetLocation) -> PresetDiscoveryTarget {
    PresetDiscoveryTarget {
        provider_id: "com.studio.plughost.presets".into(),
        location,
    }
}
fn make_chain() -> Chain {
    let mut chain = spawn_with(
        &[fixture(
            PluginFormat::Clap,
            "plughost-test-presets",
            "com.studio.plughost.test-presets",
        )],
        &HostIdentity::default(),
        {
            let mut timeouts = Timeouts::default();
            timeouts.load = TIMEOUT;
            timeouts
        },
    );
    let config = chain
        .main_bus_config(48000.0, 32, Layout::None, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    chain
}
fn output(chain: &mut Chain) -> Vec<f32> {
    let mut left = vec![0.0; 32];
    let mut right = vec![0.0; 32];
    chain
        .process_audio_f32(
            &BlockContext::new(32),
            &[],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    left
}
#[test]
#[ignore = "needs helper and test plugins"]
fn provider_queries_are_isolated_and_failure_does_not_affect_live_chain() {
    let identity = HostIdentity::default();
    let mut chain = make_chain();
    let providers = preset_providers(&helper(), &bundle(), &identity, TIMEOUT).unwrap();
    assert_eq!(providers.len(), 1);
    let presets = discover_presets(
        &helper(),
        &bundle(),
        &identity,
        &query(PresetLocation::Plugin),
        TIMEOUT,
    )
    .unwrap();
    assert_eq!(presets.presets.len(), 2);
    for (suffix, kind) in [
        ("fail", FailureKind::State),
        ("duplicate", FailureKind::State),
        ("crash", FailureKind::Crashed),
        ("hang", FailureKind::TimedOut),
    ] {
        let query = query(PresetLocation::File {
            path: plugins().join(format!("{suffix}.phcp")),
        });
        let error = discover_presets(&helper(), &bundle(), &identity, &query, TIMEOUT).unwrap_err();
        assert_eq!(error.kind(), kind, "{suffix}");
        assert_eq!(output(&mut chain), vec![1.0; 32]);
    }
}
#[test]
#[ignore = "needs helper and test plugins"]
fn crawled_presets_load_through_the_file_they_were_found_in() {
    let directory = tempfile::tempdir().unwrap();
    for file in ["a.phcp", "fail.phcp"] {
        std::fs::write(directory.path().join(file), []).unwrap();
    }
    let discovery = discover_presets(
        &helper(),
        &bundle(),
        &HostIdentity::default(),
        &query(PresetLocation::File {
            path: directory.path().to_owned(),
        }),
        TIMEOUT,
    )
    .unwrap();
    assert_eq!(discovery.presets.len(), 2);
    assert_eq!(
        discovery.failed_files[0].path,
        directory.path().join("fail.phcp")
    );
    let quarter = discovery
        .presets
        .iter()
        .find(|preset| preset.load_key.as_deref() == Some("quarter"))
        .unwrap();
    assert_eq!(
        quarter.location,
        PresetLocation::File {
            path: directory.path().join("a.phcp")
        }
    );
    let mut chain = make_chain();
    chain
        .load_discovered_preset(0, &quarter.location, quarter.load_key.as_deref())
        .unwrap();
    assert_eq!(output(&mut chain), vec![0.25; 32]);
}
#[test]
#[ignore = "needs helper and test plugins"]
fn returned_preset_load_failures_preserve_prepared_output_and_success_round_trips() {
    let mut chain = make_chain();
    for key in ["fail", "callback-error"] {
        assert_eq!(
            chain
                .load_discovered_preset(0, &PresetLocation::Plugin, Some(key))
                .unwrap_err()
                .kind(),
            FailureKind::State
        );
        assert_eq!(output(&mut chain), vec![1.0; 32]);
    }
    assert!(
        chain
            .take_diagnostics()
            .unwrap()
            .records
            .iter()
            .any(|r| r.message.contains("fixture preset callback failure"))
    );
    chain
        .load_discovered_preset(0, &PresetLocation::Plugin, Some("quarter"))
        .unwrap();
    assert_eq!(output(&mut chain), vec![0.25; 32]);
    let state = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    let mut fresh = make_chain();
    fresh
        .restore_state(0, &state, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(output(&mut fresh), vec![0.25; 32]);
}
#[test]
#[ignore = "needs helper and test plugins"]
fn preset_load_crash_and_timeout_are_reported_and_chain_can_recover() {
    for (key, kind) in [
        ("crash", FailureKind::Crashed),
        ("hang", FailureKind::TimedOut),
    ] {
        let mut chain = make_chain();
        assert_eq!(
            chain
                .load_discovered_preset(0, &PresetLocation::Plugin, Some(key))
                .unwrap_err()
                .kind(),
            kind
        );
        chain = chain.recover(&[]).unwrap();
        assert_eq!(output(&mut chain), vec![1.0; 32]);
    }
}
