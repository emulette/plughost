//! MIDI the unit sends through its `MIDIOutputEventBlock` while it renders, split into messages
//! within the block's budgets, while the unit's bytes are still valid.
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2_audio_toolbox::{AUAudioUnit, AUEventSampleTime};
use objc2_foundation::NSInteger;
use plughost_core::{MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, MidiData, MidiEvent};

use super::super::errors::AuError;

type OutputEvent = dyn Fn(AUEventSampleTime, u8, NSInteger, NonNull<u8>) -> i32;

struct Collected {
    /// Render timestamp and length of the block being rendered.
    start: AUEventSampleTime,
    frames: usize,
    /// The cables prepared for delivery; events on other cables are dropped.
    cables: Vec<u64>,
    events: Vec<MidiEvent>,
    sysex: usize,
    /// The unit sent more events or system exclusive bytes than the block allows.
    overflow: bool,
}

impl Collected {
    fn push(&mut self, event: MidiEvent) {
        let sysex = match &event.data {
            MidiData::SysEx(bytes) => bytes.len(),
            MidiData::Channel(_) => 0,
        };
        if self.overflow
            || self.events.len() == MAX_BLOCK_EVENTS
            || self.sysex.saturating_add(sysex) > MAX_BLOCK_SYSEX_BYTES
        {
            self.overflow = true;
            return;
        }
        self.sysex += sysex;
        self.events.push(event);
    }
}

pub(super) struct Output {
    collected: Arc<Mutex<Collected>>,
    _block: RcBlock<OutputEvent>,
}

fn lock(collected: &Mutex<Collected>) -> std::sync::MutexGuard<'_, Collected> {
    collected
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Output {
    /// Installs the collector as the unit's MIDI output block, before render resources are
    /// allocated. Messages with no MIDI 1.0 event form are counted in `unconvertible`.
    pub fn install(
        unit: &AUAudioUnit,
        cables: Vec<u64>,
        unconvertible: Arc<AtomicU64>,
    ) -> Result<Output, AuError> {
        let mut events = Vec::new();
        events
            .try_reserve_exact(MAX_BLOCK_EVENTS)
            .map_err(|_| AuError::ProcessingStorage)?;
        let collected = Arc::new(Mutex::new(Collected {
            start: 0,
            frames: 0,
            cables,
            events,
            sysex: 0,
            overflow: false,
        }));
        let sink = Arc::clone(&collected);
        let block = RcBlock::new(
            move |time: AUEventSampleTime, cable: u8, length: NSInteger, bytes: NonNull<u8>| {
                let Ok(length) = usize::try_from(length) else {
                    return 0;
                };
                // SAFETY: the unit passes `length` readable bytes, valid during this call.
                let bytes = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), length) };
                let mut collected = lock(&sink);
                if !collected.cables.contains(&u64::from(cable)) {
                    return 0;
                }
                // Events outside the block, and immediate ones, land on its nearest frame.
                let last = collected.frames.saturating_sub(1);
                let offset = usize::try_from(time.saturating_sub(collected.start))
                    .unwrap_or(0)
                    .min(last);
                let skipped = split(bytes, |data| {
                    collected.push(MidiEvent {
                        offset,
                        port: usize::from(cable),
                        data,
                    })
                });
                if skipped > 0 {
                    unconvertible.fetch_add(skipped, Ordering::Relaxed);
                }
                0
            },
        );
        unsafe { unit.setMIDIOutputEventBlock(RcBlock::as_ptr(&block)) };
        Ok(Output {
            collected,
            _block: block,
        })
    }

    /// Starts collecting the block rendered at `start` for `frames` samples.
    pub fn begin(&self, start: f64, frames: usize) {
        let mut collected = lock(&self.collected);
        collected.start = start as AUEventSampleTime;
        collected.frames = frames;
        collected.events.clear();
        collected.sysex = 0;
        collected.overflow = false;
    }

    /// Moves the block's events into `into` in offset order, keeping the unit's order at equal
    /// offsets; fails when the unit exceeded the block's budgets, whose events are discarded.
    pub fn take(&self, into: &mut Vec<MidiEvent>) -> Result<(), AuError> {
        let mut collected = lock(&self.collected);
        if collected.overflow {
            collected.events.clear();
            return Err(AuError::OutputEventCapacity);
        }
        collected.events.sort_by_key(|event| event.offset);
        into.append(&mut collected.events);
        Ok(())
    }
}

/// Splits a MIDI byte stream into channel and complete system exclusive messages, following
/// running status. Returns how many messages had no such form (system common and real-time
/// messages, or malformed bytes, which end the stream).
fn split(mut bytes: &[u8], mut each: impl FnMut(MidiData)) -> u64 {
    let mut unconvertible = 0;
    let mut running = None;
    while let Some(&first) = bytes.first() {
        let (status, rest) = match (first, running) {
            (0x80.., _) => (first, &bytes[1..]),
            (_, Some(status)) => (status, bytes),
            _ => return unconvertible + 1,
        };
        match status {
            0xF0 => {
                let Some(end) = rest.iter().position(|&byte| byte >= 0x80) else {
                    return unconvertible + 1;
                };
                if rest[end] != 0xF7 {
                    return unconvertible + 1;
                }
                let mut message = Vec::with_capacity(end + 2);
                message.push(status);
                message.extend_from_slice(&rest[..=end]);
                each(MidiData::SysEx(message));
                running = None;
                bytes = &rest[end + 1..];
            }
            0x80..=0xEF => {
                let length = if matches!(status & 0xF0, 0xC0 | 0xD0) {
                    1
                } else {
                    2
                };
                let Some(data) = rest
                    .get(..length)
                    .filter(|data| data.iter().all(|&b| b < 0x80))
                else {
                    return unconvertible + 1;
                };
                let mut message = [status, 0, 0];
                message[1..=length].copy_from_slice(data);
                each(MidiData::Channel(message));
                running = Some(status);
                bytes = &rest[length..];
            }
            _ => {
                unconvertible += 1;
                let length = match status {
                    0xF1 | 0xF3 => 1,
                    0xF2 => 2,
                    _ => 0,
                };
                // System common messages cancel running status; real-time messages do not.
                if status < 0xF8 {
                    running = None;
                }
                bytes = rest.get(length..).unwrap_or_default();
            }
        }
    }
    unconvertible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(bytes: &[u8]) -> (Vec<MidiData>, u64) {
        let mut found = Vec::new();
        let skipped = split(bytes, |data| found.push(data));
        (found, skipped)
    }

    #[test]
    fn a_byte_stream_splits_into_channel_and_system_exclusive_messages() {
        let (found, skipped) = messages(&[
            0x90, 60, 100, 62, 90,   // note on, then another under running status
            0xF8, // clock, between messages
            0xC1, 5, // program change
            0xF0, 1, 2, 3, 0xF7, // system exclusive
            0xB0, 7, 99,
        ]);
        assert_eq!(
            found,
            [
                MidiData::Channel([0x90, 60, 100]),
                MidiData::Channel([0x90, 62, 90]),
                MidiData::Channel([0xC1, 5, 0]),
                MidiData::SysEx(vec![0xF0, 1, 2, 3, 0xF7]),
                MidiData::Channel([0xB0, 7, 99]),
            ]
        );
        assert_eq!(skipped, 1);
    }

    #[test]
    fn malformed_bytes_end_the_stream_as_unconvertible() {
        for bytes in [
            &[0x90, 60][..],           // truncated channel message
            &[60, 100][..],            // data without status
            &[0xF0, 1, 2][..],         // unterminated system exclusive
            &[0xF0, 1, 0x90, 60][..],  // system exclusive cut by a status byte
            &[0x90, 60, 0x80, 60][..], // status byte inside the data
        ] {
            let (found, skipped) = messages(bytes);
            assert!(found.is_empty(), "{bytes:?}");
            assert_eq!(skipped, 1, "{bytes:?}");
        }
        let (found, skipped) = messages(&[0xB0, 1, 2, 0xF2]);
        assert_eq!(found, [MidiData::Channel([0xB0, 1, 2])]);
        assert_eq!(skipped, 1);
    }
}
