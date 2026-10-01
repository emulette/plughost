//! The synth as a VST3 instrument: one component that is also its controller, with a stereo
//! output and two event input buses, the second inverted like the second CLAP note port. It plays
//! note events by their note IDs, poly pressure, and tuning note expression values; its controller
//! lists tuning as the note expression of both buses.

#![allow(non_snake_case)]
// VST3 enum constants are i32 on Windows and u32 elsewhere, so a cast needed on one platform is
// a no-op on the other.
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void};
use std::sync::{Mutex, MutexGuard};

use vst3::Steinberg::Vst::NoteExpressionTypeIDs_::kTuningTypeID;
use vst3::Steinberg::Vst::NoteExpressionTypeInfo_::NoteExpressionTypeFlags_::kIsBipolar;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

use crate::voices::{Expression, PORTS, Target, Voices};

const CID: TUID = uid(0x706C7567, 0x686F7374, 0x53796E74, 0x68000001);
const NAME: &str = "plughost test synth";
const NOTE_ON: u32 = Event_::EventTypes_::kNoteOnEvent as u32;
const NOTE_OFF: u32 = Event_::EventTypes_::kNoteOffEvent as u32;
const POLY_PRESSURE: u32 = Event_::EventTypes_::kPolyPressureEvent as u32;
const NOTE_EXPRESSION_VALUE: u32 = Event_::EventTypes_::kNoteExpressionValueEvent as u32;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn copy_c(text: &str, out: &mut [c_char]) {
    for (byte, slot) in text.bytes().chain([0]).zip(out) {
        *slot = byte as c_char;
    }
}

fn copy_wide(text: &str, out: &mut [TChar]) {
    for (unit, slot) in text.encode_utf16().chain([0]).zip(out) {
        *slot = unit;
    }
}

struct Synth {
    sample_rate: Mutex<f64>,
    voices: Mutex<Voices>,
}

impl Class for Synth {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        INoteExpressionController,
    );
}

/// The voices a VST3 note event addresses on `bus`; note ID -1 addresses notes by key.
fn target(bus: i32, channel: i16, pitch: i16, id: i32) -> Option<Target> {
    let port = usize::try_from(bus).ok().filter(|port| *port < PORTS)?;
    Some(Target {
        port,
        channel: u16::try_from(channel).ok(),
        key: u16::try_from(pitch).ok(),
        id: u32::try_from(id).ok(),
    })
}

impl Synth {
    /// Applies one input event to the voices.
    fn apply(&self, voices: &mut Voices, event: &Event) {
        // SAFETY: the union field read matches the event type the host declared.
        unsafe {
            match u32::from(event.r#type) {
                NOTE_ON => {
                    let on = event.__field0.noteOn;
                    if let Some(target) = target(event.busIndex, on.channel, on.pitch, on.noteId) {
                        voices.note_on(target, f64::from(on.velocity));
                    }
                }
                NOTE_OFF => {
                    let off = event.__field0.noteOff;
                    if let Some(target) = target(event.busIndex, off.channel, off.pitch, off.noteId)
                    {
                        voices.note_off(target);
                    }
                }
                POLY_PRESSURE => {
                    let pressure = event.__field0.polyPressure;
                    let (channel, pitch) = (pressure.channel, pressure.pitch);
                    if let Some(target) = target(event.busIndex, channel, pitch, pressure.noteId) {
                        let value = Expression::Pressure(f64::from(pressure.pressure));
                        voices.expression(target, &value);
                    }
                }
                NOTE_EXPRESSION_VALUE => {
                    let value = event.__field0.noteExpressionValue;
                    if value.typeId == kTuningTypeID
                        && let Some(target) = target(event.busIndex, -1, -1, value.noteId)
                    {
                        let semitones = (value.value - 0.5) * 240.0;
                        voices.expression(target, &Expression::Tuning(semitones));
                    }
                }
                _ => {}
            }
        }
    }
}

impl IPluginBaseTrait for Synth {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Synth {
    unsafe fn getControllerClassId(&self, _class: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: i32, direction: i32) -> int32 {
        let input = direction == BusDirections_::kInput as i32;
        match (media == MediaTypes_::kAudio as i32, input) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => PORTS as i32,
            (false, false) => 0,
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: i32,
        direction: i32,
        index: int32,
        out: *mut BusInfo,
    ) -> tresult {
        if !(0..unsafe { self.getBusCount(media, direction) }).contains(&index) {
            return kInvalidArgument;
        }
        let out = unsafe { &mut *out };
        out.mediaType = media;
        out.direction = direction;
        out.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        out.busType = if index == 0 {
            BusTypes_::kMain as i32
        } else {
            BusTypes_::kAux as i32
        };
        let name = if media == MediaTypes_::kAudio as i32 {
            out.channelCount = 2;
            "Output"
        } else {
            out.channelCount = 16;
            if index == 0 {
                "Notes"
            } else {
                "Inverted notes"
            }
        };
        copy_wide(name, &mut out.name);
        kResultOk
    }
    unsafe fn getRoutingInfo(
        &self,
        _input: *mut RoutingInfo,
        _output: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(
        &self,
        _media: i32,
        _direction: i32,
        _index: int32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        *lock(&self.voices) = Voices::new(*lock(&self.sample_rate));
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
}

impl IAudioProcessorTrait for Synth {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        num_inputs: int32,
        outputs: *mut SpeakerArrangement,
        num_outputs: int32,
    ) -> tresult {
        if num_inputs == 0 && num_outputs == 1 && unsafe { *outputs } == SpeakerArr::kStereo {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn getBusArrangement(
        &self,
        direction: int32,
        index: int32,
        arrangement: *mut SpeakerArrangement,
    ) -> tresult {
        if direction != BusDirections_::kOutput as i32 || index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arrangement = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        *lock(&self.sample_rate) = unsafe { (*setup).sampleRate };
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &*data };
        let frames = data.numSamples as usize;
        let mut events = Vec::new();
        if let Some(input) = unsafe { ComRef::from_raw(data.inputEvents) } {
            for index in 0..unsafe { input.getEventCount() } {
                // SAFETY: Event is plain C data the host fills in.
                let mut event: Event = unsafe { std::mem::zeroed() };
                if unsafe { input.getEvent(index, &mut event) } == kResultOk {
                    events.push(event);
                }
            }
        }
        events.sort_by_key(|event| event.sampleOffset);
        let mut voices = lock(&self.voices);
        let mut pending = events.iter().peekable();
        let mut rendered = vec![0.0f32; frames];
        for (frame, sample) in rendered.iter_mut().enumerate() {
            while let Some(event) = pending.next_if(|e| e.sampleOffset as usize <= frame) {
                self.apply(&mut voices, event);
            }
            *sample = voices.next_sample();
        }
        for event in pending {
            self.apply(&mut voices, event);
        }
        if frames == 0 || data.numOutputs < 1 {
            return kResultOk;
        }
        let output = unsafe { &mut *data.outputs };
        let pointers = unsafe { output.__field0.channelBuffers32 };
        for channel in 0..output.numChannels as usize {
            let samples = unsafe { std::slice::from_raw_parts_mut(*pointers.add(channel), frames) };
            samples.copy_from_slice(&rendered);
        }
        output.silenceFlags = 0;
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Synth {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> int32 {
        0
    }
    unsafe fn getParameterInfo(&self, _index: int32, _info: *mut ParameterInfo) -> tresult {
        kInvalidArgument
    }
    unsafe fn getParamStringByValue(
        &self,
        _id: ParamID,
        _value: ParamValue,
        _text: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }
    unsafe fn getParamValueByString(
        &self,
        _id: ParamID,
        _text: *mut TChar,
        _value: *mut ParamValue,
    ) -> tresult {
        kInvalidArgument
    }
    unsafe fn normalizedParamToPlain(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn plainParamToNormalized(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn getParamNormalized(&self, _id: ParamID) -> ParamValue {
        0.0
    }
    unsafe fn setParamNormalized(&self, _id: ParamID, _value: ParamValue) -> tresult {
        kInvalidArgument
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl INoteExpressionControllerTrait for Synth {
    unsafe fn getNoteExpressionCount(&self, bus: int32, channel: int16) -> int32 {
        i32::from((0..PORTS as i32).contains(&bus) && channel == 0)
    }
    unsafe fn getNoteExpressionInfo(
        &self,
        bus: int32,
        channel: int16,
        index: int32,
        info: *mut NoteExpressionTypeInfo,
    ) -> tresult {
        if index >= unsafe { self.getNoteExpressionCount(bus, channel) } || index < 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.typeId = kTuningTypeID;
        copy_wide("Tuning", &mut info.title);
        info.shortTitle = info.title;
        copy_wide("semitones", &mut info.units);
        info.unitId = 0;
        info.valueDesc = NoteExpressionValueDescription {
            defaultValue: 0.5,
            minimum: 0.0,
            maximum: 1.0,
            stepCount: 0,
        };
        info.associatedParameterId = 0;
        info.flags = kIsBipolar as int32;
        kResultOk
    }
    unsafe fn getNoteExpressionStringByValue(
        &self,
        _bus: int32,
        _channel: int16,
        _id: NoteExpressionTypeID,
        _value: NoteExpressionValue,
        _text: *mut String128,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn getNoteExpressionValueByString(
        &self,
        _bus: int32,
        _channel: int16,
        _id: NoteExpressionTypeID,
        _text: *const TChar,
        _value: *mut NoteExpressionValue,
    ) -> tresult {
        kNotImplemented
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
        info.cid = CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(NAME, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if unsafe { *(cid as *const TUID) } != CID {
            return kInvalidArgument;
        }
        let synth = Synth {
            sample_rate: Mutex::new(48_000.0),
            voices: Mutex::new(Voices::new(48_000.0)),
        };
        let Some(instance) = ComWrapper::new(synth).to_com_ptr::<FUnknown>() else {
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
        info.cid = CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(NAME, &mut info.name);
        info.classFlags = 0;
        copy_c("Instrument|Synth", &mut info.subCategories);
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
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |factory| factory.into_raw())
}
