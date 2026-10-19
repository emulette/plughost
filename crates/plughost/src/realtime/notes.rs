//! The notes the callback delivered to the application through `produced` that have not ended.
//! A new generation starts from reset or replaced plugins, which will not send their note offs,
//! so its first callback sends them instead. The table has a fixed capacity, reserved with the
//! endpoint; the callback records and ends notes without allocating.

use plughost_core::{Event, EventData, Message, Note};

/// The most notes the table holds, across the chain's event outputs.
pub(crate) const CAPACITY: usize = 256;
const CHANNELS: usize = 16;
const KEYS: usize = 128;

/// How a note started, which is how its note off goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    /// A MIDI 1.0 note on.
    Midi,
    /// A note event, with its ID if it had one.
    Note(Option<u32>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Held {
    port: usize,
    channel: u8,
    key: u8,
    form: Form,
}

pub(crate) struct Notes {
    held: Vec<Held>,
    /// Held notes on each channel and key, on any port, so that a note off on a key without one
    /// looks no further.
    keys: Vec<u16>,
    /// Note ons the table had no room for since it was last emptied.
    unrecorded: u64,
}

impl Notes {
    pub fn new() -> Notes {
        Notes {
            held: Vec::with_capacity(CAPACITY),
            keys: vec![0; CHANNELS * KEYS],
            unrecorded: 0,
        }
    }

    /// Follows the note ons and offs among `events`. A note off with an ID ends the note with
    /// that ID; one without, as a MIDI note off, ends every note on its port, channel and key.
    pub fn record(&mut self, events: &[Event]) {
        for event in events {
            match &event.data {
                EventData::NoteOn(note) => {
                    self.start(event.port, note.channel, note.key, Form::Note(note.id));
                }
                EventData::NoteOff(note) => {
                    self.end(event.port, note.channel, note.key, note.id);
                }
                EventData::Midi(_) => match event.message() {
                    Some(Message::NoteOn { channel, key, .. }) => {
                        self.start(event.port, channel, key, Form::Midi);
                    }
                    Some(Message::NoteOff { channel, key, .. }) => {
                        self.end(event.port, channel, key, None);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Appends a note off at offset 0 for each held note to `events` while its capacity lasts,
    /// and forgets them. Returns how many note offs could not be sent: those beyond the
    /// capacity, and one for each note on the table had no room for.
    pub fn end_all(&mut self, events: &mut Vec<Event>) -> u64 {
        let mut dropped = std::mem::take(&mut self.unrecorded);
        for held in self.held.drain(..) {
            if events.len() == events.capacity() {
                dropped += 1;
                continue;
            }
            let data = match held.form {
                Form::Midi => EventData::Midi([0x80 | held.channel, held.key, 0]),
                Form::Note(id) => {
                    let note = Note::new(held.channel, held.key, 0.0);
                    EventData::NoteOff(id.map_or(note, |id| note.with_id(id)))
                }
            };
            events.push(Event::new(0, data).on_port(held.port));
        }
        self.keys.fill(0);
        dropped
    }

    fn start(&mut self, port: usize, channel: u8, key: u8, form: Form) {
        let held = Held {
            port,
            channel,
            key,
            form,
        };
        let index = index(channel, key);
        if self.keys[index] > 0 && self.held.contains(&held) {
            return;
        }
        if self.held.len() == CAPACITY {
            self.unrecorded += 1;
            return;
        }
        self.held.push(held);
        self.keys[index] += 1;
    }

    fn end(&mut self, port: usize, channel: u8, key: u8, id: Option<u32>) {
        let index = index(channel, key);
        if self.keys[index] == 0 {
            return;
        }
        let before = self.held.len();
        self.held.retain(|held| {
            held.port != port
                || held.channel != channel
                || held.key != key
                || id.is_some_and(|id| held.form != Form::Note(Some(id)))
        });
        self.keys[index] -= (before - self.held.len()) as u16;
    }
}

/// Validated events have channels below 16 and keys below 128.
fn index(channel: u8, key: u8) -> usize {
    usize::from(channel) * KEYS + usize::from(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_on(port: usize, channel: u8, key: u8, id: Option<u32>) -> Event {
        let note = Note::new(channel, key, 1.0);
        Event::new(0, EventData::NoteOn(id.map_or(note, |id| note.with_id(id)))).on_port(port)
    }

    fn note_off(port: usize, channel: u8, key: u8, id: Option<u32>) -> Event {
        let note = Note::new(channel, key, 0.0);
        Event::new(
            0,
            EventData::NoteOff(id.map_or(note, |id| note.with_id(id))),
        )
        .on_port(port)
    }

    fn ended(notes: &mut Notes) -> Vec<Event> {
        let mut events = Vec::with_capacity(CAPACITY);
        assert_eq!(notes.end_all(&mut events), 0);
        events
    }

    #[test]
    fn notes_that_did_not_end_get_note_offs_in_their_form_and_port() {
        let mut notes = Notes::new();
        notes.record(&[
            Event::note_on(0, 1, 60, 100).on_port(1),
            Event::note_on(0, 1, 62, 100),
            Event::note_on(0, 1, 62, 0),
            note_on(0, 2, 64, Some(7)),
            note_on(0, 2, 64, Some(8)),
            note_off(0, 2, 64, Some(8)),
            note_on(2, 3, 65, None),
            Event::control_change(0, 1, 20, 60),
        ]);
        assert_eq!(
            ended(&mut notes),
            [
                Event::note_off(0, 1, 60, 0).on_port(1),
                note_off(0, 2, 64, Some(7)),
                note_off(2, 3, 65, None),
            ]
        );
        assert!(ended(&mut notes).is_empty());
    }

    #[test]
    fn a_note_off_without_an_id_ends_every_note_on_its_key_and_port() {
        let mut notes = Notes::new();
        notes.record(&[
            note_on(0, 0, 60, Some(1)),
            note_on(0, 0, 60, Some(2)),
            Event::note_on(0, 0, 60, 100),
            Event::note_on(0, 0, 60, 100).on_port(1),
            Event::note_off(0, 0, 60, 0),
        ]);
        assert_eq!(ended(&mut notes), [Event::note_off(0, 0, 60, 0).on_port(1)]);
    }

    #[test]
    fn notes_beyond_the_table_or_the_events_capacity_are_counted() {
        let mut notes = Notes::new();
        let events: Vec<Event> = (0..CAPACITY + 3)
            .map(|index| note_on(0, 0, 0, Some(index as u32)))
            .collect();
        notes.record(&events);
        let mut produced = Vec::with_capacity(CAPACITY - 10);
        produced.push(Event::note_on(0, 0, 1, 1));
        let room = produced.capacity() - 1;
        assert_eq!(notes.end_all(&mut produced), (CAPACITY - room + 3) as u64);
        assert_eq!(produced.len(), room + 1);
        assert_eq!(notes.end_all(&mut Vec::new()), 0);
    }
}
