//! Output events of one block: parameter notifications go to their queue; note and MIDI events
//! become MIDI messages within the block's budgets, copied while the plugin's data is valid.
use super::*;
use clack_host::events::UnknownEvent;
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use plughost_core::{MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MidiData};

pub(super) struct ProcessOutput<'a> {
    pub parameters: events::ParameterOutput<'a>,
    pub events: &'a mut Vec<MidiEvent>,
    pub sysex: usize,
    /// The plugin pushed more events or system exclusive bytes than the block allows.
    pub overflow: bool,
    /// Events with no MIDI 1.0 form (note expression, choke, MIDI 2.0, wildcard notes).
    pub unconvertible: u64,
}

/// A note event as a channel message, when its port, channel and key are specific.
fn note(status: u8, pckn: Pckn, velocity: f64) -> Option<(usize, MidiData)> {
    let port = *pckn.port_index.as_specific()?;
    let channel = u8::try_from(*pckn.channel.as_specific()?)
        .ok()
        .filter(|c| *c < 16)?;
    let key = u8::try_from(*pckn.key.as_specific()?)
        .ok()
        .filter(|k| *k < 0x80)?;
    let mut velocity = (velocity * 127.0).round().clamp(0.0, 127.0) as u8;
    // A note on keeps a velocity of at least 1, since 0 would be a MIDI note off.
    if status == 0x90 {
        velocity = velocity.max(1);
    }
    Some((
        usize::from(port),
        MidiData::Channel([status | channel, key, velocity]),
    ))
}

impl OutputEventBuffer for ProcessOutput<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let converted = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(event)) => note(0x90, event.pckn(), event.velocity()),
            Some(CoreEventSpace::NoteOff(event)) => note(0x80, event.pckn(), event.velocity()),
            Some(CoreEventSpace::Midi(event)) => Some((
                usize::from(event.port_index()),
                MidiData::Channel(event.data()),
            )),
            // SAFETY: the plugin's buffer is valid during this call; it is copied now.
            Some(CoreEventSpace::MidiSysEx(event)) => Some((
                usize::from(event.port_index()),
                MidiData::SysEx(unsafe { event.data() }.to_vec()),
            )),
            Some(
                CoreEventSpace::ParamValue(_)
                | CoreEventSpace::ParamMod(_)
                | CoreEventSpace::ParamGestureBegin(_)
                | CoreEventSpace::ParamGestureEnd(_),
            ) => return self.parameters.try_push(event),
            _ => None,
        };
        let Some((port, data)) = converted else {
            self.unconvertible += 1;
            return Ok(());
        };
        let event = MidiEvent {
            offset: event.header().time() as usize,
            port,
            data,
        };
        if !event.is_valid() {
            self.unconvertible += 1;
            return Ok(());
        }
        let sysex = match &event.data {
            MidiData::SysEx(bytes) => bytes.len(),
            MidiData::Channel(_) => 0,
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
