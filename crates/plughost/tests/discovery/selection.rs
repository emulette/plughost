use super::*;
use plughost::{ScanAction, ScanControl, ScanEvent, ScanPolicy, ScanStage, ScanTarget};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs helper and SDK-generated moduleinfo fixtures"]
fn sdk_metadata_matches_the_native_factory() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["again", "adelay", "note-expression-synth"] {
        let path = plugins().join(format!("{name}.vst3"));
        let scanner = Scanner::new(&helper(), &directory.path().join(format!("{name}.json")))
            .directories(vec![plugins()])
            .policy(ScanPolicy::new(ScanAction::Block).bundle(&path, ScanAction::Allow));
        let discovered = scanner.discover();
        let metadata = discovered
            .bundles
            .iter()
            .find(|scan| scan.path == path)
            .unwrap();
        assert_eq!(metadata.source, Source::ModuleInfo);
        let loaded = scanner.retry(&path).unwrap();
        assert_eq!(loaded.source, Source::Loaded);
        assert_eq!(
            classes(&metadata.outcome),
            classes(&loaded.outcome),
            "{name}"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs helper and Apple AU registry; no instantiation"]
fn audio_unit_policy_filters_both_progress_and_catalog() {
    use std::sync::Mutex;
    let directory = tempfile::tempdir().unwrap();
    let selected = "6175667864656C796170706C";
    let policy = ScanPolicy::new(ScanAction::Block)
        .audio_unit(selected, ScanAction::Block)
        .audio_unit(selected.to_ascii_lowercase(), ScanAction::Allow);
    let scanner = Scanner::new(&helper(), &directory.path().join("cache.json"))
        .directories(Vec::new())
        .policy(policy);
    let notified = Mutex::new(Vec::new());
    let catalog = scanner.discover_controlled(&ScanControl::default(), &|event| {
        if let ScanEvent::Progress {
            stage: ScanStage::ClassFound(class),
            ..
        } = event
        {
            notified.lock().unwrap().push(class.class_id.clone());
        }
    });
    assert_eq!(*notified.lock().unwrap(), [selected]);
    let units = classes(&catalog.audio_units);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].class_id, selected);
    assert_eq!(catalog.plugins().len(), 1);
    let blocked = scanner.policy(ScanPolicy::default().audio_unit(selected, ScanAction::Block));
    assert!(
        !classes(&blocked.discover().audio_units)
            .iter()
            .any(|unit| unit.class_id == selected)
    );
    assert!(!directory.path().join("cache.json").exists());
}

#[test]
#[ignore = "needs helper and repository crash/routing fixtures"]
fn explicit_policy_blocks_native_scan_and_retry_without_changing_cached_failures() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("plughost-test-crash-on-scan.vst3");
    copy(&plugins().join("plughost-test-crash-on-scan.vst3"), &bundle);
    let cache = directory.path().join("cache.json");
    let scanner = Scanner::new(&helper(), &cache).directories(vec![directory.path().to_owned()]);
    for _ in 0..2 {
        scanner.scan(&|_| {}).unwrap();
    }
    let saved = fs::read(&cache).unwrap();
    let blocked = scanner.policy(ScanPolicy::default().bundle(&bundle, ScanAction::Block));
    let native = AtomicUsize::new(0);
    let events = |event: &ScanEvent| {
        if let ScanEvent::Progress {
            item,
            stage: ScanStage::StartingHelper,
        } = event
            && matches!(item.target, ScanTarget::Bundle(_))
        {
            native.fetch_add(1, Ordering::Relaxed);
        }
    };
    let control = ScanControl::default();
    for scan in [
        blocked
            .discover_controlled(&control, &events)
            .bundles
            .remove(0),
        blocked
            .scan_controlled(&control, &events)
            .unwrap()
            .bundles
            .remove(0),
        blocked
            .retry_controlled(&bundle, &control, &events)
            .unwrap(),
    ] {
        assert_eq!(scan.outcome, ScanOutcome::Blocked);
        assert_eq!((scan.failures, scan.excluded), (2, true));
    }
    assert_eq!(native.load(Ordering::Relaxed), 0);
    assert_eq!(fs::read(&cache).unwrap(), saved);
    let allowed =
        blocked.policy(ScanPolicy::new(ScanAction::Block).bundle(&bundle, ScanAction::Allow));
    assert_eq!(
        allowed.scan(&|_| {}).unwrap().bundles[0].source,
        Source::Cache
    );
    let retried = allowed.retry(&bundle).unwrap();
    assert_eq!(retried.source, Source::Loaded);
    assert_eq!(retried.failures, 3);
}

#[test]
#[ignore = "needs helper and repository fixtures"]
fn allow_only_selection_keeps_unselected_modules_unloaded() {
    let directory = tempfile::tempdir().unwrap();
    let plugins = plugins();
    let selected = plugins.join("plughost-test-routing.vst3");
    let policy = ScanPolicy::new(ScanAction::Block)
        .bundle(&selected, ScanAction::Block)
        .bundle(&selected, ScanAction::Allow);
    let catalog = Scanner::new(&helper(), &directory.path().join("cache.json"))
        .directories(vec![plugins])
        .policy(policy)
        .scan(&|_| {})
        .unwrap();
    assert!(catalog.bundles.len() > 2);
    for scan in &catalog.bundles {
        if scan.path == selected {
            assert_eq!(scan.source, Source::Loaded);
            assert_eq!(classes(&scan.outcome)[0].vendor, "plughost");
        } else {
            assert_eq!(scan.outcome, ScanOutcome::Blocked);
            assert_eq!((scan.failures, scan.excluded), (0, false));
        }
    }
    assert_eq!(catalog.plugins().len(), 1);
    assert!(matches!(catalog.audio_units, ScanOutcome::Found(ref items) if items.is_empty()));
}

#[test]
#[ignore = "needs helper and routing VST3 fixture"]
fn file_changes_invalidate_native_results_without_rewriting_discovery_cache() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("routing.vst3");
    let original = plugins().join("plughost-test-routing.vst3");
    #[cfg(target_os = "macos")]
    copy(&original, &bundle);
    #[cfg(target_os = "windows")]
    {
        let native = bundle.join("Contents/x86_64-win/routing.vst3");
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        copy(&original, &native);
    }
    let cache = directory.path().join("cache.json");
    let scanner = Scanner::new(&helper(), &cache).directories(vec![directory.path().to_owned()]);
    scanner.scan(&|_| {}).unwrap();
    #[cfg(target_os = "macos")]
    let executable = bundle.join("Contents/MacOS/plughost-test-routing");
    #[cfg(target_os = "windows")]
    let executable = bundle.join("Contents/x86_64-win/routing.vst3");
    let plist = bundle.join("Contents/Info.plist");
    let metadata = bundle.join("Contents/Resources/moduleinfo.json");
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    for file in [&executable, &plist, &metadata] {
        let before = fs::read(&cache).unwrap();
        if file.exists() {
            let modified = fs::metadata(file).unwrap().modified().unwrap() + Duration::from_secs(2);
            fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        } else {
            fs::write(
                file,
                if file == &metadata {
                    METADATA
                } else {
                    "fixture"
                },
            )
            .unwrap();
        }
        assert_ne!(scanner.discover().bundles[0].source, Source::Cache);
        assert_eq!(fs::read(&cache).unwrap(), before);
        let scan = scanner.scan(&|_| {}).unwrap().bundles.remove(0);
        assert_eq!(scan.source, Source::Loaded);
        assert_eq!(classes(&scan.outcome)[0].class_id, ROUTING_ID);
        assert_eq!(scanner.discover().bundles[0].source, Source::Cache);
    }
    fs::remove_file(&metadata).unwrap();
    assert_eq!(
        scanner.discover().bundles[0].outcome,
        ScanOutcome::NotInspected
    );
    assert_eq!(
        scanner.scan(&|_| {}).unwrap().bundles[0].source,
        Source::Loaded
    );
    fs::remove_file(&executable).unwrap();
    assert_eq!(
        scanner.scan(&|_| {}).unwrap().bundles[0].outcome,
        ScanOutcome::InvalidBundle
    );
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    assert!(saved["bundles"].as_object().unwrap().is_empty());
}

#[test]
#[ignore = "needs helper and crash-on-scan fixture"]
fn changed_binary_resets_automatic_failure_exclusion() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("plughost-test-crash-on-scan.vst3");
    copy(&plugins().join("plughost-test-crash-on-scan.vst3"), &bundle);
    let scanner = Scanner::new(&helper(), &directory.path().join("cache.json"))
        .directories(vec![directory.path().to_owned()]);
    for _ in 0..2 {
        scanner.scan(&|_| {}).unwrap();
    }
    assert!(scanner.scan(&|_| {}).unwrap().bundles[0].excluded);
    #[cfg(target_os = "macos")]
    let executable = bundle.join("Contents/MacOS/plughost-test-crash-on-scan");
    #[cfg(target_os = "windows")]
    let executable = bundle;
    let modified = fs::metadata(&executable).unwrap().modified().unwrap() + Duration::from_secs(2);
    fs::File::options()
        .write(true)
        .open(&executable)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let changed = scanner.scan(&|_| {}).unwrap().bundles.remove(0);
    assert_eq!(changed.source, Source::Loaded);
    assert_eq!((changed.failures, changed.excluded), (1, false));
}
