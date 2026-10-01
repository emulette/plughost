//! Output events of one block: parameter notifications go to their queue; notes, note expressions
//! and MIDI become events within the block's budgets, copied while the plugin's data is valid.
use super::*;
use clack_host::events::UnknownEvent;
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clap_sys::events::{
    CLAP_NOTE_EXPRESSION_BRIGHTNESS, CLAP_NOTE_EXPRESSION_EXPRESSION, CLAP_NOTE_EXPRESSION_PAN,
    CLAP_NOTE_EXPRESSION_PRESSURE, CLAP_NOTE_EXPRESSION_TUNING, CLAP_NOTE_EXPRESSION_VIBRATO,
    CLAP_NOTE_EXPRESSION_VOLUME, clap_note_expression,
};
use plughost_core::{
    EventData, ExpressionKind, MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MAX_NOTE_ID, Note,
    NoteExpression,
};
use std::collections::HashMap;

/// Where the plugin's output notes with IDs play, by ID, from the note on it delivered to their
/// note off: their port, channel and key. A note off or expression may name its note by ID alone.
pub(super) type Sounding = HashMap<u32, (usize, u8, u8)>;

/// Room for twice the notes it follows, so that it does not grow on the processing thread after
/// notes come and go.
pub(super) fn sounding() -> Result<Sounding, ClapError> {
    let mut sounding = HashMap::new();
    sounding
        .try_reserve(2 * MAX_BLOCK_EVENTS)
        .map_err(|_| ClapError::EventStorage)?;
    Ok(sounding)
}

pub(super) struct ProcessOutput<'a> {
    pub parameters: events::ParameterOutput<'a>,
    pub events: &'a mut Vec<plughost_core::Event>,
    pub sounding: &'a mut Sounding,
    /// The block's length; events at or after its end land on its last frame.
    pub frames: usize,
    pub sysex: usize,
    /// The plugin pushed more events or system exclusive bytes than the block allows.
    pub overflow: bool,
    /// Events with no portable form (choke, MIDI 2.0, notes for any key that no ID names); a
    /// note end only reports a voice that ended and is not counted.
    pub unconvertible: u64,
}

/// The portable expression of a CLAP one.
fn expression_kind(id: clap_note_expression) -> Option<ExpressionKind> {
    Some(match id {
        CLAP_NOTE_EXPRESSION_VOLUME => ExpressionKind::Volume,
        CLAP_NOTE_EXPRESSION_PAN => ExpressionKind::Pan,
        CLAP_NOTE_EXPRESSION_TUNING => ExpressionKind::Tuning,
        CLAP_NOTE_EXPRESSION_VIBRATO => ExpressionKind::Vibrato,
        CLAP_NOTE_EXPRESSION_EXPRESSION => ExpressionKind::Expression,
        CLAP_NOTE_EXPRESSION_BRIGHTNESS => ExpressionKind::Brightness,
        CLAP_NOTE_EXPRESSION_PRESSURE => ExpressionKind::Pressure,
        _ => return None,
    })
}

impl ProcessOutput<'_> {
    /// The port, channel, key and ID a note event addresses: its own when they are specific, or
    /// those of the note its ID names.
    fn address(&self, pckn: Pckn) -> Option<(usize, u8, u8, Option<u32>)> {
        let id = pckn.note_id.into_specific().filter(|id| *id <= MAX_NOTE_ID);
        let (Match::Specific(port), Match::Specific(channel), Match::Specific(key)) =
            (pckn.port_index, pckn.channel, pckn.key)
        else {
            let &(port, channel, key) = self.sounding.get(&id?)?;
            return Some((port, channel, key, id));
        };
        let channel = u8::try_from(channel).ok()?;
        let key = u8::try_from(key).ok()?;
        Some((usize::from(port), channel, key, id))
    }

    fn note(&self, pckn: Pckn, velocity: f64) -> Option<(usize, Note)> {
        let (port, channel, key, id) = self.address(pckn)?;
        // Plugins compute velocities; one slightly out of range is not a different note.
        let mut note = Note::new(channel, key, velocity.clamp(0.0, 1.0));
        note.id = id;
        Some((port, note))
    }

    /// Forgets the notes a note off names.
    fn end(&mut self, pckn: Pckn) {
        self.sounding.retain(|&id, &mut (port, channel, key)| {
            !pckn.matches(&Pckn::new(
                port as u16,
                u16::from(channel),
                u16::from(key),
                id,
            ))
        });
    }

    fn push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let converted = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(event)) => self
                .note(event.pckn(), event.velocity())
                .map(|(port, note)| (port, EventData::NoteOn(note))),
            Some(CoreEventSpace::NoteOff(event)) => self
                .note(event.pckn(), event.velocity())
                .map(|(port, note)| (port, EventData::NoteOff(note))),
            Some(CoreEventSpace::NoteExpression(event)) => self
                .address(event.pckn())
                .zip(expression_kind(event.as_raw().expression_id))
                .map(|((port, channel, key, id), kind)| {
                    let value = event
                        .value()
                        .clamp(*kind.range().start(), *kind.range().end());
                    let mut expression = NoteExpression::new(channel, key, kind, value);
                    expression.id = id;
                    (port, EventData::Expression(expression))
                }),
            Some(CoreEventSpace::Midi(event)) => Some((
                usize::from(event.port_index()),
                EventData::Midi(event.data()),
            )),
            // SAFETY: the plugin's buffer is valid during this call; it is copied now.
            Some(CoreEventSpace::MidiSysEx(event)) => Some((
                usize::from(event.port_index()),
                EventData::SysEx(unsafe { event.data() }.to_vec()),
            )),
            Some(
                CoreEventSpace::ParamValue(_)
                | CoreEventSpace::ParamMod(_)
                | CoreEventSpace::ParamGestureBegin(_)
                | CoreEventSpace::ParamGestureEnd(_),
            ) => return self.parameters.try_push(event),
            // The plugin reports a voice it ended; there is nothing to deliver.
            Some(CoreEventSpace::NoteEnd(_)) => return Ok(()),
            _ => None,
        };
        let Some((port, data)) = converted else {
            self.unconvertible += 1;
            return Ok(());
        };
        let offset = (event.header().time() as usize).min(self.frames.saturating_sub(1));
        let event = plughost_core::Event::new(offset, data).on_port(port);
        if !event.is_valid() {
            self.unconvertible += 1;
            return Ok(());
        }
        let sysex = match &event.data {
            EventData::SysEx(bytes) => bytes.len(),
            _ => 0,
        };
        if self.events.len() == MAX_BLOCK_EVENTS
            || self.sysex.saturating_add(sysex) > MAX_BLOCK_SYSEX_BYTES
        {
            self.overflow = true;
            return Err(TryPushError::new());
        }
        self.sysex += sysex;
        if let EventData::NoteOn(note) = &event.data
            && let Some(id) = note.id
            && (self.sounding.len() < MAX_BLOCK_EVENTS || self.sounding.contains_key(&id))
        {
            self.sounding
                .insert(id, (event.port, note.channel, note.key));
        }
        self.events.push(event);
        Ok(())
    }
}

impl OutputEventBuffer for ProcessOutput<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let pushed = self.push(event);
        // A note off ends the notes it names whether or not it is delivered.
        if let Some(CoreEventSpace::NoteOff(off)) = event.as_core_event() {
            self.end(off.pckn());
        }
        pushed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clack_host::events::Event as _;
    use clack_host::events::event_types::{NoteExpressionEvent, NoteExpressionType};
    use plughost_core::ParameterEventBuffer;

    /// What the plugin's `events` become in one block, and how many had no portable form.
    fn block(sounding: &mut Sounding, events: &[&UnknownEvent]) -> (Vec<EventData>, u64) {
        let (parameters, queue) = (ParameterCache::default(), ParameterEventBuffer::default());
        let mut produced = Vec::new();
        let mut output = ProcessOutput {
            parameters: events::ParameterOutput {
                parameters: &parameters,
                queue: &queue,
            },
            events: &mut produced,
            sounding,
            frames: 8,
            sysex: 0,
            overflow: false,
            unconvertible: 0,
        };
        for event in events {
            assert!(output.try_push(event).is_ok());
        }
        let unconvertible = output.unconvertible;
        (
            produced.into_iter().map(|event| event.data).collect(),
            unconvertible,
        )
    }

    #[test]
    fn a_note_off_or_expression_may_name_its_note_by_id_alone() {
        let mut sounding = sounding().unwrap();
        let on = NoteOnEvent::new(0, Pckn::new(1u16, 2u16, 64u16, 5u32), 0.5);
        let (data, _) = block(&mut sounding, &[on.as_unknown()]);
        assert_eq!(data, [EventData::NoteOn(Note::new(2, 64, 0.5).with_id(5))]);

        let by_id = Pckn::new(Match::All, Match::All, Match::All, 5u32);
        let tuning = NoteExpressionEvent::new(1, by_id, NoteExpressionType::Tuning, 12.0);
        let off = NoteOffEvent::new(2, by_id, 0.0);
        let (data, unconvertible) = block(&mut sounding, &[tuning.as_unknown(), off.as_unknown()]);
        let expression = NoteExpression::new(2, 64, ExpressionKind::Tuning, 12.0).with_id(5);
        assert_eq!(
            (data, unconvertible),
            (
                vec![
                    EventData::Expression(expression),
                    EventData::NoteOff(Note::new(2, 64, 0.0).with_id(5)),
                ],
                0
            )
        );

        // The note ended; its ID names no note any more.
        let (data, unconvertible) = block(&mut sounding, &[tuning.as_unknown()]);
        assert_eq!((data, unconvertible), (vec![], 1));
    }
}
