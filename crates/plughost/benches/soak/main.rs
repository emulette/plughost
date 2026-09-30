//! Repeated long renders through one chain of installed effects: every round resets the chain,
//! restores the states saved before the first round, and renders the same noise again. Reports
//! whether each round reproduces the first, the render time, and the helper's memory.
mod errors;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use plughost::{
    Chain, HostIdentity, Layout, PluginRef, RenderOptions, ScanOutcome, Scanner, StatePurpose,
    TailPolicy, Timeouts, render,
};

use errors::Result;

const USAGE: &str = "cargo bench -p plughost --bench soak -- --plugin BUNDLE [--plugin BUNDLE ...] [--rounds 120] [--seconds 60] [--helper FILE]\n\
Renders a chain of the first class of each effect bundle repeatedly. Build the helper first.";
const RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

struct Options {
    plugins: Vec<PathBuf>,
    rounds: usize,
    seconds: f64,
    helper: PathBuf,
}

fn invalid<E>(_: E) -> errors::BenchError {
    errors::fail(errors::ARGUMENTS)
}

fn options() -> Result<Option<Options>> {
    let mut options = Options {
        plugins: Vec::new(),
        rounds: 120,
        seconds: 60.0,
        helper: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if cfg!(target_os = "macos") {
            "../../target/helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
        } else {
            "../../target/helper/plughost-helper.exe"
        }),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--bench" {
            continue; // Cargo supplies this to custom benchmark harnesses.
        }
        if argument == "--help" {
            out!("{USAGE}");
            return Ok(None);
        }
        let value = arguments.next().ok_or_else(|| invalid(()))?;
        match argument.as_str() {
            "--plugin" => options.plugins.push(PathBuf::from(value)),
            "--rounds" => options.rounds = value.parse().map_err(invalid)?,
            "--seconds" => options.seconds = value.parse().map_err(invalid)?,
            "--helper" => options.helper = PathBuf::from(value),
            _ => return Err(invalid(())),
        }
    }
    if options.plugins.is_empty() || options.rounds == 0 {
        return Err(invalid(()));
    }
    Ok(Some(options))
}

/// Uniform white noise in [-0.25, 0.25), the same on every run.
fn noise(frames: usize) -> Vec<Vec<f32>> {
    let mut state = 1u32;
    (0..2)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5) / 2.0
                })
                .collect()
        })
        .collect()
}

/// The helper's memory in MiB, read with the platform's process tool: the working set on
/// Windows, and on macOS the physical footprint, since resident size there also counts the
/// plugin binaries and resources mapped from disk.
fn memory_mib(pid: u32) -> Option<f64> {
    let output = if cfg!(target_os = "windows") {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {pid}).WorkingSet64"),
            ])
            .output()
    } else {
        Command::new("footprint")
            .args(["-f", "bytes", &pid.to_string()])
            .output()
    }
    .ok()?;
    let output = String::from_utf8_lossy(&output.stdout);
    let bytes = if cfg!(target_os = "windows") {
        output.trim()
    } else {
        // "phys_footprint: 13896640 B"
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix("phys_footprint: "))?
            .trim_end_matches(" B")
    };
    let bytes: f64 = bytes.parse().ok()?;
    Some(bytes / 1_048_576.0)
}

fn plugin_ref(helper: &Path, bundle: &Path) -> Result<PluginRef> {
    let cache = std::env::temp_dir().join(format!("plughost-soak-{}.json", std::process::id()));
    let scan = Scanner::new(helper, &cache).retry(bundle);
    let _ = std::fs::remove_file(&cache);
    let ScanOutcome::Found(classes) = scan?.outcome else {
        return Err(errors::fail(errors::NO_CLASS));
    };
    let class = classes
        .into_iter()
        .next()
        .ok_or_else(|| errors::fail(errors::NO_CLASS))?;
    out!(
        "slot: {} | {} | {} | {:?}",
        class.name,
        class.vendor,
        class.version,
        class.format
    );
    Ok(PluginRef {
        format: class.format,
        bundle: Some(bundle.to_path_buf()),
        class_id: class.class_id,
    })
}

fn run(options: &Options) -> Result<()> {
    let plugins = options
        .plugins
        .iter()
        .map(|bundle| plugin_ref(&options.helper, bundle))
        .collect::<Result<Vec<_>>>()?;
    let mut chain = Chain::spawn(
        &options.helper,
        &plugins,
        &HostIdentity::default(),
        Timeouts::default(),
    )?;
    let config = chain.main_bus_config(
        RATE,
        BLOCK,
        Layout::Stereo,
        &vec![Layout::Stereo; plugins.len()],
    )?;
    chain.prepare_audio(&config)?;
    let states = (0..plugins.len())
        .map(|slot| chain.save_state(slot, StatePurpose::Project))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let input = noise((options.seconds * RATE) as usize);
    let options_render = RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    };
    let pid = chain.helper_monitor().process_id();
    let first_memory = memory_mib(pid);
    let mut first: Option<Vec<Vec<f32>>> = None;
    let mut worst = 0.0f32;
    let started = Instant::now();
    for round in 1..=options.rounds {
        for (slot, state) in states.iter().enumerate() {
            chain.restore_state(slot, state, StatePurpose::Project)?;
        }
        chain.reset()?;
        let start = Instant::now();
        let rendered = render(
            &mut chain,
            &[&input[0], &input[1]],
            input[0].len(),
            &[],
            &options_render,
        )?;
        let seconds = start.elapsed().as_secs_f64();
        let difference = match &first {
            Some(first) => first
                .iter()
                .flatten()
                .zip(rendered.channels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max),
            None => {
                first = Some(rendered.channels);
                0.0
            }
        };
        worst = worst.max(difference);
        if round == 1 || round % 10 == 0 || round == options.rounds {
            out!(
                "round {round}: render {:.0} ms | max difference from round 1 {difference:.3e} | helper {:.1} MiB",
                seconds * 1e3,
                memory_mib(pid).unwrap_or(f64::NAN)
            );
        }
    }
    out!(
        "{} rounds, {:.1} minutes of audio in {:.1} s | max difference from round 1 {worst:.3e} | helper memory {:.1} -> {:.1} MiB",
        options.rounds,
        options.rounds as f64 * options.seconds / 60.0,
        started.elapsed().as_secs_f64(),
        first_memory.unwrap_or(f64::NAN),
        memory_mib(pid).unwrap_or(f64::NAN)
    );
    Ok(())
}

fn main() -> std::process::ExitCode {
    if cfg!(debug_assertions) {
        out!("{}", errors::RELEASE);
        return std::process::ExitCode::FAILURE;
    }
    let result = options().and_then(|options| match options {
        Some(options) => run(&options),
        None => Ok(()),
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            out!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
