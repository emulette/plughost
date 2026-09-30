//! Owned, bounded standard input events; Clack supplies the native list callbacks.
use super::*;
use clack_host::events::event_types::MidiSysExEvent;
use clack_host::events::io::InputEventBuffer;
use clack_host::events::{Event, UnknownEvent};
use plughost_core::MidiData;

// Pending control edits and the current block each have their own bounded input budget.
const CAPACITY: usize = 2 * plughost_core::MAX_BLOCK_EVENTS;

enum NativeEvent {
    Parameter(ParamValueEvent),
    NoteOn(NoteOnEvent),
    NoteOff(NoteOffEvent),
    Midi(ClapMidiEvent),
    /// Points into the block's input events, which outlive the process call.
    SysEx(MidiSysExEvent),
}
impl NativeEvent {
    fn unknown(&self) -> &UnknownEvent {
        match self {
            Self::Parameter(event) => event.as_unknown(),
            Self::NoteOn(event) => event.as_unknown(),
            Self::NoteOff(event) => event.as_unknown(),
            Self::Midi(event) => event.as_unknown(),
            Self::SysEx(event) => event.as_unknown(),
        }
    }
}

pub(super) struct InputBuffer {
    events: Vec<(usize, NativeEvent)>,
}
impl InputBuffer {
    pub fn new() -> Result<Self, ClapError> {
        let mut events = Vec::new();
        events
            .try_reserve_exact(CAPACITY)
            .map_err(|_| ClapError::EventStorage)?;
        Ok(Self { events })
    }
    pub fn clear(&mut self) {
        self.events.clear();
    }
    fn push(&mut self, event: NativeEvent) {
        self.events.push((self.events.len(), event));
    }
    pub fn parameter(&mut self, offset: u32, id: ClapId, value: f64) {
        self.push(NativeEvent::Parameter(ParamValueEvent::new(
            offset,
            id,
            Pckn::match_all(),
            value,
        )));
    }
    /// Queues `event` for its port, whose dialects decide the native form: notes as CLAP note
    /// events where the port takes them, other channel messages (and notes on MIDI-only ports)
    /// and system exclusive data as MIDI events where it takes MIDI; the rest are not delivered.
    /// A system exclusive event refers to `event`'s bytes until the buffer is cleared.
    pub fn note(&mut self, event: &MidiEvent, dialects: NoteDialects) {
        let time = event.offset as u32;
        let port = event.port as u16;
        let note = |channel: u8, key: u8| {
            Pckn::new(port, u16::from(channel), u16::from(key), Match::<u32>::All)
        };
        if let MidiData::SysEx(bytes) = &event.data {
            if dialects.supports(NoteDialect::Midi) {
                // SAFETY: the caller keeps `event` alive until the buffer is cleared, after the
                // process call that reads it.
                let sysex = unsafe { MidiSysExEvent::new(time, port, bytes) };
                self.push(NativeEvent::SysEx(sysex));
            }
            return;
        }
        let clap_notes = dialects.supports(NoteDialect::Clap);
        match event.message() {
            Some(Message::NoteOn {
                channel,
                key,
                velocity,
            }) if clap_notes => {
                self.push(NativeEvent::NoteOn(NoteOnEvent::new(
                    time,
                    note(channel, key),
                    f64::from(velocity) / 127.0,
                )));
            }
            Some(Message::NoteOff {
                channel,
                key,
                velocity,
            }) if clap_notes => {
                self.push(NativeEvent::NoteOff(NoteOffEvent::new(
                    time,
                    note(channel, key),
                    f64::from(velocity) / 127.0,
                )));
            }
            Some(_) if dialects.supports(NoteDialect::Midi) => {
                if let MidiData::Channel(data) = event.data {
                    self.push(NativeEvent::Midi(ClapMidiEvent::new(time, port, data)));
                }
            }
            _ => {}
        }
    }
    pub fn sort(&mut self) {
        // Insertion order breaks ties, preserving pending edits, automation, then MIDI at a time.
        self.events
            .sort_unstable_by_key(|(order, event)| (event.unknown().header().time(), *order));
    }
}
impl InputEventBuffer for InputBuffer {
    fn len(&self) -> u32 {
        self.events.len() as u32
    }
    fn get(&self, index: u32) -> Option<&UnknownEvent> {
        self.events
            .get(index as usize)
            .map(|(_, event)| event.unknown())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clack_host::events::spaces::CoreEventSpace;

    #[test]
    fn ordered_native_events_keep_duplicate_edits_and_note_off_after_note_on() {
        let mut input = InputBuffer::new().unwrap();
        input.parameter(0, ClapId::new(7), 0.25);
        input.parameter(2, ClapId::new(7), 0.5);
        input.parameter(2, ClapId::new(7), 0.75);
        input.note(&MidiEvent::note_on(1, 0, 60, 127), NoteDialects::CLAP);
        input.note(&MidiEvent::note_off(1, 0, 60, 0), NoteDialects::CLAP);
        input.note(&MidiEvent::channel(2, [0xb0, 1, 64]), NoteDialects::MIDI);
        input.sort();
        let events = InputEvents::from_buffer(&input);
        let times: Vec<_> = events.iter().map(|e| e.header().time()).collect();
        assert_eq!(times, [0, 1, 1, 2, 2, 2]);
        assert!(matches!(
            input.get(1).unwrap().as_core_event(),
            Some(CoreEventSpace::NoteOn(_))
        ));
        assert!(matches!(
            input.get(2).unwrap().as_core_event(),
            Some(CoreEventSpace::NoteOff(_))
        ));
        for (index, expected) in [(0, 0.25), (3, 0.5), (4, 0.75)] {
            let Some(CoreEventSpace::ParamValue(event)) = input.get(index).unwrap().as_core_event()
            else {
                panic!("expected a parameter value");
            };
            assert_eq!(event.value(), expected);
        }
        assert!(matches!(
            input.get(5).unwrap().as_core_event(),
            Some(CoreEventSpace::Midi(_))
        ));
        input.clear();
        input.note(&MidiEvent::note_off(0, 3, 65, 0), NoteDialects::MIDI);
        assert_eq!(input.len(), 1);
        assert!(matches!(
            input.get(0).unwrap().as_core_event(),
            Some(CoreEventSpace::Midi(_))
        ));
    }
}
