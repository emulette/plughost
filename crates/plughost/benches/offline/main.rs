//! Repeated offline renders of repository-owned routing fixtures; never scans installed plugins.
mod allocations;
mod cases;
mod errors;
mod lifecycle;
mod measure;
mod native;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use plughost::{Chain, HostIdentity, Layout, PluginFormat, PluginRef, Timeouts};

use cases::Case;
use errors::Result;

const USAGE: &str = "cargo bench -p plughost --bench offline -- [--seconds 1] [--trials 3] [--blocks 64,128,512,4096] [--output DIR] [--helper FILE] [--assets DIR]\n\
Runs 48 VST3 routing-fixture cases per block size in release mode, with one warm-up render per case.\n\
Build repository test plugins and the matching helper first. Only plughost-test-routing.vst3 is loaded.";

struct Options {
    seconds: f64,
    trials: usize,
    blocks: Vec<usize>,
    output: PathBuf,
    helper: PathBuf,
    assets: PathBuf,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn unix_nanos() -> Result<u128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
}

fn options() -> Result<Option<Options>> {
    let target = root().join("target");
    let mut options = Options {
        seconds: 1.0,
        trials: 3,
        blocks: vec![64, 128, 512, 4096],
        output: target
            .join("benchmarks")
            .join(format!("offline-{}", unix_nanos()?)),
        helper: target.join(if cfg!(target_os = "macos") {
            "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
        } else {
            "helper/plughost-helper.exe"
        }),
        assets: target.join("test-plugins"),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => continue, // Cargo supplies this to custom benchmark harnesses.
            "--help" => {
                writeln!(std::io::stdout(), "{USAGE}")?;
                return Ok(None);
            }
            "--seconds" | "--trials" | "--blocks" | "--output" | "--helper" | "--assets" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| errors::fail(errors::ARGUMENTS))?;
                match argument.as_str() {
                    "--seconds" => {
                        options.seconds =
                            value.parse().map_err(|_| errors::fail(errors::ARGUMENTS))?
                    }
                    "--trials" => {
                        options.trials =
                            value.parse().map_err(|_| errors::fail(errors::ARGUMENTS))?
                    }
                    "--blocks" => {
                        let mut blocks = Vec::new();
                        for item in value.split(',') {
                            let block: usize =
                                item.parse().map_err(|_| errors::fail(errors::ARGUMENTS))?;
                            if block == 0 || block > i32::MAX as usize || blocks.contains(&block) {
                                return Err(errors::fail(errors::ARGUMENTS));
                            }
                            blocks.push(block);
                        }
                        options.blocks = blocks;
                    }
                    "--output" => options.output = value.into(),
                    "--helper" => options.helper = value.into(),
                    "--assets" => options.assets = value.into(),
                    _ => unreachable!(),
                }
            }
            _ => return Err(errors::fail(errors::ARGUMENTS)),
        }
    }
    let largest = options.seconds * 96_000.0;
    if !options.seconds.is_finite()
        || (options.seconds * 48_000.0).round() < 8.0
        || largest >= (usize::MAX / (8 * std::mem::size_of::<f32>())) as f64
        || options.trials == 0
    {
        return Err(errors::fail(errors::ARGUMENTS));
    }
    Ok(Some(options))
}

fn isolated(case: Case, options: &Options, plugin: &PluginRef) -> Result<measure::Measurement> {
    let (mut chain, load) = lifecycle::timed(|| {
        Chain::spawn(
            &options.helper,
            &vec![plugin.clone(); case.plugins],
            &HostIdentity::default(),
            Timeouts::default(),
        )
    })?;
    let config = if case.multiple() {
        case.routed_config()
    } else {
        chain.main_bus_config(
            case.sample_rate.into(),
            case.max_block_size,
            Layout::Stereo,
            &vec![Layout::Stereo; case.plugins],
        )?
    };
    let (_, prepare) = lifecycle::timed(|| chain.prepare_audio(&config))?;
    let mut states = Vec::new();
    for slot in 0..case.plugins {
        let (saved, save) =
            lifecycle::timed(|| chain.save_state(slot, plughost::StatePurpose::Project))?;
        chain.set_parameter(slot, 0, 0.125)?;
        if chain.save_state(slot, plughost::StatePurpose::Project)? == saved {
            return Err(errors::fail(errors::STATE));
        }
        let ((), restore) = lifecycle::timed(|| {
            chain.restore_state(slot, &saved, plughost::StatePurpose::Project)
        })?;
        if chain.save_state(slot, plughost::StatePurpose::Project)? != saved {
            return Err(errors::fail(errors::STATE));
        }
        states.push(lifecycle::StateCost {
            save_seconds: save,
            restore_seconds: restore,
            payload_bytes: saved.component.len() + saved.controller.len(),
        });
    }
    if chain.latency() != 0 || chain.tail() != plughost::Tail::Samples(0) {
        return Err(errors::fail(errors::TIMING));
    }
    measure::run(
        chain,
        case,
        options.trials,
        lifecycle::Lifecycle {
            load_seconds: load,
            prepare_seconds: prepare,
            states,
        },
    )
}

fn run(options: Options) -> Result<()> {
    if cfg!(debug_assertions) {
        return Err(errors::fail(errors::RELEASE));
    }
    let bundle = options
        .assets
        .join("plughost-test-routing.vst3")
        .canonicalize()?;
    let plugin = PluginRef {
        format: PluginFormat::Vst3,
        bundle: Some(bundle),
        class_id: "706C7567686F7374526F7574696E6701".into(),
    };
    let cases = cases::matrix(options.seconds, &options.blocks);
    fs::create_dir_all(&options.output)?;
    let mut results = BufWriter::new(File::create(options.output.join("results.jsonl"))?);
    writeln!(
        std::io::stderr(),
        "Benchmark output: {}",
        options.output.display()
    )?;
    for (index, case) in cases.iter().copied().enumerate() {
        writeln!(
            std::io::stderr(),
            "[{}/{}] {:?}",
            index + 1,
            cases.len(),
            case
        )?;
        let measurement = match case.path {
            cases::Path::Isolated => isolated(case, &options, &plugin)?,
            cases::Path::Direct => {
                let native::Prepared {
                    owners,
                    processor,
                    lifecycle,
                } = native::prepare(case, &plugin)?;
                let trials = options.trials;
                let result =
                    std::thread::spawn(move || measure::run(processor, case, trials, lifecycle))
                        .join()
                        .map_err(|_| errors::fail(errors::THREAD))?;
                drop(owners);
                result?
            }
        };
        serde_json::to_writer(&mut results, &measurement)?;
        writeln!(results)?;
        results.flush()?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let result = options().and_then(|options| match options {
        Some(options) => run(options),
        None => Ok(()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{error}");
            ExitCode::FAILURE
        }
    }
}
