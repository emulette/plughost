#![cfg(feature = "clap")]
use plughost_core::{FailureKind, HostIdentity, PresetDiscoveryTarget, PresetLocation};
use plughost_formats::clap::{self, ClapError, Plugin};
use plughost_formats::{Error, HostedPlugin};
use std::path::{Path, PathBuf};
const CLASS: &str = "com.studio.plughost.test-presets";
const PROVIDER: &str = "com.studio.plughost.presets";
fn bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins/plughost-test-presets.clap")
}
fn target(location: PresetLocation) -> PresetDiscoveryTarget {
    PresetDiscoveryTarget {
        provider_id: PROVIDER.into(),
        location,
    }
}
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn provider_declarations_and_metadata_preserve_native_load_identity() {
    let identity = HostIdentity::default();
    let providers = clap::preset_providers(&bundle(), &identity).unwrap();
    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    assert_eq!(provider.id, PROVIDER);
    assert_eq!(provider.name, "Fixture presets");
    assert_eq!(provider.file_types[0].extension.as_deref(), Some("phcp"));
    assert_eq!(provider.locations[0].location, PresetLocation::Plugin);
    let discovery =
        clap::discover_presets(&bundle(), &identity, &target(PresetLocation::Plugin)).unwrap();
    assert!(discovery.failed_files.is_empty());
    let presets = discovery.presets;
    assert_eq!(presets.len(), 2);
    assert_eq!(presets[0].location, PresetLocation::Plugin);
    assert_eq!(presets[0].name.as_deref(), Some("Half"));
    assert_eq!(presets[0].load_key.as_deref(), Some("half"));
    assert_eq!(presets[0].plugin_ids[0].abi, "clap");
    assert_eq!(presets[0].plugin_ids[0].id, CLASS);
    assert_eq!(presets[0].creators, ["plughost"]);
    assert_eq!(presets[0].features, ["Test"]);
    let mut plugin = Plugin::new(&bundle(), CLASS, &identity).unwrap();
    for (preset, gain) in presets.iter().zip([0.5f32, 0.25]) {
        plugin
            .load_discovered_preset(&PresetLocation::Plugin, preset.load_key.as_deref())
            .unwrap();
        assert_eq!(
            plugin
                .save_state(plughost_core::StatePurpose::Project)
                .unwrap()
                .component,
            gain.to_le_bytes()
        );
    }
}
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn directories_are_crawled_for_declared_file_types_and_refused_files_are_listed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("nested")).unwrap();
    for file in [
        "a.phcp",
        "nested/B.PHCP",
        "fail.phcp",
        "duplicate.phcp",
        "notes.txt",
    ] {
        std::fs::write(root.join(file), []).unwrap();
    }
    let discovery = clap::discover_presets(
        &bundle(),
        &HostIdentity::default(),
        &target(PresetLocation::File {
            path: root.to_owned(),
        }),
    )
    .unwrap();
    let found: Vec<(PathBuf, Option<&str>)> = discovery
        .presets
        .iter()
        .map(|preset| match &preset.location {
            PresetLocation::File { path } => (path.clone(), preset.load_key.as_deref()),
            PresetLocation::Plugin => panic!("a crawled preset belongs to its file"),
        })
        .collect();
    assert_eq!(
        found,
        [
            (root.join("a.phcp"), Some("half")),
            (root.join("a.phcp"), Some("quarter")),
            (root.join("nested").join("B.PHCP"), Some("half")),
            (root.join("nested").join("B.PHCP"), Some("quarter")),
        ]
    );
    let failed: Vec<(PathBuf, FailureKind)> = discovery
        .failed_files
        .iter()
        .map(|failure| (failure.path.clone(), failure.failure.kind))
        .collect();
    assert_eq!(
        failed,
        [
            (root.join("duplicate.phcp"), FailureKind::State),
            (root.join("fail.phcp"), FailureKind::State),
        ]
    );
}
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn malformed_and_failed_provider_results_do_not_return_partial_metadata() {
    for (file, expected) in [
        ("fail.phcp", ClapError::PresetDiscovery),
        ("duplicate.phcp", ClapError::PresetMetadata),
    ] {
        let location = PresetLocation::File {
            path: bundle().parent().unwrap().join(file),
        };
        assert_eq!(
            clap::discover_presets(&bundle(), &HostIdentity::default(), &target(location)),
            Err(expected)
        );
    }
}
#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 (or .sh)"]
fn load_observes_return_status_and_error_callbacks() {
    let mut plugin = Plugin::new(&bundle(), CLASS, &HostIdentity::default()).unwrap();
    for key in ["fail", "callback-error"] {
        assert_eq!(
            plugin.load_discovered_preset(&PresetLocation::Plugin, Some(key)),
            Err(Error::Clap(ClapError::PresetLoad))
        );
    }
    assert!(
        plugin
            .take_diagnostics()
            .records
            .iter()
            .any(|r| r.message.contains("fixture preset callback failure"))
    );
    plugin
        .load_discovered_preset(&PresetLocation::Plugin, Some("no-completion"))
        .unwrap();
    plugin
        .load_discovered_preset(&PresetLocation::Plugin, Some("quarter"))
        .unwrap();
    assert_eq!(
        plugin
            .save_state(plughost_core::StatePurpose::Project)
            .unwrap()
            .component,
        0.25f32.to_le_bytes()
    );
}
#[test]
fn invalid_discovery_input_is_rejected_before_loading_a_module() {
    let bundle = Path::new("does-not-exist.clap");
    assert_eq!(
        clap::discover_presets(
            bundle,
            &HostIdentity::default(),
            &target(PresetLocation::File {
                path: "relative.phcp".into()
            })
        ),
        Err(ClapError::PresetInput)
    );
    let target = PresetDiscoveryTarget {
        provider_id: "bad\0id".into(),
        location: PresetLocation::Plugin,
    };
    assert_eq!(
        clap::discover_presets(bundle, &HostIdentity::default(), &target),
        Err(ClapError::PresetInput)
    );
}
