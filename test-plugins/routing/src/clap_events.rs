use super::*;
use clack_extensions::note_ports::*;
use clack_plugin::events::event_types::{MidiEvent, MidiSysExEvent, NoteEndEvent};
use clack_plugin::events::{Match, Pckn};

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, input: bool) -> u32 {
        if input { crate::EVENT_INPUTS } else { 1 }
    }
    fn get(&self, index: u32, input: bool, writer: &mut NotePortInfoWriter) {
        let name: &[u8] = match (input, index) {
            (false, 0) => b"Notes out",
            (true, 0) => b"Notes",
            (true, 1) => b"Octave notes",
            _ => return,
        };
        // The second input takes MIDI with MPE only, as MPE controllers' ports do.
        let (supported_dialects, preferred_dialect) = if input && index == 1 {
            (NoteDialects::MIDI_MPE, NoteDialect::MidiMpe)
        } else {
            (NoteDialects::MIDI, NoteDialect::Midi)
        };
        writer.set(&NotePortInfo {
            id: ClapId::new(index),
            name,
            supported_dialects,
            preferred_dialect: Some(preferred_dialect),
        });
    }
}

/// Moves input events to output port 0 as the crate documentation describes.
pub(super) fn route(input: &InputEvents, output: &mut OutputEvents, frames: u32) {
    for event in input {
        match event.as_core_event() {
            Some(CoreEventSpace::Midi(midi)) => {
                let time = midi.header().time();
                let [status, key, velocity] = midi.data();
                if matches!(status & 0xF0, 0x80 | 0x90) {
                    let port = usize::from(midi.port_index());
                    let note = [status, crate::transpose(port, key), velocity];
                    if status & 0xF0 == 0x80 && key == crate::LATE_KEY {
                        let channel = u16::from(status & 0x0F);
                        let end = Pckn::new(0u16, channel, u16::from(note[1]), Match::All);
                        let _ = output.try_push(MidiEvent::new(frames, 0, note));
                        let _ = output.try_push(NoteEndEvent::new(frames, end));
                        continue;
                    }
                    let _ = output.try_push(MidiEvent::new(time, 0, note));
                    if status & 0xF0 == 0x90 && velocity > 0 {
                        let channel = status & 0x0F;
                        let controller = [0xB0 | channel, crate::KEY_CONTROLLER, key];
                        let _ = output.try_push(MidiEvent::new(time, 0, controller));
                    }
                }
            }
            Some(CoreEventSpace::MidiSysEx(sysex)) => {
                // SAFETY: the host's buffer is valid during this process call, and the host
                // copies it as it takes the output event.
                let bytes = unsafe { sysex.data() };
                let echo = unsafe { MidiSysExEvent::new(sysex.header().time(), 0, bytes) };
                let copies = if bytes == crate::FLOOD {
                    crate::FLOOD_EVENTS
                } else {
                    1
                };
                for _ in 0..copies {
                    let _ = output.try_push(echo);
                }
            }
            _ => {}
        }
    }
}
