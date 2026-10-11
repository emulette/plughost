use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const CHILD_ROOT: &str = "PLUGHOST_CACHE_INTERRUPTION_TEST";
const READY: &str = "cache serializer paused";

fn entry(name: &str) -> Entry {
    Entry {
        fingerprint: Vec::new(),
        outcome: ScanOutcome::Found(vec![plughost_core::PluginInfo::new(
            plughost_core::PluginFormat::Clap,
            "test-cache",
            name,
            plughost_core::PluginKind::Effect,
        )]),
        failures: 0,
    }
}

/// The real cache model serializes into the real temporary file before this external
/// serialization boundary stops. It never returns, so the buffer cannot flush or rename yet.
struct Paused<'a>(&'a Cache);
impl Serialize for Paused<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let _ = self.0.serialize(serializer)?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{READY}").unwrap();
        stdout.flush().unwrap();
        loop {
            std::thread::park();
        }
    }
}

fn interrupted_writer_child(root: PathBuf) -> ! {
    let path = root.join("cache.json");
    let mut writer = Writer::open(&path).unwrap();
    writer.cache.bundles.insert(
        root.join("second.clap"),
        entry(&"large name ".repeat(4_000)),
    );
    snapshot(&path, &root, &Paused(&writer.cache)).unwrap();
    panic!("the parent must terminate the paused writer");
}

#[test]
fn killing_a_paused_snapshot_writer_preserves_the_previous_cache() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        interrupted_writer_child(root.into());
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache.json");
    let first = root.path().join("first.clap");
    let second = root.path().join("second.clap");
    fs::write(&first, []).unwrap();
    fs::write(&second, []).unwrap();
    let mut writer = Writer::open(&path).unwrap();
    writer.update(&first, Some(entry("first"))).unwrap();
    writer.finish().unwrap();
    let before = fs::read(&path).unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "scan::cache::tests::killing_a_paused_snapshot_writer_preserves_the_previous_cache",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, root.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready, entered) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().ends_with(READY) {
                let _ = ready.send(());
                break;
            }
        }
    });
    let observed = entered.recv_timeout(Duration::from_secs(10));
    // Always reap before asserting, including when the child failed before the barrier.
    let killed = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    observed.unwrap();
    killed.unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let temporary = fs::read_dir(root.path())
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".plughost-cache-")
        })
        .unwrap()
        .path();
    let partial = fs::read(&temporary).unwrap();
    assert!(!partial.is_empty());
    assert!(serde_json::from_slice::<Cache>(&partial).is_err());
    assert_eq!(load(&path).bundles.len(), 1);
    // Killing the child released the sidecar lock; an orphan temporary is never a snapshot.
    let mut writer = Writer::open(&path).unwrap();
    writer.update(&second, Some(entry("second"))).unwrap();
    writer.finish().unwrap();
    assert_eq!(load(&path).bundles.len(), 2);
    assert!(temporary.exists());
}

#[test]
fn a_finished_writer_releases_the_lock_while_its_file_is_still_open_elsewhere() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache.json");
    let writer = Writer::open(&path).unwrap();
    // Another reference to the same open lock file, like one a process being started shares.
    let shared = writer.lock.try_clone().unwrap();
    writer.finish().unwrap();
    Writer::open(&path).unwrap();
    drop(shared);
}
