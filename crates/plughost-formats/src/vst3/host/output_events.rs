//! The event list a processor fills during `process`. Each event becomes a MIDI message as it
//! arrives, while the plugin's data (system exclusive bytes) is still valid, within the block's
//! prepared budgets.
use super::*;
use crate::vst3::errors::Vst3Error;
use plughost_core::{MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MidiData, MidiEvent};
use vst3::Steinberg::Vst::ControllerNumbers_::{
    kAfterTouch, kCtrlPolyPressure, kCtrlProgramChange, kPitchBend,
};
use vst3::Steinberg::Vst::DataEvent_::DataTypes_::kMidiSysEx;
use vst3::Steinberg::Vst::Event_::EventTypes_;

#[derive(Default)]
struct Output {
    events: Vec<MidiEvent>,
    sysex: usize,
    /// The plugin added more events or system exclusive bytes than the block allows.
    overflow: bool,
    /// Events with no MIDI 1.0 form (note expression, chords, scales, out-of-range values).
    unconvertible: u64,
}

pub(crate) struct OutputEventList {
    output: Mutex<Output>,
}
impl Class for OutputEventList {
    type Interfaces = (IEventList,);
}
impl OutputEventList {
    pub fn new() -> Result<Self, Vst3Error> {
        let mut events = Vec::new();
        events
            .try_reserve_exact(MAX_BLOCK_EVENTS)
            .map_err(|_| Vst3Error::EventStorage)?;
        Ok(Self {
            output: Mutex::new(Output {
                events,
                ..Output::default()
            }),
        })
    }
    /// Moves the events added since the last call into `into` in offset order, keeping the
    /// plugin's order at equal offsets; fails when the plugin exceeded the block's budgets, whose
    /// events are discarded.
    pub fn take(&self, into: &mut Vec<MidiEvent>) -> Result<(), Vst3Error> {
        let mut output = lock(&self.output);
        output.sysex = 0;
        if std::mem::take(&mut output.overflow) {
            output.events.clear();
            return Err(Vst3Error::OutputEventCapacity);
        }
        output.events.sort_by_key(|event| event.offset);
        into.append(&mut output.events);
        Ok(())
    }
    /// Events that had no MIDI form, since the last call.
    pub fn take_unconvertible(&self) -> u64 {
        std::mem::take(&mut lock(&self.output).unconvertible)
    }
    pub fn ptr(list: &ComWrapper<Self>) -> *mut IEventList {
        list.as_com_ref::<IEventList>()
            .map_or(std::ptr::null_mut(), |l| l.as_ptr())
    }
}

// Event and data type constants have the platform's C enum type; events store them as integers.
const NOTE_ON: u32 = EventTypes_::kNoteOnEvent as u32;
const NOTE_OFF: u32 = EventTypes_::kNoteOffEvent as u32;
const POLY_PRESSURE: u32 = EventTypes_::kPolyPressureEvent as u32;
const DATA: u32 = EventTypes_::kDataEvent as u32;
const LEGACY_MIDI_CC_OUT: u32 = EventTypes_::kLegacyMIDICCOutEvent as u32;
const SYSEX: u32 = kMidiSysEx as u32;

/// A 7-bit value from a normalized one.
fn seven_bits(value: f32) -> u8 {
    (f64::from(value) * 127.0).round().clamp(0.0, 127.0) as u8
}

/// The MIDI form of one output event, `None` when it has none.
fn convert(event: &Event) -> Option<(usize, usize, MidiData)> {
    let offset = usize::try_from(event.sampleOffset).ok()?;
    let port = usize::try_from(event.busIndex).ok()?;
    let channel_message = |status: u8, channel: i32, first: i32, second: u8| {
        let channel = u8::try_from(channel).ok().filter(|c| *c < 16)?;
        let first = u8::try_from(first).ok().filter(|b| *b < 0x80)?;
        Some(MidiData::Channel([status | channel, first, second]))
    };
    // SAFETY: the union field read matches the event type the plugin declared.
    let data = unsafe {
        match u32::from(event.r#type) {
            NOTE_ON => {
                let note = event.__field0.noteOn;
                // A note on keeps a velocity of at least 1, since 0 would be a MIDI note off.
                let velocity = seven_bits(note.velocity).max(1);
                channel_message(0x90, note.channel.into(), note.pitch.into(), velocity)?
            }
            NOTE_OFF => {
                let note = event.__field0.noteOff;
                let velocity = seven_bits(note.velocity);
                channel_message(0x80, note.channel.into(), note.pitch.into(), velocity)?
            }
            POLY_PRESSURE => {
                let pressure = event.__field0.polyPressure;
                channel_message(
                    0xA0,
                    pressure.channel.into(),
                    pressure.pitch.into(),
                    seven_bits(pressure.pressure),
                )?
            }
            DATA => {
                let data = event.__field0.data;
                if data.r#type != SYSEX || data.bytes.is_null() {
                    return None;
                }
                // The plugin owns the bytes for this call; they are copied now.
                let bytes = std::slice::from_raw_parts(data.bytes, data.size as usize).to_vec();
                MidiData::SysEx(bytes)
            }
            LEGACY_MIDI_CC_OUT => {
                let cc = event.__field0.midiCCOut;
                let (value, value2) = (u8::try_from(cc.value).ok()?, u8::try_from(cc.value2).ok()?);
                let channel = i32::from(cc.channel);
                match i16::from(cc.controlNumber) {
                    number if number < 128 => channel_message(0xB0, channel, number.into(), value)?,
                    number if number == kAfterTouch as i16 => {
                        channel_message(0xD0, channel, value.into(), 0)?
                    }
                    number if number == kPitchBend as i16 => {
                        channel_message(0xE0, channel, value.into(), value2)?
                    }
                    number if number == kCtrlProgramChange as i16 => {
                        channel_message(0xC0, channel, value.into(), 0)?
                    }
                    number if number == kCtrlPolyPressure as i16 => {
                        channel_message(0xA0, channel, value.into(), value2)?
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    };
    let event = MidiEvent { offset, port, data };
    event.is_valid().then_some((offset, port, event.data))
}

impl IEventListTrait for OutputEventList {
    unsafe fn getEventCount(&self) -> int32 {
        lock(&self.output).events.len() as i32
    }
    /// Converted events are not handed back to the plugin.
    unsafe fn getEvent(&self, _index: int32, _event: *mut Event) -> tresult {
        kResultFalse
    }
    unsafe fn addEvent(&self, event: *mut Event) -> tresult {
        if event.is_null() {
            return kInvalidArgument;
        }
        let mut output = lock(&self.output);
        let Some((offset, port, data)) = convert(unsafe { &*event }) else {
            output.unconvertible += 1;
            return kResultOk;
        };
        let sysex = match &data {
            MidiData::SysEx(bytes) => bytes.len(),
            MidiData::Channel(_) => 0,
        };
        if output.events.len() == MAX_BLOCK_EVENTS
            || output.sysex.saturating_add(sysex) > MAX_BLOCK_SYSEX_BYTES
        {
            output.overflow = true;
            return kResultFalse;
        }
        output.sysex += sysex;
        output.events.push(MidiEvent { offset, port, data });
        kResultOk
    }
}
