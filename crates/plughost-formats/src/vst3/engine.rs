//! A plugin's processing side, shared between the thread that owns the plugin and one processing
//! thread.
//!
//! Lock ownership, following the VST3 threading model: the engine mutex guards the processor and
//! the component's processing lifecycle. `process` holds it for one block; the owning thread takes
//! it for `setupProcessing`, `setActive`, `setProcessing`, bus negotiation, component state, and
//! the flush that delivers pending edits. The edit controller is never called under this mutex
//! nor from the processing thread: the owning thread calls it directly, and values the processor
//! reports back reach it through [`ParameterCache`]. The latency and tail getters are owning
//! thread calls too: the latency is read as the plugin activates, the tail when the owning thread
//! asks for the timing, and the processing thread reads both from [`Prepared`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use plughost_core::render::Tail;
use plughost_core::{
    AudioBusInfo, AudioConfig, AudioDirection, EventData, ExpressionKind, Layout, Message,
    ParameterChange, PluginTiming, ProcessConfig, ProcessMode, Sample, SampleFormat, events_fit,
};
use vst3::Steinberg::Vst::ControllerNumbers_::{kAfterTouch, kCtrlProgramChange, kPitchBend};
use vst3::Steinberg::Vst::DataEvent_::DataTypes_;
use vst3::Steinberg::Vst::Event_::{EventTypes, EventTypes_};
use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_;
use vst3::Steinberg::Vst::{
    AudioBusBuffers, DataEvent, Event, Event__type0, IAudioProcessor, IAudioProcessorTrait,
    IComponentTrait, NoteExpressionValueEvent, NoteOffEvent, NoteOnEvent, ParamID,
    PolyPressureEvent, ProcessContext, ProcessData, ProcessModes_, ProcessSetup, RestartFlags_,
    SymbolicSampleSizes_, kInfiniteTail,
};
use vst3::Steinberg::kResultOk;
use vst3::{ComPtr, ComWrapper};

use super::buffers::AudioBuffers;
use super::errors::Vst3Error;
use super::host::{ComponentHandler, EventList, OutputEventList, ParameterChanges};
use super::input_notes::InputNotes;
use super::instance::{Instance, MidiAssignment, midi_assignments};
use super::note_expression;
use super::parameter_cache::ParameterCache;

const RESTART_REQUIRED: i32 = RestartFlags_::kIoChanged | RestartFlags_::kReloadComponent;

pub(crate) struct Engine {
    /// `None` once the plugin is closed.
    pub instance: Option<Instance>,
    pub prepared: Option<Prepared>,
}

pub(crate) struct Prepared {
    pub config: ProcessConfig,
    pub audio_config: AudioConfig,
    pub audio_buses: Vec<AudioBusInfo>,
    pub context_requirements: Option<u32>,
    /// The latency the plugin reported as it activated, which holds until it activates again.
    latency: u32,
    tail: Tail,
    buffers: AudioBuffers,
    position: i64,
    parameters: Arc<ParameterCache>,
    /// Per cached parameter: the value the processor has, as far as the host knows.
    held: Vec<Held>,
    values_generation: u32,
    /// The block's validated automation: parameter index, offset, value.
    automation: Vec<(usize, i32, f64)>,
    changes: ComWrapper<ParameterChanges>,
    output_changes: ComWrapper<ParameterChanges>,
    events: ComWrapper<EventList>,
    /// The notes `events` started and have not ended.
    notes: InputNotes,
    output_events: ComWrapper<OutputEventList>,
    /// Per event input port, whether it is active; events reach only active ports.
    pub event_inputs: Vec<bool>,
    /// Event input buses the plugin declares. A controller-only port beyond them takes mapped
    /// controllers alone.
    event_buses: usize,
    /// MIDI controller assignments by (event bus, channel, controller number).
    pub midi_assignments: HashMap<(u8, u8, u8), MidiAssignment>,
}

/// A parameter's last point sent to the processor: its value (NaN when unknown) and the block
/// position and offset it was sent at.
#[derive(Clone, Copy)]
struct Held {
    value: f64,
    block: i64,
    offset: i32,
}

impl Held {
    /// The point that keeps the previous value until a new point at `offset` of the block at
    /// `position`: none at the start of the block, when the value is unknown, or when this block
    /// already has a point right before or at `offset`.
    fn hold_before(self, position: i64, offset: i32) -> Option<(i32, f64)> {
        let queued_before = self.block == position && self.offset >= offset - 1;
        (offset > 0 && !self.value.is_nan() && !queued_before).then_some((offset - 1, self.value))
    }
}

const UNKNOWN: Held = Held {
    value: f64::NAN,
    block: -1,
    offset: 0,
};

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;

pub(crate) type Shared = Arc<Mutex<Engine>>;

pub(crate) fn lock(engine: &Shared) -> MutexGuard<'_, Engine> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Engine {
    pub fn instance(&self) -> Result<&Instance, Vst3Error> {
        self.instance.as_ref().ok_or(Vst3Error::Closed)
    }

    /// Negotiates the main bus layouts, sets up processing, and activates the plugin.
    pub fn prepare(
        &mut self,
        config: &ProcessConfig,
        parameters: &Arc<ParameterCache>,
    ) -> Result<(), Vst3Error> {
        let audio = self.instance()?.main_audio_config(config)?;
        self.prepare_audio(&audio, parameters)?;
        if let Some(prepared) = &mut self.prepared {
            prepared.config = *config;
        }
        Ok(())
    }

    pub fn prepare_audio(
        &mut self,
        config: &AudioConfig,
        parameters: &Arc<ParameterCache>,
    ) -> Result<Vec<AudioBusInfo>, Vst3Error> {
        config.validate().map_err(Vst3Error::Input)?;
        if config.configuration.is_some() {
            return Err(Vst3Error::AudioConfigurationsUnsupported);
        }
        self.unprepare();
        let instance = self.instance()?;
        instance.handler.take_restart_flags();
        let processor = &instance.processor;
        let sample_size = symbolic_sample_size(config.sample_format);
        if unsafe { processor.canProcessSampleSize(sample_size) } != kResultOk {
            return Err(Vst3Error::SampleFormatUnsupported(config.sample_format));
        }
        let audio_buses = instance.negotiate_audio(config)?;
        let buffers = AudioBuffers::new(&audio_buses, config.sample_format, config.max_block_size)?;
        let limit = plughost_core::MAX_BLOCK_EVENTS;
        // Pending edits and the block's inputs each take up to `limit` parameters. Inputs after
        // the start of the block take a second point that holds the previous value until them.
        let changes = ComWrapper::new(ParameterChanges::new(2 * limit, 3 * limit)?);
        let output_changes = ComWrapper::new(ParameterChanges::latest(limit)?);
        // A note off or an expression without an ID becomes one event per note on its key.
        let events = ComWrapper::new(EventList::new(2 * limit)?);
        let notes = InputNotes::new()?;
        let output_events = ComWrapper::new(OutputEventList::new()?);
        let automation = scratch(limit)?;
        let mut setup = ProcessSetup {
            processMode: process_mode(config.mode),
            symbolicSampleSize: sample_size,
            maxSamplesPerBlock: config.max_block_size as i32,
            sampleRate: config.sample_rate,
        };
        let result = unsafe { processor.setupProcessing(&mut setup) };
        if result != kResultOk {
            return Err(Vst3Error::SetupRejected(result));
        }
        let event_inputs = instance.activate_events(&config.events)?;
        let event_buses = instance.event_buses();
        let midi_assignments = midi_assignments(&instance.controller, parameters, &event_inputs);
        let result = unsafe { instance.component.setActive(1) };
        if result != kResultOk {
            return Err(Vst3Error::Activate(result));
        }
        let latency = activated(processor);
        let tail = tail(processor);
        // Restart requests made while taking this configuration are about the configuration
        // itself; kept, they would ask for the same preparation again forever.
        instance.handler.take_restart_flags();
        self.prepared = Some(Prepared {
            config: ProcessConfig {
                sample_rate: config.sample_rate,
                max_block_size: config.max_block_size,
                sample_format: config.sample_format,
                mode: config.mode,
                input: main_layout(&audio_buses, AudioDirection::Input),
                output: main_layout(&audio_buses, AudioDirection::Output),
            },
            audio_config: config.clone(),
            audio_buses: audio_buses.clone(),
            context_requirements: {
                use vst3::Steinberg::Vst::{
                    IProcessContextRequirements, IProcessContextRequirementsTrait,
                };
                processor
                    .cast::<IProcessContextRequirements>()
                    .map(|requirements| unsafe { requirements.getProcessContextRequirements() })
            },
            latency,
            tail,
            buffers,
            position: 0,
            parameters: Arc::clone(parameters),
            held: vec![UNKNOWN; parameters.len()],
            values_generation: instance.handler.values_generation(),
            automation,
            changes,
            output_changes,
            events,
            notes,
            output_events,
            event_inputs,
            event_buses,
            midi_assignments,
        });
        Ok(audio_buses)
    }

    /// Stops processing and deactivates the plugin.
    pub fn unprepare(&mut self) {
        if let (Some(_), Some(instance)) = (self.prepared.take(), &self.instance) {
            unsafe {
                instance.processor.setProcessing(0);
                instance.component.setActive(0);
            }
        }
    }

    /// Clears the processor's internal audio state the way VST3 defines it: deactivating and
    /// reactivating keeps the configuration and parameters. Does nothing unless prepared.
    pub fn reset(&mut self) -> Result<(), Vst3Error> {
        let instance = self.instance()?;
        if self.prepared.is_none() {
            return Ok(());
        }
        let result = unsafe {
            instance.processor.setProcessing(0);
            instance.component.setActive(0);
            instance.component.setActive(1)
        };
        if result != kResultOk {
            self.prepared = None;
            return Err(Vst3Error::Activate(result));
        }
        let latency = activated(&instance.processor);
        let tail = tail(&instance.processor);
        instance.handler.take_restart_flags();
        if let Some(prepared) = &mut self.prepared {
            prepared.notes.clear();
            prepared.output_events.forget_notes();
            prepared.latency = latency;
            prepared.tail = tail;
        }
        Ok(())
    }

    /// Processes one block; `produced` receives the plugin's output events in offset order, on
    /// their event bus indices.
    pub fn process<S: Sample>(
        &mut self,
        context: &plughost_core::BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        changes: &[ParameterChange],
        events: &[plughost_core::Event],
        produced: &mut Vec<plughost_core::Event>,
    ) -> Result<(), Vst3Error> {
        produced.clear();
        let instance = self.instance.as_ref().ok_or(Vst3Error::Closed)?;
        // The owning thread deactivates and prepares again; the processing thread only reports.
        let flags = instance.handler.restart_flags();
        if flags & RESTART_REQUIRED != 0 {
            return Err(Vst3Error::RestartRequired(flags));
        }
        let prepared = self.prepared.as_mut().ok_or(Vst3Error::NotPrepared)?;
        let native_type = match S::FORMAT {
            SampleFormat::F32 => std::any::TypeId::of::<S>() == std::any::TypeId::of::<f32>(),
            SampleFormat::F64 => std::any::TypeId::of::<S>() == std::any::TypeId::of::<f64>(),
        };
        if !native_type || S::FORMAT != prepared.config.sample_format {
            return Err(Vst3Error::SampleFormatMismatch);
        }
        context.validate().map_err(Vst3Error::Input)?;
        if let Some(transport) = context.transport {
            transport
                .validate_at_rate(prepared.config.sample_rate)
                .map_err(Vst3Error::Input)?;
        }
        plughost_core::validate_event_budget(changes.len(), events).map_err(Vst3Error::Input)?;
        if let Some(event) = events.iter().find(|event| {
            !prepared
                .event_inputs
                .get(event.port)
                .copied()
                .unwrap_or(false)
        }) {
            return Err(Vst3Error::Input(plughost_core::InputError::EventPort {
                port: event.port,
            }));
        }
        let frames = context.frames;
        let (main_inputs, main_outputs) = prepared.buffers.channels();
        if input.len() != main_inputs
            || output.len() != main_outputs
            || frames > prepared.config.max_block_size
            || input.iter().any(|c| c.len() != frames)
            || output.iter().any(|c| c.len() != frames)
            || changes.iter().any(|c| c.offset >= frames)
            || changes
                .windows(2)
                .any(|pair| pair[0].offset > pair[1].offset)
            || !events_fit(events, frames)
        {
            return Err(Vst3Error::Buffers);
        }
        if frames == 0 {
            return Ok(());
        }
        prepared.stage_automation(changes)?;
        prepared.buffers.bind(input, output, 0..frames)?;
        prepared.begin(&instance.handler);
        prepared.push_inputs(events);
        let result = call_process(instance, prepared, context);
        prepared.buffers.clear();
        prepared.position += frames as i64;
        prepared.take_outputs(&instance.handler);
        if result != kResultOk {
            prepared.output_events.discard();
            return Err(Vst3Error::Process(result));
        }
        prepared.output_events.take(produced, frames)
    }

    /// Delivers edits that have not reached the processor yet, with a process call that carries
    /// no audio (`numSamples = 0`). Events the plugin outputs meanwhile go with the next block.
    pub fn flush(&mut self) -> Result<(), Vst3Error> {
        let instance = self.instance.as_ref().ok_or(Vst3Error::Closed)?;
        if !instance.handler.has_pending() {
            return Ok(());
        }
        let Some(prepared) = self.prepared.as_mut() else {
            return flush_inactive(instance);
        };
        prepared.begin(&instance.handler);
        let result = call_process(instance, prepared, &plughost_core::BlockContext::new(0));
        prepared.take_outputs(&instance.handler);
        if result != kResultOk {
            return Err(Vst3Error::Process(result));
        }
        Ok(())
    }

    /// The timing read on the owning thread; no native call. A latency the plugin announced with
    /// `kLatencyChanged` is valid once it activates again (VST3 asks the host to deactivate and
    /// activate it), so it is pending until then.
    pub fn timing(&self) -> Result<PluginTiming, Vst3Error> {
        let flags = self.instance()?.handler.restart_flags();
        let prepared = self.prepared.as_ref().ok_or(Vst3Error::NotPrepared)?;
        let mut timing = PluginTiming::new(prepared.latency, prepared.tail);
        timing.restart_required = flags & RESTART_REQUIRED != 0;
        timing.latency_changed = flags & RestartFlags_::kLatencyChanged != 0;
        Ok(timing)
    }

    /// Reads the tail again, on the owning thread. VST3 has no notification for it.
    pub fn refresh_tail(&mut self) {
        if let (Some(instance), Some(prepared)) = (&self.instance, &mut self.prepared) {
            prepared.tail = tail(&instance.processor);
        }
    }
}

fn tail(processor: &ComPtr<IAudioProcessor>) -> Tail {
    match unsafe { processor.getTailSamples() } {
        samples if samples == kInfiniteTail => Tail::Infinite,
        samples => Tail::Samples(samples),
    }
}

/// The processing setup of a flush while unprepared. No audio is processed.
const FLUSH_SETUP: (f64, i32) = (48_000.0, 512);

/// Delivers pending edits to an unprepared plugin the way the SDK's validator flushes: activated
/// on its current buses for one process call without audio buffers, then deactivated.
fn flush_inactive(instance: &Instance) -> Result<(), Vst3Error> {
    let limit = plughost_core::MAX_BLOCK_EVENTS;
    let changes = ComWrapper::new(ParameterChanges::new(limit, limit)?);
    let output_changes = ComWrapper::new(ParameterChanges::latest(limit)?);
    let events = ComWrapper::new(EventList::new(limit)?);
    let output_events = ComWrapper::new(OutputEventList::new()?);
    let processor = &instance.processor;
    let (sample_rate, max_block_size) = FLUSH_SETUP;
    let mut setup = ProcessSetup {
        processMode: ProcessModes_::kOffline as i32,
        symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
        maxSamplesPerBlock: max_block_size,
        sampleRate: sample_rate,
    };
    let result = unsafe { processor.setupProcessing(&mut setup) };
    if result != kResultOk {
        return Err(Vst3Error::SetupRejected(result));
    }
    let result = unsafe { instance.component.setActive(1) };
    if result != kResultOk {
        return Err(Vst3Error::Activate(result));
    }
    activated(processor);
    instance
        .handler
        .drain_pending(|id, value| changes.push(id, 0, value));
    let mut data = ProcessData {
        processMode: setup.processMode,
        symbolicSampleSize: setup.symbolicSampleSize,
        numSamples: 0,
        numInputs: 0,
        numOutputs: 0,
        inputs: std::ptr::null_mut(),
        outputs: std::ptr::null_mut(),
        inputParameterChanges: ParameterChanges::ptr(&changes),
        outputParameterChanges: ParameterChanges::ptr(&output_changes),
        inputEvents: EventList::ptr(&events),
        outputEvents: OutputEventList::ptr(&output_events),
        processContext: std::ptr::null_mut(),
    };
    let result = unsafe {
        let result = processor.process(&mut data);
        processor.setProcessing(0);
        instance.component.setActive(0);
        result
    };
    if result != kResultOk {
        return Err(Vst3Error::Process(result));
    }
    Ok(())
}

/// Starts processing an activated plugin and returns its latency. The SDK's call sequence reads
/// the latency after every `setActive(true)` and before the first process call, which may be a
/// parameter flush.
fn activated(processor: &ComPtr<IAudioProcessor>) -> u32 {
    unsafe {
        let latency = processor.getLatencySamples();
        // Plugins without a processing state return kNotImplemented, which is not an error.
        processor.setProcessing(1);
        latency
    }
}

fn main_layout(buses: &[AudioBusInfo], direction: AudioDirection) -> Layout {
    buses
        .iter()
        .find(|bus| {
            bus.direction == direction
                && bus.role == plughost_core::AudioBusRole::Main
                && bus.active == Some(true)
        })
        .and_then(|bus| bus.layout)
        .unwrap_or(Layout::None)
}

/// Processes a plugin on a thread other than the one that owns it, for example while its editor
/// runs on the main thread. Calls wait for any other processor call on the same plugin. Once the
/// plugin is dropped, calls fail with [`Vst3Error::Closed`].
#[derive(Clone)]
pub struct Processor {
    engine: Shared,
}

impl Processor {
    pub(crate) fn new(engine: &Shared) -> Processor {
        Processor {
            engine: Arc::clone(engine),
        }
    }

    /// See [`crate::vst3::Plugin::process`]. Channels are flattened in native active-bus order.
    pub(crate) fn process<S: Sample>(
        &self,
        context: &plughost_core::BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        changes: &[ParameterChange],
        events: &[plughost_core::Event],
        produced: &mut Vec<plughost_core::Event>,
    ) -> Result<(), Vst3Error> {
        lock(&self.engine).process(context, input, output, changes, events, produced)
    }

    pub(crate) fn timing(&self) -> Result<PluginTiming, Vst3Error> {
        lock(&self.engine).timing()
    }

    /// The plugin asked to be prepared again (changed buses or a reload request).
    pub(crate) fn restart_required(&self) -> bool {
        lock(&self.engine)
            .instance()
            .is_ok_and(|instance| instance.handler.restart_flags() & RESTART_REQUIRED != 0)
    }
}

enum NativeInput {
    Note(Event),
    /// A note expression value without an ID, for the notes on this channel and key.
    KeyExpression(Event, i16, i16),
    Parameter(ParamID, i32, f64),
}

const NOTE_ON: u32 = EventTypes_::kNoteOnEvent as u32;
const NOTE_OFF: u32 = EventTypes_::kNoteOffEvent as u32;

fn scratch<T>(limit: usize) -> Result<Vec<T>, Vst3Error> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(limit)
        .map_err(|_| Vst3Error::EventStorage)?;
    Ok(values)
}

impl Prepared {
    /// Switches to a list the owning thread read again after the controller changed it.
    pub fn set_parameters(&mut self, parameters: &Arc<ParameterCache>) {
        self.held = vec![UNKNOWN; parameters.len()];
        self.parameters = Arc::clone(parameters);
    }

    /// The processor's values changed in a way the host did not send (restored state).
    pub fn forget_held_values(&mut self) {
        self.held.fill(UNKNOWN);
    }

    /// Takes the controller's values, read at `generation`, as the processor's: a first point
    /// after the start of a block then holds them until it instead of ramping from wherever the
    /// plugin is. `values` follow the cached parameter list.
    pub fn seed_held_values(&mut self, values: &[f64], generation: u32) {
        if values.len() != self.held.len() {
            return;
        }
        for (held, &value) in self.held.iter_mut().zip(values) {
            *held = Held {
                value,
                block: -1,
                offset: 0,
            };
        }
        self.values_generation = generation;
    }

    /// Validates every automation point before the block consumes pending edits.
    fn stage_automation(&mut self, changes: &[ParameterChange]) -> Result<(), Vst3Error> {
        self.automation.clear();
        for change in changes {
            let index = self.parameters.index(change.id).ok_or(Vst3Error::Input(
                plughost_core::InputError::UnknownParameter { id: change.id },
            ))?;
            let value = self
                .parameters
                .info(index)
                .automation_value(change.value)
                .map_err(Vst3Error::Input)?;
            self.automation.push((index, change.offset as i32, value));
        }
        Ok(())
    }

    /// Clears the queues and sends the controller's pending edits at the start of the block.
    fn begin(&mut self, handler: &ComponentHandler) {
        self.changes.clear();
        self.output_changes.clear();
        self.events.clear();
        let generation = handler.values_generation();
        if generation != self.values_generation {
            self.values_generation = generation;
            self.forget_held_values();
        }
        handler.drain_pending(|id, value| {
            self.changes.push(id, 0, value);
            if let Some(index) = self.parameters.index(u64::from(id)) {
                self.held[index] = Held {
                    value,
                    block: self.position,
                    offset: 0,
                };
            }
        });
    }

    /// Queues the staged automation and `events` in offset order; automation goes first at equal
    /// offsets.
    fn push_inputs(&mut self, events: &[plughost_core::Event]) {
        let mut staged = 0;
        for event in events {
            while let Some(&(index, offset, value)) = self.automation.get(staged)
                && offset as usize <= event.offset
            {
                self.push_point(index, offset, value);
                staged += 1;
            }
            match self.translate(event) {
                Some(NativeInput::Note(note)) => self.push_note(note),
                Some(NativeInput::KeyExpression(expression, channel, pitch)) => {
                    for id in self.notes.ids(expression.busIndex, channel, pitch) {
                        // SAFETY: `native` built this event as a note expression value.
                        let mut value = unsafe { expression.__field0.noteExpressionValue };
                        value.noteId = id;
                        self.events.push(Event {
                            __field0: Event__type0 {
                                noteExpressionValue: value,
                            },
                            ..expression
                        });
                    }
                }
                Some(NativeInput::Parameter(id, offset, value)) => {
                    match self.parameters.index(u64::from(id)) {
                        Some(index) => self.push_point(index, offset, value),
                        None => self.changes.push(id, offset, value),
                    }
                }
                None => {}
            }
        }
        while let Some(&(index, offset, value)) = self.automation.get(staged) {
            self.push_point(index, offset, value);
            staged += 1;
        }
    }

    /// Queues a note event, following the notes it starts and ends. A note off without an ID
    /// ends each note on its key by that note's ID.
    fn push_note(&mut self, event: Event) {
        let (notes, events) = (&mut self.notes, &self.events);
        // SAFETY: the union field read matches the event type set where the event was built.
        match u32::from(event.r#type) {
            NOTE_ON => {
                let on = unsafe { event.__field0.noteOn };
                notes.note_on(event.busIndex, on.channel, on.pitch, on.noteId);
                events.push(event);
            }
            NOTE_OFF => {
                let off = unsafe { event.__field0.noteOff };
                notes.note_off(event.busIndex, off.channel, off.pitch, off.noteId, |id| {
                    events.push(Event {
                        __field0: Event__type0 {
                            noteOff: NoteOffEvent { noteId: id, ..off },
                        },
                        ..event
                    });
                });
            }
            _ => events.push(event),
        }
    }

    /// Queues a point that holds from `offset` on. VST3 plugins may ramp between points, so a
    /// point after the start of the block is preceded by one that holds the previous value.
    fn push_point(&mut self, index: usize, offset: i32, value: f64) {
        let id = self.parameters.native_id(index);
        if let Some((hold, previous)) = self.held[index].hold_before(self.position, offset) {
            self.changes.push(id, hold, previous);
        }
        self.changes.push(id, offset, value);
        self.held[index] = Held {
            value,
            block: self.position,
            offset,
        };
        self.parameters.hand_to_controller(index, value, false);
    }

    /// Hands the processor's output parameter values to the controller and counts output events
    /// that have no MIDI form.
    fn take_outputs(&mut self, handler: &ComponentHandler) {
        let (parameters, held) = (&self.parameters, &mut self.held);
        self.output_changes.each_last_value(|id, value| {
            if let Some(index) = parameters.index(u64::from(id)) {
                held[index].value = value;
                parameters.hand_to_controller(index, value, true);
            }
        });
        handler.add_unconvertible_output_events(self.output_events.take_unconvertible());
    }

    /// The native form of a validated event on an active port. A controller-only port passes
    /// mapped controllers alone, since it has no bus to carry notes or data.
    fn translate(&self, event: &plughost_core::Event) -> Option<NativeInput> {
        let native = self.native(event)?;
        (event.port < self.event_buses || matches!(native, NativeInput::Parameter(..)))
            .then_some(native)
    }

    /// The native form of an event. System exclusive data points into `event`, which outlives
    /// the process call. Notes keep their IDs; an expression other than pressure reaches a note
    /// by its ID alone, so one without an ID goes to the notes on its key that have one.
    fn native(&self, event: &plughost_core::Event) -> Option<NativeInput> {
        let native = |kind, body| {
            Some(NativeInput::Note(note_event(
                event.offset as i32,
                event.port as i32,
                kind,
                body,
            )))
        };
        let id = |id: Option<u32>| id.map_or(-1, |id| id as i32);
        let note_on = |channel: u8, key: u8, velocity: f32, id: i32| {
            native(
                EventTypes_::kNoteOnEvent,
                Event__type0 {
                    noteOn: NoteOnEvent {
                        channel: i16::from(channel),
                        pitch: i16::from(key),
                        tuning: 0.0,
                        velocity,
                        length: 0,
                        noteId: id,
                    },
                },
            )
        };
        let note_off = |channel: u8, key: u8, velocity: f32, id: i32| {
            native(
                EventTypes_::kNoteOffEvent,
                Event__type0 {
                    noteOff: NoteOffEvent {
                        channel: i16::from(channel),
                        pitch: i16::from(key),
                        velocity,
                        noteId: id,
                        tuning: 0.0,
                    },
                },
            )
        };
        let poly_pressure = |channel: u8, key: u8, pressure: f32, id: i32| {
            native(
                EventTypes_::kPolyPressureEvent,
                Event__type0 {
                    polyPressure: PolyPressureEvent {
                        channel: i16::from(channel),
                        pitch: i16::from(key),
                        pressure,
                        noteId: id,
                    },
                },
            )
        };
        match &event.data {
            EventData::SysEx(bytes) => {
                return native(
                    EventTypes_::kDataEvent,
                    Event__type0 {
                        data: DataEvent {
                            size: bytes.len() as u32,
                            r#type: DataTypes_::kMidiSysEx as u32,
                            bytes: bytes.as_ptr(),
                        },
                    },
                );
            }
            EventData::NoteOn(note) => {
                return note_on(note.channel, note.key, note.velocity as f32, id(note.id));
            }
            EventData::NoteOff(note) => {
                return note_off(note.channel, note.key, note.velocity as f32, id(note.id));
            }
            EventData::Expression(expression) if expression.kind == ExpressionKind::Pressure => {
                let (channel, key) = (expression.channel, expression.key);
                return poly_pressure(channel, key, expression.value as f32, id(expression.id));
            }
            EventData::Expression(expression) => {
                let value = note_event(
                    event.offset as i32,
                    event.port as i32,
                    EventTypes_::kNoteExpressionValueEvent,
                    Event__type0 {
                        noteExpressionValue: NoteExpressionValueEvent {
                            typeId: note_expression::type_id(expression.kind)?,
                            noteId: id(expression.id),
                            value: note_expression::normalized(expression.kind, expression.value),
                        },
                    },
                );
                return Some(match expression.id {
                    Some(_) => NativeInput::Note(value),
                    None => NativeInput::KeyExpression(
                        value,
                        i16::from(expression.channel),
                        i16::from(expression.key),
                    ),
                });
            }
            _ => {}
        }
        let (channel, controller, value) = match event.message() {
            Some(Message::NoteOn {
                channel,
                key,
                velocity,
            }) => return note_on(channel, key, f32::from(velocity) / 127.0, -1),
            Some(Message::NoteOff {
                channel,
                key,
                velocity,
            }) => return note_off(channel, key, f32::from(velocity) / 127.0, -1),
            Some(Message::PolyPressure {
                channel,
                key,
                pressure,
            }) => return poly_pressure(channel, key, f32::from(pressure) / 127.0, -1),
            Some(Message::ControlChange {
                channel,
                controller,
                value,
            }) => (channel, controller, u16::from(value)),
            Some(Message::ChannelPressure { channel, pressure }) => {
                (channel, kAfterTouch as u8, u16::from(pressure))
            }
            Some(Message::PitchBend { channel, value }) => (channel, kPitchBend as u8, value),
            Some(Message::ProgramChange { channel, program }) => {
                (channel, kCtrlProgramChange as u8, u16::from(program))
            }
            None => return None,
        };
        self.midi_assignments
            .get(&(event.port as u8, channel, controller))
            .map(|assignment| {
                let normalized = (f64::from(value) / assignment.full_scale).clamp(0.0, 1.0);
                NativeInput::Parameter(assignment.id, event.offset as i32, normalized)
            })
    }
}

fn note_event(offset: i32, bus: i32, kind: EventTypes, body: Event__type0) -> Event {
    Event {
        busIndex: bus,
        sampleOffset: offset,
        ppqPosition: 0.0,
        flags: 0,
        r#type: kind as u16,
        __field0: body,
    }
}

fn process_mode(mode: ProcessMode) -> i32 {
    match mode {
        ProcessMode::Offline => ProcessModes_::kOffline as i32,
        ProcessMode::Realtime => ProcessModes_::kRealtime as i32,
    }
}

fn symbolic_sample_size(format: SampleFormat) -> i32 {
    match format {
        SampleFormat::F32 => SymbolicSampleSizes_::kSample32 as i32,
        SampleFormat::F64 => SymbolicSampleSizes_::kSample64 as i32,
    }
}

fn call_process(
    instance: &Instance,
    prepared: &mut Prepared,
    block: &plughost_core::BlockContext,
) -> i32 {
    let config = &prepared.config;
    // SAFETY: ProcessContext is plain C data; the fields that matter offline are set below.
    let mut context: ProcessContext = unsafe { std::mem::zeroed() };
    context.state = StatesAndFlags_::kContTimeValid as u32;
    context.sampleRate = config.sample_rate;
    context.continousTimeSamples = prepared.position;
    if let Some(transport) = block.transport {
        context.projectTimeSamples = transport.sample_position;
        if transport.playing {
            context.state |= StatesAndFlags_::kPlaying as u32;
        }
        if let Some(tempo) = transport.tempo {
            context.tempo = tempo;
            context.state |= StatesAndFlags_::kTempoValid as u32;
        }
        if let Some(beat) = transport.beat_position {
            context.projectTimeMusic = beat;
            context.state |= StatesAndFlags_::kProjectTimeMusicValid as u32;
        }
        if let Some(bar) = transport.bar_position {
            context.barPositionMusic = bar.start;
            context.state |= StatesAndFlags_::kBarPositionValid as u32;
        }
        if let Some(meter) = transport.time_signature {
            context.timeSigNumerator = i32::from(meter.numerator);
            context.timeSigDenominator = i32::from(meter.denominator);
            context.state |= StatesAndFlags_::kTimeSigValid as u32;
        }
        if let Some(region) = transport.loop_region {
            context.cycleStartMusic = region.start;
            context.cycleEndMusic = region.end;
            context.state |= (StatesAndFlags_::kCycleActive | StatesAndFlags_::kCycleValid) as u32;
        }
    }
    let nullable = |buses: &mut [AudioBusBuffers]| {
        if buses.is_empty() {
            std::ptr::null_mut()
        } else {
            buses.as_mut_ptr()
        }
    };
    let (inputs, outputs) = if block.frames == 0 {
        (&mut [][..], &mut [][..])
    } else {
        prepared.buffers.raw()
    };
    let mut data = ProcessData {
        processMode: process_mode(config.mode),
        symbolicSampleSize: symbolic_sample_size(config.sample_format),
        numSamples: block.frames as i32,
        numInputs: inputs.len() as i32,
        numOutputs: outputs.len() as i32,
        inputs: nullable(inputs),
        outputs: nullable(outputs),
        inputParameterChanges: ParameterChanges::ptr(&prepared.changes),
        outputParameterChanges: ParameterChanges::ptr(&prepared.output_changes),
        inputEvents: EventList::ptr(&prepared.events),
        outputEvents: OutputEventList::ptr(&prepared.output_events),
        // Always present: many plugins read the sample rate and time from it without checking.
        processContext: &mut context,
    };
    unsafe { instance.processor.process(&mut data) }
}
