//! Test plugin in VST3 and CLAP from one binary: a stereo effect that delays its input by exactly
//! [`LATENCY`] samples and reports that latency. The VST3 side is a single component that
//! processes 32-bit and 64-bit samples, with a gain kept in its state and mapped to a MIDI
//! controller, a read-only parameter telling whether the last restored state was marked as
//! project state, and a parameter that switches the gain's MIDI controller (7 or 11) and reports
//! the change with `kMidiCCAssignmentChanged`. Asked for six channels, it answers 6.0 instead
//! of 5.1. The CLAP side (`clap.rs`) scales the delay with
//! the sample rate and adds a gain parameter kept in its state.
//!
//! The binary also plays faulty variants, each with its own class ID, selected at run time by the
//! name of the module the host loaded (see `variant.rs`), to test crash and hang isolation:
//! `crash-in-process`, `hang-in-process`, `crash-on-scan` (in `GetPluginFactory`), and
//! `hang-on-scan`. `noop-reset` reproduces a CLAP plugin whose reset leaves audio behind.
//! `stall-main-thread` makes the CLAP side hold up the host's main thread once, for
//! [`MAIN_THREAD_STALL`], from a main-thread callback it requests while processing. On macOS,
//! `editor` gives the CLAP side an editor inside the host's window (see `editor.rs`).
//! `restart-on-activate` makes the VST3 side report changed buses each time it activates.
//! `timers` gives the CLAP side two timers, the first of which removes the second (`timers.rs`).

#![allow(non_snake_case)]
// VST3 enum constants are i32 on Windows and u32 elsewhere, so a cast needed on one platform is
// a no-op on the other.
#![allow(clippy::unnecessary_cast)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, MutexGuard};

use vst3::Steinberg::Vst::ControllerNumbers_::{kCtrlExpression, kCtrlVolume};
use vst3::Steinberg::Vst::ParameterInfo_::ParameterFlags_;
use vst3::Steinberg::Vst::RestartFlags_::kMidiCCAssignmentChanged;
use vst3::Steinberg::Vst::{
    BusDirections_, BusInfo, BusInfo_, BusTypes_, CtrlNumber, IAttributeListTrait, IAudioProcessor,
    IAudioProcessorTrait, IComponent, IComponentHandler, IComponentHandlerTrait, IComponentTrait,
    IEditController, IEditControllerTrait, IHostApplication, IHostApplicationTrait, IMidiMapping,
    IMidiMappingTrait, IParamValueQueueTrait, IParameterChangesTrait, IProcessContextRequirements,
    IProcessContextRequirementsTrait, IStreamAttributes, IStreamAttributesTrait, IoMode,
    MediaTypes_, ParamID, ParamValue, ParameterInfo, ProcessData, ProcessSetup, RoutingInfo,
    SpeakerArr, SpeakerArrangement, String128, SymbolicSampleSizes_, TChar,
};
use vst3::Steinberg::{
    FIDString, FUnknown, IBStream, IBStreamTrait, IPlugView, IPluginBaseTrait, IPluginFactory,
    IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo_,
    PClassInfo2, PFactoryInfo, PFactoryInfo_, TBool, TUID, int16, int32, kInvalidArgument,
    kNotImplemented, kResultFalse, kResultOk, kResultTrue, tresult,
};
use vst3::{Class, ComPtr, ComRef, ComWrapper, uid};

mod clap;
#[cfg(target_os = "macos")]
mod editor;
mod errors;
mod state_payload;
mod timers;
mod units;
mod variant;

use variant::{Variant, variant};

/// The delay, and the latency the plugin reports, in samples (for CLAP, at 48 kHz).
pub const LATENCY: usize = 480;
/// How long the `stall-main-thread` variant holds up the host's main thread.
pub const MAIN_THREAD_STALL: std::time::Duration = std::time::Duration::from_secs(3);
/// VST3 parameters: the output gain, whether the last restored state was marked as project
/// state (read-only), and the MIDI controller mapped to the gain (0: volume, 1: expression).
const GAIN: ParamID = 0;
const PROJECT_STATE: ParamID = 1;
const GAIN_CONTROLLER: ParamID = 2;
const HOST_NAME: ParamID = 3;
const PROGRAM: ParamID = 6;
const EVENT_PROBE: ParamID = 7;
const PARAMETERS: [(&str, i32, ParamValue, i32); 8] = [
    ("Gain", 0, 1.0, ParameterFlags_::kCanAutomate),
    ("Project State", 1, 0.0, ParameterFlags_::kIsReadOnly),
    ("Gain Controller", 1, 0.0, ParameterFlags_::kIsList),
    ("Host Name", 0, 0.0, ParameterFlags_::kIsReadOnly),
    ("Transport probe", 1, 0.0, ParameterFlags_::kCanAutomate),
    ("Processed frames", 0, 0.0, ParameterFlags_::kIsReadOnly),
    (
        "Program",
        2,
        0.0,
        ParameterFlags_::kIsProgramChange | ParameterFlags_::kIsList,
    ),
    ("Event probe", 1, 0.0, ParameterFlags_::kCanAutomate),
];

fn class_id() -> TUID {
    uid(0x706C7567, 0x686F7374, 0x44656C61, variant().id())
}

/// Stops the process the way a crashing plugin does, or never returns, the way a plugin stuck in
/// a dialog or a deadlock does.
fn fail() {
    if matches!(variant(), Variant::CrashInProcess | Variant::CrashOnScan) {
        std::process::abort();
    }
    // The lifetime harness opts into a real descendant before this native call stops returning.
    let _descendant = std::env::var_os("PLUGHOST_TEST_DESCENDANT").map(|executable| {
        let directory = std::env::var_os("PLUGHOST_TEST_LIFETIME_DIR").expect(errors::DESCENDANT);
        std::fs::write(
            std::path::Path::new(&directory).join("helper.pid"),
            std::process::id().to_string(),
        )
        .expect(errors::DESCENDANT);
        std::process::Command::new(executable)
            .args(["--exact", "--ignored", "--nocapture", "lifetime_descendant"])
            .spawn()
            .expect(errors::DESCENDANT)
    });
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn program_gain(value: f64) -> f64 {
    [1.0, 0.25, 0.5][(value * 2.0).round() as usize]
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn copy_c(text: &str, out: &mut [c_char]) {
    for (byte, slot) in text.bytes().chain(std::iter::once(0)).zip(out.iter_mut()) {
        *slot = byte as c_char;
    }
}

fn copy_wide(text: &str, out: &mut [TChar]) {
    for (unit, slot) in text
        .encode_utf16()
        .chain(std::iter::once(0))
        .zip(out.iter_mut())
    {
        *slot = unit;
    }
}

struct Delay {
    host_name: Mutex<String>,
    lines: Mutex<Vec<VecDeque<f64>>>,
    /// The gain the processor applies, as delivered with parameter changes or restored state.
    gain: Mutex<f64>,
    /// Controller values, normalized, indexed by parameter ID.
    values: Mutex<[ParamValue; PARAMETERS.len()]>,
    handler: Mutex<Option<ComPtr<IComponentHandler>>>,
    /// The main buses' arrangement: stereo, or 6.0 after a six-channel proposal.
    arrangement: Mutex<SpeakerArrangement>,
    /// The thread that created the plugin, the only one VST3 lets call the edit controller.
    owner: std::thread::ThreadId,
}

// SAFETY: the component handler is only called from the thread the host calls the controller on.
unsafe impl Send for Delay {}
unsafe impl Sync for Delay {}

impl Delay {
    fn new() -> Delay {
        Delay {
            host_name: Mutex::new(String::new()),
            lines: Mutex::new(vec![VecDeque::from(vec![0.0; LATENCY]); 2]),
            gain: Mutex::new(1.0),
            values: Mutex::new(PARAMETERS.map(|(_, _, default, _)| default)),
            handler: Mutex::new(None),
            arrangement: Mutex::new(SpeakerArr::kStereo),
            owner: std::thread::current().id(),
        }
    }

    /// Reads the gain from a state stream.
    fn read_gain(state: *mut IBStream) -> Option<f64> {
        let stream = unsafe { ComRef::from_raw(state) }?;
        let mut bytes = [0u8; 8];
        let mut read = 0;
        let result = unsafe { stream.read(bytes.as_mut_ptr().cast(), 8, &mut read) };
        (result == kResultOk && read == 8).then(|| f64::from_le_bytes(bytes))
    }

    /// Whether the stream's attributes mark it as project state.
    fn is_project_state(state: *mut IBStream) -> bool {
        let Some(attributes) = unsafe { ComRef::from_raw(state) }
            .and_then(|stream| stream.cast::<IStreamAttributes>())
            .and_then(|stream| unsafe { ComRef::from_raw(stream.getAttributes()) })
        else {
            return false;
        };
        let mut value: [TChar; 64] = [0; 64];
        let found = unsafe {
            attributes.getString(
                c"StateType".as_ptr(),
                value.as_mut_ptr(),
                size_of_val(&value) as u32,
            )
        } == kResultOk;
        let project: Vec<TChar> = "Project".encode_utf16().chain([0]).collect();
        found && value.starts_with(&project)
    }

    /// Takes the last gain change of the block.
    fn apply_changes(&self, data: &ProcessData) {
        let Some(changes) = (unsafe { ComRef::from_raw(data.inputParameterChanges) }) else {
            return;
        };
        for index in 0..unsafe { changes.getParameterCount() } {
            let Some(queue) = (unsafe { ComRef::from_raw(changes.getParameterData(index)) }) else {
                continue;
            };
            let points = unsafe { queue.getPointCount() };
            let (mut offset, mut value) = (0, 0.0);
            if points > 0
                && unsafe { queue.getPoint(points - 1, &mut offset, &mut value) } == kResultOk
            {
                match unsafe { queue.getParameterId() } {
                    GAIN => *lock(&self.gain) = value,
                    PROGRAM => *lock(&self.gain) = program_gain(value),
                    _ => {}
                }
            }
        }
    }

    unsafe fn run<S: Copy + Into<f64>>(
        &self,
        inputs: *mut *mut S,
        outputs: *mut *mut S,
        frames: usize,
        from_f64: fn(f64) -> S,
        context: Option<&vst3::Steinberg::Vst::ProcessContext>,
        gains: &[f64],
    ) {
        let mut lines = lock(&self.lines);
        let probe = lock(&self.values)[4] >= 0.5;
        for (channel, line) in lines.iter_mut().enumerate() {
            let (input, output) = unsafe {
                (
                    std::slice::from_raw_parts(*inputs.add(channel), frames),
                    std::slice::from_raw_parts_mut(*outputs.add(channel), frames),
                )
            };
            for (index, (sample, out)) in input.iter().zip(output.iter_mut()).enumerate() {
                let value = if probe {
                    context.map_or(-1.0, |time| {
                        use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_ as Flags;
                        let valid = (Flags::kProjectTimeMusicValid | Flags::kTempoValid) as u32;
                        if time.state & valid != valid {
                            return -2.0;
                        }
                        let advance = if time.state & Flags::kPlaying as u32 != 0 {
                            index as f64 / time.sampleRate * time.tempo / 60.0
                        } else {
                            0.0
                        };
                        if channel == 1 {
                            return (time.projectTimeSamples as f64
                                + if time.state & Flags::kPlaying as u32 != 0 {
                                    index as f64
                                } else {
                                    0.0
                                })
                                / 10_000.0
                                + (time.continousTimeSamples as f64 + index as f64)
                                    / 100_000_000.0;
                        }
                        if (time.projectTimeMusic + advance).rem_euclid(1.0) < 0.25 {
                            f64::from(time.timeSigNumerator) / 4.0
                                * if time.state & Flags::kCycleActive as u32 != 0 {
                                    0.5
                                } else {
                                    1.0
                                }
                        } else {
                            0.0
                        }
                    })
                } else {
                    (*sample).into()
                };
                line.push_back(value);
                *out = from_f64(line.pop_front().unwrap_or_default() * gains[index]);
            }
        }
    }
}

impl Class for Delay {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        IMidiMapping,
        IProcessContextRequirements,
        vst3::Steinberg::Vst::IUnitInfo,
    );
}

impl IPluginBaseTrait for Delay {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        let Some(host) =
            (unsafe { ComRef::from_raw(context) }).and_then(|c| c.cast::<IHostApplication>())
        else {
            return kInvalidArgument;
        };
        let mut name = [0; 128];
        if unsafe { host.getName(&mut name) } != kResultOk {
            return kInvalidArgument;
        }
        let length = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        *lock(&self.host_name) = String::from_utf16_lossy(&name[..length]);
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Delay {
    unsafe fn getControllerClassId(&self, _classId: *mut TUID) -> tresult {
        kNotImplemented
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media: i32, _dir: i32) -> int32 {
        i32::from(media == MediaTypes_::kAudio as i32)
    }

    unsafe fn getBusInfo(&self, media: i32, dir: i32, index: int32, bus: *mut BusInfo) -> tresult {
        if media != MediaTypes_::kAudio as i32 || index != 0 {
            return kInvalidArgument;
        }
        let bus = unsafe { &mut *bus };
        bus.mediaType = media;
        bus.direction = dir;
        bus.channelCount = 2;
        copy_wide(
            if dir == BusDirections_::kInput as i32 {
                "Input"
            } else {
                "Output"
            },
            &mut bus.name,
        );
        bus.busType = BusTypes_::kMain as i32;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        kResultOk
    }

    unsafe fn getRoutingInfo(
        &self,
        _inInfo: *mut RoutingInfo,
        _outInfo: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(&self, _media: i32, _dir: i32, _index: int32, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state != 0 {
            for line in lock(&self.lines).iter_mut() {
                line.iter_mut().for_each(|sample| *sample = 0.0);
            }
            // Some plugins announce the buses they settle on as they activate.
            if variant() == Variant::RestartOnActivate
                && let Some(handler) = lock(&self.handler).clone()
            {
                use vst3::Steinberg::Vst::RestartFlags_::kIoChanged;
                unsafe { handler.restartComponent(kIoChanged as int32) };
            }
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(gain) = Delay::read_gain(state) else {
            return kResultFalse;
        };
        let project = if Delay::is_project_state(state) {
            1.0
        } else {
            0.0
        };
        lock(&self.values)[PROJECT_STATE as usize] = project;
        *lock(&self.gain) = gain;
        // A corrupt payload can partially mutate a real plugin before it reports failure.
        if gain < 0.0 {
            return kResultFalse;
        }
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(stream) = (unsafe { ComRef::from_raw(state) }) else {
            return kInvalidArgument;
        };
        let gain = *lock(&self.gain);
        let bytes = gain.to_le_bytes();
        let mut written = 0;
        let result = unsafe { stream.write(bytes.as_ptr().cast_mut().cast(), 8, &mut written) };
        if variant() == Variant::LargeState {
            state_payload::write(gain, |bytes| {
                // Deliberately ignore rejected writes, as a misbehaving native plugin might.
                unsafe {
                    stream.write(
                        bytes.as_ptr().cast_mut().cast(),
                        bytes.len() as i32,
                        &mut written,
                    )
                };
            });
        }
        result
    }
}

impl IAudioProcessorTrait for Delay {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        if num_ins != 1 || num_outs != 1 {
            return kResultFalse;
        }
        let (input, output) = unsafe { (*inputs, *outputs) };
        if input == SpeakerArr::kStereo && output == SpeakerArr::kStereo {
            *lock(&self.arrangement) = SpeakerArr::kStereo;
            // Some plugins answer kResultFalse even when they took the requested arrangements.
            return if variant() == Variant::ArrangementFalse {
                kResultFalse
            } else {
                kResultOk
            };
        }
        // Six channels are refused and answered with the closest arrangement the plugin has, as
        // the VST3 rules allow: same channel count, other speakers.
        if input.count_ones() == 6 && output.count_ones() == 6 {
            *lock(&self.arrangement) = SpeakerArr::k60Cine;
        }
        kResultFalse
    }

    unsafe fn getBusArrangement(
        &self,
        _dir: i32,
        index: int32,
        arrangement: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arrangement = *lock(&self.arrangement) };
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, _size: int32) -> tresult {
        kResultOk
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        if variant() == Variant::LatencyOverflow {
            u32::MAX
        } else {
            LATENCY as u32
        }
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        if *lock(&self.gain) == 42.0 {
            return kResultFalse;
        }
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &*data };
        let mut gains = vec![*lock(&self.gain); data.numSamples as usize];
        if let Some(changes) = unsafe { ComRef::from_raw(data.inputParameterChanges) } {
            for index in 0..unsafe { changes.getParameterCount() } {
                if let Some(queue) = unsafe { ComRef::from_raw(changes.getParameterData(index)) } {
                    for point in 0..unsafe { queue.getPointCount() } {
                        let (mut offset, mut value) = (0, 0.0);
                        if unsafe { queue.getPoint(point, &mut offset, &mut value) } == kResultOk {
                            match unsafe { queue.getParameterId() } {
                                GAIN if (offset as usize) < gains.len() => {
                                    gains[offset as usize..].fill(value)
                                }
                                PROGRAM if (offset as usize) < gains.len() => {
                                    gains[offset as usize..].fill(program_gain(value))
                                }
                                4 => lock(&self.values)[4] = value,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        self.apply_changes(data);
        if lock(&self.values)[EVENT_PROBE as usize] > 0.5
            && let Some(changes) = unsafe { ComRef::from_raw(data.outputParameterChanges) }
        {
            let mut index = 0;
            if let Some(queue) =
                unsafe { ComRef::from_raw(changes.addParameterData(&GAIN, &mut index)) }
            {
                unsafe {
                    queue.addPoint(0, 0.375, &mut index);
                }
            }
        }
        lock(&self.values)[5] += f64::from(data.numSamples) / 1_000_000.0;
        if data.numSamples == 0 || data.numInputs < 1 || data.numOutputs < 1 {
            return kResultOk;
        }
        if matches!(variant(), Variant::CrashInProcess | Variant::HangInProcess) {
            fail();
        }
        let (input, output) = unsafe { (&*data.inputs, &*data.outputs) };
        if input.numChannels != 2 || output.numChannels != 2 {
            return kInvalidArgument;
        }
        let frames = data.numSamples as usize;
        unsafe {
            if data.symbolicSampleSize == SymbolicSampleSizes_::kSample64 as i32 {
                self.run(
                    input.__field0.channelBuffers64,
                    output.__field0.channelBuffers64,
                    frames,
                    |v| v,
                    data.processContext.as_ref(),
                    &gains,
                );
            } else {
                self.run(
                    input.__field0.channelBuffers32,
                    output.__field0.channelBuffers32,
                    frames,
                    |v| v as f32,
                    data.processContext.as_ref(),
                    &gains,
                );
            }
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Delay {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        if let Some(gain) = Delay::read_gain(state) {
            lock(&self.values)[GAIN as usize] = gain;
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        if let Some(value) = Delay::read_gain(state) {
            lock(&self.values)[GAIN as usize] = value;
            if value < 0.0 {
                return kResultFalse;
            }
        }
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> int32 {
        PARAMETERS.len() as int32
    }

    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        let Some(&(title, steps, default, flags)) = PARAMETERS.get(index as usize) else {
            return kInvalidArgument;
        };
        let flags = if index == GAIN as i32 && lock(&self.values)[EVENT_PROBE as usize] > 0.5 {
            flags | ParameterFlags_::kIsReadOnly
        } else {
            flags
        };
        let info = unsafe { &mut *info };
        info.id = index as ParamID;
        copy_wide(title, &mut info.title);
        if index as ParamID == HOST_NAME {
            copy_wide(&lock(&self.host_name), &mut info.title);
        }
        copy_wide(title, &mut info.shortTitle);
        copy_wide(if index == 0 { "units" } else { "" }, &mut info.units);
        info.stepCount = steps;
        info.defaultNormalizedValue = default;
        info.unitId = if index == 0 || index as ParamID == PROGRAM {
            7
        } else {
            0
        };
        info.flags = flags;
        kResultOk
    }

    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        value: ParamValue,
        string: *mut String128,
    ) -> tresult {
        if id == PROGRAM {
            let Some(string) = (unsafe { string.as_mut() }) else {
                return kInvalidArgument;
            };
            copy_wide(
                ["Unity", "Quarter", "Half"][(value * 2.0).round() as usize],
                string,
            );
            return kResultOk;
        }
        if id == GAIN_CONTROLLER {
            if let Some(string) = unsafe { string.as_mut() } {
                copy_wide(if value < 0.5 { "Volume" } else { "Expression" }, string);
                return kResultOk;
            }
            return kInvalidArgument;
        }
        if id != GAIN {
            return kNotImplemented;
        }
        let Some(string) = (unsafe { string.as_mut() }) else {
            return kInvalidArgument;
        };
        copy_wide(&format!("{:.3} units", value * value * 100.0), string);
        kResultOk
    }

    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        string: *mut TChar,
        value: *mut ParamValue,
    ) -> tresult {
        if id != GAIN || string.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        let mut length = 0;
        while length < 4096 && unsafe { *string.add(length) } != 0 {
            length += 1;
        }
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string, length) });
        let Some(plain) = text
            .trim()
            .strip_suffix(" units")
            .and_then(|v| v.parse::<f64>().ok())
        else {
            return kResultFalse;
        };
        if !plain.is_finite() || !(0.0..=100.0).contains(&plain) {
            return kResultFalse;
        }
        unsafe {
            *value = (plain / 100.0).sqrt();
        }
        kResultOk
    }

    unsafe fn normalizedParamToPlain(&self, id: ParamID, value: ParamValue) -> ParamValue {
        if id == GAIN {
            value * value * 100.0
        } else {
            value
        }
    }

    unsafe fn plainParamToNormalized(&self, id: ParamID, value: ParamValue) -> ParamValue {
        if id == GAIN {
            (value / 100.0).sqrt()
        } else {
            value
        }
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        lock(&self.values).get(id as usize).copied().unwrap_or(0.0)
    }

    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        // A controller called from another thread (the audio thread) asks to be reloaded, which
        // makes the host's violation observable.
        if std::thread::current().id() != self.owner {
            if let Some(handler) = lock(&self.handler).clone() {
                use vst3::Steinberg::Vst::RestartFlags_::kReloadComponent;
                unsafe { handler.restartComponent(kReloadComponent as int32) };
            }
            return kResultFalse;
        }
        let changed = {
            let mut values = lock(&self.values);
            let Some(slot) = values.get_mut(id as usize) else {
                return kInvalidArgument;
            };
            let changed = *slot != value;
            *slot = value;
            if id == PROGRAM {
                values[GAIN as usize] = program_gain(value);
            }
            changed
        };
        let handler = lock(&self.handler).clone();
        if id == EVENT_PROBE
            && changed
            && let Some(handler) = &handler
        {
            use vst3::Steinberg::Vst::RestartFlags_::{kParamTitlesChanged, kParamValuesChanged};
            use vst3::Steinberg::Vst::{IComponentHandler2, IComponentHandler2Trait};
            unsafe {
                if value > 0.5 {
                    handler.beginEdit(GAIN);
                    handler.performEdit(GAIN, 0.25);
                    handler.endEdit(GAIN);
                }
                handler.restartComponent((kParamTitlesChanged | kParamValuesChanged) as int32);
                if let Some(extended) = handler.cast::<IComponentHandler2>() {
                    extended.setDirty(1);
                }
            }
        }
        if id == GAIN_CONTROLLER
            && changed
            && let Some(handler) = handler
        {
            unsafe { handler.restartComponent(kMidiCCAssignmentChanged as int32) };
        }
        kResultOk
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        *lock(&self.handler) = unsafe { ComRef::from_raw(handler) }.map(|h| h.to_com_ptr());
        kResultOk
    }

    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl IMidiMappingTrait for Delay {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: int32,
        _channel: int16,
        controller: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        let mapped = if lock(&self.values)[GAIN_CONTROLLER as usize] < 0.5 {
            kCtrlVolume
        } else {
            kCtrlExpression
        };
        if bus != 0 || controller as i64 != mapped as i64 {
            return kResultFalse;
        }
        unsafe { *id = GAIN };
        kResultTrue
    }
}

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = unsafe { &mut *info };
        copy_c("plughost", &mut info.vendor);
        copy_c("", &mut info.url);
        copy_c("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> int32 {
        1
    }

    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.cid = class_id();
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(variant().name(), &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if unsafe { *(cid as *const TUID) } != class_id() {
            return kInvalidArgument;
        }
        let Some(instance) = ComWrapper::new(Delay::new()).to_com_ptr::<FUnknown>() else {
            return kResultFalse;
        };
        let ptr = instance.as_ptr();
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.cid = class_id();
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(variant().name(), &mut info.name);
        info.classFlags = 0;
        copy_c("Fx|Delay", &mut info.subCategories);
        copy_c("plughost", &mut info.vendor);
        copy_c("0.0.0", &mut info.version);
        copy_c("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    if matches!(variant(), Variant::CrashOnScan | Variant::HangOnScan) {
        fail();
    }
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |factory| factory.into_raw())
}

impl IProcessContextRequirementsTrait for Delay {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        (1 << 1) | (1 << 2) | (1 << 6) | (1 << 7) | (1 << 10)
    }
}
