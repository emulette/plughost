//! Manual editor check: opens a plugin's editor in a helper and keeps processing a test tone while
//! you edit, printing the output level. When you close the window, the state is saved right away
//! and restored in a new helper, and parameters and output are compared.
//!
//! usage: editor <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use plughost::{
    Chain, Error, Layout, MidiEvent, PluginFormat, PluginInfo, PluginKind, PluginRef, ScanOutcome,
    Scanner, Timeouts,
};

const USAGE: &str = "usage: editor <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index]";
const NO_CLASS: &str = "no plugin class found for that bundle and index or class ID";
const RATE: f64 = 48_000.0;
const BLOCK: usize = 1024;

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

fn tone(channels: usize, frames: usize, start: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|_| {
            (start..start + frames)
                .map(|i| 0.25 * (i as f64 * 440.0 / RATE * std::f64::consts::TAU).sin() as f32)
                .collect()
        })
        .collect()
}

/// Instruments play middle C for a quarter second every half second instead of the tone.
fn notes(channels: usize, start: usize) -> Vec<MidiEvent> {
    const PERIOD: usize = RATE as usize / 2;
    if channels > 0 {
        return Vec::new();
    }
    (start..start + BLOCK)
        .filter_map(|i| match i % PERIOD {
            0 => Some(MidiEvent::note_on(i - start, 0, 60, 100)),
            phase if phase == PERIOD / 2 => Some(MidiEvent::note_off(i - start, 0, 60, 0)),
            _ => None,
        })
        .collect()
}

/// Processes the block that starts `start` samples into the test signal.
fn process(chain: &mut Chain, channels: usize, start: usize) -> Result<Vec<Vec<f32>>, Error> {
    let input = tone(channels, BLOCK, start);
    let slices: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0; BLOCK]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    chain.process_audio_f32(
        &plughost_core::BlockContext::new(outputs.first().map_or(0, |channel| channel.len())),
        &slices,
        &mut outputs,
        &[],
        &notes(channels, start),
        &mut Vec::new(),
    )?;
    Ok(output)
}

fn rms(blocks: &[Vec<Vec<f32>>]) -> f64 {
    let samples: Vec<f64> = blocks
        .iter()
        .flatten()
        .flatten()
        .map(|&s| f64::from(s))
        .collect();
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len().max(1) as f64).sqrt()
}

fn render(chain: &mut Chain, channels: usize, seconds: f64) -> Result<Vec<Vec<Vec<f32>>>, Error> {
    let blocks = (seconds * RATE) as usize / BLOCK;
    (0..blocks)
        .map(|b| process(chain, channels, b * BLOCK))
        .collect()
}

/// Finds the class to check: class `index` of a `.vst3` or `.clap` bundle, or a registered Audio Unit
/// given by its class ID.
fn find(
    helper: &Path,
    target: &Path,
    index: usize,
) -> Result<Option<(PluginRef, PluginInfo)>, Error> {
    let cache = std::env::temp_dir().join(format!("plughost-editor-{}.json", std::process::id()));
    let scanner = Scanner::new(helper, &cache);
    let found = if target
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vst3") || e.eq_ignore_ascii_case("clap"))
    {
        let scanned = scanner.retry(target)?;
        match scanned.outcome {
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
        let catalog = scanner.directories(Vec::new()).scan(&|_| {})?;
        let class_id = target.to_string_lossy();
        catalog
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

fn run(helper: &Path, target: &Path, index: usize) -> Result<bool, Error> {
    let Some((plugin, class)) = find(helper, target, index)? else {
        out!("{NO_CLASS}");
        return Ok(false);
    };
    let input = match class.kind {
        PluginKind::Effect => Layout::Stereo,
        PluginKind::Instrument => Layout::None,
    };
    let channels = input.channels();

    let mut chain = Chain::spawn(
        helper,
        std::slice::from_ref(&plugin),
        &plughost::HostIdentity::default(),
        Timeouts::default(),
    )?;
    let config = chain.main_bus_config(RATE, BLOCK, input, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    out!("capabilities: {:?}", chain.capabilities(0)?);
    chain.open_editor(0)?;
    out!(
        "{}: edit in the window while it processes, then close the window",
        class.name
    );
    let mut position = 0;
    let mut second = Vec::new();
    let mut checked = Instant::now();
    loop {
        second.push(process(&mut chain, channels, position)?);
        position += BLOCK;
        if checked.elapsed() >= Duration::from_secs(1) {
            let diagnostics = chain.take_diagnostics()?;
            for record in diagnostics.records {
                out!(
                    "{:?} slot {:?}: {}",
                    record.severity,
                    record.slot,
                    record.message
                );
            }
            if diagnostics.dropped > 0 {
                out!("diagnostics dropped: {}", diagnostics.dropped);
            }
            out!("output rms {:.4}", rms(&second));
            second.clear();
            checked = Instant::now();
            if !chain.editor_open(0)? {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let state = chain.save_state(0, plughost::StatePurpose::Project)?;
    let before = chain.parameters(0)?;
    let mut fresh = Chain::spawn(
        helper,
        &[plugin],
        &plughost::HostIdentity::default(),
        Timeouts::default(),
    )?;
    fresh.prepare_audio(&config)?;
    fresh.restore_state(0, &state, plughost::StatePurpose::Project)?;
    let after = fresh.parameters(0)?;
    let mismatched: Vec<String> = before
        .iter()
        .zip(&after)
        .filter(|((info, a), (_, b))| !info.flags.read_only && (a - b).abs() > 1e-3)
        .map(|((info, a), (_, b))| format!("{} {a:.4} -> {b:.4}", info.title))
        .collect();
    out!(
        "fresh helper parameters: {} mismatched {mismatched:?}",
        mismatched.len()
    );

    chain.reset()?;
    fresh.reset()?;
    let original = render(&mut chain, channels, 1.0)?;
    let restored = render(&mut fresh, channels, 1.0)?;
    let difference = original
        .iter()
        .flatten()
        .flatten()
        .zip(restored.iter().flatten().flatten())
        .map(|(a, b)| f64::from((a - b).abs()))
        .fold(0.0, f64::max);
    out!(
        "fresh helper output: max difference {difference:.3e} (original rms {:.4})",
        rms(&original)
    );
    Ok(mismatched.is_empty())
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (Some(helper), Some(bundle)) = (args.first(), args.get(1)) else {
        out!("{USAGE}");
        return ExitCode::from(2);
    };
    let index = args
        .get(2)
        .and_then(|i| i.to_str()?.parse().ok())
        .unwrap_or(0);
    match run(helper, bundle, index) {
        Ok(true) => {
            out!("result: PASS");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            out!("result: FAIL");
            ExitCode::FAILURE
        }
        Err(error) => {
            out!("result: FAILED {error}");
            ExitCode::FAILURE
        }
    }
}
