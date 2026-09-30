use plughost::{Chain, Error, FailureKind, HostIdentity, Layout, PluginFormat, Timeouts};

mod support;

use support::{delay, delay_variant, helper, prepare, spawn};

fn check(error: Error, expected: FailureKind, slot: usize) {
    assert_eq!(error.kind(), expected);
    match error {
        Error::Operation {
            slot: reported,
            failure,
        } => {
            assert_eq!(reported, Some(slot));
            assert_eq!(failure.kind, expected);
            assert!(!failure.message.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn overflowing_chain_latency_is_reported_without_losing_the_helper() {
    let plugin = delay_variant(PluginFormat::Vst3, "latency-overflow", 7);
    let mut chain = spawn(&[plugin.clone(), plugin]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo; 2])
        .unwrap();
    // The unrepresentable sum belongs to the chain, not to one slot.
    match chain.prepare_audio(&config).unwrap_err() {
        Error::Operation {
            slot: None,
            failure,
        } => assert_eq!(failure.kind, FailureKind::Configuration),
        other => panic!("{other:?}"),
    }
    let input = [0.0; 512];
    let (mut left, mut right) = ([42.0; 512], [42.0; 512]);
    // Preparation commits only after timing validation, so a first failed prepare
    // leaves the chain unprepared instead of installing an unusable configuration.
    assert_eq!(
        chain
            .process_audio_f32(
                &plughost_core::BlockContext::new(left.len()),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap_err()
            .kind(),
        FailureKind::NotPrepared,
    );
    assert_eq!(left, [42.0; 512]);
    assert_eq!(right, left);
    // The failure belongs to the chain's unrepresentable sum, not a crashed plugin.
    assert!(
        !chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap()
            .component
            .is_empty()
    );
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn format_errors_keep_their_category_and_slot_through_ipc() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let plugin = delay(format);
        let mut wrong = plugin.clone();
        wrong.class_id = "00000000000000000000000000000000".to_owned();
        let error = match Chain::spawn(
            &helper(),
            &[wrong],
            &HostIdentity::default(),
            Timeouts::default(),
        ) {
            Ok(_) => panic!("unexpected successful load"),
            Err(error) => error,
        };
        check(error, FailureKind::NotFound, 0);
        let mut chain = spawn(&[plugin.clone(), plugin]);
        check(
            chain.open_editor(1).unwrap_err(),
            FailureKind::Unsupported,
            1,
        );
        let bad = chain
            .main_bus_config(
                48_000.0,
                512,
                Layout::Stereo,
                &[Layout::Stereo, Layout::Mono],
            )
            .unwrap();
        check(
            chain.prepare_audio(&bad).unwrap_err(),
            FailureKind::Configuration,
            1,
        );
        prepare(&mut chain);
        let mut state = chain
            .save_state(1, plughost::StatePurpose::Project)
            .unwrap();
        let original = state.clone();
        state.class_id = "different-plugin".to_owned();
        check(
            chain
                .restore_state(1, &state, plughost::StatePurpose::Project)
                .unwrap_err(),
            FailureKind::StateMismatch,
            1,
        );
        chain
            .restore_state(1, &original, plughost::StatePurpose::Project)
            .unwrap();
        assert_eq!(
            chain
                .save_state(1, plughost::StatePurpose::Project)
                .unwrap(),
            original
        );
    }
}

#[test]
#[ignore = "needs scripts/build-helper.ps1; Windows PE scan fixture"]
#[cfg(target_os = "windows")]
fn scan_failure_category_survives_disk_cache() {
    use plughost::{ScanOutcome, Scanner, Source};
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("not-a-plugin.clap");
    // A real PE executable passes architecture inspection but has no CLAP entry point.
    std::fs::copy(std::env::current_exe().unwrap(), &bundle).unwrap();
    let cache = directory.path().join("cache.json");
    let scanner = Scanner::new(&helper(), &cache).directories(vec![directory.path().into()]);
    let mut failure = None;
    for index in 0..3 {
        let catalog = scanner.scan(&|_| {}).unwrap();
        let scan = catalog
            .bundles
            .iter()
            .find(|scan| scan.path == bundle)
            .unwrap();
        let ScanOutcome::Failed(current) = &scan.outcome else {
            panic!("{:?}", scan.outcome);
        };
        assert_eq!(current.kind, FailureKind::Load);
        assert!(!current.message.is_empty());
        if let Some(previous) = &failure {
            assert_eq!(previous, current);
        }
        failure = Some(current.clone());
        if index == 2 {
            assert_eq!(scan.source, Source::Cache);
            assert!(scan.excluded);
        } else {
            assert_eq!(scan.source, Source::Loaded);
        }
    }
    // A cache from before structured failures must be rescanned, including excluded bundles.
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("schema");
    std::fs::write(&cache, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let rescanned = scanner.scan(&|_| {}).unwrap();
    let scan = rescanned
        .bundles
        .iter()
        .find(|scan| scan.path == bundle)
        .unwrap();
    assert_eq!(scan.source, Source::Loaded);
    assert!(!scan.excluded);
}
