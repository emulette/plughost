//! Realtime quanta between an application's audio callback and the helper's worker.
//!
//! A ring has `lanes` single-slot [`SharedAudio`] mappings and one small control mapping. Quantum
//! `n` travels in lane `n % lanes`: the callback writes its input there, and the worker its
//! output. The callback reuses a lane only once the worker has completed the quantum that used it
//! before, and reads an output `latency` quanta after submitting it, so with `lanes = latency + 2`
//! the two processes never access a lane at the same time. Each lane's header in the control
//! mapping says what the lane holds; its sequence word, written last with release ordering and
//! read with acquire ordering, hands the lane over. The callback validates everything the helper
//! writes, and checks after reading an output that the lane still holds it.
//!
//! The control mapping also holds the start generation, the last submitted and completed
//! sequences, the doorbell counter, and the worker's heartbeat and late-completion count. Each
//! side writes one of the sequences: the callback the submitted one, the worker the completed
//! one. A start resets both while neither runs; a resync under a running worker resets each on
//! its own side, the submitted one while the callback is held off and the completed one as the
//! worker adopts the new generation.

use std::io;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::doorbell::Doorbell;
use super::errors::{BLOCK, CONFIGURATION, EVENTS, GENERATION, invalid};
use super::memory::Memory;
use super::{Descriptor, SharedAudio, SlotConfig, Submission, clock};
use crate::render::Tail;
use crate::{AutomationEvent, BlockContext, Event, FailureKind, Transport};

/// The most lanes a ring has: a latency of three quanta plus two.
pub const MAX_LANES: usize = 5;
/// Changes whenever the control mapping's layout changes; the helper refuses another.
const FORMAT: u64 = 1;

const VERSION: usize = 0;
const GENERATION_WORD: usize = 64;
const SUBMITTED: usize = 128;
const COMPLETED: usize = 192;
const BELL: usize = 256;
const HEARTBEAT: usize = 320;
const LATE: usize = 384;
const DEADLINE: usize = 448;
const HEADERS: usize = 512;
/// Each lane has an input header and an output header of this size.
const HEADER_BYTES: usize = 256;
const CONTROL_BYTES: usize = HEADERS + MAX_LANES * 2 * HEADER_BYTES;

// Header words, by byte offset within a header.
const SEQUENCE: usize = 0;
const HEADER_GENERATION: usize = 8;
const FRAMES: usize = 16;
const AUTOMATION_COUNT: usize = 24;
const EVENT_COUNT: usize = 32;
const STAMP: usize = 40;
const STATUS: usize = 48;
const SLOT: usize = 56;
const LATENCY: usize = 64;
const PAYLOAD_LENGTH: usize = 72;
/// Postcard bytes: the input's transport, or the output's tail and failure kind.
const PAYLOAD: usize = 80;
const PAYLOAD_BYTES: usize = HEADER_BYTES - PAYLOAD;

const STATUS_DONE: u64 = 1;
const STATUS_FAILED: u64 = 2;

/// The ring's shape, fixed when it is created. Lane `n` is the [`SharedAudio`] generation
/// `first_lane + n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RingDescriptor {
    /// The control mapping's transfer tag.
    pub control: u64,
    pub first_lane: u64,
    pub lanes: usize,
    /// Each lane holds one quantum of at most `config.max_frames` frames.
    pub config: SlotConfig,
}

impl RingDescriptor {
    fn lane(&self, index: usize) -> Descriptor {
        Descriptor {
            generation: self.first_lane + index as u64,
            config: self.config,
        }
    }

    fn validate(&self) -> io::Result<()> {
        if !(3..=MAX_LANES).contains(&self.lanes) || self.control == 0 || self.first_lane == 0 {
            return Err(invalid(CONFIGURATION));
        }
        Ok(())
    }
}

/// How the worker finished a quantum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// The output holds the quantum; `latency` and `tail` are the chain's afterwards. The worker
    /// completed it `elapsed` [`clock`] ticks after its submission.
    Done {
        latency: u32,
        tail: Tail,
        elapsed: u64,
    },
    /// The quantum failed in `slot`, or in the chain, with `kind`.
    Failed {
        slot: Option<usize>,
        kind: FailureKind,
    },
}

/// What the worker reports with an output.
#[derive(Serialize, Deserialize)]
enum Report {
    Done(Tail),
    Failed(FailureKind),
}

/// A received quantum, ready for processing.
pub struct Quantum {
    pub generation: u64,
    pub sequence: u64,
    pub context: BlockContext,
    /// When the application submitted it, in [`clock`] ticks.
    pub submitted_at: u64,
}

/// The control mapping, shared by everything that rings the doorbell.
pub struct Control {
    memory: Memory,
    doorbell: Doorbell,
}

// SAFETY: the mapping is accessed only through atomics and through header ranges a sequence
// word hands over; the doorbell's handle may be used from any thread.
unsafe impl Send for Control {}
unsafe impl Sync for Control {}

impl Control {
    fn word(&self, offset: usize) -> &AtomicU64 {
        assert!(offset.is_multiple_of(8) && offset + 8 <= CONTROL_BYTES);
        // SAFETY: the page-aligned mapping has a fixed length and outlives `self`; both processes
        // access these words only atomically.
        unsafe { &*self.memory.as_ptr().add(offset).cast::<AtomicU64>() }
    }

    fn bell(&self) -> &AtomicU32 {
        // SAFETY: as in `word`; the doorbell counter is a 32-bit word of its own.
        unsafe { &*self.memory.as_ptr().add(BELL).cast::<AtomicU32>() }
    }

    fn header(&self, lane: usize, output: bool) -> usize {
        HEADERS + (2 * lane + usize::from(output)) * HEADER_BYTES
    }

    /// Starts generation `generation`, whose quanta are numbered from 1 and must each complete
    /// within `deadline` ticks of their submission. The application calls this while neither its
    /// callback nor the worker runs.
    pub fn start(&self, generation: u64, deadline: u64) {
        self.word(SUBMITTED).store(0, Ordering::Relaxed);
        self.word(COMPLETED).store(0, Ordering::Relaxed);
        self.word(DEADLINE).store(deadline, Ordering::Relaxed);
        self.word(GENERATION_WORD)
            .store(generation, Ordering::Release);
    }

    /// Starts generation `generation` under a running worker, once the callback no longer
    /// submits the current one: clears the submitted sequence, so the worker takes no further
    /// quantum of the current generation, and publishes the new one, which the worker adopts
    /// between quanta with [`Ring::adopt`] before the callback submits its first quantum.
    pub fn resync(&self, generation: u64) {
        self.word(SUBMITTED).store(0, Ordering::Release);
        self.word(GENERATION_WORD)
            .store(generation, Ordering::Release);
    }

    /// The last quantum the application submitted and the last the worker completed.
    pub fn progress(&self) -> (u64, u64) {
        (
            self.word(SUBMITTED).load(Ordering::Acquire),
            self.word(COMPLETED).load(Ordering::Acquire),
        )
    }

    /// How many quanta the worker completed later than their deadline, and its heartbeat: the
    /// time it last completed one, in [`clock`] ticks.
    pub fn worker(&self) -> (u64, u64) {
        (
            self.word(LATE).load(Ordering::Relaxed),
            self.word(HEARTBEAT).load(Ordering::Relaxed),
        )
    }

    /// Wakes the worker, as the helper's main thread does after queueing a command.
    pub fn ring(&self) {
        self.doorbell.ring(self.bell());
    }

    /// The doorbell counter, for [`Control::wait`].
    pub fn bell_count(&self) -> u32 {
        self.bell().load(Ordering::Acquire)
    }

    /// Returns once the doorbell rang after the worker read `last`, or spuriously.
    pub fn wait(&self, last: u32) {
        self.doorbell.wait(self.bell(), last);
    }

    fn write_payload(&self, header: usize, payload: &impl Serialize) -> io::Result<()> {
        let mut bytes = [0; PAYLOAD_BYTES];
        let used = postcard::to_slice(payload, &mut bytes)
            .map_err(|_| invalid(BLOCK))?
            .len();
        // SAFETY: the range lies inside this header, which the writer owns until it publishes the
        // header's sequence; no atomic word overlaps the payload.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.memory.as_ptr().add(header + PAYLOAD),
                used,
            )
        };
        self.word(header + PAYLOAD_LENGTH)
            .store(used as u64, Ordering::Relaxed);
        Ok(())
    }

    fn read_payload<T: for<'a> Deserialize<'a>>(&self, header: usize) -> io::Result<T> {
        let length = self.word(header + PAYLOAD_LENGTH).load(Ordering::Relaxed) as usize;
        if length > PAYLOAD_BYTES {
            return Err(invalid(BLOCK));
        }
        let mut bytes = [0; PAYLOAD_BYTES];
        // SAFETY: as in `write_payload`, for a range the length check keeps inside the header.
        unsafe {
            ptr::copy_nonoverlapping(
                self.memory.as_ptr().add(header + PAYLOAD),
                bytes.as_mut_ptr(),
                length,
            )
        };
        postcard::from_bytes(&bytes[..length]).map_err(|_| invalid(BLOCK))
    }
}

/// One side of a ring: the application's callback, or the helper's worker.
pub struct Ring {
    descriptor: RingDescriptor,
    control: Arc<Control>,
    lanes: Vec<SharedAudio>,
}

impl Ring {
    /// Creates the application's ring.
    pub fn new(descriptor: RingDescriptor) -> io::Result<Ring> {
        descriptor.validate()?;
        let control = Control {
            memory: Memory::new(CONTROL_BYTES)?,
            doorbell: Doorbell::new()?,
        };
        control.word(VERSION).store(FORMAT, Ordering::Release);
        let lanes = (0..descriptor.lanes)
            .map(|index| SharedAudio::new(descriptor.lane(index)))
            .collect::<io::Result<_>>()?;
        Ok(Ring {
            descriptor,
            control: Arc::new(control),
            lanes,
        })
    }

    /// # Safety
    ///
    /// The handles must be the control mapping and lanes of the peer's ring for `descriptor`, in
    /// order, and on Windows its doorbell event.
    pub(super) unsafe fn open(
        descriptor: RingDescriptor,
        control: super::memory::Handle,
        lanes: Vec<super::memory::Handle>,
        doorbell: Doorbell,
    ) -> io::Result<Ring> {
        descriptor.validate()?;
        if lanes.len() != descriptor.lanes {
            return Err(invalid(CONFIGURATION));
        }
        // SAFETY: forwarded from the caller.
        let memory = unsafe { Memory::open(control, CONTROL_BYTES) }?;
        let control = Control { memory, doorbell };
        if control.word(VERSION).load(Ordering::Acquire) != FORMAT {
            return Err(invalid(CONFIGURATION));
        }
        let lanes = lanes
            .into_iter()
            .enumerate()
            // SAFETY: forwarded from the caller.
            .map(|(index, handle)| unsafe { SharedAudio::open(handle, descriptor.lane(index)) })
            .collect::<io::Result<_>>()?;
        Ok(Ring {
            descriptor,
            control: Arc::new(control),
            lanes,
        })
    }

    pub fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    pub(super) fn control_memory(&self) -> &Memory {
        &self.control.memory
    }

    pub(super) fn lane_audio(&self) -> &[SharedAudio] {
        &self.lanes
    }

    #[cfg(windows)]
    pub(super) fn doorbell(&self) -> &Doorbell {
        &self.control.doorbell
    }

    /// The control mapping, which the helper's main thread rings after queueing a command.
    pub fn control(&self) -> Arc<Control> {
        Arc::clone(&self.control)
    }

    fn lane(&self, sequence: u64) -> usize {
        (sequence % self.descriptor.lanes as u64) as usize
    }

    /// See [`Control::start`].
    pub fn start(&self, generation: u64, deadline: u64) {
        self.control.start(generation, deadline);
    }

    /// The generation the application started, and the deadline of its quanta in ticks.
    pub fn started(&self) -> (u64, u64) {
        let generation = self.control.word(GENERATION_WORD).load(Ordering::Acquire);
        (
            generation,
            self.control.word(DEADLINE).load(Ordering::Relaxed),
        )
    }

    /// Worker, between quanta: adopts the generation [`Control::resync`] published, whose quanta
    /// are numbered from 1, and clears the completed sequence, so the callback reuses the lanes
    /// of the new generation only as the worker completes its quanta.
    pub fn adopt(&self) -> u64 {
        self.control.word(COMPLETED).store(0, Ordering::Release);
        self.control.word(GENERATION_WORD).load(Ordering::Acquire)
    }

    /// The last quantum the application submitted.
    pub fn submitted(&self) -> u64 {
        self.control.word(SUBMITTED).load(Ordering::Acquire)
    }

    /// The last quantum the worker completed, which also frees every earlier lane.
    pub fn completed(&self) -> u64 {
        self.control.word(COMPLETED).load(Ordering::Acquire)
    }

    /// See [`Control::worker`].
    pub fn worker(&self) -> (u64, u64) {
        self.control.worker()
    }

    /// Whether the callback may reuse the lane of quantum `sequence`.
    pub fn lane_free(&self, sequence: u64) -> bool {
        sequence <= self.descriptor.lanes as u64
            || self.completed() >= sequence - self.descriptor.lanes as u64
    }

    fn submit<T: super::samples::WireSample>(
        &mut self,
        generation: u64,
        sequence: u64,
        context: &BlockContext,
        input: &[&[T]],
        automation: &[AutomationEvent],
        events: &[Event],
    ) -> io::Result<()> {
        let lane = self.lane(sequence);
        let header = self.control.header(lane, false);
        self.lanes[lane].write_input(sequence, context.frames, input, automation, events)?;
        let control = &self.control;
        control.write_payload(header, &context.transport)?;
        control
            .word(header + HEADER_GENERATION)
            .store(generation, Ordering::Relaxed);
        control
            .word(header + FRAMES)
            .store(context.frames as u64, Ordering::Relaxed);
        control
            .word(header + AUTOMATION_COUNT)
            .store(automation.len() as u64, Ordering::Relaxed);
        control
            .word(header + EVENT_COUNT)
            .store(events.len() as u64, Ordering::Relaxed);
        control
            .word(header + STAMP)
            .store(clock::now(), Ordering::Relaxed);
        control
            .word(header + SEQUENCE)
            .store(sequence, Ordering::Release);
        control.word(SUBMITTED).store(sequence, Ordering::Release);
        control.ring();
        Ok(())
    }

    fn take<T: super::samples::WireSample>(
        &mut self,
        generation: u64,
        sequence: u64,
        scratch: &mut [Vec<T>],
        automation: &mut Vec<AutomationEvent>,
        events: &mut Vec<Event>,
    ) -> io::Result<Quantum> {
        let lane = self.lane(sequence);
        let header = self.control.header(lane, false);
        let control = &self.control;
        if control.word(header + SEQUENCE).load(Ordering::Acquire) != sequence
            || control
                .word(header + HEADER_GENERATION)
                .load(Ordering::Relaxed)
                != generation
        {
            return Err(invalid(GENERATION));
        }
        let count = |offset| {
            usize::try_from(control.word(header + offset).load(Ordering::Relaxed))
                .map_err(|_| invalid(BLOCK))
        };
        let submission = Submission {
            generation: self.descriptor.lane(lane).generation,
            sequence,
            frames: count(FRAMES)?,
            automation: count(AUTOMATION_COUNT)?,
            events: count(EVENT_COUNT)?,
        };
        let transport: Option<Transport> = control.read_payload(header)?;
        let submitted_at = control.word(header + STAMP).load(Ordering::Relaxed);
        self.lanes[lane].read_input(submission, scratch, automation, events)?;
        let mut context = BlockContext::new(submission.frames);
        context.transport = transport;
        Ok(Quantum {
            generation,
            sequence,
            context,
            submitted_at,
        })
    }

    fn finish(&self, header: usize, generation: u64, sequence: u64, submitted_at: u64) {
        let control = &self.control;
        control
            .word(header + HEADER_GENERATION)
            .store(generation, Ordering::Relaxed);
        let now = clock::now();
        control.word(header + STAMP).store(now, Ordering::Relaxed);
        control
            .word(header + SEQUENCE)
            .store(sequence, Ordering::Release);
        if now.saturating_sub(submitted_at) > control.word(DEADLINE).load(Ordering::Relaxed) {
            control.word(LATE).fetch_add(1, Ordering::Relaxed);
        }
        control.word(HEARTBEAT).store(now, Ordering::Relaxed);
        control.word(COMPLETED).store(sequence, Ordering::Release);
    }

    fn complete<T: super::samples::WireSample>(
        &mut self,
        quantum: &Quantum,
        output: &[&[T]],
        events: &[Event],
        latency: u32,
        tail: Tail,
    ) -> io::Result<()> {
        let sequence = quantum.sequence;
        let lane = self.lane(sequence);
        let header = self.control.header(lane, true);
        let submission = Submission {
            generation: self.descriptor.lane(lane).generation,
            sequence,
            frames: quantum.context.frames,
            automation: 0,
            events: events.len(),
        };
        self.lanes[lane].write_output(submission, output, events)?;
        let control = &self.control;
        control.write_payload(header, &Report::Done(tail))?;
        control
            .word(header + STATUS)
            .store(STATUS_DONE, Ordering::Relaxed);
        control
            .word(header + LATENCY)
            .store(u64::from(latency), Ordering::Relaxed);
        control
            .word(header + FRAMES)
            .store(quantum.context.frames as u64, Ordering::Relaxed);
        control
            .word(header + EVENT_COUNT)
            .store(events.len() as u64, Ordering::Relaxed);
        self.finish(header, quantum.generation, sequence, quantum.submitted_at);
        Ok(())
    }

    /// Reports that quantum `sequence` failed in `slot`, or in the chain, with `kind`.
    pub fn fail(
        &mut self,
        generation: u64,
        sequence: u64,
        submitted_at: u64,
        slot: Option<usize>,
        kind: FailureKind,
    ) -> io::Result<()> {
        let header = self.control.header(self.lane(sequence), true);
        let control = &self.control;
        control.write_payload(header, &Report::Failed(kind))?;
        control
            .word(header + STATUS)
            .store(STATUS_FAILED, Ordering::Relaxed);
        control
            .word(header + SLOT)
            .store(slot.map_or(0, |slot| slot as u64 + 1), Ordering::Relaxed);
        self.finish(header, generation, sequence, submitted_at);
        Ok(())
    }

    fn read<T: super::samples::WireSample>(
        &mut self,
        generation: u64,
        sequence: u64,
        output: &mut [&mut [T]],
        event_ports: usize,
        produced: &mut Vec<Event>,
    ) -> io::Result<Option<(Outcome, usize)>> {
        let lane = self.lane(sequence);
        let header = self.control.header(lane, true);
        let control = &self.control;
        let published = |control: &Control| {
            control.word(header + SEQUENCE).load(Ordering::Acquire) == sequence
                && control
                    .word(header + HEADER_GENERATION)
                    .load(Ordering::Relaxed)
                    == generation
        };
        if !published(control) {
            return Ok(None);
        }
        let report: Report = control.read_payload(header)?;
        let outcome = match (
            control.word(header + STATUS).load(Ordering::Relaxed),
            report,
        ) {
            (STATUS_DONE, Report::Done(tail)) => {
                let latency = u32::try_from(control.word(header + LATENCY).load(Ordering::Relaxed))
                    .map_err(|_| invalid(BLOCK))?;
                let submitted = control.header(lane, false) + STAMP;
                let elapsed = control
                    .word(header + STAMP)
                    .load(Ordering::Relaxed)
                    .saturating_sub(control.word(submitted).load(Ordering::Relaxed));
                Outcome::Done {
                    latency,
                    tail,
                    elapsed,
                }
            }
            (STATUS_FAILED, Report::Failed(kind)) => {
                let slot = control.word(header + SLOT).load(Ordering::Relaxed);
                let slot = slot
                    .checked_sub(1)
                    .map(|slot| usize::try_from(slot).map_err(|_| invalid(BLOCK)))
                    .transpose()?;
                return Ok(Some((Outcome::Failed { slot, kind }, 0)));
            }
            _ => return Err(invalid(BLOCK)),
        };
        let frames = control.word(header + FRAMES).load(Ordering::Relaxed);
        if output.iter().any(|channel| channel.len() as u64 != frames) {
            return Err(invalid(BLOCK));
        }
        let count = usize::try_from(control.word(header + EVENT_COUNT).load(Ordering::Relaxed))
            .map_err(|_| invalid(EVENTS))?;
        let submission = Submission {
            generation: self.descriptor.lane(lane).generation,
            sequence,
            frames: frames as usize,
            automation: 0,
            events: count,
        };
        let skipped = self.lanes[lane].read_output_skipping_sysex(
            submission,
            output,
            count,
            event_ports,
            produced,
        )?;
        // A worker that rewrote the lane while it was read wrote it out of turn.
        if !published(&self.control) {
            return Err(invalid(GENERATION));
        }
        Ok(Some((outcome, skipped)))
    }

    /// Waits for the doorbell after the worker read `last`.
    pub fn wait(&self, last: u32) {
        self.control.wait(last);
    }

    /// The doorbell counter.
    pub fn bell_count(&self) -> u32 {
        self.control.bell_count()
    }
}

macro_rules! ring_samples {
    ($sample:ty, $submit:ident, $take:ident, $complete:ident, $read:ident) => {
        impl Ring {
            /// Writes quantum `sequence` of `generation` into its lane, publishes it and rings
            /// the doorbell. The caller checks [`Ring::lane_free`] first and validated the input.
            pub fn $submit(
                &mut self,
                generation: u64,
                sequence: u64,
                context: &BlockContext,
                input: &[&[$sample]],
                automation: &[AutomationEvent],
                events: &[Event],
            ) -> io::Result<()> {
                self.submit(generation, sequence, context, input, automation, events)
            }
            /// Copies quantum `sequence` of `generation` into preallocated local storage.
            pub fn $take(
                &mut self,
                generation: u64,
                sequence: u64,
                scratch: &mut [Vec<$sample>],
                automation: &mut Vec<AutomationEvent>,
                events: &mut Vec<Event>,
            ) -> io::Result<Quantum> {
                self.take(generation, sequence, scratch, automation, events)
            }
            /// Publishes a quantum's output and events, and completes it.
            pub fn $complete(
                &mut self,
                quantum: &Quantum,
                output: &[&[$sample]],
                events: &[Event],
                latency: u32,
                tail: Tail,
            ) -> io::Result<()> {
                self.complete(quantum, output, events, latency, tail)
            }
            /// Reads quantum `sequence` of `generation` into `output`, and its events except
            /// system exclusive ones into `produced` while its capacity lasts. `None` while the
            /// worker has not completed it; otherwise the outcome and how many events were left
            /// out. On a failed outcome or an error, `output` is unspecified.
            pub fn $read(
                &mut self,
                generation: u64,
                sequence: u64,
                output: &mut [&mut [$sample]],
                event_ports: usize,
                produced: &mut Vec<Event>,
            ) -> io::Result<Option<(Outcome, usize)>> {
                self.read(generation, sequence, output, event_ports, produced)
            }
        }
    };
}
ring_samples!(f32, submit_f32, take_f32, complete_f32, read_f32);
ring_samples!(f64, submit_f64, take_f64, complete_f64, read_f64);

#[cfg(test)]
mod tests;
