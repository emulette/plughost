//! The realtime worker. While the application's endpoint runs, the processing thread waits on the
//! ring's doorbell instead of the queue. Each wake it first runs the commands the main thread
//! queued, which ring the doorbell too, and then processes every submitted quantum in order into
//! its lane. A quantum's failure is reported in its lane; the worker goes on with the next. A
//! resync command switches it to a new generation between quanta, without leaving its loop.

mod schedule;

use std::io;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::Duration;

use plughost_core::ipc::AudioSamples;
use plughost_core::ipc::shared::realtime::{Control, Quantum, Ring};
use plughost_core::render::Tail;
use plughost_core::{AutomationEvent, Event, Failure, FailureKind, InputError, MAX_BLOCK_EVENTS};

use super::{Command, Pipeline, Work, audio};
use crate::Responder;
use schedule::Schedule;

/// The installed ring and the worker's local copy of the quantum it processes.
pub(crate) struct Transport {
    ring: Ring,
    input: AudioSamples,
    automation: Vec<AutomationEvent>,
    events: Vec<Event>,
}

impl Transport {
    pub fn new(ring: Ring) -> io::Result<Transport> {
        let input = ring.descriptor().config.input_storage()?;
        Ok(Transport {
            ring,
            input,
            automation: Vec::with_capacity(MAX_BLOCK_EVENTS),
            events: Vec::with_capacity(MAX_BLOCK_EVENTS),
        })
    }

    pub fn ring(&self) -> &Ring {
        &self.ring
    }
}

/// What a realtime command does to the worker.
pub(crate) enum Realtime {
    /// Replaces the installed ring, or removes it.
    Install(Option<Box<Transport>>),
    /// Runs the worker for the generation the application started.
    Start,
    /// Returns the processing thread to its queue.
    Stop,
    /// Adopts the generation the application published in the ring, numbered from 1, without
    /// leaving the worker.
    Resync,
}

/// Hands `start` back once the worker is scheduled, so that the application's first quanta find
/// it ready, and runs it until a stop command. Returns false once the connection closed.
pub(super) fn run(
    pipeline: &mut Pipeline,
    start: Command,
    queue: &Receiver<Work>,
    handed_back: &SyncSender<Command>,
    responder: &Responder,
) -> bool {
    let (Some(transport), Some(config)) = (&pipeline.realtime, &pipeline.audio_config) else {
        return handed_back.send(start).is_ok();
    };
    let control = transport.ring.control();
    let (generation, deadline) = transport.ring.started();
    let quantum = transport.ring.descriptor().config.max_frames as f64 / config.sample_rate;
    let schedule = Schedule::enter(Duration::from_secs_f64(quantum), deadline, responder);
    defer_restarts(pipeline, true);
    if handed_back.send(start).is_err() {
        return false;
    }
    let connected = work(
        pipeline,
        &schedule,
        &control,
        generation,
        queue,
        handed_back,
        responder,
    );
    defer_restarts(pipeline, false);
    connected
}

/// The stream goes on with the plugins' current activations while a restart request waits; the
/// application prepares the chain again after it stops the endpoint.
fn defer_restarts(pipeline: &mut Pipeline, deferred: bool) {
    pipeline.restart_deferred = deferred;
    for processor in &pipeline.processors {
        processor.defer_restart(deferred);
    }
}

fn work(
    pipeline: &mut Pipeline,
    schedule: &Schedule,
    control: &Control,
    mut generation: u64,
    queue: &Receiver<Work>,
    handed_back: &SyncSender<Command>,
    responder: &Responder,
) -> bool {
    let mut processed = 0;
    loop {
        let bell = control.bell_count();
        loop {
            match queue.try_recv() {
                Ok(Work::Command(mut command)) => {
                    let stop = matches!(command, Command::Realtime(Realtime::Stop));
                    // The application cleared the submitted sequence before it asked, and its
                    // callback submits nothing until the answer: what is submitted from now on
                    // belongs to the adopted generation.
                    if let (Command::Realtime(Realtime::Resync), Some(transport)) =
                        (&command, &pipeline.realtime)
                    {
                        generation = transport.ring.adopt();
                        processed = 0;
                    }
                    pipeline.apply(&mut command);
                    if handed_back.send(command).is_err() {
                        return false;
                    }
                    if stop || pipeline.realtime.is_none() {
                        return true;
                    }
                }
                Ok(Work::Observe(slot, timing)) => pipeline.changes.observe(slot, timing),
                // The application does not send blocks while its endpoint runs.
                Ok(Work::Block { .. }) => {
                    let failure =
                        Failure::new(FailureKind::Processing, crate::errors::REALTIME_RUNNING);
                    if responder.fail(None, failure).is_err() {
                        return false;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        let submitted = pipeline
            .realtime
            .as_ref()
            .map_or(processed, |transport| transport.ring.submitted());
        while processed < submitted {
            processed += 1;
            let _audit = plughost_core::audit::processing();
            crate::events::with_pool(|| quantum(pipeline, schedule, generation, processed));
        }
        control.wait(bell);
    }
}

/// Processes quantum `sequence` into its lane.
fn quantum(pipeline: &mut Pipeline, schedule: &Schedule, generation: u64, sequence: u64) {
    let Some(mut transport) = pipeline.realtime.take() else {
        return;
    };
    let Transport {
        ring,
        input,
        automation,
        events,
    } = &mut *transport;
    let taken = match input {
        AudioSamples::F32(input) => ring.take_f32(generation, sequence, input, automation, events),
        AudioSamples::F64(input) => ring.take_f64(generation, sequence, input, automation, events),
    };
    // A lane the application could not have written, or that failed to publish, fails alone.
    let _ = match taken {
        Ok(quantum) => {
            let _interval = schedule.interval(quantum.submitted_at);
            let mut sink = Lane {
                ring,
                quantum: &quantum,
            };
            let processed = audio::process(
                pipeline,
                input,
                &quantum.context,
                automation,
                events,
                &mut sink,
            );
            processed.or_else(|_| {
                ring.fail(
                    generation,
                    sequence,
                    quantum.submitted_at,
                    None,
                    FailureKind::Protocol,
                )
            })
        }
        Err(_) => ring.fail(generation, sequence, 0, None, FailureKind::Protocol),
    };
    pipeline.realtime = Some(transport);
}

/// A quantum's result, published in its lane.
struct Lane<'a> {
    ring: &'a mut Ring,
    quantum: &'a Quantum,
}

impl Lane<'_> {
    fn failed(&mut self, slot: Option<usize>, kind: FailureKind) -> io::Result<()> {
        let quantum = self.quantum;
        self.ring.fail(
            quantum.generation,
            quantum.sequence,
            quantum.submitted_at,
            slot,
            kind,
        )
    }
}

impl audio::Sink for Lane<'_> {
    fn fail(&mut self, slot: Option<usize>, failure: Failure) -> io::Result<()> {
        self.failed(slot, failure.kind)
    }

    fn reject(&mut self, slot: Option<usize>, _error: InputError) -> io::Result<()> {
        self.failed(slot, FailureKind::InvalidInput)
    }

    fn complete_f32(
        &mut self,
        output: &[&[f32]],
        events: &[Event],
        latency: u32,
        tail: Tail,
    ) -> io::Result<()> {
        self.ring
            .complete_f32(self.quantum, output, events, latency, tail)
    }

    fn complete_f64(
        &mut self,
        output: &[&[f64]],
        events: &[Event],
        latency: u32,
        tail: Tail,
    ) -> io::Result<()> {
        self.ring
            .complete_f64(self.quantum, output, events, latency, tail)
    }
}
