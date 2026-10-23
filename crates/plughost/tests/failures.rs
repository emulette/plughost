use plughost::{Chain, Error, FailureKind, Layout, PluginFormat, PluginRef};

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
fn a_reset_that_fails_partway_leaves_the_chain_to_be_prepared_again() {
    let failing = delay_variant(PluginFormat::Vst3, "reset-failure", 0x1C);
    let mut chain = spawn(&[delay(PluginFormat::Vst3), failing]);
    prepare(&mut chain);
    support::block(&mut chain, 512, 0.25);
    // The first slot is reset natively before the second fails to activate again.
    check(chain.reset().unwrap_err(), FailureKind::Configuration, 1);
    // Their alignment history no longer matches, so the chain processes nothing, rather than
    // the first slot's reset output with the old delays.
    let input = [0.25f32; 512];
    let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
    let result = chain.process_audio_f32(
        &plughost::BlockContext::new(512),
        &[&input, &input],
        &mut [&mut left, &mut right],
        &[],
        &[],
        &mut Vec::new(),
    );
    match result {
        Err(Error::Operation {
            slot: None,
            failure,
        }) => assert_eq!(failure.kind, FailureKind::NotPrepared),
        other => panic!("{other:?}"),
    }
    // An edit of the first slot takes effect even though the second one's timing cannot be read.
    chain.set_parameter(0, 0, 0.5).unwrap();
    assert_eq!(chain.parameters(0).unwrap()[0].1, 0.5);
    chain.reprepare().unwrap();
    support::block(&mut chain, 512, 0.25);
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
            .component()
            .is_empty()
    );
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn format_errors_keep_their_category_and_slot_through_ipc() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let plugin = delay(format);
        let bundle = plugin.bundle().unwrap();
        let unknown = "00000000000000000000000000000000";
        let wrong = match format {
            PluginFormat::Vst3 => PluginRef::vst3(bundle, unknown),
            _ => PluginRef::clap(bundle, unknown),
        };
        let error = match Chain::spawn(
            &helper(),
            &[wrong.into()],
            &plughost::SpawnOptions::default(),
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
        let original = chain
            .save_state(1, plughost::StatePurpose::Project)
            .unwrap();
        let other = plughost::PluginInfo::new(
            plughost::PluginId::new(original.id().format(), "different-plugin"),
            "",
            plughost::PluginKind::Effect,
        );
        let state = plughost::PluginState::new(
            &other,
            original.component().to_vec(),
            original.controller().map(<[u8]>::to_vec),
        );
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
