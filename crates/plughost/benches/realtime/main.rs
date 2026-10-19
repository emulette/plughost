//! Isolated realtime processing at the audio rate for minutes, the gate for changes to realtime
//! processing. A synthetic audio device calls the realtime endpoint of a chain of the
//! repository's VST3 routing fixture on an absolute grid of quanta at 48 kHz, and skips to its
//! next slot when it wakes late instead of calling back in a burst; that delay is the driver's and
//! is reported apart. The run passes when the helper completed every quantum within the
//! endpoint's latency, the endpoint never resynchronized, callbacks at p99.9 took no longer than
//! the limit, and none took longer than its quantum. The longest callback is reported: the system
//! can stop any thread for a few hundred microseconds, which the thread cannot tell from its own
//! work. With `--stall`, the CLAP stall-in-process fixture holds up one process call for
//! `--stall-ms`; then only the callback limits apply, and the control thread resynchronizes the
//! endpoint (`Chain::resync_realtime`) after the resync that follows, as an application does.
//! `--load` keeps every core busy.
//! `--automation` gives every quantum eight gain points, and `--control` edits the gain from the
//! control thread every 10 ms and saves the state every 100 ms, as an editor and an autosave do;
//! with either, the run records its results and only the callback limits apply. `--reconfigure`
//! replaces the plugin under the running endpoint every `--fault-interval`, alternately with
//! `restore_state` of a saved state with another gain and with `reprepare_slot`; these are not
//! faults, so the stream must go on without a resync or late quantum, and a replacement that
//! fails fails the run.
//!
//! The fault-injection runs check what an application does when the helper fails or the audio
//! configuration changes while its callback goes on with the same endpoint. `--crash` kills the
//! helper every `--fault-interval` and recovers the endpoint from the last snapshot, which the
//! control thread saves every second while the helper is well. `--hang` holds up the helper's
//! process call with the stall fixture, which never returns, notices it once the endpoint has
//! stalled for `--hang-ms`, and recovers the same way; every recovered helper hangs again, so it
//! injects `--faults` hangs back to back. `--overflow` makes the callback submit one block of
//! more events than a block may hold every `--fault-interval`, which the endpoint refuses and
//! resynchronizes on, and the control thread resynchronizes it without stopping it
//! (`Chain::resync_realtime`). `--rate-change` stops the endpoint every
//! `--fault-interval`, prepares the chain at another rate and quantum, and starts a new endpoint,
//! which the driver takes over between two callbacks. Only the callback limits and the recovery
//! are judged: the misses, late quanta and fallbacks of a fault are expected, and its recovery
//! time and fallback quanta are reported.
mod callback;
mod driver;
mod errors;
mod faults;
mod usage;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use plughost::{
    Chain, HostIdentity, Layout, PluginFormat, PluginRef, RealtimeConfig, StatePurpose, Timeouts,
};
use plughost_core::ipc::shared::clock;

use callback::{Feed, Handover, Inputs, Profile, Shared};
use errors::Result;
use faults::{Fault, Faults, Hang};

const USAGE: &str = "cargo bench -p plughost --bench realtime -- [--seconds 600] [--quantum 128] [--latency 1] [--load] [--automation] [--control] [--stall] [--stall-ms 100] [--crash | --hang | --overflow | --rate-change] [--reconfigure] [--fault-interval 20] [--faults N] [--hang-ms 100] [--callback-limit 200] [--helper FILE] [--assets DIR]\n\
Calls a routing-fixture chain's realtime endpoint at 48 kHz and exits with a failure when the gate fails.\n\
--callback-limit is in microseconds, at p99.9. Build the repository test plugins and the helper first.\n\
--crash kills the helper, --overflow submits too many events and --rate-change prepares the chain at\n\
44.1 kHz and 1.5 quanta and back every --fault-interval seconds (at most --faults times); --hang (stall\n\
fixture) hangs every recovered helper until --faults (default 10) hangs are injected. Each is recovered\n\
from, and the recovery time and the fallback quanta per fault are reported. --reconfigure replaces the\n\
plugin every --fault-interval seconds with restore_state and reprepare_slot; the stream must go on.";
const RATE: f64 = 48_000.0;
/// The rate `--rate-change` alternates to, with a quantum of one and a half.
const OTHER_RATE: f64 = 44_100.0;
/// How many hangs `--hang` injects unless `--faults` says otherwise.
const HANGS: u64 = 10;
/// How long a fault run goes on past `--seconds` at most, for the control thread to bring the
/// endpoint back from its last fault, in the callbacks the callback thread may make.
const GRACE: f64 = 60.0;
/// The routing fixture's gain parameter, which the stall fixture also has.
const GAIN: u64 = 0;

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
    automation: bool,
    control: bool,
    /// How long the stall fixture holds up its process call, with `--stall`.
    stall: Option<Duration>,
    /// The fault to inject, with `--crash`, `--hang` or `--rate-change`.
    fault: Option<Fault>,
    /// How long after the run starts or the last recovery `--crash` and `--rate-change` inject
    /// the next fault.
    fault_interval: Duration,
    /// How many faults to inject at most.
    faults: Option<u64>,
    /// How long the endpoint stalls before `--hang` calls it hung.
    hang: Duration,
    /// With `--reconfigure`, whether the control thread replaces the plugin periodically.
    reconfigure: bool,
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
        automation: false,
        control: false,
        stall: None,
        fault: None,
        fault_interval: Duration::from_secs(20),
        faults: None,
        hang: Duration::from_millis(100),
        reconfigure: false,
        // About three times the longest callback of the first measurement, 63 µs.
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
    let mut faults = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => continue, // Cargo supplies this to custom benchmark harnesses.
            "--help" => {
                out!("{USAGE}");
                return Ok(None);
            }
            "--load" => options.load = true,
            "--automation" => options.automation = true,
            "--control" => options.control = true,
            "--stall" => stall = true,
            "--crash" => faults.push(Fault::Crash),
            "--hang" => faults.push(Fault::Hang),
            "--overflow" => faults.push(Fault::Overflow),
            "--reconfigure" => options.reconfigure = true,
            "--rate-change" => faults.push(Fault::RateChange),
            _ => {
                let value = arguments.next().ok_or_else(|| invalid(()))?;
                match argument.as_str() {
                    "--seconds" => options.seconds = value.parse().map_err(invalid)?,
                    "--quantum" => options.quantum = value.parse().map_err(invalid)?,
                    "--latency" => options.latency = value.parse().map_err(invalid)?,
                    "--stall-ms" => stall_ms = value.parse().map_err(invalid)?,
                    "--fault-interval" => {
                        options.fault_interval =
                            Duration::try_from_secs_f64(value.parse().map_err(invalid)?)
                                .map_err(invalid)?;
                    }
                    "--faults" => options.faults = Some(value.parse().map_err(invalid)?),
                    "--hang-ms" => {
                        options.hang = Duration::from_millis(value.parse().map_err(invalid)?);
                    }
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
    // One fault per run, which neither the stall nor the control thread's edits go along with, and
    // no replacements either.
    if faults.len() > 1
        || ((!faults.is_empty() || options.reconfigure) && (stall || options.control))
        || (!faults.is_empty() && options.reconfigure)
    {
        return Err(invalid(()));
    }
    options.fault = faults.pop();
    options.stall = stall.then(|| Duration::from_millis(stall_ms));
    Ok(Some(options))
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

/// The CLAP stall-in-process fixture, which finds its barrier's address in the host name.
fn stall_fixture(assets: &Path, address: String) -> (PluginRef, HostIdentity) {
    let identity = HostIdentity {
        name: address,
        ..HostIdentity::default()
    };
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        bundle: Some(assets.join("plughost-test-stall-in-process.clap")),
        class_id: "com.studio.plughost.test-delay.79000018".into(),
    };
    (plugin, identity)
}

fn run(options: &Options) -> Result<()> {
    let mut hang = None;
    let (plugin, identity, stalled) = match (options.stall, options.fault) {
        (Some(hold), _) => {
            let (address, stalled) = barrier(hold)?;
            let (plugin, identity) = stall_fixture(&options.assets, address);
            (plugin, identity, Some(stalled))
        }
        (None, Some(Fault::Hang)) => {
            let barrier = Hang::bind(options.faults.unwrap_or(HANGS))?;
            let (plugin, identity) = stall_fixture(&options.assets, barrier.address().into());
            hang = Some(barrier);
            (plugin, identity, None)
        }
        (None, _) => (
            PluginRef {
                format: PluginFormat::Vst3,
                bundle: Some(options.assets.join("plughost-test-routing.vst3")),
                class_id: "706C7567686F7374526F7574696E6701".into(),
            },
            HostIdentity::default(),
            None,
        ),
    };
    let mut chain = Chain::spawn(&options.helper, &[plugin], &identity, Timeouts::default())?;
    let config = chain.main_bus_config(RATE, options.quantum, Layout::Stereo, &[Layout::Stereo])?;
    chain.prepare_audio(&config)?;
    // What a crash or a hang is recovered from, taken before the stall fixture can hang.
    let snapshot = matches!(options.fault, Some(Fault::Crash | Fault::Hang))
        .then(|| chain.snapshot())
        .transpose()?;
    // What `--reconfigure` restores: the plugin's state at its gain, and at another.
    let states = if options.reconfigure {
        let state = chain.save_state(0, StatePurpose::Project)?;
        chain.set_parameter(0, GAIN, 0.5)?;
        let other = chain.save_state(0, StatePurpose::Project)?;
        chain.restore_state(0, &state, StatePurpose::Project)?;
        Some([state, other])
    } else {
        None
    };
    let config = RealtimeConfig::new(options.quantum).with_latency(options.latency);
    let endpoint = chain.realtime_endpoint(&config)?;
    let latency = chain.start_realtime()?;
    let processes = [std::process::id(), chain.helper_monitor().process_id()];
    let before = processes.map(usage::usage);
    let started = Instant::now();
    let load = options.load.then(Load::start);
    // A fault run goes on for its faults to be recovered from; the control thread ends it.
    let callbacks = ((options.seconds + if options.fault.is_some() { GRACE } else { 0.0 }) * RATE
        / options.quantum as f64) as usize;
    let profiles = [
        Profile {
            rate: RATE,
            quantum: options.quantum,
        },
        Profile {
            rate: OTHER_RATE,
            quantum: options.quantum * 3 / 2,
        },
    ];
    let shared = Arc::new(Shared::default());
    let (handover, next) = channel();
    let (retired_sender, retired) = channel();
    let feed = (options.fault == Some(Fault::RateChange)).then_some(Feed {
        next,
        retired: retired_sender,
    });
    let first = Handover {
        endpoint,
        profile: profiles[0],
    };
    let callback = thread::Builder::new()
        .name("realtime benchmark callback".into())
        .spawn({
            let shared = Arc::clone(&shared);
            let inputs = Inputs {
                automation: options.automation,
                overflow: options.fault == Some(Fault::Overflow),
            };
            move || callback::drive(first, callbacks, inputs, &shared, feed)
        })?;
    let (mut resyncs, mut edits, mut saves, mut replacements) = (0, 0, 0, 0);
    let report = if let Some(fault) = options.fault {
        Some(
            Faults {
                fault,
                options,
                shared: &shared,
                snapshot,
                hang,
                profiles,
                handover: (fault == Fault::RateChange).then_some(handover),
                retired: (fault == Fault::RateChange).then_some(retired),
            }
            .run(&mut chain, &callback, started),
        )
    } else {
        // Edit and save as an editor and an autosave do, replace the plugin, and resynchronize the
        // endpoint after a resync, as an application does.
        let mut replaced = Instant::now();
        for tick in 0u64.. {
            if callback.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
            if options.control {
                chain.set_parameter(0, GAIN, if tick % 2 == 0 { 0.5 } else { 1.0 })?;
                edits += 1;
            }
            if let Some(states) = &states
                && replaced.elapsed() >= options.fault_interval
            {
                if replacements % 2 == 0 {
                    chain.restore_state(
                        0,
                        &states[(replacements / 2 + 1) % 2],
                        StatePurpose::Project,
                    )?;
                } else {
                    chain.reprepare_slot(0)?;
                }
                replacements += 1;
                replaced = Instant::now();
            }
            if tick % 10 != 0 {
                continue;
            }
            if options.control {
                chain.save_state(0, StatePurpose::Project)?;
                saves += 1;
            }
            if chain.realtime_status().is_some_and(|status| status.resync) {
                chain.resync_realtime()?;
                resyncs += 1;
            }
        }
        None
    };
    let mut measured = callback.join().map_err(|_| errors::fail(errors::THREAD))?;
    let after = processes.map(usage::usage);
    let wall = started.elapsed();
    drop(load);
    let status = chain.realtime_status().unwrap_or_default();
    // A fault that was not recovered from may leave the helper unable to stop.
    let stopped = chain.stop_realtime();
    if report
        .as_ref()
        .is_none_or(|report| report.failure.is_none())
    {
        stopped?;
    }

    let budget = Duration::from_secs_f64((options.latency * options.quantum) as f64 / RATE);
    out!(
        "{}, added latency {} x {} frames ({:.2} ms, {latency} frames in all), {}, {:.0} s, {}",
        if options.fault == Some(Fault::RateChange) {
            format!(
                "{} frames at 48 kHz and {} frames at 44.1 kHz",
                options.quantum, profiles[1].quantum
            )
        } else {
            format!("{} frames at 48 kHz", options.quantum)
        },
        options.latency,
        options.quantum,
        budget.as_secs_f64() * 1e3,
        if options.load {
            "every core busy"
        } else {
            "idle"
        },
        options.seconds,
        match (options.stall, options.fault) {
            (Some(hold), _) => format!("stall fixture held {} ms", hold.as_millis()),
            (None, Some(Fault::Hang)) => "stall fixture held until recovered".into(),
            (None, _) => "routing fixture".into(),
        }
    );
    if options.automation || options.control {
        out!(
            "automation points per quantum {} | control edits {edits} | state saves {saves}",
            if options.automation { 8 } else { 0 }
        );
    }
    if options.reconfigure {
        out!("plugin replacements {replacements} (restore_state and reprepare_slot, alternately)");
    }
    let [stopped, priming, missed, resync] = measured.fallbacks;
    out!(
        "callbacks {} | processed {} | fallback: stopped {stopped}, priming {priming}, missed {missed}, resync {resync} | driver skipped {} slots",
        measured.callbacks.len(),
        shared.processed(),
        measured.skipped
    );
    out!(
        "helper late {} | overflows {} | size mismatches {} | failures {} | resyncs {resyncs}",
        status.late,
        status.overflows,
        status.size_mismatches,
        status.failures
    );
    measured.delays.sort_unstable();
    measured.callbacks.sort_unstable();
    measured.completions.sort_unstable();
    let callback = clock::duration(quantile(&measured.callbacks, 0.999));
    let longest = clock::duration(measured.callbacks.last().copied().unwrap_or(0));
    let quantum = measured.shortest;
    let completion =
        |quantile_at: f64| quantile(&measured.completions, quantile_at).as_secs_f64() * 1e6;
    out!(
        "driver delay µs p99.9 {:.1} max {:.1} | callback µs p99.9 {:.1} (limit {}) max {:.1} of {:.1} | completion µs p99.9 {:.1} max {:.1} of {:.1}",
        micros(quantile(&measured.delays, 0.999)),
        micros(measured.delays.last().copied().unwrap_or(0)),
        callback.as_secs_f64() * 1e6,
        options.callback_limit.as_micros(),
        longest.as_secs_f64() * 1e6,
        quantum.as_secs_f64() * 1e6,
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
    if let Some(report) = &report {
        let mut times: Vec<Duration> = report
            .recoveries
            .iter()
            .map(|recovery| recovery.time)
            .collect();
        let mut fallbacks: Vec<u64> = report
            .recoveries
            .iter()
            .map(|recovery| recovery.fallbacks)
            .collect();
        times.sort_unstable();
        fallbacks.sort_unstable();
        let millis = |quantile_at: f64| quantile(&times, quantile_at).as_secs_f64() * 1e3;
        out!(
            "faults injected {} | recovered {} | recovery ms median {:.1} max {:.1} | fallback quanta per fault median {} max {} | snapshots {}",
            report.injected,
            report.recoveries.len(),
            millis(0.5),
            millis(1.0),
            quantile(&fallbacks, 0.5),
            quantile(&fallbacks, 1.0),
            report.snapshots
        );
        if let Some(failure) = &report.failure {
            out!("{failure}");
        }
    }

    if let Some(stalled) = stalled
        && !(stalled.is_finished() && stalled.join().unwrap_or(false))
    {
        return Err(errors::fail(errors::STALL));
    }
    if callback > options.callback_limit {
        return Err(errors::fail(errors::CALLBACK));
    }
    if longest > quantum {
        return Err(errors::fail(errors::DEADLINE));
    }
    if let Some(report) = &report {
        if report.failure.is_some() || report.recoveries.len() as u64 != report.injected {
            return Err(errors::fail(errors::RECOVERY));
        }
        if report.injected == 0 {
            return Err(errors::fail(errors::NO_FAULTS));
        }
    } else if options.stall.is_none() && !options.automation && !options.control {
        if options.reconfigure && replacements == 0 {
            return Err(errors::fail(errors::NO_REPLACEMENTS));
        }
        if status.late > 0 {
            return Err(errors::fail(errors::LATE));
        }
        if resyncs > 0 || resync > 0 {
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
