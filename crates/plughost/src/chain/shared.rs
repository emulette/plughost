//! Prepared shared transport. Caller output is written only after the helper completes a block.
use plughost_core::ipc::shared::{Descriptor, SharedAudio, SlotConfig};
use plughost_core::{AutomationEvent, BlockContext, Failure, FailureKind};

use super::*;

pub(super) struct Transport {
    pub memory: SharedAudio,
    sequence: u64,
}

fn storage_error(error: std::io::Error) -> Error {
    Error::Operation {
        slot: None,
        failure: Failure::new(FailureKind::Configuration, error.to_string()),
    }
}

impl Chain {
    pub(super) fn prepare_transport(
        &mut self,
        candidate: &Transport,
        request: impl FnOnce(u64) -> Request,
    ) -> Result<Response, Error> {
        let result = self
            .helper
            .prepare_shared(&candidate.memory, request, self.timeouts.control);
        if matches!(
            result,
            Err(Error::Protocol | Error::Crashed { .. } | Error::TimedOut { .. })
        ) {
            self.shared = None;
        }
        result
    }
    pub(super) fn transport_candidate(&mut self, config: SlotConfig) -> Result<Transport, Error> {
        let generation = self.generation.checked_add(1).ok_or(Error::Protocol)?;
        self.generation = generation;
        let memory = SharedAudio::new(Descriptor { generation, config }).map_err(storage_error)?;
        Ok(Transport {
            memory,
            sequence: 0,
        })
    }

    pub(super) fn protocol_error(&mut self) -> Error {
        self.helper.kill();
        self.shared = None;
        Error::Protocol
    }
}

macro_rules! process {
    ($sample:ty, $method:ident, $write:ident, $read:ident) => {
        impl Chain {
            pub(super) fn $method(
                &mut self,
                context: &BlockContext,
                input: &[&[$sample]],
                output: &mut [&mut [$sample]],
                automation: &[AutomationEvent],
                events: &[Event],
                produced: &mut Vec<Event>,
            ) -> Result<(), Error> {
                let mut transport = self.shared.take().ok_or(Error::NotPrepared)?;
                let result = (|| {
                    transport.sequence =
                        transport.sequence.checked_add(1).ok_or(Error::Protocol)?;
                    let submission = transport
                        .memory
                        .$write(
                            transport.sequence,
                            context.frames,
                            input,
                            automation,
                            events,
                        )
                        .map_err(|_| Error::Protocol)?;
                    let reply = self.helper.request(
                        Request::Process {
                            context: *context,
                            submission,
                        },
                        self.timeouts.process,
                    )?;
                    let Response::Processed {
                        submission: completed,
                        events: event_count,
                        latency,
                        tail,
                    } = reply
                    else {
                        return Err(Error::Protocol);
                    };
                    if completed != submission {
                        return Err(Error::Protocol);
                    }
                    produced.clear();
                    produced.reserve(event_count.min(plughost_core::MAX_BLOCK_EVENTS));
                    transport
                        .memory
                        .$read(submission, output, event_count, produced)
                        .map_err(|_| Error::Protocol)?;
                    self.latency = latency;
                    self.tail = tail;
                    Ok(())
                })();
                match &result {
                    Err(Error::Protocol) => {
                        self.helper.kill();
                    }
                    Err(Error::Crashed { .. } | Error::TimedOut { .. }) => {}
                    _ => {
                        self.shared = Some(transport);
                    }
                }
                result
            }
        }
    };
}
process!(f32, process_shared_f32, write_input_f32, read_output_f32);
process!(f64, process_shared_f64, write_input_f64, read_output_f64);
