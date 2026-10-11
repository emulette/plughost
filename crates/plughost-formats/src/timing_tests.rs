use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use plughost_core::{Layout, PluginTiming, ProcessConfig, ProcessMode, SampleFormat};

pub(crate) fn config() -> ProcessConfig {
    ProcessConfig {
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
}

pub(crate) fn while_busy<T: Send + 'static>(
    engine: Arc<Mutex<T>>,
    poll: impl Fn() -> Option<PluginTiming>,
) {
    let (entered, ready) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _engine = engine.lock().unwrap();
        entered.send(()).unwrap();
        // A blocking implementation returns Some after this cleanup deadline and fails below.
        let _ = resume.recv_timeout(Duration::from_secs(2));
    });
    ready.recv().unwrap();
    let busy = poll();
    let _ = release.send(());
    worker.join().unwrap();
    assert_eq!(busy, None, "an idle poll waited for the processing engine");
    assert!(
        poll().is_some(),
        "the next poll must retry the available engine"
    );
}
