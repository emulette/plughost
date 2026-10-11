use super::*;

#[test]
#[ignore = "needs helper and native CLAP delay fixture"]
#[allow(clippy::print_stdout)]
fn cached_bundle_benchmark() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let helper = target.join(if cfg!(target_os = "macos") {
        "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
    } else {
        "helper/plughost-helper.exe"
    });
    let bundle = target.join("test-plugins/plughost-test-delay.clap");
    let directory = tempfile::tempdir().unwrap();
    let cache_path = directory.path().join("cache.json");
    let scanner = Scanner::new(&helper, &cache_path);
    let expected = scanner.retry(&bundle).unwrap().outcome;
    assert!(matches!(&expected, ScanOutcome::Found(classes) if classes.len() == 1));
    let cache = cache::load(&cache_path);
    let cached = cache.bundles.get(&bundle);
    let control = ScanControl::default();
    let work = control
        .begin(ScanTarget::Bundle(bundle.clone()), &|_| {})
        .unwrap();
    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        let (scan, _) = scanner.scan_bundle(&bundle, cached, ScanMode::Metadata, &work);
        assert_eq!(scan.source, Source::Cache);
        assert_eq!(scan.outcome, expected);
    }
    println!("10,000 cached bundle inspections: {:?}", start.elapsed());
}
