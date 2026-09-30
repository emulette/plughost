//! Persistence is observed through scanner callbacks and separate scanner processes.
use super::*;
use plughost::{ScanControl, ScanEvent};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn run() {
    completion_is_persisted_before_notification();
    abrupt_exit_keeps_progress_and_releases_writer_lock();
    interrupted_snapshot_write_preserves_the_previous_cache();
    parallel_completions_publish_valid_snapshots();
    another_process_cannot_overwrite_an_active_writer();
    #[cfg(unix)]
    failed_save_aborts_active_helpers_without_losing_previous_results();
}

fn completion_is_persisted_before_notification() {
    let directory = Directory::new();
    let root = directory.path();
    let plugins = root.join("plugins");
    fs::create_dir(&plugins).unwrap();
    let old = bundle(root, "old");
    fs::write(old.with_extension("release"), []).unwrap();
    let first = bundle(&plugins, "a");
    let second = bundle(&plugins, "b");
    fs::write(first.with_extension("release"), []).unwrap();
    let cache = root.join("cache.json");
    let scanner =
        Scanner::new(&std::env::current_exe().unwrap(), &cache).directories(vec![plugins]);
    scanner.retry(&old).unwrap();
    let durable = AtomicBool::new(false);
    let control = ScanControl::default();
    let result = scanner
        .scan_controlled(&control, &|event| {
            if let ScanEvent::BundleFinished(scan) = event
                && scan.path == first
            {
                let saved: serde_json::Value =
                    serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
                durable.store(
                    saved["bundles"].get(first.to_str().unwrap()).is_some()
                        && saved["bundles"].get(old.to_str().unwrap()).is_some(),
                    Ordering::Relaxed,
                );
                control.cancel();
            }
        })
        .unwrap();
    let has_unwanted_work = second.with_extension("started").exists();
    assert_eq!(result.bundles.len(), 1);
    assert!(!has_unwanted_work);
    assert!(
        durable.load(Ordering::Relaxed),
        "completed entry must reach the cache before its notification"
    );
}

impl Directory {
    fn read(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.path().join("cache.json")).unwrap()).unwrap()
    }
}
fn wait(path: &Path) -> u32 {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(pid) = text.parse()
        {
            return pid;
        }
        assert!(
            Instant::now() < until,
            "helper did not reach its controlled gate"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn child(mode: &str, root: &Path) {
    let scanner = Scanner::new(&std::env::current_exe().unwrap(), &root.join("cache.json"))
        .directories(vec![root.to_path_buf()]);
    match mode {
        "exit" => {
            scanner.scan(&|_| std::process::exit(23)).unwrap();
            panic!("expected completion callback");
        }
        "busy" => {
            let result = scanner.retry(&root.join("b.clap"));
            assert!(
                matches!(result, Err(plughost::Error::Cache(error)) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
        "large-write" => {
            scanner.retry(&root.join("b.clap")).unwrap();
        }
        _ => panic!("unknown cache child mode"),
    }
}

fn interrupted_snapshot_write_preserves_the_previous_cache() {
    let directory = Directory::new();
    let first = bundle(directory.path(), "a");
    let second = bundle(directory.path(), "b");
    fs::write(directory.path().join("release"), []).unwrap();
    directory.scanner().retry(&first).unwrap();
    let before = fs::read(directory.path().join("cache.json")).unwrap();
    fs::write(directory.path().join("large-class"), []).unwrap();
    let mut child = child_command("large-write", directory.path())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let temporary = loop {
        let pending = fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".plughost-cache-")
                    // Windows directory entries report a stale size while the file is being written.
                    && fs::metadata(entry.path()).is_ok_and(|metadata| metadata.len() > 0)
            });
        if let Some(pending) = pending {
            break pending.path();
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "scanner finished before the interrupted write was observed"
        );
        assert!(Instant::now() < deadline, "snapshot write never started");
        thread::sleep(Duration::from_millis(1));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        fs::read(directory.path().join("cache.json")).unwrap(),
        before
    );
    assert!(temporary.exists());
    fs::remove_file(directory.path().join("large-class")).unwrap();
    directory.scanner().retry(&second).unwrap();
    assert_eq!(directory.read()["bundles"].as_object().unwrap().len(), 2);
    // An orphan temporary is not a published result and is never interpreted as one.
    assert!(temporary.exists());
}
fn child_command(mode: &str, root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.arg("cache-child").arg(mode).arg(root);
    command
}
fn abrupt_exit_keeps_progress_and_releases_writer_lock() {
    let directory = Directory::new();
    let first = bundle(directory.path(), "a");
    let second = bundle(directory.path(), "b");
    fs::write(first.with_extension("release"), []).unwrap();
    let output = child_command("exit", directory.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let saved = directory.read();
    assert!(saved["bundles"].get(first.to_str().unwrap()).is_some());
    assert!(saved["bundles"].get(second.to_str().unwrap()).is_none());
    assert!(!second.with_extension("started").exists());
    // The abrupt exit ran no Rust destructors. A new process can still acquire the cache lock.
    fs::write(second.with_extension("release"), []).unwrap();
    let retried = directory.scanner().retry(&second).unwrap();
    assert!(matches!(retried.outcome, ScanOutcome::Found(_)));
    assert_eq!(directory.read()["bundles"].as_object().unwrap().len(), 2);
    assert_eq!(
        directory.scanner().discover().bundles[0].source,
        plughost::Source::Cache
    );
}
fn parallel_completions_publish_valid_snapshots() {
    use std::sync::atomic::AtomicUsize;
    let directory = Directory::new();
    for index in 0..8 {
        bundle(directory.path(), &format!("{index}"));
    }
    fs::write(directory.path().join("release"), []).unwrap();
    let completed = AtomicUsize::new(0);
    let catalog = directory
        .scanner()
        .jobs(4)
        .scan(&|scan| {
            let snapshot = directory.read();
            assert!(
                snapshot["bundles"]
                    .get(scan.path.to_str().unwrap())
                    .is_some()
            );
            assert_eq!(snapshot["schema"], 2);
            completed.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    assert_eq!(catalog.bundles.len(), 8);
    assert_eq!(completed.load(Ordering::Relaxed), 8);
    assert_eq!(directory.read()["bundles"].as_object().unwrap().len(), 8);
    assert!(!fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".plughost-cache-")
    }));
}
fn another_process_cannot_overwrite_an_active_writer() {
    let directory = Directory::new();
    let first = bundle(directory.path(), "a");
    let second = bundle(directory.path(), "b");
    fs::write(second.with_extension("release"), []).unwrap();
    let control = ScanControl::default();
    let worker_control = control.clone();
    let scanner = directory.scanner();
    let worker = thread::spawn(move || scanner.scan_controlled(&worker_control, &|_| {}).unwrap());
    wait(&first.with_extension("started"));
    let began = Instant::now();
    let competing = child_command("busy", directory.path()).output().unwrap();
    let elapsed = began.elapsed();
    let started_second = second.with_extension("started").exists();
    // Read-only discovery remains available while the native writer is in a stalled plugin.
    let discovered = directory.scanner().discover();
    control.cancel();
    worker.join().unwrap();
    assert!(competing.status.success(), "{competing:?}");
    assert!(elapsed < Duration::from_secs(1));
    assert!(!started_second);
    assert!(
        discovered
            .bundles
            .iter()
            .all(|scan| scan.outcome == ScanOutcome::NotInspected)
    );
    assert!(!directory.path().join("cache.json").exists());
    directory.scanner().retry(&second).unwrap();
}
#[cfg(unix)]
fn failed_save_aborts_active_helpers_without_losing_previous_results() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    let directory = Directory::new();
    let plugins = directory.path().join("plugins");
    fs::create_dir(&plugins).unwrap();
    for name in ["a", "b", "c", "d"] {
        bundle(&plugins, name);
    }
    for name in ["a", "c", "d"] {
        fs::write(plugins.join(format!("{name}.release")), []).unwrap();
    }
    let snapshot = Mutex::new(Vec::new());
    let notified = Mutex::new(Vec::new());
    let control = ScanControl::default();
    let permissions = fs::metadata(directory.path()).unwrap().permissions();
    let scanner = directory
        .scanner()
        .directories(vec![plugins.clone()])
        .jobs(2);
    let start = Instant::now();
    let result = scanner.scan_controlled(&control, &|event| {
        if let ScanEvent::BundleFinished(scan) = event {
            notified.lock().unwrap().push(scan.path.clone());
            if scan.path == plugins.join("a.clap") {
                wait(&plugins.join("b.started"));
                *snapshot.lock().unwrap() = fs::read(directory.path().join("cache.json")).unwrap();
                fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
            }
        }
    });
    fs::set_permissions(directory.path(), permissions).unwrap();
    assert!(
        matches!(result, Err(plughost::Error::Cache(_))),
        "{result:?}"
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        !control.is_cancelled(),
        "internal cache abort must not mutate the caller's control"
    );
    assert_eq!(
        fs::read(directory.path().join("cache.json")).unwrap(),
        *snapshot.lock().unwrap()
    );
    assert_eq!(*notified.lock().unwrap(), [plugins.join("a.clap")]);
    assert!(!plugins.join("d.started").exists());
    let pid = wait(&plugins.join("b.started"));
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Only the failed item is retried; the earlier cached result is preserved.
    let retried = scanner.retry(&plugins.join("c.clap")).unwrap();
    assert!(matches!(retried.outcome, ScanOutcome::Found(_)));
    assert_eq!(directory.read()["bundles"].as_object().unwrap().len(), 2);
}
