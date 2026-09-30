//! Static discovery and native inspection through the public Scanner API.
use std::fs;
use std::time::Duration;

use plughost::{ScanOutcome, Scanner, Source};

mod support;

use support::{copy, helper, plugins};

#[path = "discovery/selection.rs"]
mod selection;

const ROUTING_ID: &str = "706C7567686F7374526F7574696E6701";
const STATIC_ID: &str = "00000000000000000000000000000001";
const METADATA: &str = r#"{"Classes":[{"CID":"00000000000000000000000000000001","Category":"Audio Module Class","Name":"Static only"}]}"#;

fn classes(outcome: &ScanOutcome) -> &[plughost::PluginInfo] {
    let ScanOutcome::Found(classes) = outcome else {
        panic!("{outcome:?}")
    };
    classes
}

#[test]
#[ignore = "needs repository test plugins; no native bundle loading"]
fn discovery_leaves_crashing_and_hanging_modules_unloaded_and_does_not_write_cache() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache.json");
    let plugins = plugins();
    let scanner = Scanner::new(&directory.path().join("missing-helper"), &cache)
        .directories(vec![plugins.clone()]);
    let catalog = scanner.discover();
    for name in [
        "plughost-test-crash-on-scan",
        "plughost-test-hang-on-scan",
        "plughost-test-delay",
    ] {
        for extension in ["vst3", "clap"] {
            let path = plugins.join(format!("{name}.{extension}"));
            let scan = catalog
                .bundles
                .iter()
                .find(|scan| scan.path == path)
                .unwrap();
            assert_eq!(scan.outcome, ScanOutcome::NotInspected);
            assert_eq!(
                (scan.source, scan.failures, scan.excluded),
                (Source::Inspected, 0, false)
            );
        }
    }
    assert!(!cache.exists());
    // Registry errors are reported independently; they cannot force native bundle loading.
    #[cfg(target_os = "macos")]
    assert!(matches!(catalog.audio_units, ScanOutcome::Failed(_)));
}

#[test]
#[ignore = "needs helper and routing VST3 fixture"]
fn static_metadata_never_substitutes_for_native_inspection_or_native_cache() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("routing.vst3");
    let original = plugins().join("plughost-test-routing.vst3");
    #[cfg(target_os = "macos")]
    copy(&original, &bundle);
    #[cfg(target_os = "windows")]
    {
        // The Windows fixture is a single module; use the standard directory bundle layout.
        let native = bundle.join("Contents/x86_64-win/routing.vst3");
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        copy(&original, &native);
    }
    let metadata = bundle.join("Contents/Resources/moduleinfo.json");
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    fs::write(&metadata, METADATA).unwrap();
    let cache = directory.path().join("cache.json");
    let scanner = Scanner::new(&helper(), &cache).directories(vec![directory.path().to_owned()]);
    let discovered = scanner.discover();
    assert_eq!(discovered.bundles[0].source, Source::ModuleInfo);
    assert_eq!(
        classes(&discovered.bundles[0].outcome)[0].class_id,
        STATIC_ID
    );
    assert!(!cache.exists());
    let inspected = scanner.scan(&|_| {}).unwrap();
    assert_eq!(inspected.bundles[0].source, Source::Loaded);
    assert_eq!(
        classes(&inspected.bundles[0].outcome)[0].class_id,
        ROUTING_ID
    );
    let saved = fs::read(&cache).unwrap();
    for catalog in [scanner.discover(), scanner.scan(&|_| {}).unwrap()] {
        assert_eq!(catalog.bundles[0].source, Source::Cache);
        assert_eq!(classes(&catalog.bundles[0].outcome)[0].class_id, ROUTING_ID);
    }
    assert_eq!(fs::read(&cache).unwrap(), saved);
    // Schema 1 cannot tell static metadata from native results; even successes must be rechecked.
    let mut legacy: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    legacy["schema"] = 1.into();
    fs::write(&cache, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(scanner.discover().bundles[0].source, Source::ModuleInfo);
    assert_eq!(
        scanner.scan(&|_| {}).unwrap().bundles[0].source,
        Source::Loaded
    );
    // Invalid metadata invalidates the old fingerprint and remains explicitly uninspected.
    fs::write(&metadata, "invalid metadata").unwrap();
    let before = fs::read(&cache).unwrap();
    assert_eq!(
        scanner.discover().bundles[0].outcome,
        ScanOutcome::NotInspected
    );
    assert_eq!(fs::read(&cache).unwrap(), before);
    assert_eq!(
        classes(&scanner.scan(&|_| {}).unwrap().bundles[0].outcome)[0].class_id,
        ROUTING_ID
    );
    // Explicit retry also always calls native code, even when moduleinfo is valid.
    fs::write(&metadata, METADATA).unwrap();
    let retried = scanner.retry(&bundle).unwrap();
    assert_eq!(retried.source, Source::Loaded);
    assert_eq!(classes(&retried.outcome)[0].class_id, ROUTING_ID);
}

#[test]
#[ignore = "needs helper and crash-on-scan VST3 fixture"]
fn discovery_preserves_a_cached_failure_without_retrying_or_incrementing_it() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("plughost-test-crash-on-scan.vst3");
    copy(&plugins().join("plughost-test-crash-on-scan.vst3"), &bundle);
    let cache = directory.path().join("cache.json");
    let scanned = Scanner::new(&helper(), &cache)
        .directories(vec![directory.path().to_owned()])
        .scan(&|_| {})
        .unwrap();
    assert!(matches!(
        scanned.bundles[0].outcome,
        ScanOutcome::Crashed(_)
    ));
    let before = fs::read(&cache).unwrap();
    let scanner = Scanner::new(&directory.path().join("missing-helper"), &cache)
        .directories(vec![directory.path().to_owned()]);
    for _ in 0..2 {
        let discovered = scanner.discover();
        let result = &discovered.bundles[0];
        assert_eq!(result.source, Source::Cache);
        assert_eq!(result.outcome, scanned.bundles[0].outcome);
        assert_eq!((result.failures, result.excluded), (1, false));
        assert_eq!(fs::read(&cache).unwrap(), before);
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs routing CLAP fixture"]
fn clap_metadata_cannot_be_misreported_as_vst3_classes() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("routing.clap");
    copy(&plugins().join("plughost-test-routing.clap"), &bundle);
    let metadata = bundle.join("Contents/Resources/moduleinfo.json");
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    fs::write(metadata, METADATA).unwrap();
    let catalog = Scanner::new(&helper(), &directory.path().join("cache.json"))
        .directories(vec![directory.path().to_owned()])
        .discover();
    assert_eq!(catalog.bundles[0].outcome, ScanOutcome::NotInspected);
}
