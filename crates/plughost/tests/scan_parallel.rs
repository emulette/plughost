//! Repeated uncached native scans after warming the plugin binary and system registry.
use plughost::{ScanOutcome, Scanner};
use std::path::Path;

#[test]
#[ignore = "needs helper and native CLAP delay fixture"]
#[allow(clippy::print_stdout)]
fn native_scan_scheduling_benchmark() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let helper = target.join(if cfg!(target_os = "macos") {
        "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
    } else {
        "helper/plughost-helper.exe"
    });
    let fixture = target.join("test-plugins/plughost-test-delay.clap");
    let directory = tempfile::tempdir().unwrap();
    let bundle = directory.path().join("delay.clap");
    #[cfg(target_os = "macos")]
    std::os::unix::fs::symlink(fixture, &bundle).unwrap();
    #[cfg(target_os = "windows")]
    std::fs::copy(fixture, &bundle).unwrap();
    let cache = directory.path().join("cache.json");
    let mut scanner = Scanner::new(&helper, &cache).directories(vec![directory.path().into()]);
    let expected = scanner.scan(&|_| {}).unwrap();
    assert_eq!(expected.bundles.len(), 1);
    assert!(
        matches!(&expected.bundles[0].outcome, ScanOutcome::Found(classes) if classes.len() == 1)
    );
    for jobs in [1, 2] {
        scanner = scanner.jobs(jobs);
        let start = std::time::Instant::now();
        for _ in 0..30 {
            std::fs::remove_file(&cache).unwrap();
            let actual = scanner.scan(&|_| {}).unwrap();
            assert_eq!(actual.bundles.len(), 1);
            assert_eq!(actual.bundles[0].outcome, expected.bundles[0].outcome);
            assert_eq!(actual.audio_units, expected.audio_units);
        }
        println!(
            "30 native bundle + registry scans, jobs {jobs}: {:?}",
            start.elapsed()
        );
    }
}
