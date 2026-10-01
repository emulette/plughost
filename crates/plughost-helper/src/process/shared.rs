//! Local input storage filled from the shared mapping, and shared-output publication, on the
//! processing thread.
use std::io;

use plughost_core::ipc::shared::{Descriptor, SharedAudio, Submission, transfer};
use plughost_core::ipc::{AudioSamples, Response};
use plughost_core::{AutomationEvent, BlockContext, Event, Failure, FailureKind, MAX_BLOCK_EVENTS};

use super::{Pipeline, audio, prepared};
use crate::Responder;

pub(crate) struct Transport {
    memory: SharedAudio,
    input: AudioSamples,
    automation: Vec<AutomationEvent>,
    events: Vec<Event>,
}

impl Transport {
    pub fn receive(
        receiver: &transfer::Receiver,
        descriptor: Descriptor,
        handle: u64,
    ) -> io::Result<Self> {
        // SAFETY: the parent creates and transfers one immutable-size backing file per request.
        // The private connection supplies its exact descriptor and unique handle token once.
        let memory = unsafe { receiver.receive(descriptor, handle) }?;
        let input = descriptor.config.input_storage()?;
        Ok(Self {
            memory,
            input,
            automation: Vec::with_capacity(MAX_BLOCK_EVENTS),
            events: Vec::with_capacity(MAX_BLOCK_EVENTS),
        })
    }
    pub fn descriptor(&self) -> Descriptor {
        self.memory.descriptor()
    }
}

pub(super) fn process(
    pipeline: &mut Pipeline,
    context: &BlockContext,
    submission: Submission,
    responder: &Responder,
) -> io::Result<()> {
    let Some(mut transport) = pipeline.transport.take() else {
        return responder.fail(
            None,
            Failure::new(
                FailureKind::NotPrepared,
                plughost_core::messages::CHAIN_NOT_PREPARED,
            ),
        );
    };
    let result = (|| {
        let read = match &mut transport.input {
            AudioSamples::F32(input) => transport.memory.read_input_f32(
                submission,
                input,
                &mut transport.automation,
                &mut transport.events,
            ),
            AudioSamples::F64(input) => transport.memory.read_input_f64(
                submission,
                input,
                &mut transport.automation,
                &mut transport.events,
            ),
        };
        if let Err(error) = read {
            return responder.fail(None, Failure::new(FailureKind::Protocol, error.to_string()));
        }
        let mut completion = Completion {
            memory: &mut transport.memory,
            submission,
            responder,
        };
        audio::process(
            pipeline,
            &transport.input,
            context,
            &transport.automation,
            &transport.events,
            responder,
            &mut completion,
        )
    })();
    pipeline.transport = Some(transport);
    result
}

pub(super) struct Completion<'a> {
    memory: &'a mut SharedAudio,
    submission: Submission,
    responder: &'a Responder,
}

macro_rules! complete {
    ($sample:ty, $name:ident, $write:ident) => {
        impl Completion<'_> {
            pub fn $name(
                &mut self,
                output: &[Vec<$sample>],
                events: &[Event],
                latency: u32,
                tail: plughost_core::render::Tail,
            ) -> io::Result<()> {
                let result = prepared::with_inputs(output, |channels| {
                    self.memory.$write(self.submission, channels, events)
                });
                if let Err(error) = result {
                    return self
                        .responder
                        .fail(None, Failure::new(FailureKind::Protocol, error.to_string()));
                }
                self.responder.send(&Response::Processed {
                    submission: self.submission,
                    events: events.len(),
                    latency,
                    tail,
                })
            }
        }
    };
}
complete!(f32, f32, write_output_f32);
complete!(f64, f64, write_output_f64);
