//! Scanner scheduling against a real external helper boundary with controlled completion gates.
#[path = "scan_scheduling/cache.rs"]
mod cache;
#[path = "scan_scheduling/control.rs"]
mod control;

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Stream};
use plughost::{ScanOutcome, Scanner};
use plughost_core::ipc::{
    Hello, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, Response,
};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("cache-child") {
        cache::child(&args[2], Path::new(&args[3]));
        return;
    }
    if args.get(1).map(String::as_str) == Some("scan") {
        peer(&args[2], u64::from_str_radix(&args[3], 16).unwrap());
        return;
    }
    for jobs in [None, Some(0), Some(2)] {
        scheduling(jobs);
    }
    control::run();
    cache::run();
}

fn peer(name: &str, token: u64) {
    let executable = std::env::current_exe().unwrap();
    let root = executable.parent().unwrap();
    let fault = fs::read_to_string(root.join("fault")).unwrap_or_default();
    if fault == "connect" {
        fs::write(root.join("boot.started"), std::process::id().to_string()).unwrap();
        loop {
            thread::park();
        }
    }
    let mut stream = Stream::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
    if fault == "hello" {
        use std::io::Write;
        stream.write_all(&[1]).unwrap();
        fs::write(root.join("boot.started"), std::process::id().to_string()).unwrap();
        loop {
            thread::park();
        }
    }
    let (read, write) = stream.split();
    let mut read = MessageReader::new(read);
    let mut write = MessageWriter::new(write);
    write
        .write(&Hello {
            protocol: PROTOCOL_VERSION,
            token,
        })
        .unwrap();
    if fault == "closed" {
        drop(read);
        drop(write);
        fs::write(root.join("boot.started"), std::process::id().to_string()).unwrap();
        loop {
            thread::park();
        }
    }
    while let Ok(request) = read.read::<Request>() {
        match request {
            Request::Scan { bundle, .. } => {
                let gate = bundle.parent().unwrap();
                fs::write(
                    bundle.with_extension("started"),
                    std::process::id().to_string(),
                )
                .unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !gate.join("release").exists() && !bundle.with_extension("release").exists() {
                    assert!(
                        Instant::now() < deadline,
                        "completion gate was never released"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                write.write(&Response::ModuleLoaded).unwrap();
                if gate.join("large-class").exists() {
                    write
                        .write(&Response::Class(plughost_core::PluginInfo {
                            format: plughost_core::PluginFormat::Clap,
                            class_id: "cache-write-boundary".into(),
                            name: "\n".repeat(4 << 20),
                            vendor: String::new(),
                            version: String::new(),
                            sdk_version: String::new(),
                            kind: plughost_core::PluginKind::Effect,
                            categories: Vec::new(),
                        }))
                        .unwrap();
                }
                write.write(&Response::Done).unwrap();
            }
            Request::ListAudioUnits => {
                if fault == "registry" {
                    fs::write(
                        root.join("registry.started"),
                        std::process::id().to_string(),
                    )
                    .unwrap();
                    loop {
                        thread::park();
                    }
                }
                write.write(&Response::Done).unwrap();
            }
            Request::Shutdown => return,
            _ => panic!("unexpected request"),
        }
    }
}

fn bundle(root: &Path, name: &str) -> PathBuf {
    let path = root.join(format!("{name}.clap"));
    #[cfg(target_os = "macos")]
    {
        let contents = path.join("Contents");
        fs::create_dir_all(contents.join("MacOS")).unwrap();
        fs::write(contents.join("Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>fixture</string><key>CFBundlePackageType</key><string>BNDL</string></dict></plist>").unwrap();
        fs::copy(
            std::env::current_exe().unwrap(),
            contents.join("MacOS/fixture"),
        )
        .unwrap();
    }
    #[cfg(target_os = "windows")]
    fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
    path
}

/// A temporary directory that holds bundles, gates, and a scan cache.
struct Directory(tempfile::TempDir);
impl Directory {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn scanner(&self) -> Scanner {
        Scanner::new(
            &std::env::current_exe().unwrap(),
            &self.path().join("cache.json"),
        )
        .directories(vec![self.path().to_owned()])
    }
}

fn scheduling(jobs: Option<usize>) {
    let directory = Directory::new();
    let root = directory.path();
    let first = bundle(root, "a");
    let second = bundle(root, "b");
    let mut scanner = directory.scanner();
    if let Some(jobs) = jobs {
        scanner = scanner.jobs(jobs);
    }
    let worker = thread::spawn(move || scanner.scan(&|_| {}).unwrap());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !first.with_extension("started").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let first_started = first.with_extension("started").exists();
    // Keep the first helper blocked. A second worker has ample time to launch if enabled.
    let deadline = Instant::now()
        + if jobs == Some(2) {
            Duration::from_secs(3)
        } else {
            Duration::from_millis(300)
        };
    while !second.with_extension("started").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let overlapped = second.with_extension("started").exists();
    fs::write(root.join("release"), []).unwrap();
    let catalog = worker.join().unwrap();
    let both_completed = catalog.bundles.len() == 2
        && catalog
            .bundles
            .iter()
            .all(|s| matches!(&s.outcome, ScanOutcome::Found(classes) if classes.is_empty()));
    assert!(first_started && both_completed);
    assert_eq!(
        overlapped,
        jobs == Some(2),
        "jobs={jobs:?}: native helper admissions overlapped"
    );
}
