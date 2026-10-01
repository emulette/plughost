//! A version 2 Audio Unit this test registers in its own process, which the system hosts through
//! its v2 bridge like an installed v2 component. The unit is a MIDI processor with a stereo
//! output that sends the MIDI it receives back on its MIDI output. Its first parameter is a list;
//! setting its second one renames the list's values, which the unit announces through
//! `kAudioUnitProperty_ParameterValueStrings` listeners. Its third parameter is an index from 0 to
//! 22, whose values the unit records as it receives them.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard, Once};

/// The sample times of the renders of a unit whose `STEPS` is `TIMED`.
static RENDER_TIMES: Mutex<Vec<f64>> = Mutex::new(Vec::new());
const TIMED: f32 = 7.0;
/// A note on this key makes the next render fail.
const FAIL_KEY: u8 = 127;

/// The values the unit received for `STEPS`: set, or scheduled at a buffer offset.
static STEPS_RECEIVED: Mutex<Vec<(Option<u32>, f32)>> = Mutex::new(Vec::new());

use objc2_audio_toolbox::{
    AUChannelInfo, AudioComponentDescription, AudioComponentInstance, AudioComponentMethod,
    AudioComponentPlugInInterface, AudioComponentRegister, AudioUnit, AudioUnitParameterInfo,
    AudioUnitParameterOptions, AudioUnitParameterStringFromValue, AudioUnitParameterUnit,
    AudioUnitPropertyID, AudioUnitPropertyListenerProc, AudioUnitRenderActionFlags, AudioUnitScope,
    kAudioUnitErr_FormatNotSupported, kAudioUnitErr_InvalidElement, kAudioUnitErr_InvalidParameter,
    kAudioUnitErr_InvalidProperty, kAudioUnitProperty_ElementCount, kAudioUnitProperty_Latency,
    kAudioUnitProperty_MIDIOutputCallback, kAudioUnitProperty_MIDIOutputCallbackInfo,
    kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitProperty_ParameterInfo,
    kAudioUnitProperty_ParameterList, kAudioUnitProperty_ParameterStringFromValue,
    kAudioUnitProperty_ParameterValueStrings, kAudioUnitProperty_SampleRate,
    kAudioUnitProperty_StreamFormat, kAudioUnitProperty_SupportedNumChannels,
    kAudioUnitProperty_TailTime, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitScope_Output, kAudioUnitType_MIDIProcessor,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFArray, CFRetained, CFString};
use plughost_core::{
    AudioBusConfig, AudioConfig, AudioDirection, Event, EventConfig, Layout, ParameterEvent,
    ProcessMode, SampleFormat,
};
use plughost_formats::HostedPlugin;
use plughost_formats::au::{self, Plugin};

// Property IDs under names that can be matched on.
const ELEMENT_COUNT: AudioUnitPropertyID = kAudioUnitProperty_ElementCount;
const LATENCY: AudioUnitPropertyID = kAudioUnitProperty_Latency;
const MIDI_OUTPUT_CALLBACK: AudioUnitPropertyID = kAudioUnitProperty_MIDIOutputCallback;
const MIDI_OUTPUT_CALLBACK_INFO: AudioUnitPropertyID = kAudioUnitProperty_MIDIOutputCallbackInfo;
const MAXIMUM_FRAMES_PER_SLICE: AudioUnitPropertyID = kAudioUnitProperty_MaximumFramesPerSlice;
const PARAMETER_INFO: AudioUnitPropertyID = kAudioUnitProperty_ParameterInfo;
const PARAMETER_LIST: AudioUnitPropertyID = kAudioUnitProperty_ParameterList;
const PARAMETER_STRING_FROM_VALUE: AudioUnitPropertyID =
    kAudioUnitProperty_ParameterStringFromValue;
const PARAMETER_VALUE_STRINGS: AudioUnitPropertyID = kAudioUnitProperty_ParameterValueStrings;
const SAMPLE_RATE: AudioUnitPropertyID = kAudioUnitProperty_SampleRate;
const STREAM_FORMAT: AudioUnitPropertyID = kAudioUnitProperty_StreamFormat;
const SUPPORTED_NUM_CHANNELS: AudioUnitPropertyID = kAudioUnitProperty_SupportedNumChannels;
const TAIL_TIME: AudioUnitPropertyID = kAudioUnitProperty_TailTime;

type OsStatus = i32;
type OutputCallback = unsafe extern "C-unwind" fn(
    *mut c_void,
    *const AudioTimeStamp,
    u32,
    *const PacketList,
) -> OsStatus;

/// AUMIDIOutputCallbackStruct.
#[repr(C)]
#[derive(Clone, Copy)]
struct OutputTarget {
    callback: Option<OutputCallback>,
    user_data: *mut c_void,
}

/// MIDIPacket and a MIDIPacketList of one packet, which CoreMIDI packs to 4 bytes.
#[repr(C, packed(4))]
struct Packet {
    time: u64,
    length: u16,
    data: [u8; 256],
}
#[repr(C, packed(4))]
struct PacketList {
    count: u32,
    packet: Packet,
}

const MODE: u32 = 0;
const RENAME: u32 = 1;
const STEPS: u32 = 2;
const NAMES: [&str; 3] = ["Mode", "Rename", "Steps"];

struct Listener {
    property: AudioUnitPropertyID,
    proc: AudioUnitPropertyListenerProc,
    user_data: *mut c_void,
}

struct State {
    unit: AudioUnit,
    format: AudioStreamBasicDescription,
    max_frames: u32,
    values: [f32; 3],
    renamed: bool,
    listeners: Vec<Listener>,
    output: Option<OutputTarget>,
    /// MIDI received for the next render, with its offset into it.
    received: Vec<(u32, Vec<u8>)>,
    scratch: Vec<f32>,
    names: [CFRetained<CFString>; 3],
}

impl State {
    fn mode_names(&self) -> [&'static str; 2] {
        if self.renamed {
            ["Dry", "Wet"]
        } else {
            ["Off", "On"]
        }
    }
}

#[repr(C)]
struct Instance {
    interface: AudioComponentPlugInInterface,
    state: Mutex<State>,
}

fn state<'a>(this: NonNull<c_void>) -> MutexGuard<'a, State> {
    // SAFETY: every method receives the instance the factory created.
    unsafe { this.cast::<Instance>().as_ref() }
        .state
        .lock()
        .unwrap()
}

fn stereo(sample_rate: f64) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat
            | kAudioFormatFlagIsPacked
            | kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: 2,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

unsafe extern "C-unwind" fn factory(
    _: NonNull<AudioComponentDescription>,
) -> *mut AudioComponentPlugInInterface {
    let instance = Box::new(Instance {
        interface: AudioComponentPlugInInterface {
            Open: open,
            Close: close,
            Lookup: lookup,
            reserved: std::ptr::null_mut(),
        },
        state: Mutex::new(State {
            unit: std::ptr::null_mut(),
            format: stereo(48_000.0),
            max_frames: 1156,
            values: [0.0; 3],
            renamed: false,
            listeners: Vec::new(),
            output: None,
            received: Vec::new(),
            scratch: Vec::new(),
            names: NAMES.map(CFString::from_str),
        }),
    });
    Box::into_raw(instance).cast()
}

unsafe extern "C-unwind" fn open(this: NonNull<c_void>, unit: AudioComponentInstance) -> OsStatus {
    state(this).unit = unit;
    0
}

unsafe extern "C-unwind" fn close(this: NonNull<c_void>) -> OsStatus {
    drop(unsafe { Box::from_raw(this.cast::<Instance>().as_ptr()) });
    0
}

unsafe extern "C-unwind" fn lookup(selector: i16) -> AudioComponentMethod {
    let method: *const c_void = match selector {
        0x0001 | 0x0002 => no_op as *const c_void,
        0x0003 => get_property_info as *const c_void,
        0x0004 => get_property as *const c_void,
        0x0005 => set_property as *const c_void,
        0x0006 => get_parameter as *const c_void,
        0x0007 => set_parameter as *const c_void,
        0x0009 => reset as *const c_void,
        0x000A => add_listener as *const c_void,
        0x000B => remove_listener as *const c_void,
        0x0012 => remove_listener_with_user_data as *const c_void,
        0x000E => render as *const c_void,
        0x0011 => schedule_parameters as *const c_void,
        0x0101 => midi_event as *const c_void,
        0x0102 => sysex as *const c_void,
        _ => return None,
    };
    // SAFETY: each function has the signature of its selector's API call.
    unsafe { std::mem::transmute::<*const c_void, AudioComponentMethod>(method) }
}

unsafe extern "C-unwind" fn no_op(_: NonNull<c_void>) -> OsStatus {
    0
}

unsafe extern "C-unwind" fn reset(_: NonNull<c_void>, _: AudioUnitScope, _: u32) -> OsStatus {
    0
}

/// The size of a property, or None when the unit does not have it there.
fn property_size(id: AudioUnitPropertyID, scope: AudioUnitScope, element: u32) -> Option<usize> {
    let global = scope == kAudioUnitScope_Global;
    Some(match id {
        SAMPLE_RATE | LATENCY | TAIL_TIME => size_of::<f64>(),
        PARAMETER_LIST => usize::from(global) * NAMES.len() * size_of::<u32>(),
        PARAMETER_INFO if global && element <= STEPS => size_of::<AudioUnitParameterInfo>(),
        PARAMETER_VALUE_STRINGS if global && element == MODE => size_of::<*const CFArray>(),
        PARAMETER_STRING_FROM_VALUE => size_of::<AudioUnitParameterStringFromValue>(),
        STREAM_FORMAT if scope == kAudioUnitScope_Output && element == 0 => {
            size_of::<AudioStreamBasicDescription>()
        }
        ELEMENT_COUNT | MAXIMUM_FRAMES_PER_SLICE => size_of::<u32>(),
        SUPPORTED_NUM_CHANNELS => size_of::<AUChannelInfo>(),
        MIDI_OUTPUT_CALLBACK_INFO => size_of::<*const CFArray>(),
        MIDI_OUTPUT_CALLBACK => size_of::<OutputTarget>(),
        _ => return None,
    })
}

unsafe extern "C-unwind" fn get_property_info(
    _: NonNull<c_void>,
    id: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: u32,
    size: *mut u32,
    writable: *mut u8,
) -> OsStatus {
    let Some(bytes) = property_size(id, scope, element) else {
        return kAudioUnitErr_InvalidProperty;
    };
    // SAFETY: the caller passes null or valid outputs.
    unsafe {
        if let Some(size) = size.as_mut() {
            *size = bytes as u32;
        }
        if let Some(writable) = writable.as_mut() {
            *writable = u8::from(matches!(
                id,
                SAMPLE_RATE | STREAM_FORMAT | MAXIMUM_FRAMES_PER_SLICE | MIDI_OUTPUT_CALLBACK
            ));
        }
    }
    0
}

fn parameter_info(state: &State, id: u32) -> AudioUnitParameterInfo {
    AudioUnitParameterInfo {
        name: [0; 52],
        unitName: std::ptr::null(),
        clumpID: 0,
        cfNameString: CFRetained::as_ptr(&state.names[id as usize]).as_ptr(),
        unit: AudioUnitParameterUnit::Indexed,
        minValue: 0.0,
        maxValue: if id == STEPS { 22.0 } else { 1.0 },
        defaultValue: 0.0,
        flags: AudioUnitParameterOptions::Flag_IsReadable
            | AudioUnitParameterOptions::Flag_IsWritable
            | AudioUnitParameterOptions::Flag_HasCFNameString,
    }
}

/// Writes `value` to a property's output, checking the size the caller offers.
///
/// # Safety
///
/// `data` must be writable for `*size` bytes.
unsafe fn put<T>(data: *mut c_void, size: *mut u32, value: T) -> OsStatus {
    unsafe {
        if (*size as usize) < size_of::<T>() {
            return kAudioUnitErr_InvalidProperty;
        }
        data.cast::<T>().write_unaligned(value);
        *size = size_of::<T>() as u32;
    }
    0
}

/// A retained CFArray of `names`, which the caller of a property releases.
fn names_array(names: &[&str]) -> *const CFArray {
    let names: Vec<_> = names.iter().map(|name| CFString::from_str(name)).collect();
    CFRetained::into_raw(CFArray::from_retained_objects(&names))
        .as_ptr()
        .cast_const()
        .cast()
}

unsafe extern "C-unwind" fn get_property(
    this: NonNull<c_void>,
    id: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: u32,
    data: *mut c_void,
    size: *mut u32,
) -> OsStatus {
    if property_size(id, scope, element).is_none() {
        return kAudioUnitErr_InvalidProperty;
    }
    let state = state(this);
    // SAFETY: the caller passes an output of the size the property info reported.
    unsafe {
        match id {
            SAMPLE_RATE => put(data, size, state.format.mSampleRate),
            LATENCY | TAIL_TIME => put(data, size, 0.0f64),
            PARAMETER_LIST if scope == kAudioUnitScope_Global => {
                put(data, size, [MODE, RENAME, STEPS])
            }
            PARAMETER_LIST => {
                *size = 0;
                0
            }
            PARAMETER_INFO => put(data, size, parameter_info(&state, element)),
            PARAMETER_VALUE_STRINGS => put(data, size, names_array(&state.mode_names())),
            PARAMETER_STRING_FROM_VALUE => {
                // Formats values the way AUBase does, from the value strings.
                let request = data.cast::<AudioUnitParameterStringFromValue>();
                if (*request).inParamID != MODE {
                    return kAudioUnitErr_InvalidParameter;
                }
                let index = (*request).inValue.read().round() as usize;
                let Some(name) = state.mode_names().get(index).copied() else {
                    return kAudioUnitErr_InvalidParameter;
                };
                (*request).outString = CFRetained::into_raw(CFString::from_str(name)).as_ptr();
                0
            }
            STREAM_FORMAT => put(data, size, state.format),
            ELEMENT_COUNT => put(data, size, u32::from(scope != kAudioUnitScope_Input)),
            MAXIMUM_FRAMES_PER_SLICE => put(data, size, state.max_frames),
            SUPPORTED_NUM_CHANNELS => put(
                data,
                size,
                AUChannelInfo {
                    inChannels: 0,
                    outChannels: 2,
                },
            ),
            MIDI_OUTPUT_CALLBACK_INFO => put(data, size, names_array(&["Out"])),
            _ => kAudioUnitErr_InvalidProperty,
        }
    }
}

unsafe extern "C-unwind" fn set_property(
    this: NonNull<c_void>,
    id: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: u32,
    data: *const c_void,
    size: u32,
) -> OsStatus {
    let mut state = state(this);
    let size = size as usize;
    // SAFETY: the caller passes `size` readable bytes.
    unsafe {
        match id {
            SAMPLE_RATE if size == size_of::<f64>() => {
                state.format.mSampleRate = data.cast::<f64>().read_unaligned();
                0
            }
            STREAM_FORMAT
                if scope == kAudioUnitScope_Output
                    && element == 0
                    && size == size_of::<AudioStreamBasicDescription>() =>
            {
                let format = data.cast::<AudioStreamBasicDescription>().read_unaligned();
                if format != stereo(format.mSampleRate) {
                    return kAudioUnitErr_FormatNotSupported;
                }
                state.format = format;
                0
            }
            STREAM_FORMAT => kAudioUnitErr_InvalidElement,
            MAXIMUM_FRAMES_PER_SLICE if size == size_of::<u32>() => {
                state.max_frames = data.cast::<u32>().read_unaligned();
                0
            }
            MIDI_OUTPUT_CALLBACK if size == size_of::<OutputTarget>() => {
                state.output = Some(data.cast::<OutputTarget>().read_unaligned());
                0
            }
            // The bridge also sets properties this unit has no use for, such as host callbacks.
            _ => 0,
        }
    }
}

unsafe extern "C-unwind" fn add_listener(
    this: NonNull<c_void>,
    property: AudioUnitPropertyID,
    proc: AudioUnitPropertyListenerProc,
    user_data: *mut c_void,
) -> OsStatus {
    state(this).listeners.push(Listener {
        property,
        proc,
        user_data,
    });
    0
}

fn same_proc(a: AudioUnitPropertyListenerProc, b: AudioUnitPropertyListenerProc) -> bool {
    a.map(|proc| proc as usize) == b.map(|proc| proc as usize)
}

unsafe extern "C-unwind" fn remove_listener(
    this: NonNull<c_void>,
    property: AudioUnitPropertyID,
    proc: AudioUnitPropertyListenerProc,
) -> OsStatus {
    state(this)
        .listeners
        .retain(|listener| listener.property != property || !same_proc(listener.proc, proc));
    0
}

unsafe extern "C-unwind" fn remove_listener_with_user_data(
    this: NonNull<c_void>,
    property: AudioUnitPropertyID,
    proc: AudioUnitPropertyListenerProc,
    user_data: *mut c_void,
) -> OsStatus {
    state(this).listeners.retain(|listener| {
        listener.property != property
            || !same_proc(listener.proc, proc)
            || listener.user_data != user_data
    });
    0
}

unsafe extern "C-unwind" fn get_parameter(
    this: NonNull<c_void>,
    id: u32,
    scope: AudioUnitScope,
    _: u32,
    value: *mut f32,
) -> OsStatus {
    match state(this).values.get(id as usize) {
        Some(&current) if scope == kAudioUnitScope_Global => {
            // SAFETY: the caller passes a valid output.
            unsafe { *value = current };
            0
        }
        _ => kAudioUnitErr_InvalidParameter,
    }
}

unsafe extern "C-unwind" fn set_parameter(
    this: NonNull<c_void>,
    id: u32,
    scope: AudioUnitScope,
    _: u32,
    value: f32,
    _: u32,
) -> OsStatus {
    let mut state = state(this);
    if scope != kAudioUnitScope_Global || id as usize >= state.values.len() {
        return kAudioUnitErr_InvalidParameter;
    }
    if id == STEPS {
        STEPS_RECEIVED.lock().unwrap().push((None, value));
    }
    state.values[id as usize] = value;
    if id != RENAME || value < 0.5 || state.renamed {
        return 0;
    }
    state.renamed = true;
    let unit = state.unit;
    let listeners: Vec<_> = state
        .listeners
        .iter()
        .filter(|listener| listener.property == PARAMETER_VALUE_STRINGS)
        .map(|listener| (listener.proc, listener.user_data))
        .collect();
    // Listeners may query the unit, so they run without its lock.
    drop(state);
    for (proc, user_data) in listeners {
        if let (Some(proc), Some(user_data)) = (proc, NonNull::new(user_data)) {
            unsafe {
                proc(
                    user_data,
                    unit,
                    PARAMETER_VALUE_STRINGS,
                    kAudioUnitScope_Global,
                    MODE,
                )
            };
        }
    }
    0
}

/// AudioUnitParameterEvent: an immediate event's values are its buffer offset and value.
#[repr(C)]
struct ScheduledEvent {
    scope: u32,
    element: u32,
    parameter: u32,
    kind: u32,
    values: [u32; 4],
}
const IMMEDIATE: u32 = 1;

unsafe extern "C-unwind" fn schedule_parameters(
    _: NonNull<c_void>,
    events: *const ScheduledEvent,
    count: u32,
) -> OsStatus {
    // SAFETY: the caller passes `count` events.
    for event in unsafe { std::slice::from_raw_parts(events, count as usize) } {
        if event.parameter == STEPS && event.kind == IMMEDIATE {
            STEPS_RECEIVED
                .lock()
                .unwrap()
                .push((Some(event.values[0]), f32::from_bits(event.values[1])));
        }
    }
    0
}

unsafe extern "C-unwind" fn midi_event(
    this: NonNull<c_void>,
    status: u32,
    data1: u32,
    data2: u32,
    offset: u32,
) -> OsStatus {
    let length = if matches!(status & 0xF0, 0xC0 | 0xD0) {
        2
    } else {
        3
    };
    let bytes = [status as u8, data1 as u8, data2 as u8];
    state(this)
        .received
        .push((offset, bytes[..length].to_vec()));
    0
}

/// v2 units receive system exclusive messages without a time; this one sends them at the start.
unsafe extern "C-unwind" fn sysex(this: NonNull<c_void>, data: *const u8, length: u32) -> OsStatus {
    // SAFETY: the caller passes `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec();
    state(this).received.push((0, bytes));
    0
}

unsafe extern "C-unwind" fn render(
    this: NonNull<c_void>,
    _: *mut AudioUnitRenderActionFlags,
    time: *const AudioTimeStamp,
    _: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OsStatus {
    let mut state = state(this);
    if state.values[STEPS as usize] == TIMED {
        // SAFETY: the caller passes a valid time stamp.
        RENDER_TIMES
            .lock()
            .unwrap()
            .push(unsafe { (*time).mSampleTime });
    }
    if state
        .received
        .iter()
        .any(|(_, bytes)| bytes[..] == [0x90, FAIL_KEY, 1])
    {
        state.received.clear();
        return kAudioUnitErr_InvalidParameter;
    }
    let frames = frames as usize;
    state.scratch.resize(2 * frames, 0.0);
    let scratch = state.scratch.as_mut_ptr();
    // SAFETY: the caller passes a buffer list with its declared buffers; a null buffer asks the
    // unit for its own.
    let buffers = unsafe {
        std::slice::from_raw_parts_mut(
            (*data).mBuffers.as_mut_ptr(),
            (*data).mNumberBuffers as usize,
        )
    };
    for (index, buffer) in buffers.iter_mut().enumerate() {
        if buffer.mData.is_null() {
            buffer.mData = unsafe { scratch.add(index * frames) }.cast();
        }
        buffer.mDataByteSize = (frames * size_of::<f32>()) as u32;
        unsafe { std::ptr::write_bytes(buffer.mData.cast::<f32>(), 0, frames) };
    }
    let received = std::mem::take(&mut state.received);
    let Some(OutputTarget {
        callback: Some(callback),
        user_data,
    }) = state.output
    else {
        return 0;
    };
    for (offset, bytes) in received {
        let mut data = [0; 256];
        data[..bytes.len()].copy_from_slice(&bytes);
        // A packet's time is its offset into the rendered block.
        let list = PacketList {
            count: 1,
            packet: Packet {
                time: u64::from(offset),
                length: bytes.len() as u16,
                data,
            },
        };
        unsafe { callback(user_data, time, 0, &list) };
    }
    0
}

/// Registers the unit once and returns its class ID.
fn class_id() -> String {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        AudioComponentRegister(
            NonNull::from(&AudioComponentDescription {
                componentType: kAudioUnitType_MIDIProcessor,
                componentSubType: u32::from_be_bytes(*b"v2ec"),
                componentManufacturer: u32::from_be_bytes(*b"Plgh"),
                componentFlags: 0,
                componentFlagsMask: 0,
            }),
            &CFString::from_str("plughost: v2 echo"),
            1,
            Some(factory),
        );
    });
    au::components()
        .into_iter()
        .find(|component| component.name == "v2 echo")
        .unwrap()
        .class_id
}

fn prepared() -> Plugin {
    let mut plugin = Plugin::new(&class_id()).unwrap();
    let output = plugin
        .audio_buses()
        .unwrap()
        .into_iter()
        .find(|bus| bus.direction == AudioDirection::Output)
        .unwrap();
    plugin
        .prepare_audio(&AudioConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            configuration: None,
            inputs: Vec::new(),
            outputs: vec![AudioBusConfig {
                id: output.id,
                layout: Layout::Stereo,
                active: true,
            }],
            events: EventConfig {
                inputs: vec![0],
                outputs: vec![0],
            },
        })
        .unwrap();
    plugin
}

fn block(plugin: &mut Plugin, events: &[Event]) -> Vec<Event> {
    let mut output = vec![vec![0.0f32; 512]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let mut produced = Vec::new();
    plugin
        .process(
            &plughost_core::BlockContext::new(512),
            &[],
            &mut outputs,
            &[],
            events,
            &mut produced,
        )
        .unwrap();
    produced
}

#[test]
fn midi_a_v2_unit_sends_arrives_through_the_bridge_at_its_offsets() {
    let mut plugin = prepared();
    let events = [
        Event::note_on(100, 0, 60, 100),
        Event::control_change(300, 1, 7, 64),
    ];
    assert_eq!(block(&mut plugin, &events), events);
    let sysex = Event::sysex(200, vec![0xF0, 0x7D, 0x01, 0x02, 0xF7]);
    assert_eq!(
        block(&mut plugin, std::slice::from_ref(&sysex)),
        [sysex.clone().at(0)]
    );
    assert_eq!(block(&mut plugin, &[]), []);
}

#[test]
fn renamed_parameter_values_are_reported_as_changed_metadata() {
    let mut plugin = Plugin::new(&class_id()).unwrap();
    assert_eq!(plugin.parameter_text(u64::from(MODE), 1.0).unwrap(), "On");
    assert!(plugin.take_parameter_events().events.is_empty());

    plugin.set_parameter(u64::from(RENAME), 1.0).unwrap();
    let batch = plugin.take_parameter_events();
    assert_eq!(batch.events, [ParameterEvent::MetadataChanged]);
    assert!(batch.resync_required);
    assert_eq!(plugin.parameter_text(u64::from(MODE), 1.0).unwrap(), "Wet");
    assert!(plugin.take_parameter_events().events.is_empty());
}

#[test]
fn index_values_reach_the_unit_whole_and_scheduled_at_their_offsets() {
    let mut plugin = prepared();
    // 13 / 22 times 22 is 12.999999 in single precision, which a unit that truncates reads as 12.
    let thirteen = 13.0 / 22.0;
    plugin.set_parameter(u64::from(STEPS), thirteen).unwrap();
    let mut output = vec![vec![0.0f32; 512]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    plugin
        .process(
            &plughost_core::BlockContext::new(512),
            &[],
            &mut outputs,
            &[plughost_core::ParameterChange {
                id: u64::from(STEPS),
                offset: 256,
                value: thirteen,
            }],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    // Another test times its renders by setting `TIMED`.
    let received: Vec<_> = STEPS_RECEIVED
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|&(_, value)| value != TIMED)
        .collect();
    assert_eq!(received, [(None, 13.0), (Some(256), 13.0)]);
}

#[test]
fn a_failed_render_still_moves_the_timeline_on() {
    let mut plugin = prepared();
    plugin
        .set_parameter(u64::from(STEPS), f64::from(TIMED) / 22.0)
        .unwrap();
    let mut render = |events: &[Event]| {
        let mut output = vec![vec![0.0f32; 512]; 2];
        let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
        plugin.process(
            &plughost_core::BlockContext::new(512),
            &[],
            &mut outputs,
            &[],
            events,
            &mut Vec::new(),
        )
    };
    render(&[]).unwrap();
    render(&[Event::note_on(0, 0, FAIL_KEY, 1)]).unwrap_err();
    render(&[]).unwrap();
    assert_eq!(*RENDER_TIMES.lock().unwrap(), [0.0, 512.0, 1024.0]);
}
