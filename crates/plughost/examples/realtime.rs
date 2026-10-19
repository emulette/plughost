//! The flow of a live application: find a plugin class, prepare a chain at the default output
//! device's rate, and play it through that device with a realtime endpoint. Effects process a
//! quiet stereo tone and instruments play a repeating note. The stream asks the device for
//! callbacks of one quantum, since the endpoint expects one call per quantum's time: quanta
//! processed in a burst find the previous one unfinished. A quantum the device cannot call back
//! with becomes its nearest callback size: WASAPI's shared mode has only its period. Devices may
//! still call back with other sizes (WASAPI's shared mode does as it starts), so the callback cuts
//! its output from quanta, which adds up to one quantum of latency. The control thread prints the
//! endpoint's status each second and handles its failures: after a resynchronization it
//! resynchronizes the endpoint without stopping it. It snapshots the chain every few seconds
//! while the chain is well, and recovers the endpoint in a new helper from the last snapshot when
//! the helper exits or its processing stalls; the callback keeps its endpoint through the move.
//! A failed recovery ends the example: nothing restarts the helper in a loop. Ends after the
//! given seconds.
//!
//! usage: realtime <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index] [seconds] [quantum]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plughost::{
    BlockContext, Chain, Error, Event, HostIdentity, Layout, MAX_BLOCK_EVENTS, PluginFormat,
    PluginKind, PluginRef, RealtimeConfig, RealtimeEndpoint, ScanOutcome, Scanner, Timeouts,
};

const USAGE: &str = "usage: realtime <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index] [seconds] [quantum]";
const NO_CLASS: &str = "no plugin class found for that bundle and index or class ID";
const NO_DEVICE: &str = "no default output device with f32 samples";
/// The tone effects hear, and the instruments' note.
const PITCH: f64 = 220.0;
const KEY: u8 = 57;
/// Seconds between snapshots of a chain that is well.
const SNAPSHOT_EVERY: u64 = 3;
/// How long the helper may go without completing a quantum before the processing counts as hung:
/// many quanta, and long enough not to mistake a slow moment for a hang.
const HANG_AFTER: Duration = Duration::from_millis(500);

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Finds class `index` of a `.vst3` or `.clap` bundle, or a registered Audio Unit by class ID.
fn find(helper: &Path, target: &Path, index: usize) -> Result<Option<(PluginRef, bool)>, Error> {
    let cache = std::env::temp_dir().join(format!("plughost-realtime-{}.json", std::process::id()));
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
                (plugin, class.kind == PluginKind::Instrument)
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
            .map(|(plugin, info)| (plugin, info.kind == PluginKind::Instrument))
    };
    let _ = std::fs::remove_file(&cache);
    Ok(found)
}

/// The audio callback's state, all reserved before the stream starts: the endpoint, the
/// quantum it played last, and the input it is fed.
struct Callback {
    endpoint: RealtimeEndpoint,
    /// Frames per quantum: each endpoint call processes one.
    quantum: usize,
    instrument: bool,
    rate: f64,
    output: [Vec<f32>; 2],
    /// Frames of `output` already given to the device.
    played: usize,
    tone: [Vec<f32>; 2],
    /// Quanta processed, which time the tone and the notes.
    quanta: u64,
    produced: Vec<Event>,
}

impl Callback {
    /// Fills an interleaved device buffer of `channels` channels.
    fn fill(&mut self, data: &mut [f32], channels: usize) {
        for frame in data.chunks_mut(channels) {
            if self.played == self.quantum {
                self.next();
            }
            for (channel, sample) in frame.iter_mut().enumerate() {
                *sample = self
                    .output
                    .get(channel)
                    .map_or(0.0, |output| output[self.played]);
            }
            self.played += 1;
        }
    }

    /// Processes the next quantum.
    fn next(&mut self) {
        let start = self.quanta * self.quantum as u64;
        for (frame, sample) in self.tone[0].iter_mut().enumerate() {
            let time = (start + frame as u64) as f64 / self.rate;
            *sample = 0.1 * (time * PITCH * std::f64::consts::TAU).sin() as f32;
        }
        let [left_tone, right_tone] = &mut self.tone;
        right_tone.copy_from_slice(left_tone);
        // Instruments play the note for one second of every two.
        let second = (self.rate / self.quantum as f64) as u64;
        let notes = match self.quanta % (2 * second) {
            0 if self.instrument => Some(Event::note_on(0, 0, KEY, 100)),
            phase if self.instrument && phase == second => Some(Event::note_off(0, 0, KEY, 0)),
            _ => None,
        };
        let input: [&[f32]; 2] = [left_tone, right_tone];
        let input = if self.instrument { &[][..] } else { &input[..] };
        let [left, right] = &mut self.output;
        let played = self.endpoint.process_audio_f32(
            &BlockContext::new(self.quantum),
            input,
            &mut [left, right],
            &[],
            notes.as_slice(),
            &mut self.produced,
        );
        // Invalid buffers leave the output as it was; play silence instead.
        if played.is_err() {
            left.fill(0.0);
            right.fill(0.0);
        }
        self.quanta += 1;
        self.played = 0;
    }
}

fn run(
    helper: &Path,
    target: &Path,
    index: usize,
    seconds: u64,
    quantum: usize,
) -> Result<bool, Error> {
    let Some((plugin, instrument)) = find(helper, target, index)? else {
        out!("{NO_CLASS}");
        return Ok(false);
    };
    let device = cpal::default_host().default_output_device();
    let Some((device, device_config)) = device.and_then(|device| {
        let config = device.default_output_config().ok()?;
        (config.sample_format() == cpal::SampleFormat::F32).then_some((device, config))
    }) else {
        out!("{NO_DEVICE}");
        return Ok(false);
    };
    let rate = f64::from(device_config.sample_rate());
    let channels = usize::from(device_config.channels());
    let mut stream_config = device_config.config();
    let mut quantum = quantum;
    // The device calls back with sizes in this range only; WASAPI's shared mode has one, its
    // period.
    if let cpal::SupportedBufferSize::Range { min, max } = *device_config.buffer_size() {
        quantum = quantum.clamp(min as usize, max as usize);
        stream_config.buffer_size = cpal::BufferSize::Fixed(quantum as u32);
    }

    let mut chain = Chain::spawn(
        helper,
        &[plugin],
        &HostIdentity::default(),
        Timeouts::default(),
    )?;
    let input = if instrument {
        Layout::None
    } else {
        Layout::Stereo
    };
    let mut config = chain.main_bus_config(rate, quantum, input, &[Layout::Stereo])?;
    config.mode = plughost::ProcessMode::Realtime;
    chain.prepare_audio(&config)?;
    let endpoint = chain.realtime_endpoint(&RealtimeConfig::new(quantum))?;
    let latency = chain.start_realtime()?;
    out!(
        "playing at {rate} Hz in quanta of {quantum} frames, latency {latency} frames plus up to {quantum} for the device's callback size"
    );

    let mut callback = Callback {
        endpoint,
        quantum,
        instrument,
        rate,
        output: [vec![0.0; quantum], vec![0.0; quantum]],
        played: quantum,
        tone: [vec![0.0; quantum], vec![0.0; quantum]],
        quanta: 0,
        produced: Vec::with_capacity(MAX_BLOCK_EVENTS),
    };
    let stream = device
        .build_output_stream::<f32, _, _>(
            stream_config,
            move |data, _| callback.fill(data, channels),
            |error| out!("device: {error}"),
            None,
        )
        .and_then(|stream| stream.play().map(|()| stream));
    let stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            out!("device: {error}");
            return Ok(false);
        }
    };

    // Snapshots restore the plugins' settings in a new helper, so take them while the chain is
    // well.
    let mut snapshot = chain.snapshot()?;
    for second in 1..=seconds {
        std::thread::sleep(Duration::from_secs(1));
        let exit = chain.helper_monitor().exit_status();
        let Some(status) = chain.realtime_status() else {
            break;
        };
        out!(
            "{second} s: {} processed, {} missed, {} late, stalled {:?}, latency {} frames",
            status.processed,
            status.missed,
            status.late,
            status.stalled,
            status.latency
        );
        let mut recover = exit.is_some() || status.stalled > HANG_AFTER;
        if status.resync && !recover {
            out!("resynchronizing after {:?}", status.last_failure);
            // A helper that hung or died since the check above fails the resynchronization.
            if let Err(error) = chain.resync_realtime() {
                out!("resynchronization failed: {}", describe(&error));
                recover = true;
            }
        }
        if recover {
            out!(
                "recovering: helper exit {exit:?}, stalled {:?}",
                status.stalled
            );
            // The endpoint moves to the new chain; the callback keeps it. The old chain, whose
            // helper may hang on, is dropped as the new one replaces it.
            match chain.recover_realtime(&snapshot) {
                Ok(recovered) => chain = recovered,
                Err(error) => {
                    out!("recovery failed: {}", describe(&error));
                    return Ok(false);
                }
            }
        } else if second % SNAPSHOT_EVERY == 0 && !status.resync && status.stalled < HANG_AFTER / 10
        {
            // A helper that died since the check above fails the snapshot; the next second's
            // check finds its exit and recovers from the previous one.
            match chain.snapshot() {
                Ok(taken) => snapshot = taken,
                Err(error) => out!("snapshot failed: {}", describe(&error)),
            }
        }
    }
    // The stream owns the endpoint; dropping it stops the callbacks before the chain stops.
    drop(stream);
    chain.stop_realtime()?;
    Ok(true)
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (Some(helper), Some(target)) = (args.first(), args.get(1)) else {
        out!("{USAGE}");
        return ExitCode::from(2);
    };
    let number = |index: usize, default: u64| {
        args.get(index)
            .and_then(|value| value.to_str()?.parse().ok())
            .unwrap_or(default)
    };
    let (index, seconds, quantum) = (number(2, 0), number(3, 10), number(4, 256));
    match run(helper, target, index as usize, seconds, quantum as usize) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            out!("{}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

/// The error followed by each cause; plughost errors keep the native reason in `source()`.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        text = format!("{text}: {error}");
        cause = error.source();
    }
    text
}
