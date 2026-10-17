//! The flow of a live application: find a plugin class, prepare a chain at the default output
//! device's rate, and play it through that device with a realtime endpoint. Effects process a
//! quiet stereo tone and instruments play a repeating note. Devices may ask for any number of
//! frames per callback (WASAPI's shared mode does), so the callback cuts its output from quanta
//! of a fixed size, which adds up to one quantum of latency. The control thread prints the
//! endpoint's status each second and, after a resynchronization, stops the endpoint, resets the
//! chain and starts it again. Ends after the given seconds.
//!
//! usage: realtime <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index] [seconds]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plughost::{
    BlockContext, Chain, Error, Event, HostIdentity, Layout, MAX_BLOCK_EVENTS, PluginFormat,
    PluginKind, PluginRef, RealtimeConfig, RealtimeEndpoint, ScanOutcome, Scanner, Timeouts,
};

const USAGE: &str = "usage: realtime <helper executable> <bundle.vst3 | bundle.clap | Audio Unit class ID> [class index] [seconds]";
const NO_CLASS: &str = "no plugin class found for that bundle and index or class ID";
const NO_DEVICE: &str = "no default output device with f32 samples";
/// Frames per quantum: each endpoint call processes one.
const QUANTUM: usize = 256;
/// The tone effects hear, and the instruments' note.
const PITCH: f64 = 220.0;
const KEY: u8 = 57;

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
            if self.played == QUANTUM {
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
        let start = self.quanta * QUANTUM as u64;
        for (frame, sample) in self.tone[0].iter_mut().enumerate() {
            let time = (start + frame as u64) as f64 / self.rate;
            *sample = 0.1 * (time * PITCH * std::f64::consts::TAU).sin() as f32;
        }
        let [left_tone, right_tone] = &mut self.tone;
        right_tone.copy_from_slice(left_tone);
        // Instruments play the note for one second of every two.
        let second = (self.rate / QUANTUM as f64) as u64;
        let notes = match self.quanta % (2 * second) {
            0 if self.instrument => Some(Event::note_on(0, 0, KEY, 100)),
            phase if self.instrument && phase == second => Some(Event::note_off(0, 0, KEY, 0)),
            _ => None,
        };
        let input: [&[f32]; 2] = [left_tone, right_tone];
        let input = if self.instrument { &[][..] } else { &input[..] };
        let [left, right] = &mut self.output;
        let played = self.endpoint.process_audio_f32(
            &BlockContext::new(QUANTUM),
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

fn run(helper: &Path, target: &Path, index: usize, seconds: u64) -> Result<bool, Error> {
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
    let mut config = chain.main_bus_config(rate, QUANTUM, input, &[Layout::Stereo])?;
    config.mode = plughost::ProcessMode::Realtime;
    chain.prepare_audio(&config)?;
    let endpoint = chain.realtime_endpoint(&RealtimeConfig::new(QUANTUM))?;
    let latency = chain.start_realtime()?;
    out!(
        "playing at {rate} Hz, latency {latency} frames plus up to {QUANTUM} for the device's callback size"
    );

    let mut callback = Callback {
        endpoint,
        instrument,
        rate,
        output: [vec![0.0; QUANTUM], vec![0.0; QUANTUM]],
        played: QUANTUM,
        tone: [vec![0.0; QUANTUM], vec![0.0; QUANTUM]],
        quanta: 0,
        produced: Vec::with_capacity(MAX_BLOCK_EVENTS),
    };
    let stream = device
        .build_output_stream::<f32, _, _>(
            device_config.config(),
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

    for second in 1..=seconds {
        std::thread::sleep(Duration::from_secs(1));
        let Some(status) = chain.realtime_status() else {
            break;
        };
        out!(
            "{second} s: {} processed, {} missed, {} late, latency {} frames",
            status.processed,
            status.missed,
            status.late,
            status.latency
        );
        if status.resync {
            out!("resynchronizing after {:?}", status.last_failure);
            chain.stop_realtime()?;
            chain.reset()?;
            chain.start_realtime()?;
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
    match run(helper, target, number(2, 0) as usize, number(3, 10)) {
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
