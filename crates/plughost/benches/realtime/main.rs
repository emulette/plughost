//! Isolated realtime processing at the audio rate for minutes, the gate of realtime design
//! decision 10. A synthetic audio device calls the realtime endpoint of a chain of the
//! repository's VST3 routing fixture on an absolute grid of quanta at 48 kHz, and skips to its
//! next slot when it wakes late instead of calling back in a burst; that delay is the driver's and
//! is reported apart. The run passes when the helper completed every quantum within the
//! endpoint's latency, the endpoint never resynchronized, and no callback took longer than the
//! limit. With `--stall`, the CLAP stall-in-process fixture holds up one process call for
//! `--stall-ms`; then only the callback limit applies, and the endpoint is restarted after the
//! resynchronization that follows, as an application does. `--load` keeps every core busy.
mod driver;
mod errors;
mod usage;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use plughost::{
    BlockContext, Chain, FallbackReason, HostIdentity, Layout, MAX_BLOCK_EVENTS, PluginFormat,
    PluginRef, RealtimeConfig, RealtimeEndpoint, RealtimeOutput, Timeouts,
};
use plughost_core::ipc::shared::clock;

use errors::Result;

const USAGE: &str = "cargo bench -p plughost --bench realtime -- [--seconds 600] [--quantum 128] [--latency 1] [--load] [--stall] [--stall-ms 100] [--callback-limit 200] [--helper FILE] [--assets DIR]\n\
Calls a routing-fixture chain's realtime endpoint at 48 kHz and exits with a failure when the gate fails.\n\
--callback-limit is in microseconds. Build the repository test plugins and the helper first.";
const RATE: f64 = 48_000.0;

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

struct Options {
    seconds: f64,
    quantum: usize,
    latency: usize,
    load: bool,
    /// How long the stall fixture holds up its process call, with `--stall`.
    stall: Option<Duration>,
    callback_limit: Duration,
    helper: PathBuf,
    assets: PathBuf,
}

fn invalid<E>(_: E) -> errors::BenchError {
    errors::fail(errors::ARGUMENTS)
}

fn options() -> Result<Option<Options>> {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let mut options = Options {
        seconds: 600.0,
        quantum: 128,
        latency: 1,
        load: false,
        stall: None,
        // The first measurement's longest callback was 63 µs (see benches/README.md).
        callback_limit: Duration::from_micros(200),
        helper: target.join(if cfg!(target_os = "macos") {
            "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
        } else {
            "helper/plughost-helper.exe"
        }),
        assets: target.join("test-plugins"),
    };
    let mut stall_ms = 100;
    let mut stall = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => continue, // Cargo supplies this to custom benchmark harnesses.
            "--help" => {
                out!("{USAGE}");
                return Ok(None);
            }
            "--load" => options.load = true,
            "--stall" => stall = true,
            _ => {
                let value = arguments.next().ok_or_else(|| invalid(()))?;
                match argument.as_str() {
                    "--seconds" => options.seconds = value.parse().map_err(invalid)?,
                    "--quantum" => options.quantum = value.parse().map_err(invalid)?,
                    "--latency" => options.latency = value.parse().map_err(invalid)?,
                    "--stall-ms" => stall_ms = value.parse().map_err(invalid)?,
                    "--callback-limit" => {
                        options.callback_limit =
                            Duration::from_micros(value.parse().map_err(invalid)?);
                    }
                    "--helper" => options.helper = value.into(),
                    "--assets" => options.assets = value.into(),
                    _ => return Err(invalid(())),
                }
            }
        }
    }
    if options.seconds <= 0.0 || options.quantum == 0 {
        return Err(invalid(()));
    }
    options.stall = stall.then(|| Duration::from_millis(stall_ms));
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

/// What the callback thread measured.
struct Measured {
    /// How late the driver woke for each callback, in clock ticks.
    delays: Vec<u64>,
    /// How long each callback took, in clock ticks.
    callbacks: Vec<u64>,
    /// How long the helper took for each output played.
    completions: Vec<Duration>,
    /// Grid slots the driver skipped after waking late.
    skipped: u64,
    /// Callbacks that played the fallback, by reason: stopped, priming, missed, resync.
    fallbacks: [u64; 4],
}

/// Calls the endpoint `callbacks` times on the grid of quanta, from a realtime thread.
fn drive(mut endpoint: RealtimeEndpoint, quantum: usize, callbacks: usize) -> Measured {
    let period = clock::ticks(Duration::from_secs_f64(quantum as f64 / RATE));
    driver::schedule(period);
    let input = noise(quantum);
    let input: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = noise(quantum);
    let mut produced = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let context = BlockContext::new(quantum);
    let mut measured = Measured {
        delays: Vec::with_capacity(callbacks),
        callbacks: Vec::with_capacity(callbacks),
        completions: Vec::with_capacity(callbacks),
        skipped: 0,
        fallbacks: [0; 4],
    };
    let start = clock::now() + period;
    let mut slot = 0;
    for _ in 0..callbacks {
        let due = start + slot * period;
        driver::wait_until(due);
        let woke = clock::now();
        let [left, right] = &mut output[..] else {
            unreachable!("the chain has two output channels");
        };
        let played = endpoint.process_audio_f32(
            &context,
            &input,
            &mut [left, right],
            &[],
            &[],
            &mut produced,
        );
        let done = clock::now();
        measured.delays.push(woke.saturating_sub(due));
        measured.callbacks.push(done.saturating_sub(woke));
        match played {
            Ok(RealtimeOutput::Processed) => {
                measured.completions.extend(endpoint.last_completion());
            }
            Ok(RealtimeOutput::Fallback(reason)) => {
                let index = match reason {
                    FallbackReason::Stopped => 0,
                    FallbackReason::Priming => 1,
                    FallbackReason::Missed => 2,
                    _ => 3,
                };
                measured.fallbacks[index] += 1;
            }
            // The driver passes valid input; a failure would show as a resync.
            Ok(_) | Err(_) => measured.fallbacks[3] += 1,
        }
        let next = (clock::now().saturating_sub(start) / period + 1).max(slot + 1);
        measured.skipped += next - slot - 1;
        slot = next;
    }
    measured
}

/// Threads that keep every core busy until dropped.
struct Load {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Load {
    fn start() -> Load {
        let stop = Arc::new(AtomicBool::new(false));
        let cores = thread::available_parallelism().map_or(1, usize::from);
        let threads = (0..cores)
            .map(|_| {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Load { stop, threads }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// The barrier the stall fixture announces its stall at; it is released after `hold`. Returns
/// the address the fixture finds in the host name, and whether the stall happened.
fn barrier(hold: Duration) -> Result<(String, JoinHandle<bool>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?.to_string();
    let stalled = thread::spawn(move || {
        let Ok((mut connection, _)) = listener.accept() else {
            return false;
        };
        let mut signal = [0];
        if connection.read_exact(&mut signal).is_err() || signal != [1] {
            return false;
        }
        thread::sleep(hold);
        connection.write_all(&[0]).is_ok()
    });
    Ok((address, stalled))
}

/// The value at quantile `quantile` of sorted `values`.
fn quantile<T: Copy + Default>(values: &[T], quantile: f64) -> T {
    let index = ((values.len() as f64 * quantile).ceil() as usize).saturating_sub(1);
    values.get(index).copied().unwrap_or_default()
}

fn micros(ticks: u64) -> f64 {
    clock::duration(ticks).as_secs_f64() * 1e6
}

fn run(options: &Options) -> Result<()> {
    let (plugin, identity, stalled) = match options.stall {
        None => (
            PluginRef {
                format: PluginFormat::Vst3,
                bundle: Some(options.assets.join("plughost-test-routing.vst3")),
                class_id: "706C7567686F7374526F7574696E6701".into(),
            },
            HostIdentity::default(),
            None,
        ),
        Some(hold) => {
            let (address, stalled) = barrier(hold)?;
            let identity = HostIdentity {
                name: address,
                ..HostIdentity::default()
            };
            let plugin = PluginRef {
                format: PluginFormat::Clap,
                bundle: Some(options.assets.join("plughost-test-stall-in-process.clap")),
                class_id: "com.studio.plughost.test-delay.79000018".into(),
            };
            (plugin, identity, Some(stalled))
        }
    };
    let mut chain = Chain::spawn(&options.helper, &[plugin], &identity, Timeouts::default())?;
    let config = chain.main_bus_config(RATE, options.quantum, Layout::Stereo, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    let config = RealtimeConfig::new(options.quantum).with_latency(options.latency);
    let endpoint = chain.realtime_endpoint(&config)?;
    let latency = chain.start_realtime()?;
    let processes = [std::process::id(), chain.helper_monitor().process_id()];
    let before = processes.map(usage::usage);
    let started = Instant::now();
    let load = options.load.then(Load::start);
    let callbacks = (options.seconds * RATE / options.quantum as f64) as usize;
    let quantum = options.quantum;
    let callback = thread::Builder::new()
        .name("realtime benchmark callback".into())
        .spawn(move || drive(endpoint, quantum, callbacks))?;
    // Restart the endpoint after a resynchronization, as an application does.
    let mut restarts = 0;
    while !callback.is_finished() {
        thread::sleep(Duration::from_millis(100));
        if chain.realtime_status().is_some_and(|status| status.resync) {
            chain.stop_realtime()?;
            chain.reset()?;
            chain.start_realtime()?;
            restarts += 1;
        }
    }
    let mut measured = callback.join().map_err(|_| errors::fail(errors::THREAD))?;
    let after = processes.map(usage::usage);
    let wall = started.elapsed();
    drop(load);
    let status = chain.realtime_status().unwrap_or_default();
    chain.stop_realtime()?;

    let budget = Duration::from_secs_f64((options.latency * options.quantum) as f64 / RATE);
    out!(
        "{} frames at 48 kHz, latency {} quanta ({:.2} ms, {latency} frames in all), {}, {:.0} s, {}",
        options.quantum,
        options.latency,
        budget.as_secs_f64() * 1e3,
        if options.load {
            "every core busy"
        } else {
            "idle"
        },
        options.seconds,
        match options.stall {
            Some(hold) => format!("stall fixture held {} ms", hold.as_millis()),
            None => "routing fixture".into(),
        }
    );
    let [stopped, priming, missed, resync] = measured.fallbacks;
    out!(
        "callbacks {} | processed {} | fallback: stopped {stopped}, priming {priming}, missed {missed}, resync {resync} | driver skipped {} slots",
        measured.callbacks.len(),
        status.processed,
        measured.skipped
    );
    out!(
        "helper late {} | overflows {} | size mismatches {} | failures {} | restarts {restarts}",
        status.late,
        status.overflows,
        status.size_mismatches,
        status.failures
    );
    measured.delays.sort_unstable();
    measured.callbacks.sort_unstable();
    measured.completions.sort_unstable();
    let longest = measured.callbacks.last().copied().unwrap_or(0);
    let completion =
        |quantile_at: f64| quantile(&measured.completions, quantile_at).as_secs_f64() * 1e6;
    out!(
        "driver delay µs p99.9 {:.1} max {:.1} | callback µs p99.9 {:.1} max {:.1} (limit {}) | completion µs p99.9 {:.1} max {:.1} of {:.1}",
        micros(quantile(&measured.delays, 0.999)),
        micros(measured.delays.last().copied().unwrap_or(0)),
        micros(quantile(&measured.callbacks, 0.999)),
        micros(longest),
        options.callback_limit.as_micros(),
        completion(0.999),
        completion(1.0),
        budget.as_secs_f64() * 1e6
    );
    // Percent of one core over the run, and memory at its end; the application's CPU includes the
    // load threads.
    let [application, helper] = [0, 1].map(|index| match (&before[index], &after[index]) {
        (Some(before), Some(after)) => format!(
            "CPU {:.1}% memory {:.1} MiB",
            after.cpu.saturating_sub(before.cpu).as_secs_f64() / wall.as_secs_f64() * 100.0,
            after.memory_mib
        ),
        _ => "unavailable".into(),
    });
    out!("application {application} | helper {helper}");

    if let Some(stalled) = stalled
        && !(stalled.is_finished() && stalled.join().unwrap_or(false))
    {
        return Err(errors::fail(errors::STALL));
    }
    if clock::duration(longest) > options.callback_limit {
        return Err(errors::fail(errors::CALLBACK));
    }
    if options.stall.is_none() {
        if status.late > 0 {
            return Err(errors::fail(errors::LATE));
        }
        if restarts > 0 || resync > 0 {
            return Err(errors::fail(errors::RESYNC));
        }
    }
    out!("pass");
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
            out!("{}", errors::describe(&*error));
            std::process::ExitCode::FAILURE
        }
    }
}
