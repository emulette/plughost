//! Load, state and render cost of one installed plugin class, processed directly in this process
//! and isolated in a helper, under the conditions `with_pedalboard.py` repeats for Pedalboard:
//! 48 kHz, stereo 32-bit white noise, reported latency removed, no tail, and a reset before every
//! timed render. The difference between isolated and direct renders, per block, is the IPC cost.
mod errors;

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use plughost::{
    Chain, HostIdentity, Layout, PluginFormat, PluginRef, ProcessMode, RenderOptions, SampleFormat,
    ScanOutcome, Scanner, StatePurpose, TailPolicy, Timeouts, render,
};
use plughost_core::render::{Process, Tail};
use plughost_core::{AutomationEvent, BlockContext, Event, ProcessConfig};
use plughost_formats::{BlockProcessor, HostedPlugin};

use errors::Result;

const USAGE: &str = "cargo bench -p plughost --bench compare -- --plugin BUNDLE|au:CLASS_ID [--class 0] [--seconds 10] [--trials 5] [--blocks 64,512,4096] [--helper FILE]\n\
Measures one installed VST3, CLAP or Audio Unit class directly and through a helper. Build the helper first.";
const RATE: f64 = 48_000.0;

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

struct Options {
    plugin: PathBuf,
    class: usize,
    seconds: f64,
    trials: usize,
    blocks: Vec<usize>,
    helper: PathBuf,
}

fn invalid<E>(_: E) -> errors::BenchError {
    errors::fail(errors::ARGUMENTS)
}

fn options() -> Result<Option<Options>> {
    let mut plugin = None;
    let mut options = Options {
        plugin: PathBuf::new(),
        class: 0,
        seconds: 10.0,
        trials: 5,
        blocks: vec![64, 512, 4096],
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
        let value = arguments
            .next()
            .ok_or_else(|| errors::fail(errors::ARGUMENTS))?;
        match argument.as_str() {
            "--plugin" => plugin = Some(PathBuf::from(value)),
            "--class" => options.class = value.parse().map_err(invalid)?,
            "--seconds" => options.seconds = value.parse().map_err(invalid)?,
            "--trials" => options.trials = value.parse().map_err(invalid)?,
            "--helper" => options.helper = PathBuf::from(value),
            "--blocks" => {
                options.blocks = value
                    .split(',')
                    .map(|block| block.parse().map_err(invalid))
                    .collect::<Result<_>>()?;
            }
            _ => return Err(errors::fail(errors::ARGUMENTS)),
        }
    }
    options.plugin = plugin.ok_or_else(|| errors::fail(errors::ARGUMENTS))?;
    Ok(Some(options))
}

/// Uniform white noise in [-0.5, 0.5), the same on every run.
fn noise(frames: usize) -> Vec<Vec<f32>> {
    let mut state = 1u32;
    (0..2)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                })
                .collect()
        })
        .collect()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn timed<T>(call: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = call();
    (value, start.elapsed().as_secs_f64())
}

/// One plugin processed in this process, from the thread that renders.
struct Direct {
    processor: Box<dyn BlockProcessor>,
    block: usize,
}

impl Process<f32> for Direct {
    type Error = plughost_formats::Error;
    fn sample_rate(&self) -> f64 {
        RATE
    }
    fn max_block_size(&self) -> usize {
        self.block
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn latency(&self) -> std::result::Result<u32, Self::Error> {
        self.processor.timing().map(|timing| timing.latency)
    }
    fn tail(&self) -> std::result::Result<Tail, Self::Error> {
        self.processor.timing().map(|timing| timing.tail)
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        _automation: &[AutomationEvent],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> std::result::Result<(), Self::Error> {
        self.processor
            .process_audio_f32(context, input, output, &[], events, produced)
    }
}

const OPTIONS: RenderOptions = RenderOptions::new(TailPolicy::Reported, 0.0);

fn plugin_ref(options: &Options) -> Result<PluginRef> {
    // Audio Units are registered classes, named by class ID instead of a bundle.
    if let Some(class_id) = options.plugin.to_str().and_then(|p| p.strip_prefix("au:")) {
        out!("plugin: {class_id} | {:?}", PluginFormat::AudioUnit);
        return Ok(PluginRef {
            format: PluginFormat::AudioUnit,
            bundle: None,
            class_id: class_id.to_owned(),
        });
    }
    let cache = std::env::temp_dir().join(format!("plughost-compare-{}.json", std::process::id()));
    let scan = Scanner::new(&options.helper, &cache).retry(&options.plugin);
    let _ = std::fs::remove_file(&cache);
    let ScanOutcome::Found(classes) = scan?.outcome else {
        return Err(errors::fail(errors::NO_CLASS));
    };
    let class = classes
        .into_iter()
        .nth(options.class)
        .ok_or_else(|| errors::fail(errors::NO_CLASS))?;
    out!(
        "plugin: {} | {} | {} | {:?}",
        class.name,
        class.vendor,
        class.version,
        class.format
    );
    Ok(PluginRef {
        format: class.format,
        bundle: Some(options.plugin.clone()),
        class_id: class.class_id,
    })
}

/// Median seconds per render of `input` through the direct plugin, prepared for `block`.
fn direct_render(
    plugin: &mut dyn HostedPlugin,
    input: &[Vec<f32>],
    block: usize,
    trials: usize,
) -> Result<f64> {
    plugin.prepare(&ProcessConfig {
        sample_rate: RATE,
        max_block_size: block,
        sample_format: SampleFormat::F32,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    })?;
    let mut times = Vec::new();
    for trial in 0..=trials {
        plugin.reset()?;
        let mut direct = Direct {
            processor: plugin.processor(),
            block,
        };
        let frames = input[0].len();
        let (rendered, seconds) = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    timed(|| render(&mut direct, &[&input[0], &input[1]], frames, &[], &OPTIONS))
                })
                .join()
        })
        .map_err(|_| errors::fail(errors::THREAD))?;
        rendered?;
        // The first render warms caches and is not counted.
        if trial > 0 {
            times.push(seconds);
        }
    }
    Ok(median(times))
}

fn isolated_render(
    chain: &mut Chain,
    input: &[Vec<f32>],
    block: usize,
    trials: usize,
) -> Result<f64> {
    let config = chain.main_bus_config(RATE, block, Layout::Stereo, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    let mut times = Vec::new();
    for trial in 0..=trials {
        chain.reset()?;
        let frames = input[0].len();
        let (rendered, seconds) =
            timed(|| render(chain, &[&input[0], &input[1]], frames, &[], &OPTIONS));
        rendered?;
        if trial > 0 {
            times.push(seconds);
        }
    }
    Ok(median(times))
}

fn run(options: &Options) -> Result<()> {
    let plugin = plugin_ref(options)?;
    let identity = HostIdentity::default();
    let (direct, load) = timed(|| plughost_formats::load(&plugin, &identity));
    let mut direct = direct?;
    let (chain, spawn) = timed(|| {
        Chain::spawn(
            &options.helper,
            std::slice::from_ref(&plugin),
            &identity,
            Timeouts::default(),
        )
    });
    let mut chain = chain?;
    out!(
        "load: direct {:.1} ms | isolated (helper start and load) {:.1} ms",
        load * 1e3,
        spawn * 1e3
    );
    let input = noise((options.seconds * RATE) as usize);
    for &block in &options.blocks {
        let direct_seconds = direct_render(direct.as_mut(), &input, block, options.trials)?;
        let isolated_seconds = isolated_render(&mut chain, &input, block, options.trials)?;
        let blocks = input[0].len().div_ceil(block);
        out!(
            "block {block}: direct {:.1} ms ({:.0}x realtime) | isolated {:.1} ms ({:.0}x realtime) | IPC {:.1} us per block",
            direct_seconds * 1e3,
            options.seconds / direct_seconds,
            isolated_seconds * 1e3,
            options.seconds / isolated_seconds,
            (isolated_seconds - direct_seconds) / blocks as f64 * 1e6,
        );
    }
    let (state, save) = timed(|| direct.save_state(StatePurpose::Project));
    let state = state?;
    let (restored, restore) = timed(|| direct.restore_state(&state, StatePurpose::Project));
    restored?;
    let (state, chain_save) = timed(|| chain.save_state(0, StatePurpose::Project));
    let state = state?;
    let (restored, chain_restore) = timed(|| chain.restore_state(0, &state, StatePurpose::Project));
    restored?;
    out!(
        "state ({} bytes): direct save {:.2} ms, restore {:.2} ms | isolated save {:.2} ms, restore into a fresh prepared instance {:.2} ms",
        state.component.len() + state.controller.len(),
        save * 1e3,
        restore * 1e3,
        chain_save * 1e3,
        chain_restore * 1e3
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
