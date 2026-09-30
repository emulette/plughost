use super::*;
use plughost::{ScanControl, ScanEvent, ScanItem, ScanStage, ScanTarget};
use std::sync::{Arc, Mutex};

impl Directory {
    fn fault(&self, fault: &str) -> Scanner {
        let helper = self.path().join(if cfg!(windows) {
            "helper.exe"
        } else {
            "helper"
        });
        fs::copy(std::env::current_exe().unwrap(), &helper).unwrap();
        fs::write(self.path().join("fault"), fault).unwrap();
        Scanner::new(&helper, &self.path().join("cache.json"))
            .directories(vec![self.path().to_owned()])
    }
}
fn started(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(pid) = text.parse()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "helper never reached {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn reaped(pid: u32) {
    #[cfg(unix)]
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap()
            .status
            .success()
    );
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: query-only access to the fixture child; close the returned owned handle once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !handle.is_null() {
                let mut code = 259;
                assert_ne!(GetExitCodeProcess(handle, &mut code), 0);
                CloseHandle(handle);
                assert_ne!(code, 259);
            }
        }
    }
}
fn cancellation_during_boot(fault: &str) {
    let directory = Directory::new();
    bundle(directory.path(), "a");
    let scanner = directory.fault(fault);
    let control = ScanControl::default();
    let worker_control = control.clone();
    let worker = thread::spawn(move || scanner.scan_controlled(&worker_control, &|_| {}).unwrap());
    let pid = started(&directory.path().join("boot.started"));
    let before = Instant::now();
    control.cancel();
    let result = worker.join().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(result.bundles[0].outcome, ScanOutcome::Cancelled);
    assert_eq!(result.bundles[0].failures, 0);
    reaped(pid);
}
fn cancellation_stops_parallel_admissions() {
    let directory = Directory::new();
    for name in ["a", "b", "c"] {
        bundle(directory.path(), name);
    }
    let scanner = directory.scanner().jobs(2);
    let control = ScanControl::default();
    let worker_control = control.clone();
    let worker = thread::spawn(move || scanner.scan_controlled(&worker_control, &|_| {}).unwrap());
    let pids = [
        started(&directory.path().join("a.started")),
        started(&directory.path().join("b.started")),
    ];
    let before = Instant::now();
    control.cancel();
    let result = worker.join().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(result.bundles.len(), 2);
    assert!(
        result
            .bundles
            .iter()
            .all(|scan| scan.outcome == ScanOutcome::Cancelled
                && scan.failures == 0
                && !scan.excluded)
    );
    assert!(!directory.path().join("c.started").exists());
    assert_eq!(result.audio_units, ScanOutcome::Cancelled);
    assert!(!directory.path().join("cache.json").exists());
    for pid in pids {
        reaped(pid);
    }
}
fn skip_only_the_current_item() {
    let directory = Directory::new();
    let first = bundle(directory.path(), "a");
    bundle(directory.path(), "b");
    fs::write(directory.path().join("b.release"), []).unwrap();
    let scanner = directory.scanner();
    let current: Arc<Mutex<Option<ScanItem>>> = Arc::default();
    let stored = current.clone();
    let stages = Arc::new(Mutex::new(Vec::new()));
    let recorded = stages.clone();
    let worker = thread::spawn(move || {
        scanner
            .scan_controlled(&ScanControl::default(), &|event| {
                if let ScanEvent::Progress { item, stage } = event {
                    if item.target == ScanTarget::Bundle(first.clone()) {
                        *stored.lock().unwrap() = Some(item.clone());
                    } else if let Some(previous) = stored.lock().unwrap().as_ref() {
                        previous.skip();
                    }
                    recorded.lock().unwrap().push(format!("{stage:?}"));
                }
            })
            .unwrap()
    });
    let pid = started(&directory.path().join("a.started"));
    let before = Instant::now();
    current.lock().unwrap().as_ref().unwrap().skip();
    let catalog = worker.join().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(catalog.bundles[0].outcome, ScanOutcome::Skipped);
    assert!(matches!(catalog.bundles[1].outcome, ScanOutcome::Found(_)));
    let stages = stages.lock().unwrap();
    for stage in ["Inspecting", "StartingHelper", "Loading", "ModuleLoaded"] {
        assert!(stages.iter().any(|seen| seen == stage));
    }
    reaped(pid);
}
fn retry_cancellation_preserves_cache() {
    let directory = Directory::new();
    let path = bundle(directory.path(), "a");
    fs::write(directory.path().join("release"), []).unwrap();
    directory.scanner().scan(&|_| {}).unwrap();
    let before = fs::read(directory.path().join("cache.json")).unwrap();
    fs::remove_file(directory.path().join("release")).unwrap();
    fs::remove_file(directory.path().join("a.started")).unwrap();
    let control = ScanControl::default();
    let worker_control = control.clone();
    let scanner = directory.scanner();
    let worker = thread::spawn(move || {
        scanner
            .retry_controlled(&path, &worker_control, &|_| {})
            .unwrap()
    });
    let pid = started(&directory.path().join("a.started"));
    let time = Instant::now();
    control.cancel();
    let scan = worker.join().unwrap();
    assert!(time.elapsed() < Duration::from_secs(1));
    assert_eq!(scan.outcome, ScanOutcome::Cancelled);
    assert_eq!(
        fs::read(directory.path().join("cache.json")).unwrap(),
        before
    );
    reaped(pid);
}
fn cancellation_before_admission_and_from_callback() {
    let directory = Directory::new();
    bundle(directory.path(), "a");
    let control = ScanControl::default();
    control.cancel();
    let catalog = directory
        .scanner()
        .scan_controlled(&control, &|event| {
            assert!(!matches!(event, ScanEvent::Progress { .. }));
        })
        .unwrap();
    assert!(catalog.bundles.is_empty());
    assert!(!directory.path().join("cache.json").exists());
    let control = ScanControl::default();
    let catalog = directory.scanner().discover_controlled(&control, &|event| {
        if matches!(
            event,
            ScanEvent::Progress {
                stage: ScanStage::Inspecting,
                ..
            }
        ) {
            control.cancel();
        }
    });
    assert_eq!(catalog.bundles[0].outcome, ScanOutcome::Cancelled);
    assert!(!directory.path().join("a.started").exists());
    assert!(!directory.path().join("cache.json").exists());
}
#[cfg(target_os = "macos")]
fn registry_cancellation() {
    let directory = Directory::new();
    let scanner = directory.fault("registry").directories(Vec::new());
    let control = ScanControl::default();
    let worker_control = control.clone();
    let worker = thread::spawn(move || scanner.discover_controlled(&worker_control, &|_| {}));
    let pid = started(&directory.path().join("registry.started"));
    let before = Instant::now();
    control.cancel();
    assert_eq!(worker.join().unwrap().audio_units, ScanOutcome::Cancelled);
    assert!(before.elapsed() < Duration::from_secs(1));
    reaped(pid);
}
pub(super) fn run() {
    cancellation_before_admission_and_from_callback();
    for phase in ["connect", "hello", "closed"] {
        cancellation_during_boot(phase);
    }
    cancellation_stops_parallel_admissions();
    skip_only_the_current_item();
    retry_cancellation_preserves_cache();
    #[cfg(target_os = "macos")]
    registry_cancellation();
}
