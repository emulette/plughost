use super::*;
use vst3::Steinberg::Vst::Event_::EventTypes_;

// Event type constants have the platform's C enum type; events store them as integers.
const NOTE_ON: u32 = EventTypes_::kNoteOnEvent as u32;
const NOTE_OFF: u32 = EventTypes_::kNoteOffEvent as u32;
const DATA: u32 = EventTypes_::kDataEvent as u32;
const POLY_PRESSURE: u32 = EventTypes_::kPolyPressureEvent as u32;
const NOTE_EXPRESSION_VALUE: u32 = EventTypes_::kNoteExpressionValueEvent as u32;

impl Routing {
    /// Moves input events to output bus 0 as the crate documentation describes. A legacy
    /// controller output carries each note on's input key.
    pub(super) unsafe fn events(&self, data: &ProcessData) {
        let (Some(input), Some(output)) = (unsafe { ComRef::from_raw(data.inputEvents) }, unsafe {
            ComRef::from_raw(data.outputEvents)
        }) else {
            return;
        };
        for index in 0..unsafe { input.getEventCount() } {
            let mut event: Event = unsafe { std::mem::zeroed() };
            if unsafe { input.getEvent(index, &mut event) } != kResultOk {
                continue;
            }
            let bus = event.busIndex as usize;
            event.busIndex = 0;
            if bus == crate::EXPRESSION_PORT {
                if matches!(
                    u32::from(event.r#type),
                    NOTE_ON | NOTE_OFF | POLY_PRESSURE | NOTE_EXPRESSION_VALUE
                ) {
                    unsafe { output.addEvent(&mut event) };
                }
                continue;
            }
            match u32::from(event.r#type) {
                NOTE_ON => {
                    let (key, channel) =
                        unsafe { (event.__field0.noteOn.pitch, event.__field0.noteOn.channel) };
                    event.__field0.noteOn.pitch = crate::transpose(bus, key as u8).into();
                    unsafe { output.addEvent(&mut event) };
                    let mut cc = Event {
                        r#type: EventTypes_::kLegacyMIDICCOutEvent as u16,
                        __field0: Event__type0 {
                            midiCCOut: LegacyMIDICCOutEvent {
                                controlNumber: crate::KEY_CONTROLLER,
                                channel: channel as i8,
                                value: key as i8,
                                value2: 0,
                            },
                        },
                        ..event
                    };
                    unsafe { output.addEvent(&mut cc) };
                }
                NOTE_OFF => {
                    let key = unsafe { event.__field0.noteOff.pitch };
                    event.__field0.noteOff.pitch = crate::transpose(bus, key as u8).into();
                    if key == i16::from(crate::LATE_KEY) {
                        event.sampleOffset = data.numSamples;
                    }
                    unsafe { output.addEvent(&mut event) };
                }
                DATA => {
                    let data = unsafe { event.__field0.data };
                    let bytes =
                        unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
                    if bytes == crate::FLOOD {
                        for _ in 0..crate::FLOOD_EVENTS {
                            unsafe { output.addEvent(&mut event) };
                        }
                    } else {
                        unsafe { output.addEvent(&mut event) };
                    }
                }
                _ => {}
            }
        }
    }
}
