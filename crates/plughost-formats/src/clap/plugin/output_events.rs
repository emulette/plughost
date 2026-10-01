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

pub(super) struct ProcessOutput<'a> {
    pub parameters: events::ParameterOutput<'a>,
    pub events: &'a mut Vec<plughost_core::Event>,
    /// The block's length; events at or after its end land on its last frame.
    pub frames: usize,
    pub sysex: usize,
    /// The plugin pushed more events or system exclusive bytes than the block allows.
    pub overflow: bool,
    /// Events with no portable form (choke, MIDI 2.0, notes for any key); a note end only reports
    /// a voice that ended and is not counted.
    pub unconvertible: u64,
}

/// The port, channel, key and ID a note event addresses, when its port, channel and key are
/// specific.
fn address(pckn: Pckn) -> Option<(usize, u8, u8, Option<u32>)> {
    let port = *pckn.port_index.as_specific()?;
    let channel = u8::try_from(*pckn.channel.as_specific()?).ok()?;
    let key = u8::try_from(*pckn.key.as_specific()?).ok()?;
    let id = match pckn.note_id {
        Match::Specific(id) => Some(id).filter(|id| *id <= MAX_NOTE_ID),
        Match::All => None,
    };
    Some((usize::from(port), channel, key, id))
}

fn note(pckn: Pckn, velocity: f64) -> Option<(usize, Note)> {
    let (port, channel, key, id) = address(pckn)?;
    // Plugins compute velocities; one slightly out of range is not a different note.
    let mut note = Note::new(channel, key, velocity.clamp(0.0, 1.0));
    note.id = id;
    Some((port, note))
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

impl OutputEventBuffer for ProcessOutput<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let converted = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(event)) => note(event.pckn(), event.velocity())
                .map(|(port, note)| (port, EventData::NoteOn(note))),
            Some(CoreEventSpace::NoteOff(event)) => note(event.pckn(), event.velocity())
                .map(|(port, note)| (port, EventData::NoteOff(note))),
            Some(CoreEventSpace::NoteExpression(event)) => address(event.pckn())
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
        self.events.push(event);
        Ok(())
    }
}
