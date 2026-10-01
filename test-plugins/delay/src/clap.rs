//! The CLAP side of the test plugin: the same delay, but [`LATENCY`] long at 48 kHz and
//! proportional to the sample rate at others, so a host that reads the latency before activation
//! gets it wrong; plus a gain parameter kept in the state, so parameter changes and state round
//! trips can be tested, a range parameter that, set while inactive, doubles the gain's range
//! and announces it with `params.rescan`, and a tail parameter whose change, with a block or a
//! flush between blocks, the plugin reports from its audio thread.

use std::collections::VecDeque;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::HostGui;
use clack_extensions::latency::{HostLatency, PluginLatency, PluginLatencyImpl};
use clack_extensions::log::{HostLog, LogSeverity};
use clack_extensions::params::{
    HostParams, ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, ParamRescanFlags,
    PluginAudioProcessorParams, PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{HostState, PluginState, PluginStateImpl};
use clack_extensions::state_context::{
    PluginStateContext, PluginStateContextImpl, StateContextType,
};
use clack_extensions::tail::{HostTail, PluginTail, PluginTailImpl, TailLength};
use clack_plugin::entry::prelude::*;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use clack_plugin::utils::Cookie;

use crate::variant::{Variant, variant};
use crate::{LATENCY, state_payload};

const GAIN: ClapId = ClapId::new(0);
const RANGE: ClapId = ClapId::new(1);
/// The `editor` variant's report of its editor's keyboard focus (see `editor.rs`).
const EDITOR_FOCUS: ClapId = ClapId::new(15);
/// The `timers` variant's report of calls for a removed timer (see `timers.rs`).
const REMOVED_TIMER_CALLS: ClapId = ClapId::new(15);

fn id() -> String {
    format!("com.studio.plughost.test-delay.{:x}", variant().id())
}

pub struct TestPlugin;

impl Plugin for TestPlugin {
    type AudioProcessor<'a> = Processor<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginLatency>()
            .register::<PluginTail>()
            .register::<PluginParams>()
            .register::<PluginState>();
        if variant() != Variant::NoopReset {
            builder.register::<PluginStateContext>();
        }
        if variant() == Variant::Timers {
            builder.register::<clack_extensions::timer::PluginTimer>();
        }
        #[cfg(target_os = "macos")]
        if variant() == Variant::Editor {
            builder.register::<clack_extensions::gui::PluginGui>();
        }
    }
}

impl DefaultPluginFactory for TestPlugin {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new(&id(), variant().name())
    }

    fn new_shared(host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        if let Some(log) = host.get_extension::<HostLog>() {
            for severity in [
                LogSeverity::Debug,
                LogSeverity::Info,
                LogSeverity::Warning,
                LogSeverity::Error,
                LogSeverity::Fatal,
                LogSeverity::HostMisbehaving,
                LogSeverity::PluginMisbehaving,
            ] {
                log.log(&host, severity, c"fixture initialized");
            }
        }
        Ok(Shared {
            gui_requests_accepted: host.get_extension::<HostGui>().map(|gui| {
                [
                    gui.request_show(&host).is_ok(),
                    gui.request_hide(&host).is_ok(),
                ]
            }),
            identity: [host.name(), host.vendor(), host.version()]
                .map(|s| s.map(|s| s.to_bytes().to_vec()).unwrap_or_default()),
            saved_context: AtomicU32::new(0),
            loaded_context: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            restart_mode: AtomicU32::new(0),
            probe: AtomicU32::new(0),
            clock: AtomicU64::new(0),
            gain: AtomicU64::new(1.0f64.to_bits()),
            gain_max: AtomicU64::new(1.0f64.to_bits()),
            latency: AtomicU32::new(0),
            editor_focus: Arc::new(AtomicU32::new(0)),
            removed_timer_calls: AtomicU32::new(0),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<MainThread<'a>, PluginError> {
        let timers = (variant() == Variant::Timers)
            .then(|| crate::timers::Timers::register(&host))
            .flatten();
        Ok(MainThread {
            timers,
            shared,
            params: host.get_extension(),
            host,
            #[cfg(target_os = "macos")]
            editor: Default::default(),
        })
    }
}

pub struct Shared {
    saved_context: AtomicU32,
    loaded_context: AtomicU32,
    gui_requests_accepted: Option<[bool; 2]>,
    identity: [Vec<u8>; 3],
    gain: AtomicU64,
    probe: AtomicU32,
    tail: AtomicU32,
    restart_mode: AtomicU32,
    clock: AtomicU64,
    /// The upper end of the gain's range: 1, or 2 once the range parameter is set.
    gain_max: AtomicU64,
    /// The delay at the active sample rate, set on activation.
    latency: AtomicU32,
    /// What the `editor` variant's editor received, reported as `editor::FOCUS`.
    editor_focus: Arc<AtomicU32>,
    /// Calls the `timers` variant received for a timer it had removed, reported as
    /// `REMOVED_TIMER_CALLS`.
    pub(crate) removed_timer_calls: AtomicU32,
}

impl Shared {
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn gain_max(&self) -> f64 {
        f64::from_bits(self.gain_max.load(Ordering::Relaxed))
    }

    fn apply(&self, events: &InputEvents) {
        for event in events {
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
                && change.param_id() == Some(ClapId::new(7))
            {
                self.probe
                    .store(u32::from(change.value() >= 0.5), Ordering::Relaxed);
            }
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
                && change.param_id() == Some(GAIN)
            {
                self.gain.store(change.value().to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl PluginShared<'_> for Shared {}

pub struct MainThread<'a> {
    pub(crate) shared: &'a Shared,
    pub(crate) host: HostMainThreadHandle<'a>,
    pub(crate) timers: Option<crate::timers::Timers>,
    params: Option<HostParams>,
    #[cfg(target_os = "macos")]
    editor: crate::editor::Slot,
}

#[cfg(target_os = "macos")]
impl MainThread<'_> {
    pub(crate) fn editor(&self) -> &crate::editor::Slot {
        &self.editor
    }

    pub(crate) fn focus(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.shared.editor_focus)
    }
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {
    /// Holds up the host's main thread once, as a modal dialog would.
    fn on_main_thread(&self) {
        static STALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if variant() == Variant::StallMainThread && !STALLED.swap(true, Ordering::Relaxed) {
            std::thread::sleep(crate::MAIN_THREAD_STALL);
        }
    }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: if is_input { b"Input" } else { b"Output" },
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginLatencyImpl for MainThread<'_> {
    fn get(&self) -> u32 {
        self.shared.latency.load(Ordering::Relaxed)
    }
}

impl PluginMainThreadParams for MainThread<'_> {
    fn count(&self) -> u32 {
        15 + u32::from(matches!(variant(), Variant::Editor | Variant::Timers))
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        match index {
            15 if variant() == Variant::Timers => info.set(&ParamInfo {
                id: REMOVED_TIMER_CALLS,
                flags: ParamInfoFlags::IS_READONLY | ParamInfoFlags::IS_STEPPED,
                cookie: Cookie::empty(),
                name: b"Removed timer calls",
                module: b"",
                min_value: 0.0,
                max_value: 1000.0,
                default_value: 0.0,
            }),
            15 => info.set(&ParamInfo {
                id: EDITOR_FOCUS,
                flags: ParamInfoFlags::IS_READONLY | ParamInfoFlags::IS_STEPPED,
                cookie: Cookie::empty(),
                name: b"Editor focus",
                module: b"",
                min_value: 0.0,
                max_value: 2.0,
                default_value: 0.0,
            }),
            13..=14 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_READONLY
                    | ParamInfoFlags::IS_STEPPED
                    | ParamInfoFlags::IS_ENUM,
                cookie: Cookie::empty(),
                name: if index == 13 {
                    b"Constant"
                } else {
                    b"Large list"
                },
                module: b"",
                min_value: if index == 13 { 5.0 } else { 0.0 },
                max_value: if index == 13 { 5.0 } else { 999.0 },
                default_value: if index == 13 { 5.0 } else { 0.0 },
            }),
            11..=12 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_READONLY | ParamInfoFlags::IS_STEPPED,
                cookie: Cookie::empty(),
                name: if index == 11 {
                    b"Saved context"
                } else {
                    b"Loaded context"
                },
                module: b"",
                min_value: 0.0,
                max_value: 3.0,
                default_value: 0.0,
            }),
            0 => info.set(&ParamInfo {
                id: GAIN,
                flags: ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Cookie::empty(),
                name: b"Gain",
                module: b"Effects/Delay",
                min_value: 0.0,
                max_value: self.shared.gain_max(),
                default_value: 1.0,
            }),
            1 => info.set(&ParamInfo {
                id: RANGE,
                flags: ParamInfoFlags::IS_STEPPED | ParamInfoFlags::IS_ENUM,
                cookie: Cookie::empty(),
                name: b"Gain Range",
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: 0.0,
            }),
            2..=4 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_READONLY,
                cookie: Cookie::empty(),
                name: &self.shared.identity[index as usize - 2],
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: 0.0,
            }),
            9..=10 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_STEPPED | ParamInfoFlags::IS_AUTOMATABLE,
                cookie: Cookie::empty(),
                name: if index == 9 {
                    b"Tail samples"
                } else {
                    b"Restart mode"
                },
                module: b"",
                min_value: 0.0,
                max_value: if index == 9 { 32.0 } else { 2.0 },
                default_value: 0.0,
            }),
            7..=8 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: if index == 7 {
                    ParamInfoFlags::IS_STEPPED | ParamInfoFlags::IS_AUTOMATABLE
                } else {
                    ParamInfoFlags::IS_READONLY
                },
                cookie: Cookie::empty(),
                name: if index == 7 {
                    b"Transport probe"
                } else {
                    b"Processed frames"
                },
                module: b"",
                min_value: 0.0,
                max_value: if index == 7 { 1.0 } else { 1_000_000.0 },
                default_value: 0.0,
            }),
            5..=6 => info.set(&ParamInfo {
                id: ClapId::new(index),
                flags: ParamInfoFlags::IS_READONLY,
                cookie: Cookie::empty(),
                name: if index == 5 {
                    b"Show request accepted"
                } else {
                    b"Hide request accepted"
                },
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: 0.0,
            }),
            _ => {}
        }
    }

    fn get_value(&self, id: ClapId) -> Option<f64> {
        if id == REMOVED_TIMER_CALLS && variant() == Variant::Timers {
            return Some(f64::from(
                self.shared.removed_timer_calls.load(Ordering::Relaxed),
            ));
        }
        if id == EDITOR_FOCUS && variant() == Variant::Editor {
            return Some(f64::from(self.shared.editor_focus.load(Ordering::Relaxed)));
        }
        if id.get() == 13 {
            return Some(5.0);
        }
        if id.get() == 14 {
            return Some(0.0);
        }
        if id.get() == 11 {
            return Some(f64::from(self.shared.saved_context.load(Ordering::Relaxed)));
        }
        if id.get() == 12 {
            return Some(f64::from(
                self.shared.loaded_context.load(Ordering::Relaxed),
            ));
        }
        if id.get() == 9 {
            return Some(f64::from(self.shared.tail.load(Ordering::Relaxed)));
        }
        if id.get() == 10 {
            return Some(f64::from(self.shared.restart_mode.load(Ordering::Relaxed)));
        }
        if id.get() == 7 {
            return Some(f64::from(self.shared.probe.load(Ordering::Relaxed)));
        }
        if id.get() == 8 {
            return Some(self.shared.clock.load(Ordering::Relaxed) as f64);
        }
        if (5..=6).contains(&id.get()) {
            return self
                .shared
                .gui_requests_accepted
                .map(|accepted| f64::from(accepted[id.get() as usize - 5]));
        }
        if (2..=4).contains(&id.get()) {
            return Some(0.0);
        }
        match id {
            GAIN => Some(self.shared.gain()),
            RANGE => Some(if self.shared.gain_max() > 1.0 {
                1.0
            } else {
                0.0
            }),
            _ => None,
        }
    }

    fn value_to_text(
        &self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        if id.get() == 14 {
            return write!(writer, "Item {}", value as i64);
        }
        if id == RANGE {
            return writer.write_str(if value < 0.5 { "Normal" } else { "Double" });
        }
        write!(writer, "{value:.3}")
    }

    fn text_to_value(&self, _id: ClapId, text: &CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }

    /// Runs while inactive, when a range change is allowed.
    fn flush(&self, input: &InputEvents, output: &mut OutputEvents) {
        self.shared.apply(input);
        report_edits(input, output);
        if input
            .iter()
            .any(|event| matches!(event.as_core_event(), Some(CoreEventSpace::ParamValue(_))))
            && let Some(state) = self.host.get_extension::<HostState>()
        {
            state.mark_dirty(&self.host);
        }
        for event in input {
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
                && change.param_id() == Some(RANGE)
            {
                let max: f64 = if change.value() >= 0.5 { 2.0 } else { 1.0 };
                self.shared.gain_max.store(max.to_bits(), Ordering::Relaxed);
                if let Some(params) = &self.params {
                    params.rescan(&self.host, ParamRescanFlags::ALL);
                }
            }
        }
    }
}

impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.gain().to_le_bytes())?;
        if variant() == Variant::LargeState {
            state_payload::write(self.shared.gain(), |bytes| {
                let _ = output.write_all(bytes);
            });
        }
        Ok(())
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut bytes = [0; 8];
        input.read_exact(&mut bytes)?;
        self.shared
            .gain
            .store(f64::from_le_bytes(bytes).to_bits(), Ordering::Relaxed);
        if f64::from_le_bytes(bytes) < 0.0 {
            return Err(PluginError::Message(crate::errors::PARTIAL_STATE));
        }
        Ok(())
    }
}

impl PluginStateContextImpl for MainThread<'_> {
    fn save(
        &self,
        output: &mut OutputStream,
        context: StateContextType,
    ) -> Result<(), PluginError> {
        self.shared
            .saved_context
            .store(context.to_raw(), Ordering::Relaxed);
        PluginStateImpl::save(self, output)
    }
    fn load(&self, input: &mut InputStream, context: StateContextType) -> Result<(), PluginError> {
        self.shared
            .loaded_context
            .store(context.to_raw(), Ordering::Relaxed);
        PluginStateImpl::load(self, input)
    }
}

pub struct Processor<'a> {
    host: HostAudioProcessorHandle<'a>,
    log: Option<HostLog>,
    shared: &'a Shared,
    lines: [VecDeque<f32>; 2],
    rate: f64,
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor<'a> {
    fn activate(
        host: HostAudioProcessorHandle<'a>,
        main_thread: &MainThread<'a>,
        shared: &'a Shared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        if shared.gain() == 42.0 {
            return Err(PluginError::Message(crate::errors::PREPARE_STATE));
        }
        let restart_mode = shared.restart_mode.load(Ordering::Relaxed);
        let latency = (LATENCY as f64 * config.sample_rate / 48_000.0).round() as usize
            + 32 * restart_mode as usize;
        shared.latency.store(latency as u32, Ordering::Relaxed);
        if let Some(extension) = main_thread.host.get_extension::<HostLatency>() {
            extension.changed(&main_thread.host);
        }
        if restart_mode == 2 {
            host.shared().request_restart();
        }
        Ok(Processor {
            rate: config.sample_rate,
            log: host.get_extension(),
            host,
            shared,
            lines: [
                VecDeque::from(vec![0.0; latency]),
                VecDeque::from(vec![0.0; latency]),
            ],
        })
    }

    fn process(
        &mut self,
        process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        if let Some(log) = self.log {
            for _ in 0..256 {
                log.log(
                    &self.host.shared(),
                    LogSeverity::Warning,
                    c"fixture process log",
                );
            }
        }
        match variant() {
            Variant::HangInProcess => hang_while_logging(self.log, self.host.shared()),
            Variant::StallMainThread => self.host.shared().request_callback(),
            Variant::CrashInProcess => crate::fail(),
            _ => {}
        }
        let count = audio.frames_count() as u64;
        self.shared
            .clock
            .store(process.steady_time.unwrap_or(0) + count, Ordering::Relaxed);
        let mut gains = vec![self.shared.gain() as f32; count as usize];
        for event in events.input {
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
                && change.param_id() == Some(GAIN)
            {
                gains[change.header().time() as usize..].fill(change.value() as f32);
            }
        }
        self.shared.apply(events.input);
        report_edits(events.input, events.output);
        self.apply_timing(events.input);
        for mut port in &mut audio {
            let Some(channels) = port.channels()?.into_f32() else {
                continue;
            };
            for (channel, (pair, line)) in
                channels.into_iter().zip(self.lines.iter_mut()).enumerate()
            {
                let mut index = 0;
                let probe = self.shared.probe.load(Ordering::Relaxed) != 0;
                let rate = self.rate;
                let mut delay = |sample: f32| {
                    let sample = if probe {
                        process.transport.map_or(-1.0, |time| {
                            use clack_plugin::events::event_types::TransportFlags;
                            if !time.flags.contains(
                                TransportFlags::HAS_BEATS_TIMELINE | TransportFlags::HAS_TEMPO,
                            ) {
                                return -2.0;
                            }
                            let advance = if time.flags.contains(TransportFlags::IS_PLAYING) {
                                index as f64 / rate * time.tempo / 60.0
                            } else {
                                0.0
                            };
                            if channel == 1 {
                                return ((time.song_pos_seconds.to_float() * rate
                                    + if time.flags.contains(TransportFlags::IS_PLAYING) {
                                        index as f64
                                    } else {
                                        0.0
                                    })
                                    / 10_000.0
                                    + (process.steady_time.unwrap_or(0) as f64 + index as f64)
                                        / 100_000_000.0)
                                    as f32;
                            }
                            if (time.song_pos_beats.to_float() + advance).rem_euclid(1.0) < 0.25 {
                                f32::from(time.time_signature_numerator) / 4.0
                                    * if time.flags.contains(TransportFlags::IS_LOOP_ACTIVE) {
                                        0.5
                                    } else {
                                        1.0
                                    }
                            } else {
                                0.0
                            }
                        })
                    } else {
                        sample
                    };
                    let gain = gains[index];
                    index += 1;
                    line.push_back(sample);
                    line.pop_front().unwrap_or_default() * gain
                };
                match pair {
                    ChannelPair::InputOutput(input, output) => {
                        for (sample, out) in input.iter().zip(output.iter_mut()) {
                            *out = delay(*sample);
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for sample in buffer.iter_mut() {
                            *sample = delay(*sample);
                        }
                    }
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(buffer) => buffer.fill(0.0),
                }
            }
        }
        Ok(ProcessStatus::ContinueIfNotQuiet)
    }

    fn reset(&mut self) {
        if variant() != Variant::NoopReset {
            for line in &mut self.lines {
                line.iter_mut().for_each(|sample| *sample = 0.0);
            }
        }
    }
}

impl Processor<'_> {
    /// Applies the tail and restart mode parameters, reporting a tail change from this audio
    /// thread as CLAP asks, whether the edits arrive with a block or a flush between blocks.
    fn apply_timing(&mut self, input: &InputEvents) {
        for event in input {
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event() {
                match change.param_id().map(|id| id.get()) {
                    Some(9) => {
                        self.shared
                            .tail
                            .store(change.value() as u32, Ordering::Relaxed);
                        if let Some(tail) = self.host.get_extension::<HostTail>() {
                            tail.changed(&mut self.host);
                        }
                    }
                    Some(10) => {
                        self.shared
                            .restart_mode
                            .store(change.value() as u32, Ordering::Relaxed);
                        self.host.shared().request_restart();
                    }
                    _ => {}
                }
            }
        }
    }
}

impl PluginAudioProcessorParams for Processor<'_> {
    fn flush(&mut self, input: &InputEvents, output: &mut OutputEvents) {
        self.shared.apply(input);
        report_edits(input, output);
        self.apply_timing(input);
    }
}

/// This fixture reports each gain edit as a native plugin gesture to exercise host delivery.
fn report_edits(input: &InputEvents, output: &mut OutputEvents) {
    use clack_plugin::events::event_types::{ParamGestureBeginEvent, ParamGestureEndEvent};
    for event in input {
        if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
            && change.param_id() == Some(GAIN)
        {
            let time = change.header().time();
            let _ = output.try_push(ParamGestureBeginEvent::new(time, GAIN));
            let _ = output.try_push(change);
            let _ = output.try_push(ParamGestureEndEvent::new(time, GAIN));
        }
    }
}

fn hang_while_logging(log: Option<HostLog>, host: HostSharedHandle<'_>) {
    loop {
        if let Some(log) = log {
            log.log(&host, LogSeverity::Warning, c"fixture hangs while logging");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The entry, which fails while the host scans for the scan-failure variants.
pub struct TestEntry(SinglePluginEntry<TestPlugin>);

impl Entry for TestEntry {
    fn new(bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        if matches!(variant(), Variant::CrashOnScan | Variant::HangOnScan) {
            crate::fail();
        }
        SinglePluginEntry::new(bundle_path).map(TestEntry)
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        self.0.declare_factories(builder);
    }
}

clack_export_entry!(TestEntry);

impl PluginTailImpl for Processor<'_> {
    fn get(&self) -> TailLength {
        TailLength::Finite(self.shared.tail.load(Ordering::Relaxed))
    }
}
