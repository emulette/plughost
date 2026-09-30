//! The flow of an offline batch application: find a plugin class, prepare a chain, keep a state
//! snapshot, and render several jobs. When the plugin crashes or hangs during a job, the chain is
//! recovered once in a new helper from the snapshot and the job starts again. Effects process a
//! stereo tone and instruments play a note; each job prints its level, and nothing is written.
//!
//! usage: render <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use plughost::{
    Chain, Error, FailureKind, HostIdentity, Layout, MidiEvent, PluginFormat, PluginInfo,
    PluginKind, PluginRef, RenderOptions, Rendered, ScanOutcome, Scanner, StatePurpose, Timeouts,
    render,
};

const USAGE: &str = "usage: render <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index]";
const NO_CLASS: &str = "no plugin class found for that bundle and index or class ID";
const RATE: f64 = 48_000.0;
const BLOCK: usize = 512;
const SECONDS: f64 = 2.0;
/// Keys for instruments; effects hear the same pitches as tones.
const JOBS: [u8; 3] = [48, 60, 72];

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Finds class `index` of a `.vst3` or `.clap` bundle, or a registered Audio Unit by class ID.
fn find(
    helper: &Path,
    target: &Path,
    index: usize,
) -> Result<Option<(PluginRef, PluginInfo)>, Error> {
    let cache = std::env::temp_dir().join(format!("plughost-render-{}.json", std::process::id()));
    let scanner = Scanner::new(helper, &cache);
    let found = if target
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vst3") || e.eq_ignore_ascii_case("clap"))
    {
        match scanner.retry(target)?.outcome {
            ScanOutcome::Found(classes) => classes.into_iter().nth(index).map(|class| {
                let plugin = PluginRef {
                    format: class.format,
                    bundle: Some(target.to_path_buf()),
                    class_id: class.class_id.clone(),
                };
                (plugin, class)
            }),
            outcome => {
                out!("scan: {outcome:?}");
                None
            }
        }
    } else {
        let class_id = target.to_string_lossy();
        scanner
            .directories(Vec::new())
            .scan(&|_| {})?
            .plugins()
            .into_iter()
            .find(|(plugin, _)| {
                plugin.format == PluginFormat::AudioUnit && plugin.class_id == class_id
            })
            .map(|(plugin, info)| (plugin, info.clone()))
    };
    let _ = std::fs::remove_file(&cache);
    Ok(found)
}

/// One job: a tone at `key`'s pitch on both channels for effects, or the note for instruments.
fn job(chain: &mut Chain, instrument: bool, key: u8) -> Result<Rendered<f32>, Error> {
    let frames = (SECONDS * RATE) as usize;
    let options = RenderOptions::default();
    if instrument {
        let notes = [
            MidiEvent::note_on(0, 0, key, 100),
            MidiEvent::note_off(frames / 2, 0, key, 0),
        ];
        return render::<f32, _>(chain, &[], frames, &notes, &options);
    }
    let pitch = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0);
    let tone: Vec<f32> = (0..frames)
        .map(|i| 0.25 * (i as f64 * pitch / RATE * std::f64::consts::TAU).sin() as f32)
        .collect();
    render(chain, &[&tone, &tone], frames, &[], &options)
}

fn report(key: u8, rendered: &Rendered<f32>) {
    let samples: Vec<f64> = rendered
        .channels
        .iter()
        .flatten()
        .map(|&s| f64::from(s))
        .collect();
    let rms = (samples.iter().map(|s| s * s).sum::<f64>() / samples.len().max(1) as f64).sqrt();
    let peak = samples.iter().fold(0.0f64, |peak, s| peak.max(s.abs()));
    out!(
        "job {key}: {:.2} s rendered, latency {} samples removed, tail {} samples, rms {rms:.4}, peak {peak:.4}",
        rendered.channels.first().map_or(0, Vec::len) as f64 / RATE,
        rendered.latency,
        rendered.tail,
    );
}

fn run(helper: &Path, target: &Path, index: usize) -> Result<bool, Error> {
    let Some((plugin, class)) = find(helper, target, index)? else {
        out!("{NO_CLASS}");
        return Ok(false);
    };
    let instrument = class.kind == PluginKind::Instrument;
    let input = if instrument {
        Layout::None
    } else {
        Layout::Stereo
    };
    let mut chain = Chain::spawn(
        helper,
        &[plugin],
        &HostIdentity::default(),
        Timeouts::default(),
    )?;
    let config = chain.main_bus_config(RATE, BLOCK, input, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    // The last known good state. An application refreshes it after the user changes settings.
    // A plugin without saved state recovers in its initial state.
    let snapshot = match chain.save_state(0, StatePurpose::Project) {
        Ok(state) => vec![state],
        Err(error) if error.kind() == FailureKind::Unsupported => {
            out!("no state snapshot: {error}");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    out!("{} prepared", class.name);

    for key in JOBS {
        let mut recovered = false;
        loop {
            chain.reset()?;
            match job(&mut chain, instrument, key) {
                Ok(rendered) => {
                    report(key, &rendered);
                    break;
                }
                Err(error)
                    if !recovered
                        && matches!(error.kind(), FailureKind::Crashed | FailureKind::TimedOut) =>
                {
                    out!("job {key}: {error}; recovering from the snapshot");
                    // The failed chain stays usable for another attempt if recovery fails.
                    chain = chain.recover(&snapshot)?;
                    recovered = true;
                }
                Err(error) => {
                    out!("job {key}: {error}");
                    return Ok(false);
                }
            }
        }
    }
    let diagnostics = chain.take_diagnostics()?;
    out!(
        "plugin diagnostics: {} records, {} dropped",
        diagnostics.records.len(),
        diagnostics.dropped
    );
    Ok(true)
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (Some(helper), Some(target)) = (args.first(), args.get(1)) else {
        out!("{USAGE}");
        return ExitCode::from(2);
    };
    let index = args
        .get(2)
        .and_then(|i| i.to_str()?.parse().ok())
        .unwrap_or(0);
    match run(helper, target, index) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            out!("{error}");
            ExitCode::FAILURE
        }
    }
}
