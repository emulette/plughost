//! Real parent/helper/descendant processes; native scan hangs come from repository fixtures.
use plughost::{
    PluginFormat, ScanAction, ScanControl, ScanEvent, ScanOutcome, ScanPolicy, ScanStage,
    ScanTarget, Scanner,
};
use std::{
    fs,
    path::Path,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

mod support;

use support::{bundle, helper, routing};

/// Environment of the processes this test starts: the fixtures read the first two, and the
/// entry tests below read the last three.
const DESCENDANT: &str = "PLUGHOST_TEST_DESCENDANT";
const LIFETIME_DIR: &str = "PLUGHOST_TEST_LIFETIME_DIR";
const SCAN_FORMAT: &str = "PLUGHOST_TEST_SCAN_FORMAT";
const SCAN_ENDING: &str = "PLUGHOST_TEST_SCAN_ENDING";

/// Runs the entry test `name` of this test binary in a new process.
fn entry(name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "--ignored", "--nocapture", name]);
    command
}

/// The process a hanging fixture starts as its descendant.
#[test]
#[ignore = "subprocess entry point invoked by the scan tests"]
fn lifetime_descendant() {
    let Some(root) = std::env::var_os(LIFETIME_DIR) else {
        return;
    };
    fs::write(
        Path::new(&root).join("descendant.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    loop {
        thread::park();
    }
}

/// The application process that scans a hanging fixture and ends the scan as `ending` says.
#[test]
#[ignore = "subprocess entry point invoked by the scan tests"]
fn lifetime_scan_parent() {
    let (Some(root), Ok(format), Ok(ending)) = (
        std::env::var_os(LIFETIME_DIR),
        std::env::var(SCAN_FORMAT),
        std::env::var(SCAN_ENDING),
    ) else {
        return;
    };
    scan_parent(Path::new(&root), &format, &ending);
}

fn pid(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(value) = fs::read_to_string(path)
            && let Ok(pid) = value.parse()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "missing process marker: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn terminate(pid: u32) {
    #[cfg(target_os = "macos")]
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!process.is_null());
        assert_ne!(TerminateProcess(process, 17), 0);
        CloseHandle(process);
    }
}

fn stopped(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let status = String::from_utf8(output.stdout).unwrap();
        // Orphan descendants are reaped by launchd; zombies no longer execute or retain sockets.
        status.trim().is_empty() || status.trim().starts_with('Z')
    }
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::*,
        };
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return true;
        }
        let ended = WaitForSingleObject(process, 0) == WAIT_OBJECT_0;
        CloseHandle(process);
        ended
    }
}

fn assert_stopped(ids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while ids.iter().any(|&id| !stopped(id)) {
        if Instant::now() >= deadline {
            for &id in ids {
                if !stopped(id) {
                    terminate(id);
                }
            }
            panic!("owned process tree outlived its owner: {ids:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("parent did not finish");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn scan_parent(root: &Path, format: &str, ending: &str) {
    let control = ScanControl::default();
    let ready = root.join("descendant.pid");
    if ending == "cancel" {
        let control = control.clone();
        let ready = ready.clone();
        thread::spawn(move || {
            pid(&ready);
            control.cancel();
        });
    }
    let format = match format {
        "vst3" => PluginFormat::Vst3,
        _ => PluginFormat::Clap,
    };
    let path = bundle(format, "plughost-test-hang-on-scan");
    let scanner = Scanner::new(&helper(), &root.join("cache.json"))
        .directories(vec![path.parent().unwrap().into()])
        .policy(ScanPolicy::new(ScanAction::Block).bundle(&path, ScanAction::Allow))
        .stall_timeout(Duration::from_millis(if ending == "timeout" {
            500
        } else {
            30_000
        }));
    let on_event = |event: &ScanEvent| {
        if ending == "skip"
            && let ScanEvent::Progress {
                item,
                stage: ScanStage::Loading,
            } = event
            && matches!(item.target, ScanTarget::Bundle(_))
        {
            let item = item.clone();
            let ready = ready.clone();
            thread::spawn(move || {
                pid(&ready);
                item.skip();
            });
        }
    };
    let result = scanner.scan_controlled(&control, &on_event).unwrap();
    let scan = result
        .bundles
        .iter()
        .find(|scan| scan.path == path)
        .unwrap();
    assert_eq!(
        scan.outcome,
        match ending {
            "cancel" => ScanOutcome::Cancelled,
            "skip" => ScanOutcome::Skipped,
            _ => ScanOutcome::TimedOut,
        }
    );
}

/// The scan of a hanging native module ends as `ending` says, and no process it started outlives it.
fn scan_tree(format: &str, ending: &str) {
    let root = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut parent = entry("lifetime_scan_parent")
        .env(SCAN_FORMAT, format)
        .env(SCAN_ENDING, ending)
        .env(DESCENDANT, &executable)
        .env(LIFETIME_DIR, root.path())
        .spawn()
        .unwrap();
    let helper = pid(&root.path().join("helper.pid"));
    let descendant = pid(&root.path().join("descendant.pid"));
    if ending == "parent" {
        parent.kill().unwrap();
    }
    let status = wait(&mut parent);
    if ending != "parent" {
        assert!(status.success(), "{format} {ending}: {status}");
    }
    assert_stopped(&[helper, descendant]);
}

macro_rules! scan_tree_tests {
    ($($name:ident: $format:literal, $ending:literal;)*) => {$(
        #[test]
        #[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
        fn $name() {
            scan_tree($format, $ending);
        }
    )*};
}

scan_tree_tests! {
    vst3_scan_tree_stops_when_the_parent_dies: "vst3", "parent";
    vst3_scan_tree_stops_when_the_scan_is_cancelled: "vst3", "cancel";
    vst3_scan_tree_stops_when_the_item_is_skipped: "vst3", "skip";
    vst3_scan_tree_stops_when_the_scan_times_out: "vst3", "timeout";
    clap_scan_tree_stops_when_the_parent_dies: "clap", "parent";
    clap_scan_tree_stops_when_the_scan_is_cancelled: "clap", "cancel";
    clap_scan_tree_stops_when_the_item_is_skipped: "clap", "skip";
    clap_scan_tree_stops_when_the_scan_times_out: "clap", "timeout";
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn idle_exit_and_recovery_are_observed_without_requests() {
    let mut chain = support::spawn(&[routing(PluginFormat::Vst3)]);
    let first = chain.helper_monitor();
    assert_eq!(first.wait_for_exit(Duration::from_millis(30)), None);
    terminate(first.process_id());
    let status = first
        .wait_for_exit(Duration::from_secs(2))
        .expect("idle helper exit not reported");
    assert!(!status.success());
    assert_eq!(first.clone().exit_status(), Some(status));
    chain = chain.recover(&[]).unwrap();
    let second = chain.helper_monitor();
    assert_ne!(first.process_id(), second.process_id());
    assert!(second.exit_status().is_none());
    assert_eq!(first.exit_status(), Some(status));
    drop(chain);
    assert!(second.wait_for_exit(Duration::from_secs(2)).is_some());
}
