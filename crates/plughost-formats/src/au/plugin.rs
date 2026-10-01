//! One Audio Unit, v2 (through the system bridge) or v3, through `AUAudioUnit`.

mod audio;
mod bypass;
mod editor;
mod events;
mod invalidation;
mod midi_output;
mod musical_context;
mod parameters;
mod programs;
mod render_audio;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_audio_toolbox::{
    AUAudioFrameCount, AUAudioUnit, AUAudioUnitStatus, AUAudioUnitV2Bridge, AUEventSampleTime,
    AUHostTransportStateFlags, AUParameter, AUParameterAddress, AUParameterListenerNotify, AUValue,
    AudioComponentDescription, AudioComponentInstantiationOptions, AudioUnitGetParameter,
    AudioUnitParameter, AudioUnitParameterOptions, AudioUnitParameterUnit,
    AudioUnitRenderActionFlags, AudioUnitSetParameter, kAudioUnitScope_Global,
};
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use objc2_foundation::{NSError, NSInteger, NSPropertyListFormat};
use plughost_core::render::Tail;
use plughost_core::{Capabilities, Support};
use plughost_core::{
    Event, EventData, ParameterInfo, PluginFormat, PluginInfo, PluginState, PluginTiming,
    ProcessConfig, ProcessMode, SampleFormat, events_fit,
};

use super::components::{components, parse_class_id};
use super::errors::AuError;
use super::plist::{self, State};
use super::wait::{Slot, run_loop_for};

type PullInput = dyn Fn(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    AUAudioFrameCount,
    NSInteger,
    NonNull<AudioBufferList>,
) -> AUAudioUnitStatus;
type Render = dyn Fn(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    AUAudioFrameCount,
    NSInteger,
    NonNull<AudioBufferList>,
    *mut DynBlock<PullInput>,
) -> AUAudioUnitStatus;
type Schedule = dyn Fn(AUEventSampleTime, AUAudioFrameCount, AUParameterAddress, AUValue);
type ScheduleMidi = dyn Fn(AUEventSampleTime, u8, NSInteger, NonNull<u8>);
type MusicalContext =
    dyn Fn(*mut f64, *mut f64, *mut NSInteger, *mut f64, *mut NSInteger, *mut f64) -> Bool;
type Transport = dyn Fn(*mut AUHostTransportStateFlags, *mut f64, *mut f64, *mut f64) -> Bool;

/// How long the run loop runs before saving after host edits. Some v2 units (Softube's, measured)
/// update the model they save from parameter listener notifications, which arrive through the
/// run loop about 20 ms after the edit.
pub const EDIT_SETTLE: Duration = Duration::from_millis(100);
/// How long a v2 unit must stay alive after a parameter listener notification. AudioToolbox sets
/// up the bridge's delivery of it asynchronously and crashes if the unit is released first.
const NOTIFY_SETTLE: Duration = Duration::from_millis(50);
/// Tails reported beyond this are treated as infinite.
const MAX_TAIL_SECONDS: f64 = 3600.0;

/// Values handed between the completion queue of an asynchronous call and the waiting thread.
struct Handoff<T>(T);
// SAFETY: the value is only moved from the completion handler to the thread that waits for it.
unsafe impl<T> Send for Handoff<T> {}

struct Engine {
    unit: Option<Retained<AUAudioUnit>>,
    prepared: Option<Prepared>,
    invalidated: Arc<AtomicBool>,
    /// MIDI output messages with no MIDI 1.0 event form, since diagnostics last took them.
    unconvertible: Arc<AtomicU64>,
}

// SAFETY: AUAudioUnit may be rendered from a processing thread and configured from the owning
// thread; the engine mutex serializes the calls this crate makes.
unsafe impl Send for Engine {}

struct Prepared {
    config: plughost_core::AudioConfig,
    buffers: render_audio::Buffers,
    input_channels: usize,
    output_channels: usize,
    /// The unit's blocks, copied: the getters return them autoreleased.
    render: RcBlock<Render>,
    schedule: RcBlock<Schedule>,
    /// None for units without MIDI input.
    schedule_midi: Option<RcBlock<ScheduleMidi>>,
    /// Collects the unit's MIDI output when event outputs were requested.
    midi_output: Option<midi_output::Output>,
    /// Parameter ranges, to turn normalized changes into plain values.
    ranges: HashMap<u64, (f32, f32, ParameterInfo)>,
    /// Samples rendered so far: the render timestamp of the next block.
    position: f64,
    timeline: Arc<Mutex<Option<plughost_core::Transport>>>,
    transport_changed: Arc<AtomicBool>,
    /// The length of the last block rendered.
    last_frames: usize,
    /// The latency as render resources were allocated, which holds until they are allocated
    /// again, and whether the unit has reported another one since.
    latency: u32,
    latency_changed: bool,
    /// The tail the owning thread last read.
    tail: Tail,
    _context: RcBlock<MusicalContext>,
    _transport: RcBlock<Transport>,
}

/// Writes through an output pointer the Audio Unit may pass as null.
fn put<T>(pointer: *mut T, value: T) {
    // SAFETY: the Audio Unit passes either null or a valid pointer.
    if let Some(slot) = unsafe { pointer.as_mut() } {
        *slot = value;
    }
}

fn lock(engine: &Mutex<Engine>) -> MutexGuard<'_, Engine> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

pub struct Plugin {
    info: PluginInfo,
    engine: Arc<Mutex<Engine>>,
    editor: Option<editor::Editor>,
    /// Parameters were set since the last save; see [`EDIT_SETTLE`].
    edited: bool,
    /// A state restored before render resources were first allocated, applied again after. Some
    /// v2 units (Soundtoys EchoBoy) set up their parameters on allocation and drop a state
    /// restored earlier.
    restored_unprepared: Option<(Retained<State>, plughost_core::StatePurpose)>,
    observation: Option<events::Observation>,
    /// None for v3 units.
    value_strings: Option<events::ValueStrings>,
    _invalidation: invalidation::Observation,
    parameter_metadata_changed: bool,
    /// The plain value this host last set per parameter of a v2 unit. The v2 bridge reports host
    /// edits back as if the unit made them.
    host_values: HashMap<u64, f32>,
    /// When this host last notified a v2 unit's parameter listeners; see [`NOTIFY_SETTLE`].
    notified: Option<std::time::Instant>,
    unconvertible: Arc<AtomicU64>,
}

impl Plugin {
    /// Instantiates the registered Audio Unit with `class_id`. AUv3 units run in their extension
    /// process, as macOS does by default.
    pub fn new(class_id: &str) -> Result<Plugin, AuError> {
        let description = parse_class_id(class_id)?;
        let info = components()
            .into_iter()
            .find(|info| info.class_id == class_id)
            .ok_or_else(|| AuError::Instantiate(class_id.to_owned()))?;
        let unit = instantiate(description)?;
        let invalidated = Arc::new(AtomicBool::new(false));
        let invalidation = invalidation::Observation::new(&unit, invalidated.clone());
        let value_strings = events::ValueStrings::new(&unit);
        let unconvertible = Arc::new(AtomicU64::new(0));
        let mut plugin = Plugin {
            info,
            engine: Arc::new(Mutex::new(Engine {
                unit: Some(unit),
                prepared: None,
                invalidated,
                unconvertible: Arc::clone(&unconvertible),
            })),
            editor: None,
            edited: false,
            restored_unprepared: None,
            observation: None,
            value_strings,
            _invalidation: invalidation,
            parameter_metadata_changed: false,
            host_values: HashMap::new(),
            notified: None,
            unconvertible,
        };
        plugin.refresh_parameter_observer();
        plugin.parameter_metadata_changed = false;
        Ok(plugin)
    }

    pub(crate) fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// Reports MIDI output messages that have no MIDI 1.0 event form, which are not delivered.
    pub(crate) fn take_diagnostics(&self) -> plughost_core::DiagnosticBatch {
        let unconvertible = self.unconvertible.swap(0, Ordering::Relaxed);
        plughost_core::DiagnosticBatch {
            records: (unconvertible > 0)
                .then(|| plughost_core::Diagnostic {
                    severity: plughost_core::DiagnosticSeverity::Warning,
                    message: super::errors::unconvertible_output_events(unconvertible),
                    plugin: None,
                    slot: None,
                    failure_kind: None,
                    truncated: false,
                })
                .into_iter()
                .collect(),
            dropped: 0,
        }
    }

    /// Queries native metadata without requesting a view controller or saving state.
    pub(crate) fn capabilities(&self) -> Result<Capabilities, AuError> {
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        Ok({
            let mut capabilities = Capabilities::default();
            capabilities.embedded_editor = if self.editor.is_some() {
                Support::Supported
            } else {
                Support::Unknown
            };
            capabilities.bus_discovery = Support::Supported;
            capabilities.note_input = (!unsafe { unit.scheduleMIDIEventBlock() }.is_null()).into();
            capabilities.note_output = (!unsafe { unit.MIDIOutputNames() }.is_empty()).into();
            capabilities.sample_accurate_automation =
                (!unsafe { unit.scheduleParameterBlock() }.is_null()).into();
            capabilities.factory_presets = unsafe { unit.factoryPresets() }
                .is_some_and(|presets| !presets.is_empty())
                .into();
            capabilities.f32 = if engine.prepared.is_some() {
                Support::Supported
            } else {
                Support::Unknown
            };
            capabilities.f64 = Support::Unsupported;
            capabilities
        })
    }

    /// Reserves host audio storage for the maximum block size. Larger maxima increase memory
    /// requirements; native plugin code and control callbacks may still allocate while processing.
    pub(crate) fn prepare(&mut self, config: &ProcessConfig) -> Result<(), AuError> {
        let midi_input = self.midi_input()?;
        let audio_config = {
            let engine = lock(&self.engine);
            audio::main_bus_config(engine.unit()?, config, midi_input)?
        };
        self.prepare_audio(&audio_config).map(|_| ())
    }

    /// Processes one block; `events` go to the unit's MIDI input on port 0 when preparation
    /// requested it. `produced` receives the MIDI the unit sent on the prepared outputs.
    pub fn process(
        &mut self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), AuError> {
        produced.clear();
        lock(&self.engine).process(context, input, output, automation, events, produced)
    }

    pub(crate) fn processor(&self) -> Processor {
        Processor {
            engine: Arc::clone(&self.engine),
        }
    }

    /// The prepared unit's timing, with its latency and tail read again on this thread.
    pub(crate) fn timing(&self) -> Result<PluginTiming, AuError> {
        let mut engine = lock(&self.engine);
        engine.refresh_timing()?;
        engine.timing()
    }

    /// Each parameter with its current value, normalized over its range.
    pub(crate) fn parameters(&self) -> Vec<(ParameterInfo, f64)> {
        let engine = lock(&self.engine);
        engine
            .unit()
            .ok()
            .map(|unit| {
                all_parameters(unit)
                    .iter()
                    .map(|p| (parameter_info(p), normalized(p, plain_value(unit, p))))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Sets a parameter's normalized value; the Audio Unit passes it to its render thread.
    pub(crate) fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), AuError> {
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        let parameter = (unsafe { unit.parameterTree() })
            .and_then(|tree| unsafe { tree.parameterWithAddress(id) })
            .ok_or(AuError::Input(
                plughost_core::InputError::UnknownParameter { id },
            ))?;
        let info = parameter_info(&parameter);
        info.validate_edit(value).map_err(AuError::Input)?;
        let (min, max) = unsafe { (parameter.minValue(), parameter.maxValue()) };
        let plain = plain(min, max, &info, value);
        self.edited = true;
        // For a v2 unit, set the parameter the way v2 hosts do and notify listeners, which is how
        // some plugins keep their own model, and so their saved state, in step. The v3 path
        // updates the DSP of such plugins but not their saved state.
        if let (Some(bridge), Ok(parameter_id)) = (
            unit.downcast_ref::<AUAudioUnitV2Bridge>(),
            u32::try_from(id),
        ) {
            let audio_unit = unsafe { bridge.audioUnit() };
            let mut target = AudioUnitParameter {
                mAudioUnit: audio_unit,
                mParameterID: parameter_id,
                mScope: kAudioUnitScope_Global,
                mElement: 0,
            };
            unsafe {
                AudioUnitSetParameter(
                    audio_unit,
                    parameter_id,
                    kAudioUnitScope_Global,
                    0,
                    plain,
                    0,
                );
                AUParameterListenerNotify(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    NonNull::from(&mut target),
                );
            }
            self.host_values.insert(id, plain);
            self.notified = Some(std::time::Instant::now());
        } else {
            // The observer token as originator keeps the edit from being reported back.
            let token = self
                .observation
                .as_ref()
                .map_or(std::ptr::null_mut(), events::Observation::token);
            unsafe { parameter.setValue_originator(plain, token) };
        }
        Ok(())
    }

    /// Saves the full state. After [`Plugin::set_parameter`], the run loop runs briefly first,
    /// so plugins that follow host edits through parameter listeners include them in their state.
    pub(crate) fn save_state(
        &mut self,
        purpose: plughost_core::StatePurpose,
    ) -> Result<PluginState, AuError> {
        if purpose == plughost_core::StatePurpose::Duplicate {
            return Err(AuError::StatePurposeUnsupported);
        }
        if std::mem::take(&mut self.edited) {
            run_loop_for(EDIT_SETTLE);
        }
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        let state = unsafe {
            match purpose {
                plughost_core::StatePurpose::Project => unit.fullStateForDocument(),
                _ => unit.fullState(),
            }
        }
        .ok_or(AuError::State)?;
        let component = plist::encode(&state, NSPropertyListFormat::BinaryFormat_v1_0)?;
        Ok(PluginState {
            format: PluginFormat::AudioUnit,
            class_id: self.info.class_id.clone(),
            name: self.info.name.clone(),
            vendor: self.info.vendor.clone(),
            version: self.info.version.clone(),
            component,
            controller: Vec::new(),
        })
    }

    pub(crate) fn restore_state(
        &mut self,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), AuError> {
        state.validate().map_err(AuError::Input)?;
        if purpose == plughost_core::StatePurpose::Duplicate {
            return Err(AuError::StatePurposeUnsupported);
        }
        if state.format != PluginFormat::AudioUnit || state.class_id != self.info.class_id {
            return Err(AuError::StateMismatch);
        }
        let dictionary = plist::decode(&state.component)?;
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        apply_state(unit, &dictionary, purpose);
        self.restored_unprepared = engine.prepared.is_none().then_some((dictionary, purpose));
        Ok(())
    }

    /// Clears the unit's internal audio state with its native `reset`, keeping its settings.
    pub(crate) fn reset(&mut self) -> Result<(), AuError> {
        let engine = lock(&self.engine);
        unsafe { engine.unit()?.reset() };
        Ok(())
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.close_editor();
        self.observation = None;
        self.value_strings = None;
        if let Some(notified) = self.notified {
            std::thread::sleep(NOTIFY_SETTLE.saturating_sub(notified.elapsed()));
        }
        let mut engine = lock(&self.engine);
        engine.unprepare();
        engine.unit = None;
    }
}

fn instantiate(description: AudioComponentDescription) -> Result<Retained<AUAudioUnit>, AuError> {
    let slot: Slot<Handoff<Result<Retained<AUAudioUnit>, String>>> = Slot::new();
    let fill = slot.clone();
    let completion = RcBlock::new(move |unit: *mut AUAudioUnit, error: *mut NSError| {
        let result = match unsafe { Retained::retain(unit) } {
            Some(unit) => Ok(unit),
            None => Err(unsafe { error.as_ref() }.map(describe).unwrap_or_default()),
        };
        fill.fill(Handoff(result));
    });
    unsafe {
        AUAudioUnit::instantiateWithComponentDescription_options_completionHandler(
            description,
            AudioComponentInstantiationOptions::empty(),
            &completion,
        );
    }
    slot.wait().0.map_err(AuError::Instantiate)
}

fn all_parameters(unit: &AUAudioUnit) -> Vec<Retained<AUParameter>> {
    let Some(tree) = (unsafe { unit.parameterTree() }) else {
        return Vec::new();
    };
    let parameters = unsafe { tree.allParameters() };
    (0..parameters.count())
        .map(|i| parameters.objectAtIndex(i))
        .collect()
}

/// Each parameter's range and description, to turn normalized changes into plain values.
fn ranges(unit: &AUAudioUnit) -> HashMap<u64, (f32, f32, ParameterInfo)> {
    all_parameters(unit)
        .iter()
        .map(|p| unsafe { (p.address(), (p.minValue(), p.maxValue(), parameter_info(p))) })
        .collect()
}

/// A parameter's current plain value. A v2 unit's own value is read the way
/// [`Plugin::set_parameter`] writes it: the bridge's parameter tree catches up with edits
/// asynchronously, and a tree it rebuilds after allocation can hold values from before them.
fn plain_value(unit: &AUAudioUnit, parameter: &AUParameter) -> f32 {
    let address = unsafe { parameter.address() };
    if let (Some(bridge), Ok(id)) = (
        unit.downcast_ref::<AUAudioUnitV2Bridge>(),
        u32::try_from(address),
    ) {
        let mut value = 0.0;
        let status = unsafe {
            AudioUnitGetParameter(
                bridge.audioUnit(),
                id,
                kAudioUnitScope_Global,
                0,
                NonNull::from(&mut value),
            )
        };
        if status == 0 {
            return value;
        }
    }
    unsafe { parameter.value() }
}

/// The plain value of normalized `value` over `min..=max`, whole for a discrete parameter. It is
/// computed in double precision: in single precision 13 / 22 of 22 is 12.999999, which units that
/// truncate read as 12.
fn plain(min: f32, max: f32, info: &ParameterInfo, value: f64) -> f32 {
    let plain = f64::from(min) + value * (f64::from(max) - f64::from(min));
    (if info.flags.discrete {
        plain.round()
    } else {
        plain
    }) as f32
}

/// `plain` normalized over the parameter's range.
fn normalized(parameter: &AUParameter, plain: f32) -> f64 {
    let (min, max) = unsafe { (parameter.minValue(), parameter.maxValue()) };
    if max > min {
        f64::from((plain - min) / (max - min))
    } else {
        0.0
    }
}

fn parameter_info(parameter: &AUParameter) -> ParameterInfo {
    let (min, max) = unsafe { (parameter.minValue(), parameter.maxValue()) };
    let options = unsafe { parameter.flags() };
    let unit = unsafe { parameter.unit() };
    let discrete =
        unit == AudioUnitParameterUnit::Indexed || unit == AudioUnitParameterUnit::Boolean;
    let title = unsafe { parameter.displayName() }.to_string();
    // AUParameter does not expose a default; never substitute the current value.
    let mut info = ParameterInfo::new(unsafe { parameter.address() }, title.clone());
    info.short_title = title.chars().take(8).collect();
    info.units = unsafe { parameter.unitName() }
        .map(|name| name.to_string())
        .unwrap_or_default();
    info.step_count = if discrete {
        (max - min).round().max(0.0) as u32
    } else {
        0
    };
    let writable = options.contains(AudioUnitParameterOptions::Flag_IsWritable);
    info.flags.discrete = discrete;
    info.flags.automatable = writable;
    info.flags.read_only = !writable;
    info.flags.list = unsafe { parameter.valueStrings() }.is_some();
    info
}

impl Engine {
    fn unit(&self) -> Result<&AUAudioUnit, AuError> {
        if self.invalidated.load(Ordering::Acquire) {
            return Err(AuError::Invalidated);
        }
        self.unit.as_deref().ok_or(AuError::Closed)
    }
    fn parts(&self) -> Result<(&AUAudioUnit, &Prepared), AuError> {
        let unit = self.unit()?;
        let prepared = self.prepared.as_ref().ok_or(AuError::NotPrepared)?;
        Ok((unit, prepared))
    }

    fn prepare_audio(&mut self, audio_config: &plughost_core::AudioConfig) -> Result<(), AuError> {
        audio_config.validate().map_err(AuError::Input)?;
        if audio_config.sample_format != SampleFormat::F32 {
            return Err(AuError::SampleFormatUnsupported(audio_config.sample_format));
        }
        let unit = self.unit()?;
        audio::validate_configuration(unit, audio_config)?;
        self.unprepare();
        let unit = self.unit()?;
        let (input_buses, output_buses) = audio::configure(unit, audio_config)?;
        let input_channels = input_buses.iter().map(|bus| bus.channels.len()).sum();
        let output_channels = output_buses.iter().map(|bus| bus.channels.len()).sum();
        let buffers =
            render_audio::Buffers::new(input_buses, output_buses, audio_config.max_block_size)?;
        let config = audio_config;
        // AUAudioFrameCount is 32 bits, but the v2 bridge declares this setter with NSUInteger;
        // objc2's message check requires the width the receiving class declares.
        let wide = unit
            .class()
            .instance_method(objc2::sel!(setMaximumFramesToRender:))
            .and_then(|method| method.argument_type(2))
            .is_some_and(|encoding| encoding.to_bytes() == b"Q");
        unsafe {
            if wide {
                let () = msg_send![unit, setMaximumFramesToRender: config.max_block_size as u64];
            } else {
                unit.setMaximumFramesToRender(config.max_block_size as AUAudioFrameCount);
            }
            unit.setRenderingOffline(config.mode == ProcessMode::Offline);
        }

        let sample_rate = config.sample_rate;
        let timeline: Arc<Mutex<Option<plughost_core::Transport>>> = Arc::new(Mutex::new(None));
        let context_timeline = Arc::clone(&timeline);
        let context: RcBlock<MusicalContext> = RcBlock::new(
            move |tempo: *mut f64,
                  numerator: *mut f64,
                  denominator: *mut NSInteger,
                  beat: *mut f64,
                  to_next_beat: *mut NSInteger,
                  downbeat: *mut f64| {
                let snapshot = *context_timeline.lock().unwrap_or_else(|p| p.into_inner());
                musical_context::fill(
                    snapshot,
                    sample_rate,
                    musical_context::Outputs {
                        tempo,
                        numerator,
                        denominator,
                        beat,
                        to_next_beat,
                        downbeat,
                    },
                )
            },
        );
        let transport_timeline = Arc::clone(&timeline);
        let transport_changed = Arc::new(AtomicBool::new(false));
        let changed = Arc::clone(&transport_changed);
        let transport: RcBlock<Transport> = RcBlock::new(
            move |flags: *mut AUHostTransportStateFlags,
                  sample: *mut f64,
                  cycle_start: *mut f64,
                  cycle_end: *mut f64| {
                let snapshot = *transport_timeline.lock().unwrap_or_else(|p| p.into_inner());
                let Some(value) = snapshot else {
                    return Bool::NO;
                };
                let mut state = AUHostTransportStateFlags::empty();
                state.set(
                    AUHostTransportStateFlags::Changed,
                    changed.load(Ordering::Relaxed),
                );
                state.set(AUHostTransportStateFlags::Moving, value.playing);
                state.set(
                    AUHostTransportStateFlags::Cycling,
                    value.loop_region.is_some(),
                );
                put(flags, state);
                put(sample, value.sample_position as f64);
                if let Some(region) = value.loop_region {
                    put(cycle_start, region.start);
                    put(cycle_end, region.end);
                }
                Bool::YES
            },
        );
        unsafe {
            unit.setMusicalContextBlock(RcBlock::as_ptr(&context));
            unit.setTransportStateBlock(RcBlock::as_ptr(&transport));
        }
        // Units read the output block when they allocate render resources.
        let midi_output = if audio_config.events.outputs.is_empty() {
            unsafe { unit.setMIDIOutputEventBlock(std::ptr::null_mut()) };
            None
        } else {
            Some(midi_output::Output::install(
                unit,
                audio_config.events.outputs.clone(),
                Arc::clone(&self.unconvertible),
            )?)
        };
        unsafe {
            unit.allocateRenderResourcesAndReturnError()
                .map_err(|error| AuError::Allocate(describe(&error)))?;
        }
        let ranges = ranges(unit);
        let latency = read_latency(unit, audio_config.sample_rate);
        let tail = read_tail(unit, audio_config.sample_rate);
        // SAFETY: the getters return valid blocks of these types, or null.
        let (render, schedule, schedule_midi) = unsafe {
            (
                RcBlock::copy(unit.renderBlock().cast()).ok_or(AuError::RenderBlocks)?,
                RcBlock::copy(unit.scheduleParameterBlock().cast()).ok_or(AuError::RenderBlocks)?,
                RcBlock::copy(unit.scheduleMIDIEventBlock().cast()),
            )
        };
        self.prepared = Some(Prepared {
            config: audio_config.clone(),
            buffers,
            input_channels,
            output_channels,
            render,
            schedule,
            schedule_midi,
            midi_output,
            ranges,
            position: 0.0,
            timeline,
            transport_changed,
            last_frames: 0,
            latency,
            latency_changed: false,
            tail,
            _context: context,
            _transport: transport,
        });
        Ok(())
    }

    fn unprepare(&mut self) {
        if let (Some(_), Ok(unit)) = (self.prepared.take(), self.unit()) {
            unsafe { unit.deallocateRenderResources() };
        }
    }

    /// The timing read on the owning thread; no call into the unit.
    fn timing(&self) -> Result<PluginTiming, AuError> {
        let (_, prepared) = self.parts()?;
        let mut timing = PluginTiming::new(prepared.latency, prepared.tail);
        timing.restart_required = self.invalidated.load(Ordering::Acquire);
        timing.latency_changed = prepared.latency_changed;
        Ok(timing)
    }

    /// Reads the latency and tail again on the owning thread. A unit changes its latency
    /// whenever it likes; the host keeps the alignment of the latency it was prepared with and
    /// reports the change.
    fn refresh_timing(&mut self) -> Result<(), AuError> {
        let (unit, prepared) = self.parts()?;
        let sample_rate = prepared.config.sample_rate;
        let (latency, tail) = (
            read_latency(unit, sample_rate),
            read_tail(unit, sample_rate),
        );
        let prepared = self.prepared.as_mut().ok_or(AuError::NotPrepared)?;
        prepared.latency_changed |= latency != prepared.latency;
        prepared.tail = tail;
        Ok(())
    }

    fn process(
        &mut self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), AuError> {
        let (_, prepared) = self.parts()?;
        context.validate().map_err(AuError::Input)?;
        if let Some(transport) = context.transport {
            transport
                .validate_at_rate(prepared.config.sample_rate)
                .map_err(AuError::Input)?;
        }
        plughost_core::validate_event_budget(automation.len(), events).map_err(AuError::Input)?;
        let midi_input = !prepared.config.events.inputs.is_empty();
        if let Some(event) = events.iter().find(|event| event.port != 0 || !midi_input) {
            return Err(AuError::Input(plughost_core::InputError::EventPort {
                port: event.port,
            }));
        }
        let frames = context.frames;
        if input.len() != prepared.input_channels
            || output.len() != prepared.output_channels
            || frames > prepared.config.max_block_size
            || input.iter().any(|c| c.len() != frames)
            || output.iter().any(|c| c.len() != frames)
            || !plughost_core::changes_fit(automation, frames)
            || !events_fit(events, frames)
        {
            return Err(AuError::Buffers);
        }
        if frames == 0 {
            return Ok(());
        }
        // Validate the whole block before scheduling any point. The second pass converts one
        // value at a time; no temporary event array or partial scheduling on invalid input.
        for change in automation {
            let (_, _, info) = prepared.ranges.get(&change.id).ok_or(AuError::Input(
                plughost_core::InputError::UnknownParameter { id: change.id },
            ))?;
            info.automation_value(change.value)
                .map_err(AuError::Input)?;
        }
        for change in automation {
            let (_, _, info) = &prepared.ranges[&change.id];
            let value = info
                .automation_value(change.value)
                .map_err(AuError::Input)?;
            self.schedule(change.id, change.offset, value)?;
        }
        {
            let mut timeline = prepared.timeline.lock().unwrap_or_else(|p| p.into_inner());
            let changed = match (*timeline, context.transport) {
                (Some(before), Some(now)) => {
                    let expected = before.sample_position.checked_add(if before.playing {
                        prepared.last_frames as i64
                    } else {
                        0
                    });
                    expected != Some(now.sample_position)
                        || before.playing != now.playing
                        || before.loop_region != now.loop_region
                }
                (None, None) => false,
                _ => true,
            };
            prepared.transport_changed.store(changed, Ordering::Relaxed);
            *timeline = context.transport;
        }
        let position = prepared.position;
        if let Some(schedule) = &prepared.schedule_midi {
            for event in events {
                // The block takes complete MIDI messages of any length, system exclusive included.
                // Notes and pressure go in their MIDI 1.0 form; other expressions have none.
                let midi = event.data.to_midi();
                let bytes = match (&event.data, &midi) {
                    (EventData::SysEx(bytes), _) => bytes.as_slice(),
                    (_, Some(midi)) if matches!(midi[0] & 0xF0, 0xC0 | 0xD0) => &midi[..2],
                    (_, Some(midi)) => midi.as_slice(),
                    (_, None) => continue,
                };
                schedule.call((
                    position as AUEventSampleTime + event.offset as AUEventSampleTime,
                    0,
                    bytes.len() as NSInteger,
                    NonNull::from(bytes).cast(),
                ));
            }
        }

        let prepared = self.prepared.as_mut().unwrap();
        if let Some(midi_output) = &prepared.midi_output {
            midi_output.begin(position, frames);
        }
        // The timeline moves on whether or not the unit rendered, as VST3's does: a unit asked
        // for the same time again can hand back the output it made for it.
        let rendered = render_audio::render(prepared, input, output, frames, position);
        prepared.position += frames as f64;
        prepared.last_frames = frames;
        rendered?;
        match &prepared.midi_output {
            Some(midi_output) => midi_output.take(produced),
            None => Ok(()),
        }
    }

    /// Schedules a normalized parameter change `offset` samples into the next render call.
    fn schedule(&self, id: u64, offset: usize, value: f64) -> Result<(), AuError> {
        let (_, prepared) = self.parts()?;
        let (min, max, info) = prepared.ranges.get(&id).ok_or(AuError::Input(
            plughost_core::InputError::UnknownParameter { id },
        ))?;
        info.validate_edit(value).map_err(AuError::Input)?;
        if offset >= prepared.config.max_block_size {
            return Err(AuError::Input(plughost_core::InputError::Automation));
        }
        let plain = plain(*min, *max, info, value);
        prepared.schedule.call((
            prepared.position as AUEventSampleTime + offset as AUEventSampleTime,
            0,
            id,
            plain,
        ));
        Ok(())
    }
}

/// Processes an Audio Unit from a processing thread.
#[derive(Clone)]
pub struct Processor {
    engine: Arc<Mutex<Engine>>,
}

impl Processor {
    /// Sticky after the system reports loss of this extension connection. A new instance is needed.
    pub(crate) fn restart_required(&self) -> bool {
        lock(&self.engine).invalidated.load(Ordering::Acquire)
    }
    /// Processes one block; `events` go to the unit's MIDI input on port 0. `produced` receives
    /// the MIDI the unit sent on the prepared outputs.
    pub(crate) fn process(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[plughost_core::ParameterChange],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), AuError> {
        produced.clear();
        lock(&self.engine).process(context, input, output, automation, events, produced)
    }

    /// Schedules normalized parameter changes for the next block.
    pub fn schedule(&self, id: u64, offset: usize, value: f64) -> Result<(), AuError> {
        lock(&self.engine).schedule(id, offset, value)
    }

    /// The timing the owning thread last read; no call into the unit.
    pub(crate) fn timing(&self) -> Result<PluginTiming, AuError> {
        lock(&self.engine).timing()
    }
}

fn read_latency(unit: &AUAudioUnit, sample_rate: f64) -> u32 {
    (unsafe { unit.latency() } * sample_rate).round() as u32
}

fn read_tail(unit: &AUAudioUnit, sample_rate: f64) -> Tail {
    let seconds = unsafe { unit.tailTime() };
    if !seconds.is_finite() || seconds > MAX_TAIL_SECONDS {
        Tail::Infinite
    } else {
        Tail::Samples((seconds * sample_rate).round() as u32)
    }
}

fn apply_state(unit: &AUAudioUnit, state: &State, purpose: plughost_core::StatePurpose) {
    unsafe {
        match purpose {
            plughost_core::StatePurpose::Project => unit.setFullStateForDocument(Some(state)),
            _ => unit.setFullState(Some(state)),
        }
    }
}
