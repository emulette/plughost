//! Fixed PCM/event slots for the synchronous helper protocol.
//!
//! The socket request/response pair hands each slot to one side at a time, so the mapping holds
//! only payloads; the [`Submission`] message carries their shape. Native processors receive local
//! copies, never mapped sample references. Notifications, scheduling and failure recovery belong
//! to the connection. This storage alone provides neither a real-time endpoint nor a sandbox for
//! hostile plugins.

mod activity;
mod errors;
mod samples;
#[cfg(test)]
mod tests;
pub mod transfer;

use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::{MmapOptions, MmapRaw};
use serde::{Deserialize, Serialize};

use crate::{
    AutomationEvent, Event, EventData, ExpressionKind, MAX_AUDIO_CHANNELS, MAX_BLOCK_EVENTS,
    MAX_BLOCK_SYSEX_BYTES, Note, NoteExpression, SampleFormat,
};
pub use activity::{Activity, Caller};
use errors::{BLOCK, CONFIGURATION, EVENTS, GENERATION, invalid};

/// Total mapping budget per prepared generation, including both PCM slots and events.
pub const MAX_SHARED_BYTES: usize = 256 << 20;
const AUTOMATION_BYTES: usize = 32;
/// One event record, four words: the offset; the port, with the kind, channel, key and
/// expression above it; the note ID plus one, or 0 without one; and the payload: the MIDI bytes,
/// the system exclusive start and length in the region's arena, the velocity, or the value.
const EVENT_BYTES: usize = 32;
const MIDI_KIND: u64 = 0;
const SYSEX_KIND: u64 = 1;
const NOTE_ON_KIND: u64 = 2;
const NOTE_OFF_KIND: u64 = 3;
const EXPRESSION_KIND: u64 = 4;

/// The shape is fixed at prepare time and never inferred from the shared mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotConfig {
    pub sample_format: SampleFormat,
    pub max_frames: usize,
    pub input_channels: usize,
    pub output_channels: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub generation: u64,
    pub config: SlotConfig,
}

/// A bounded input submission. Transport position remains in the small control notification.
/// `sequence` pairs a response with its request; the slots themselves are not versioned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub generation: u64,
    pub sequence: u64,
    pub frames: usize,
    pub automation: usize,
    pub events: usize,
}

/// Fixed event records and the system exclusive bytes they refer to, for one direction.
#[derive(Clone, Copy)]
struct Events {
    slots: usize,
    sysex: usize,
}

struct Layout {
    input: usize,
    output: usize,
    automation: usize,
    input_events: Events,
    output_events: Events,
    bytes: usize,
}

impl SlotConfig {
    /// Reserves local, precision-matched storage outside the processing path.
    pub fn input_storage(self) -> io::Result<super::AudioSamples> {
        self.storage(self.input_channels)
    }

    pub fn output_storage(self) -> io::Result<super::AudioSamples> {
        self.storage(self.output_channels)
    }

    fn storage(self, channels: usize) -> io::Result<super::AudioSamples> {
        self.layout()?;
        fn reserve<S>(channels: usize, frames: usize) -> io::Result<Vec<Vec<S>>> {
            (0..channels)
                .map(|_| {
                    let mut samples = Vec::new();
                    samples
                        .try_reserve_exact(frames)
                        .map_err(|_| invalid(errors::STORAGE))?;
                    Ok(samples)
                })
                .collect()
        }
        Ok(match self.sample_format {
            SampleFormat::F32 => super::AudioSamples::F32(reserve(channels, self.max_frames)?),
            SampleFormat::F64 => super::AudioSamples::F64(reserve(channels, self.max_frames)?),
        })
    }

    fn layout(self) -> io::Result<Layout> {
        if self.max_frames == 0
            || self.max_frames > i32::MAX as usize
            || self.input_channels > MAX_AUDIO_CHANNELS
            || self.output_channels > MAX_AUDIO_CHANNELS
        {
            return Err(invalid(CONFIGURATION));
        }
        let width = match self.sample_format {
            SampleFormat::F32 => 4,
            SampleFormat::F64 => 8,
        };
        let pcm = |channels: usize| -> io::Result<usize> {
            self.max_frames
                .checked_mul(channels)
                .and_then(|n| n.checked_mul(width))
                .and_then(|n| n.checked_add(7))
                .map(|n| n & !7)
                .ok_or_else(|| invalid(CONFIGURATION))
        };
        let input = 0;
        let output = pcm(self.input_channels)?;
        let automation = output
            .checked_add(pcm(self.output_channels)?)
            .ok_or_else(|| invalid(CONFIGURATION))?;
        // Event regions have fixed sizes; the system exclusive bytes stay 8-byte aligned.
        let events = |start: usize| -> io::Result<(Events, usize)> {
            let sysex = start
                .checked_add(MAX_BLOCK_EVENTS * EVENT_BYTES)
                .ok_or_else(|| invalid(CONFIGURATION))?;
            let end = sysex
                .checked_add(MAX_BLOCK_SYSEX_BYTES)
                .ok_or_else(|| invalid(CONFIGURATION))?;
            Ok((
                Events {
                    slots: start,
                    sysex,
                },
                end,
            ))
        };
        let (input_events, end) = events(
            automation
                .checked_add(MAX_BLOCK_EVENTS * AUTOMATION_BYTES)
                .ok_or_else(|| invalid(CONFIGURATION))?,
        )?;
        let (output_events, bytes) = events(end)?;
        if bytes > MAX_SHARED_BYTES {
            return Err(invalid(CONFIGURATION));
        }
        Ok(Layout {
            input,
            output,
            automation,
            input_events,
            output_events,
            bytes,
        })
    }
}

/// A single generation, backed by an anonymous temporary file owned by the participating processes.
/// Dropping an endpoint closes its file and mapping; no named-file cleanup protocol is required.
/// Storage is allocated outside processing. The file is never resized after it is mapped.
pub struct SharedAudio {
    mapping: MmapRaw,
    file: File,
    descriptor: Descriptor,
    layout: Layout,
}

impl SharedAudio {
    pub fn new(descriptor: Descriptor) -> io::Result<Self> {
        let layout = descriptor.config.layout()?;
        let file = tempfile::tempfile()?;
        file.set_len(layout.bytes as u64)?;
        Self::from_file(file, descriptor)
    }

    fn from_file(file: File, descriptor: Descriptor) -> io::Result<Self> {
        let layout = descriptor.config.layout()?;
        if descriptor.generation == 0 || file.metadata()?.len() != layout.bytes as u64 {
            return Err(invalid(CONFIGURATION));
        }
        let mapping = MmapOptions::new().len(layout.bytes).map_raw(&file)?;
        Ok(Self {
            mapping,
            file,
            descriptor,
            layout,
        })
    }

    pub fn descriptor(&self) -> Descriptor {
        self.descriptor
    }

    fn word(&self, offset: usize) -> &AtomicU64 {
        assert!(offset.is_multiple_of(8) && offset <= self.layout.bytes - 8);
        // SAFETY: the page-aligned mapping remains live and has immutable length. The event ranges
        // are accessed only through aligned 64-bit atomics and never overlap the PCM ranges.
        unsafe { &*self.mapping.as_mut_ptr().add(offset).cast::<AtomicU64>() }
    }

    fn load(&self, offset: usize) -> u64 {
        self.word(offset).load(Ordering::Relaxed)
    }
    fn store(&self, offset: usize, value: u64) {
        self.word(offset).store(value, Ordering::Relaxed);
    }

    /// Keeps every access inside this generation's slots.
    fn check_submission(&self, submission: Submission) -> io::Result<()> {
        if submission.generation != self.descriptor.generation {
            return Err(invalid(GENERATION));
        }
        if submission.frames > self.descriptor.config.max_frames
            || crate::validate_event_count(submission.automation, submission.events).is_err()
        {
            return Err(invalid(BLOCK));
        }
        Ok(())
    }

    fn write_automation(&self, automation: &[AutomationEvent]) {
        for (index, event) in automation.iter().enumerate() {
            let offset = self.layout.automation + index * AUTOMATION_BYTES;
            self.store(offset, event.slot as u64);
            self.store(offset + 8, event.change.id);
            self.store(offset + 16, event.change.offset as u64);
            self.store(offset + 24, event.change.value.to_bits());
        }
    }

    /// Writes events within the count and system exclusive budgets, which the caller checked.
    fn write_events(&self, region: Events, events: &[Event]) {
        let mut sysex = 0;
        for (index, event) in events.iter().enumerate() {
            let offset = region.slots + index * EVENT_BYTES;
            let note = |kind: u64, channel: u8, key: u8, expression: u64| {
                kind | u64::from(channel) << 8 | u64::from(key) << 16 | expression << 24
            };
            let id = |id: Option<u32>| id.map_or(0, |id| u64::from(id) + 1);
            let (header, id, payload) = match &event.data {
                EventData::Midi([status, first, second]) => (
                    MIDI_KIND,
                    0,
                    u64::from(u32::from_le_bytes([*status, *first, *second, 0])),
                ),
                EventData::SysEx(bytes) => {
                    let destination = self.mapping.as_mut_ptr().wrapping_add(region.sysex + sysex);
                    // SAFETY: the budget check keeps the bytes inside this region's arena, which
                    // no atomic word overlaps; the slot belongs to the writer until handed over.
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len())
                    };
                    let range = sysex as u64 | (bytes.len() as u64) << 32;
                    sysex += bytes.len();
                    (SYSEX_KIND, 0, range)
                }
                EventData::NoteOn(on) => (
                    note(NOTE_ON_KIND, on.channel, on.key, 0),
                    id(on.id),
                    on.velocity.to_bits(),
                ),
                EventData::NoteOff(off) => (
                    note(NOTE_OFF_KIND, off.channel, off.key, 0),
                    id(off.id),
                    off.velocity.to_bits(),
                ),
                EventData::Expression(expression) => (
                    note(
                        EXPRESSION_KIND,
                        expression.channel,
                        expression.key,
                        expression_code(expression.kind),
                    ),
                    id(expression.id),
                    expression.value.to_bits(),
                ),
            };
            self.store(offset, event.offset as u64);
            // Ports are event port indices, which are 32-bit.
            self.store(offset + 8, u64::from(event.port as u32) | header << 32);
            self.store(offset + 16, id);
            self.store(offset + 24, payload);
        }
    }

    fn read_automation(
        &self,
        count: usize,
        automation: &mut Vec<AutomationEvent>,
    ) -> io::Result<()> {
        if automation.capacity() < count {
            return Err(invalid(BLOCK));
        }
        automation.clear();
        for index in 0..count {
            let offset = self.layout.automation + index * AUTOMATION_BYTES;
            automation.push(AutomationEvent {
                slot: usize::try_from(self.load(offset)).map_err(|_| invalid(EVENTS))?,
                change: crate::ParameterChange {
                    id: self.load(offset + 8),
                    offset: usize::try_from(self.load(offset + 16)).map_err(|_| invalid(EVENTS))?,
                    value: f64::from_bits(self.load(offset + 24)),
                },
            });
        }
        Ok(())
    }

    /// Decodes events, checking every system exclusive range against the region's arena.
    fn read_events(&self, region: Events, count: usize, events: &mut Vec<Event>) -> io::Result<()> {
        if count > MAX_BLOCK_EVENTS || events.capacity() < count {
            return Err(invalid(BLOCK));
        }
        events.clear();
        for index in 0..count {
            let offset = region.slots + index * EVENT_BYTES;
            let port = self.load(offset + 8);
            let header = port >> 32;
            let (channel, key) = ((header >> 8) as u8, (header >> 16) as u8);
            let id = match self.load(offset + 16) {
                0 => None,
                id => Some(u32::try_from(id - 1).map_err(|_| invalid(EVENTS))?),
            };
            let payload = self.load(offset + 24);
            let note = || Note {
                id,
                channel,
                key,
                velocity: f64::from_bits(payload),
            };
            let data = match header & 0xFF {
                MIDI_KIND => {
                    let bytes = (payload as u32).to_le_bytes();
                    EventData::Midi([bytes[0], bytes[1], bytes[2]])
                }
                SYSEX_KIND => {
                    let start = (payload & u64::from(u32::MAX)) as usize;
                    let length = (payload >> 32) as usize;
                    if start
                        .checked_add(length)
                        .is_none_or(|end| end > MAX_BLOCK_SYSEX_BYTES)
                    {
                        return Err(invalid(EVENTS));
                    }
                    let mut bytes = vec![0; length];
                    let source = self.mapping.as_mut_ptr().wrapping_add(region.sysex + start);
                    // SAFETY: the range was checked against this region's arena above.
                    unsafe { std::ptr::copy_nonoverlapping(source, bytes.as_mut_ptr(), length) };
                    EventData::SysEx(bytes)
                }
                NOTE_ON_KIND => EventData::NoteOn(note()),
                NOTE_OFF_KIND => EventData::NoteOff(note()),
                EXPRESSION_KIND => EventData::Expression(NoteExpression {
                    id,
                    channel,
                    key,
                    kind: expression_kind(header >> 24 & 0xFF).ok_or_else(|| invalid(EVENTS))?,
                    value: f64::from_bits(payload),
                }),
                _ => return Err(invalid(EVENTS)),
            };
            events.push(Event {
                offset: usize::try_from(self.load(offset)).map_err(|_| invalid(EVENTS))?,
                port: (port & u64::from(u32::MAX)) as usize,
                data,
            });
        }
        Ok(())
    }
}

fn expression_code(kind: ExpressionKind) -> u64 {
    match kind {
        ExpressionKind::Volume => 0,
        ExpressionKind::Pan => 1,
        ExpressionKind::Tuning => 2,
        ExpressionKind::Vibrato => 3,
        ExpressionKind::Expression => 4,
        ExpressionKind::Brightness => 5,
        ExpressionKind::Pressure => 6,
    }
}

fn expression_kind(code: u64) -> Option<ExpressionKind> {
    ExpressionKind::ALL
        .into_iter()
        .find(|&kind| expression_code(kind) == code)
}
