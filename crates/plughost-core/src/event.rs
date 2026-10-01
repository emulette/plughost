//! Events on event ports: MIDI 1.0 messages, system exclusive data, notes told apart by their IDs,
//! and the expression of single notes. Input for instruments, MIDI effects, and MIDI-controlled
//! effects, and output of plugins that produce events.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// Maximum events one processing block takes, counting automation points too: each automation
/// point and each event (a MIDI message, a system exclusive message, a note on or off, or a note
/// expression) is one record. For a chain this is the aggregate across all slots, and overflow
/// rejects the block before DSP. A plugin's event output in one block is bounded by the same
/// count. Render timelines may contain more events provided every submitted block fits.
pub const MAX_BLOCK_EVENTS: usize = 4096;

/// Maximum system exclusive bytes, counting each message's F0 and F7, that one block's events may
/// carry: the chain's input, and each plugin's output. They are kept apart from the fixed-size
/// event records.
pub const MAX_BLOCK_SYSEX_BYTES: usize = 64 << 10;

/// The largest note ID. IDs are the non-negative 32-bit integers CLAP and VST3 use.
pub const MAX_NOTE_ID: u32 = i32::MAX as u32;

/// Checks the combined input count without overflowing while adding untrusted lengths.
pub fn validate_event_count(automation: usize, events: usize) -> Result<(), crate::InputError> {
    if automation > MAX_BLOCK_EVENTS || events > MAX_BLOCK_EVENTS - automation {
        return Err(crate::InputError::EventCapacity);
    }
    Ok(())
}

/// Checks the combined count of automation and events, and their system exclusive bytes.
pub fn validate_event_budget(automation: usize, events: &[Event]) -> Result<(), crate::InputError> {
    validate_event_count(automation, events.len())?;
    if sysex_bytes(events) > MAX_BLOCK_SYSEX_BYTES {
        return Err(crate::InputError::EventCapacity);
    }
    Ok(())
}

/// An event on an event port at a sample offset: from the start of the block when processing
/// blocks, and from the start of the input when rendering. Each format receives it in its own
/// form; see [`EventData`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Event {
    pub offset: usize,
    /// For a chain, the index of one of its external event inputs or outputs; for a plugin hosted
    /// directly, the index of its native event port in that direction.
    pub port: usize,
    pub data: EventData,
}

/// What an event carries.
///
/// Plugins receive notes and expressions in their format's note events where it has them: CLAP
/// note and note expression events on ports that take the CLAP dialect, and VST3 note events,
/// note expression values, and poly pressure. Ports that take MIDI only receive the MIDI 1.0 form
/// of [`EventData::to_midi`]; the note ID and the precision beyond 7 bits are lost there, and an
/// expression other than pressure has no such form and is not delivered. Output notes and
/// expressions in a format's note events become these variants; MIDI output stays MIDI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EventData {
    /// A MIDI 1.0 channel message. Program change and channel pressure use two bytes; the third
    /// is ignored. MPE is MIDI 1.0 on member channels and is delivered as MIDI.
    Midi([u8; 3]),
    /// A complete system exclusive message, from its F0 to its F7.
    SysEx(Vec<u8>),
    NoteOn(Note),
    /// Ends the notes it addresses; `velocity` is the release velocity.
    NoteOff(Note),
    Expression(NoteExpression),
}

/// A note, or the notes a note off addresses. A note with an ID is told apart from other notes on
/// the same key by it; without one, it addresses notes by port, channel and key, as MIDI does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Note {
    /// At most [`MAX_NOTE_ID`].
    pub id: Option<u32>,
    /// 0..=15.
    pub channel: u8,
    /// 0..=127, with 60 the middle C.
    pub key: u8,
    /// 0..=1.
    pub velocity: f64,
}

/// A change of one expression of the notes it addresses, which applies from its offset until the
/// next change of the same expression. With an ID it addresses that note; without one, every note
/// on its port, channel and key.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoteExpression {
    /// At most [`MAX_NOTE_ID`].
    pub id: Option<u32>,
    /// 0..=15.
    pub channel: u8,
    /// 0..=127.
    pub key: u8,
    pub kind: ExpressionKind,
    /// In the range of `kind`, see [`ExpressionKind::range`].
    pub value: f64,
}

/// The expressions a single note has, with the value ranges CLAP defines for them. VST3 receives
/// the first six as note expression values and pressure as poly pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExpressionKind {
    /// Linear gain, 0..=4; 1 leaves the note's level as it is.
    Volume,
    /// 0..=1: 0 is left, 0.5 center, and 1 right.
    Pan,
    /// Semitones in equal temperament, -120..=120, relative to the note's key.
    Tuning,
    /// 0..=1.
    Vibrato,
    /// 0..=1.
    Expression,
    /// 0..=1.
    Brightness,
    /// 0..=1. In MIDI 1.0, poly pressure.
    Pressure,
}

impl ExpressionKind {
    /// Every expression, in declaration order.
    pub const ALL: [ExpressionKind; 7] = [
        ExpressionKind::Volume,
        ExpressionKind::Pan,
        ExpressionKind::Tuning,
        ExpressionKind::Vibrato,
        ExpressionKind::Expression,
        ExpressionKind::Brightness,
        ExpressionKind::Pressure,
    ];

    /// The values the expression takes.
    pub fn range(self) -> RangeInclusive<f64> {
        match self {
            ExpressionKind::Volume => 0.0..=4.0,
            ExpressionKind::Tuning => -120.0..=120.0,
            ExpressionKind::Pan
            | ExpressionKind::Vibrato
            | ExpressionKind::Expression
            | ExpressionKind::Brightness
            | ExpressionKind::Pressure => 0.0..=1.0,
        }
    }
}

impl Note {
    pub fn new(channel: u8, key: u8, velocity: f64) -> Note {
        Note {
            id: None,
            channel,
            key,
            velocity,
        }
    }

    /// The same note with an ID.
    pub fn with_id(self, id: u32) -> Note {
        Note {
            id: Some(id),
            ..self
        }
    }

    fn is_valid(&self) -> bool {
        valid_address(self.id, self.channel, self.key)
            && self.velocity.is_finite()
            && (0.0..=1.0).contains(&self.velocity)
    }
}

impl NoteExpression {
    pub fn new(channel: u8, key: u8, kind: ExpressionKind, value: f64) -> NoteExpression {
        NoteExpression {
            id: None,
            channel,
            key,
            kind,
            value,
        }
    }

    /// The same expression for the note with this ID.
    pub fn with_id(self, id: u32) -> NoteExpression {
        NoteExpression {
            id: Some(id),
            ..self
        }
    }

    fn is_valid(&self) -> bool {
        valid_address(self.id, self.channel, self.key)
            && self.value.is_finite()
            && self.kind.range().contains(&self.value)
    }
}

fn valid_address(id: Option<u32>, channel: u8, key: u8) -> bool {
    id.is_none_or(|id| id <= MAX_NOTE_ID) && channel < 16 && key < 0x80
}

/// A 7-bit MIDI value from a value in 0..=1.
fn seven_bits(value: f64) -> u8 {
    (value * 127.0).round().clamp(0.0, 127.0) as u8
}

impl EventData {
    /// The MIDI 1.0 channel message form: the message itself, a note on (with a velocity of at
    /// least 1, since 0 would end the note) or note off, or poly pressure for a pressure
    /// expression. `None` for system exclusive data and the other expressions.
    pub fn to_midi(&self) -> Option<[u8; 3]> {
        Some(match self {
            EventData::Midi(bytes) => *bytes,
            EventData::NoteOn(note) => [
                0x90 | (note.channel & 0x0F),
                note.key & 0x7F,
                seven_bits(note.velocity).max(1),
            ],
            EventData::NoteOff(note) => [
                0x80 | (note.channel & 0x0F),
                note.key & 0x7F,
                seven_bits(note.velocity),
            ],
            EventData::Expression(expression) if expression.kind == ExpressionKind::Pressure => [
                0xA0 | (expression.channel & 0x0F),
                expression.key & 0x7F,
                seven_bits(expression.value),
            ],
            EventData::Expression(_) | EventData::SysEx(_) => return None,
        })
    }
}

/// A decoded channel message. Channels are 0..=15; data values are 7-bit except the 14-bit pitch
/// bend (8192 is the center).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    NoteOff {
        channel: u8,
        key: u8,
        velocity: u8,
    },
    NoteOn {
        channel: u8,
        key: u8,
        velocity: u8,
    },
    PolyPressure {
        channel: u8,
        key: u8,
        pressure: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ChannelPressure {
        channel: u8,
        pressure: u8,
    },
    PitchBend {
        channel: u8,
        value: u16,
    },
}

impl Event {
    /// An event on port 0.
    pub fn new(offset: usize, data: EventData) -> Event {
        Event {
            offset,
            port: 0,
            data,
        }
    }

    /// A MIDI 1.0 channel message on port 0.
    pub fn midi(offset: usize, data: [u8; 3]) -> Event {
        Self::new(offset, EventData::Midi(data))
    }

    /// A MIDI 1.0 note on.
    pub fn note_on(offset: usize, channel: u8, key: u8, velocity: u8) -> Event {
        Self::midi(
            offset,
            [0x90 | (channel & 0x0F), key & 0x7F, velocity & 0x7F],
        )
    }

    /// A MIDI 1.0 note off.
    pub fn note_off(offset: usize, channel: u8, key: u8, velocity: u8) -> Event {
        Self::midi(
            offset,
            [0x80 | (channel & 0x0F), key & 0x7F, velocity & 0x7F],
        )
    }

    pub fn control_change(offset: usize, channel: u8, controller: u8, value: u8) -> Event {
        Self::midi(
            offset,
            [0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F],
        )
    }

    /// A system exclusive message on port 0; `bytes` include the F0 and the F7.
    pub fn sysex(offset: usize, bytes: Vec<u8>) -> Event {
        Self::new(offset, EventData::SysEx(bytes))
    }

    /// The same event on `port`.
    pub fn on_port(self, port: usize) -> Event {
        Event { port, ..self }
    }

    /// The same event at `offset`.
    pub fn at(self, offset: usize) -> Event {
        Event { offset, ..self }
    }

    /// The MIDI 1.0 channel message, or `None` for other data or bytes that are not a channel
    /// message. A note on with velocity 0 is a note off, as in MIDI.
    pub fn message(&self) -> Option<Message> {
        let EventData::Midi([status, first, second]) = self.data else {
            return None;
        };
        if !(0x80..0xF0).contains(&status) || first > 0x7F || second > 0x7F {
            return None;
        }
        let channel = status & 0x0F;
        Some(match status & 0xF0 {
            0x80 => Message::NoteOff {
                channel,
                key: first,
                velocity: second,
            },
            0x90 if second == 0 => Message::NoteOff {
                channel,
                key: first,
                velocity: 64,
            },
            0x90 => Message::NoteOn {
                channel,
                key: first,
                velocity: second,
            },
            0xA0 => Message::PolyPressure {
                channel,
                key: first,
                pressure: second,
            },
            0xB0 => Message::ControlChange {
                channel,
                controller: first,
                value: second,
            },
            0xC0 => Message::ProgramChange {
                channel,
                program: first,
            },
            0xD0 => Message::ChannelPressure {
                channel,
                pressure: first,
            },
            _ => Message::PitchBend {
                channel,
                value: u16::from(first) | (u16::from(second) << 7),
            },
        })
    }

    /// Whether the data is a MIDI 1.0 channel message, a complete system exclusive message, or a
    /// note or expression with values in their ranges.
    pub fn is_valid(&self) -> bool {
        match &self.data {
            EventData::Midi(_) => self.message().is_some(),
            EventData::SysEx(bytes) => {
                bytes.len() >= 2
                    && bytes[0] == 0xF0
                    && bytes[bytes.len() - 1] == 0xF7
                    && bytes[1..bytes.len() - 1].iter().all(|&byte| byte < 0x80)
            }
            EventData::NoteOn(note) | EventData::NoteOff(note) => note.is_valid(),
            EventData::Expression(expression) => expression.is_valid(),
        }
    }
}

/// The system exclusive bytes in `events`, saturating instead of overflowing.
pub fn sysex_bytes(events: &[Event]) -> usize {
    events
        .iter()
        .filter_map(|event| match &event.data {
            EventData::SysEx(bytes) => Some(bytes.len()),
            _ => None,
        })
        .fold(0, usize::saturating_add)
}

/// Whether `events` can go with a block of `frames` samples: valid events in offset order, every
/// offset inside the block. Budgets and ports are checked separately.
pub fn events_fit(events: &[Event], frames: usize) -> bool {
    events.iter().all(|e| e.offset < frames && e.is_valid())
        && events
            .windows(2)
            .all(|pair| pair[0].offset <= pair[1].offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_event_budget_counts_both_inputs_without_integer_overflow() {
        assert_eq!(validate_event_count(0, 0), Ok(()));
        assert_eq!(validate_event_count(MAX_BLOCK_EVENTS, 0), Ok(()));
        assert_eq!(validate_event_count(1, MAX_BLOCK_EVENTS - 1), Ok(()));
        for (automation, events) in [
            (MAX_BLOCK_EVENTS, 1),
            (0, MAX_BLOCK_EVENTS + 1),
            (usize::MAX, usize::MAX),
            (1, usize::MAX),
        ] {
            assert_eq!(
                validate_event_count(automation, events),
                Err(crate::InputError::EventCapacity)
            );
        }
    }

    #[test]
    fn channel_messages_decode() {
        assert_eq!(
            Event::note_on(0, 1, 60, 100).message(),
            Some(Message::NoteOn {
                channel: 1,
                key: 60,
                velocity: 100
            })
        );
        assert!(matches!(
            Event::note_on(0, 0, 60, 0).message(),
            Some(Message::NoteOff { key: 60, .. })
        ));
        assert_eq!(
            Event::midi(0, [0xE3, 0x00, 0x40]).message(),
            Some(Message::PitchBend {
                channel: 3,
                value: 8192
            })
        );
        let clock = Event::midi(0, [0xF8, 0, 0]);
        assert_eq!(clock.message(), None);
        assert!(!clock.is_valid());
    }

    #[test]
    fn system_exclusive_messages_are_complete_and_budgeted() {
        let sysex = Event::sysex(0, vec![0xF0, 0x7D, 0x01, 0xF7]).on_port(1);
        assert!(sysex.is_valid());
        assert_eq!(sysex.message(), None);
        assert_eq!(sysex.data.to_midi(), None);
        assert_eq!(sysex.port, 1);
        for bytes in [vec![0xF0], vec![0xF0, 0x80, 0xF7], vec![0x7D, 0xF7]] {
            assert!(!Event::sysex(0, bytes).is_valid());
        }
        let mut large = vec![0; MAX_BLOCK_SYSEX_BYTES + 1];
        large[0] = 0xF0;
        large[MAX_BLOCK_SYSEX_BYTES] = 0xF7;
        assert_eq!(
            validate_event_budget(0, &[Event::sysex(0, large)]),
            Err(crate::InputError::EventCapacity)
        );
    }

    #[test]
    fn notes_and_expressions_are_checked_against_their_ranges() {
        let note = Note::new(15, 127, 1.0).with_id(MAX_NOTE_ID);
        assert!(Event::new(0, EventData::NoteOn(note)).is_valid());
        for invalid in [
            Note::new(16, 60, 0.5),
            Note::new(0, 128, 0.5),
            Note::new(0, 60, 1.5),
            Note::new(0, 60, f64::NAN),
            Note::new(0, 60, 0.5).with_id(MAX_NOTE_ID + 1),
        ] {
            assert!(!Event::new(0, EventData::NoteOff(invalid)).is_valid());
        }
        for kind in ExpressionKind::ALL {
            let range = kind.range();
            for (value, valid) in [
                (*range.start(), true),
                (*range.end(), true),
                (range.end() + 0.001, false),
                (f64::INFINITY, false),
            ] {
                let expression = NoteExpression::new(0, 60, kind, value).with_id(3);
                assert_eq!(
                    Event::new(0, EventData::Expression(expression)).is_valid(),
                    valid,
                    "{kind:?} {value}"
                );
            }
        }
    }

    #[test]
    fn notes_and_pressure_have_a_midi_form() {
        let note = Note::new(2, 64, 0.0).with_id(9);
        assert_eq!(EventData::NoteOn(note).to_midi(), Some([0x92, 64, 1]));
        let note = Note::new(2, 64, 0.5);
        assert_eq!(EventData::NoteOff(note).to_midi(), Some([0x82, 64, 64]));
        let pressure = NoteExpression::new(1, 60, ExpressionKind::Pressure, 1.0);
        assert_eq!(
            EventData::Expression(pressure).to_midi(),
            Some([0xA1, 60, 127])
        );
        let tuning = NoteExpression::new(1, 60, ExpressionKind::Tuning, 1.0);
        assert_eq!(EventData::Expression(tuning).to_midi(), None);
    }

    #[test]
    fn events_must_be_ordered_and_inside_the_block() {
        let on = Event::note_on(10, 0, 60, 100);
        let off = Event::note_off(20, 0, 60, 0);
        assert!(events_fit(&[on.clone(), off.clone()], 64));
        assert!(!events_fit(&[off.clone(), on.clone()], 64));
        assert!(!events_fit(&[on, off], 20));
    }
}
