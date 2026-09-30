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
    AutomationEvent, MAX_AUDIO_CHANNELS, MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MidiData,
    MidiEvent, SampleFormat,
};
pub use activity::{Activity, Caller};
use errors::{BLOCK, CONFIGURATION, EVENTS, GENERATION, invalid};

/// Total mapping budget per prepared generation, including both PCM slots and events.
pub const MAX_SHARED_BYTES: usize = 256 << 20;
const AUTOMATION_BYTES: usize = 32;
/// Offset, port, kind with the channel bytes or the system exclusive length, and its start.
const MIDI_BYTES: usize = 32;
const CHANNEL_KIND: u64 = 0;
const SYSEX_KIND: u64 = 1;

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
    pub midi: usize,
}

/// Fixed event slots and the system exclusive bytes they refer to, for one direction.
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
                .checked_add(MAX_BLOCK_EVENTS * MIDI_BYTES)
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
            || crate::validate_event_count(submission.automation, submission.midi).is_err()
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
    fn write_midi(&self, region: Events, midi: &[MidiEvent]) {
        let mut sysex = 0;
        for (index, event) in midi.iter().enumerate() {
            let offset = region.slots + index * MIDI_BYTES;
            self.store(offset, event.offset as u64);
            self.store(offset + 8, event.port as u64);
            match &event.data {
                MidiData::Channel([status, first, second]) => {
                    let bytes = u64::from(u32::from_le_bytes([*status, *first, *second, 0]));
                    self.store(offset + 16, CHANNEL_KIND | bytes << 8);
                }
                MidiData::SysEx(bytes) => {
                    self.store(offset + 16, SYSEX_KIND | (bytes.len() as u64) << 8);
                    self.store(offset + 24, sysex as u64);
                    let destination = self.mapping.as_mut_ptr().wrapping_add(region.sysex + sysex);
                    // SAFETY: the budget check keeps the bytes inside this region's arena, which
                    // no atomic word overlaps; the slot belongs to the writer until handed over.
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len())
                    };
                    sysex += bytes.len();
                }
            }
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
    fn read_midi(&self, region: Events, count: usize, midi: &mut Vec<MidiEvent>) -> io::Result<()> {
        if count > MAX_BLOCK_EVENTS || midi.capacity() < count {
            return Err(invalid(BLOCK));
        }
        midi.clear();
        for index in 0..count {
            let offset = region.slots + index * MIDI_BYTES;
            let data = self.load(offset + 16);
            let data = match data & 0xFF {
                CHANNEL_KIND => {
                    let bytes = ((data >> 8) as u32).to_le_bytes();
                    MidiData::Channel([bytes[0], bytes[1], bytes[2]])
                }
                SYSEX_KIND => {
                    let length = usize::try_from(data >> 8).map_err(|_| invalid(EVENTS))?;
                    let start =
                        usize::try_from(self.load(offset + 24)).map_err(|_| invalid(EVENTS))?;
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
                    MidiData::SysEx(bytes)
                }
                _ => return Err(invalid(EVENTS)),
            };
            midi.push(MidiEvent {
                offset: usize::try_from(self.load(offset)).map_err(|_| invalid(EVENTS))?,
                port: usize::try_from(self.load(offset + 8)).map_err(|_| invalid(EVENTS))?,
                data,
            });
        }
        Ok(())
    }
}
