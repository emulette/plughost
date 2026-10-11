//! Cache identity and failures before any plugin code can run.
use super::*;
use plughost::{Failure, FailureKind, Source};

pub(super) fn run(only: Option<&str>) {
    if only.is_none_or(|name| name == "cache-version") {
        compatible_cache_survives_library_upgrades();
    }
    if only.is_none_or(|name| name == "helper-start") {
        helper_start_failures_preserve_bundle_history();
    }
    if only.is_none_or(|name| name == "path-encoding") {
        unsupported_bundle_paths_do_not_poison_other_scans_or_start_a_chain();
    }
}

fn unsupported_bundle_paths_do_not_poison_other_scans_or_start_a_chain() {
    use plughost::{Chain, Error, HostIdentity, InputError, PluginFormat, PluginRef, Timeouts};
    let directory = Directory::new();
    let root = directory.path();
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"invalid-\xFF.clap".to_vec())
    };
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xD800, 46, 99, 108, 97, 112])
    };
    let invalid = root.join(name);
    assert!(invalid.to_str().is_none());
    // NTFS accepts lone UTF-16 surrogates. APFS rejects invalid UTF-8 names itself, so macOS
    // exercises the same request boundary with a PathBuf that cannot exist on that filesystem.
    #[cfg(windows)]
    fs::rename(bundle(root, "temporary"), &invalid).unwrap();
    let valid = bundle(root, "valid");
    let plugins: Vec<_> = [&valid, &invalid]
        .into_iter()
        .map(|path| PluginRef {
            format: PluginFormat::Clap,
            bundle: Some(path.clone()),
            class_id: "fixture".into(),
        })
        .collect();
    assert!(matches!(
        Chain::spawn(
            &root.join("missing-helper"),
            &plugins,
            &HostIdentity::default(),
            Timeouts::default()
        ),
        Err(Error::Input {
            slot: Some(1),
            error: InputError::BundlePathEncoding
        })
    ));
    fs::write(root.join("release"), []).unwrap();
    let scanner = directory.scanner();
    let bad = scanner.retry(&invalid).unwrap();
    assert_eq!(
        bad.outcome,
        ScanOutcome::Failed(Failure::new(
            FailureKind::InvalidInput,
            InputError::BundlePathEncoding.to_string()
        ))
    );
    assert_eq!(bad.failures, 0);
    assert!(!bad.excluded);
    for (catalog, source) in [
        (scanner.scan(&|_| {}).unwrap(), Source::Loaded),
        (scanner.scan(&|_| {}).unwrap(), Source::Cache),
    ] {
        assert_eq!(catalog.bundles.len(), if cfg!(windows) { 2 } else { 1 });
        let good = catalog
            .bundles
            .iter()
            .find(|scan| scan.path == valid)
            .unwrap();
        assert_eq!(good.source, source);
        assert!(matches!(good.outcome, ScanOutcome::Found(_)));
        #[cfg(windows)]
        assert_eq!(
            catalog
                .bundles
                .iter()
                .find(|scan| scan.path == invalid)
                .unwrap(),
            &bad
        );
    }
    let cache: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("cache.json")).unwrap()).unwrap();
    assert_eq!(cache["bundles"].as_object().unwrap().len(), 1);
}

fn compatible_cache_survives_library_upgrades() {
    let directory = Directory::new();
    let root = directory.path();
    let path = bundle(root, "a");
    fs::write(root.join("release"), []).unwrap();
    let scanner = directory.scanner();
    scanner.retry(&path).unwrap();
    let cache = root.join("cache.json");
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    snapshot["library"] = "0.0.0".into();
    snapshot["bundles"][path.to_str().unwrap()]["failures"] = 2.into();
    snapshot["bundles"][path.to_str().unwrap()]["outcome"] = serde_json::to_value(
        ScanOutcome::Failed(Failure::new(FailureKind::Load, "fixture load failure")),
    )
    .unwrap();
    fs::write(&cache, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let discovered = scanner.discover();
    assert_eq!(discovered.bundles.len(), 1);
    assert_eq!(discovered.bundles[0].source, Source::Cache);
    assert_eq!(discovered.bundles[0].failures, 2);
    assert!(discovered.bundles[0].excluded);
    let scanned = scanner.scan(&|_| {}).unwrap();
    assert_eq!(scanned.bundles[0], discovered.bundles[0]);

    for field in ["schema", "architecture"] {
        let mut incompatible = snapshot.clone();
        incompatible[field] = if field == "schema" {
            999.into()
        } else {
            "other-architecture".into()
        };
        fs::write(&cache, serde_json::to_vec(&incompatible).unwrap()).unwrap();
        let discovered = scanner.discover();
        assert_eq!(discovered.bundles[0].source, Source::Inspected);
        assert_eq!(discovered.bundles[0].outcome, ScanOutcome::NotInspected);
        assert_eq!(discovered.bundles[0].failures, 0);
    }
}

fn helper_start_failures_preserve_bundle_history() {
    for fault in ["missing", "wrong-hello", "exit-before-connect"] {
        let directory = Directory::new();
        let root = directory.path();
        let prior = bundle(root, "a");
        fs::write(root.join("release"), []).unwrap();
        directory.scanner().retry(&prior).unwrap();
        let cache = root.join("cache.json");
        let mut snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
        snapshot["bundles"][prior.to_str().unwrap()]["failures"] = 1.into();
        snapshot["bundles"][prior.to_str().unwrap()]["outcome"] = serde_json::to_value(
            ScanOutcome::Failed(Failure::new(FailureKind::Load, "fixture load failure")),
        )
        .unwrap();
        fs::write(&cache, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        bundle(root, "b");
        let scanner = if fault == "missing" {
            Scanner::new(&root.join("missing-helper"), &cache).directories(vec![root.to_path_buf()])
        } else {
            directory.fault(fault)
        };
        for _ in 0..2 {
            let result = scanner.scan(&|_| {}).unwrap();
            assert_eq!(result.bundles.len(), 2);
            assert_eq!(result.bundles[0].failures, 1, "{fault}");
            assert_eq!(result.bundles[1].failures, 0, "{fault}");
            assert!(result.bundles.iter().all(|scan| !scan.excluded));
            if fault == "missing" {
                let cause = std::process::Command::new(root.join("missing-helper"))
                    .spawn()
                    .unwrap_err()
                    .to_string();
                let ScanOutcome::Failed(failure) = &result.bundles[0].outcome else {
                    panic!("missing helper must report an I/O failure")
                };
                assert_eq!(
                    failure.message,
                    format!("could not start the helper: {cause}")
                );
            }
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&fs::read(&cache).unwrap()).unwrap(),
                snapshot
            );
        }
        let recovered = directory.scanner().scan(&|_| {}).unwrap();
        assert!(
            recovered
                .bundles
                .iter()
                .all(|scan| scan.source == Source::Loaded
                    && matches!(scan.outcome, ScanOutcome::Found(_))
                    && scan.failures == 0)
        );
    }
}
