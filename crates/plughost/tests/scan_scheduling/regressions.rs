//! Cache identity and failures before any plugin code can run.
use super::*;
use plughost::{Failure, FailureKind, Source};

pub(super) fn run(only: Option<&str>) {
    if only.is_none_or(|name| name == "retry-timeout") {
        retry_does_not_inherit_the_ordinary_scan_deadline();
    }
    if only.is_none_or(|name| name == "cache-version") {
        compatible_cache_survives_library_upgrades();
    }
    if only.is_none_or(|name| name == "helper-start") {
        helper_start_failures_preserve_bundle_history();
    }
    if only.is_none_or(|name| name == "protocol-version") {
        helpers_of_another_version_report_both_protocol_versions();
    }
    if only.is_none_or(|name| name == "path-encoding") {
        unsupported_bundle_paths_do_not_poison_other_scans_or_start_a_chain();
    }
    #[cfg(unix)]
    if only.is_none_or(|name| name == "cached-header") {
        unchanged_native_cache_does_not_read_the_executable();
    }
    #[cfg(target_os = "macos")]
    if only.is_none_or(|name| name == "parallel-registry") {
        audio_unit_registry_overlaps_a_blocked_bundle_within_the_job_budget();
    }
}

fn retry_does_not_inherit_the_ordinary_scan_deadline() {
    let directory = Directory::new();
    let path = bundle(directory.path(), "retry");
    fs::write(directory.path().join("release"), []).unwrap();
    let scanner = directory.scanner().stall_timeout(Duration::ZERO);
    assert!(matches!(
        scanner.retry(&path).unwrap().outcome,
        ScanOutcome::Found(_)
    ));
    assert!(
        path.with_extension("started").exists(),
        "the helper must have received native work"
    );
}

#[cfg(unix)]
fn unchanged_native_cache_does_not_read_the_executable() {
    use std::os::unix::fs::PermissionsExt;
    let directory = Directory::new();
    let path = bundle(directory.path(), "a");
    fs::write(directory.path().join("release"), []).unwrap();
    let scanner = directory.scanner();
    let original = scanner.retry(&path).unwrap();
    let executable = path.join("Contents/MacOS/fixture");
    let permissions = fs::metadata(&executable).unwrap().permissions();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        fs::File::open(&executable).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let discovered = scanner.discover();
    let scanned = scanner.scan(&|_| {}).unwrap();
    fs::set_permissions(&executable, permissions).unwrap();
    for catalog in [discovered, scanned] {
        assert_eq!(catalog.bundles[0].source, Source::Cache);
        assert_eq!(catalog.bundles[0].outcome, original.outcome);
    }
    // A changed stamp must inspect the header again instead of trusting cached success.
    fs::write(executable, b"no longer a native executable").unwrap();
    assert_eq!(
        scanner.discover().bundles[0].outcome,
        ScanOutcome::InvalidBundle
    );
}

#[cfg(target_os = "macos")]
fn audio_unit_registry_overlaps_a_blocked_bundle_within_the_job_budget() {
    use plughost::{ScanControl, ScanEvent, ScanStage, ScanTarget};
    use std::sync::mpsc;
    let directory = Directory::new();
    let path = bundle(directory.path(), "a");
    let scanner = directory.fault("registry").jobs(2);
    bundle(directory.path(), "b");
    let control = ScanControl::default();
    let worker_control = control.clone();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        scanner
            .scan_controlled(&worker_control, &|event| {
                if let ScanEvent::Progress {
                    item,
                    stage: ScanStage::Loading,
                } = event
                {
                    sent.send(item.target.clone()).unwrap();
                }
            })
            .unwrap()
    });
    let first = received.recv_timeout(Duration::from_secs(3)).ok();
    let second = received.recv_timeout(Duration::from_secs(3)).ok();
    control.cancel();
    let result = worker.join().unwrap();
    assert!(
        received.try_recv().is_err(),
        "more than two active helper jobs"
    );
    let started = [first, second];
    assert!(started.contains(&Some(ScanTarget::Bundle(path))));
    assert!(
        started.contains(&Some(ScanTarget::AudioUnits)),
        "registry was serialized after bundles"
    );
    assert_eq!(result.bundles[0].outcome, ScanOutcome::Cancelled);
    assert_eq!(result.audio_units, ScanOutcome::Cancelled);
}

fn helpers_of_another_version_report_both_protocol_versions() {
    use plughost::{Chain, Error, HostIdentity, PluginFormat, PluginRef, Timeouts};
    let directory = Directory::new();
    let root = directory.path();
    let path = bundle(root, "a");
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        bundle: Some(path.clone()),
        class_id: "fixture".into(),
    };
    let scanner = directory.fault("protocol");
    let mismatch = Error::ProtocolVersion {
        application: PROTOCOL_VERSION,
        helper: PROTOCOL_VERSION + 1,
    };
    assert_eq!(
        scanner.retry(&path).unwrap().outcome,
        ScanOutcome::Failed(Failure::new(FailureKind::Protocol, mismatch.to_string()))
    );
    let helper = root.join(if cfg!(windows) {
        "helper.exe"
    } else {
        "helper"
    });
    let result = Chain::spawn(
        &helper,
        &[plugin],
        &HostIdentity::default(),
        Timeouts::default(),
    );
    assert!(
        matches!(
            result,
            Err(Error::ProtocolVersion { application, helper })
                if application == PROTOCOL_VERSION && helper == PROTOCOL_VERSION + 1
        ),
        "{result:?}"
    );
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
            error: InputError::PathEncoding
        })
    ));
    fs::write(root.join("release"), []).unwrap();
    let scanner = directory.scanner();
    let bad = scanner.retry(&invalid).unwrap();
    assert_eq!(
        bad.outcome,
        ScanOutcome::Failed(Failure::new(
            FailureKind::InvalidInput,
            InputError::PathEncoding.to_string()
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
