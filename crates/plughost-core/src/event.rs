//! Note and MIDI events on event ports: input for instruments, MIDI effects, and MIDI-controlled
//! effects, and output of plugins that produce events.

use serde::{Deserialize, Serialize};

/// Maximum automation points and MIDI messages supplied together in one processing block.
/// For a chain this is the aggregate across all slots. Overflow rejects the block before DSP.
/// A plugin's event output in one block is bounded by the same count.
/// Render timelines may contain more events provided every submitted block fits this budget.
pub const MAX_BLOCK_EVENTS: usize = 4096;

/// Maximum system exclusive bytes, counting each message's F0 and F7, that one block's events may
/// carry: the chain's input, and each plugin's output.
pub const MAX_BLOCK_SYSEX_BYTES: usize = 64 << 10;

/// Checks the combined input count without overflowing while adding untrusted lengths.
pub fn validate_event_count(automation: usize, midi: usize) -> Result<(), crate::InputError> {
    if automation > MAX_BLOCK_EVENTS || midi > MAX_BLOCK_EVENTS - automation {
        return Err(crate::InputError::EventCapacity);
    }
    Ok(())
}

/// Checks the combined count of automation and events, and their system exclusive bytes.
pub fn validate_event_budget(
    automation: usize,
    events: &[MidiEvent],
) -> Result<(), crate::InputError> {
    validate_event_count(automation, events.len())?;
    if sysex_bytes(events) > MAX_BLOCK_SYSEX_BYTES {
        return Err(crate::InputError::EventCapacity);
    }
    Ok(())
}

/// A MIDI message on an event port at a sample offset: from the start of the block when
/// processing blocks, and from the start of the input when rendering. Each format receives it in
/// its own form (VST3 events and MIDI mapping, CLAP note or MIDI events, Audio Unit MIDI events).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiEvent {
    pub offset: usize,
    /// For a chain, the index of one of its external event inputs or outputs; for a plugin hosted
    /// directly, the index of its native event port in that direction.
    pub port: usize,
    pub data: MidiData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiData {
    /// A MIDI 1.0 channel message. Program change and channel pressure use two bytes; the third
    /// is ignored.
    Channel([u8; 3]),
    /// A complete system exclusive message, from its F0 to its F7.
    SysEx(Vec<u8>),
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

impl MidiEvent {
    /// A channel message on port 0.
    pub fn channel(offset: usize, data: [u8; 3]) -> MidiEvent {
        MidiEvent {
            offset,
            port: 0,
            data: MidiData::Channel(data),
        }
    }

    pub fn note_on(offset: usize, channel: u8, key: u8, velocity: u8) -> MidiEvent {
        Self::channel(
            offset,
            [0x90 | (channel & 0x0F), key & 0x7F, velocity & 0x7F],
        )
    }

    pub fn note_off(offset: usize, channel: u8, key: u8, velocity: u8) -> MidiEvent {
        Self::channel(
            offset,
            [0x80 | (channel & 0x0F), key & 0x7F, velocity & 0x7F],
        )
    }

    pub fn control_change(offset: usize, channel: u8, controller: u8, value: u8) -> MidiEvent {
        Self::channel(
            offset,
            [0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F],
        )
    }

    /// A system exclusive message on port 0; `bytes` include the F0 and the F7.
    pub fn sysex(offset: usize, bytes: Vec<u8>) -> MidiEvent {
        MidiEvent {
            offset,
            port: 0,
            data: MidiData::SysEx(bytes),
        }
    }

    /// The same event on `port`.
    pub fn on_port(self, port: usize) -> MidiEvent {
        MidiEvent { port, ..self }
    }

    /// The channel message, or `None` for system exclusive data or bytes that are not a MIDI 1.0
    /// channel message. A note on with velocity 0 is a note off, as in MIDI.
    pub fn message(&self) -> Option<Message> {
        let MidiData::Channel([status, first, second]) = self.data else {
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

    /// The bytes the message uses: 2 for program change and channel pressure, 3 for other
    /// channel messages, and the whole system exclusive message.
    pub fn byte_count(&self) -> usize {
        match &self.data {
            MidiData::Channel([status, ..]) if matches!(status & 0xF0, 0xC0 | 0xD0) => 2,
            MidiData::Channel(_) => 3,
            MidiData::SysEx(bytes) => bytes.len(),
        }
    }

    /// The message bytes, as they would be sent over MIDI.
    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            MidiData::Channel(bytes) => &bytes[..self.byte_count()],
            MidiData::SysEx(bytes) => bytes,
        }
    }

    /// Whether the data is a MIDI 1.0 channel message or a complete system exclusive message.
    pub fn is_valid(&self) -> bool {
        match &self.data {
            MidiData::Channel(_) => self.message().is_some(),
            MidiData::SysEx(bytes) => {
                bytes.len() >= 2
                    && bytes[0] == 0xF0
                    && bytes[bytes.len() - 1] == 0xF7
                    && bytes[1..bytes.len() - 1].iter().all(|&byte| byte < 0x80)
            }
        }
    }
}

/// The system exclusive bytes in `events`, saturating instead of overflowing.
pub fn sysex_bytes(events: &[MidiEvent]) -> usize {
    events
        .iter()
        .filter_map(|event| match &event.data {
            MidiData::SysEx(bytes) => Some(bytes.len()),
            MidiData::Channel(_) => None,
        })
        .fold(0, usize::saturating_add)
}

/// Whether `events` can go with a block of `frames` samples: valid messages in offset order,
/// every offset inside the block. Budgets and ports are checked separately.
pub fn events_fit(events: &[MidiEvent], frames: usize) -> bool {
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
        for (automation, midi) in [
            (MAX_BLOCK_EVENTS, 1),
            (0, MAX_BLOCK_EVENTS + 1),
            (usize::MAX, usize::MAX),
            (1, usize::MAX),
        ] {
            assert_eq!(
                validate_event_count(automation, midi),
                Err(crate::InputError::EventCapacity)
            );
        }
    }

    #[test]
    fn channel_messages_decode() {
        assert_eq!(
            MidiEvent::note_on(0, 1, 60, 100).message(),
            Some(Message::NoteOn {
                channel: 1,
                key: 60,
                velocity: 100
            })
        );
        assert!(matches!(
            MidiEvent::note_on(0, 0, 60, 0).message(),
            Some(Message::NoteOff { key: 60, .. })
        ));
        assert_eq!(
            MidiEvent::channel(0, [0xE3, 0x00, 0x40]).message(),
            Some(Message::PitchBend {
                channel: 3,
                value: 8192
            })
        );
        let clock = MidiEvent::channel(0, [0xF8, 0, 0]);
        assert_eq!(clock.message(), None);
        assert!(!clock.is_valid());
    }

    #[test]
    fn system_exclusive_messages_are_complete_and_budgeted() {
        let sysex = MidiEvent::sysex(0, vec![0xF0, 0x7D, 0x01, 0xF7]).on_port(1);
        assert!(sysex.is_valid());
        assert_eq!(sysex.message(), None);
        assert_eq!(sysex.bytes(), [0xF0, 0x7D, 0x01, 0xF7]);
        assert_eq!(sysex.port, 1);
        for bytes in [vec![0xF0], vec![0xF0, 0x80, 0xF7], vec![0x7D, 0xF7]] {
            assert!(!MidiEvent::sysex(0, bytes).is_valid());
        }
        let mut large = vec![0; MAX_BLOCK_SYSEX_BYTES + 1];
        large[0] = 0xF0;
        large[MAX_BLOCK_SYSEX_BYTES] = 0xF7;
        assert_eq!(
            validate_event_budget(0, &[MidiEvent::sysex(0, large)]),
            Err(crate::InputError::EventCapacity)
        );
    }

    #[test]
    fn events_must_be_ordered_and_inside_the_block() {
        let on = MidiEvent::note_on(10, 0, 60, 100);
        let off = MidiEvent::note_off(20, 0, 60, 0);
        assert!(events_fit(&[on.clone(), off.clone()], 64));
        assert!(!events_fit(&[off.clone(), on.clone()], 64));
        assert!(!events_fit(&[on, off], 20));
    }
}
