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
