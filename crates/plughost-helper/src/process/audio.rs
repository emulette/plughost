//! Serial, explicitly routed audio. Preparation resolves native bus identities to channel ranges.
//! Each block is validated here, at the helper's IPC boundary, before any slot or delay advances.
use super::*;
use plughost_core::ipc::shared::Caller;
use plughost_core::ipc::{AudioSamples, Response};
use plughost_core::{Failure, FailureKind, InputError, RoutedChainConfig, Sample, events_fit};
use std::io;
use std::ops::{Add, Mul};

trait WireSample: Sample + Add<Output = Self> + Mul<Output = Self> + From<f32> {
    fn call(
        processor: &dyn BlockProcessor,
        context: &plughost_core::BlockContext,
        input: &[&[Self]],
        output: &mut [&mut [Self]],
        changes: &[plughost_core::ParameterChange],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), plughost_formats::Error>;
    fn respond(
        output: &[Vec<Self>],
        events: &[Event],
        latency: u32,
        tail: plughost_core::render::Tail,
        completion: &mut shared::Completion<'_>,
    ) -> io::Result<()>;
}
macro_rules! wire_sample {
    ($sample:ty, $method:ident, $finish:ident) => {
        impl WireSample for $sample {
            fn call(
                p: &dyn BlockProcessor,
                c: &plughost_core::BlockContext,
                i: &[&[Self]],
                o: &mut [&mut [Self]],
                a: &[plughost_core::ParameterChange],
                e: &[Event],
                produced: &mut Vec<Event>,
            ) -> Result<(), plughost_formats::Error> {
                p.$method(c, i, o, a, e, produced)
            }
            fn respond(
                output: &[Vec<Self>],
                events: &[Event],
                latency: u32,
                tail: plughost_core::render::Tail,
                completion: &mut shared::Completion<'_>,
            ) -> io::Result<()> {
                completion.$finish(output, events, latency, tail)
            }
        }
    };
}
wire_sample!(f32, process_audio_f32, f32);
wire_sample!(f64, process_audio_f64, f64);

pub(super) fn process(
    pipeline: &mut Pipeline,
    input: &AudioSamples,
    context: &plughost_core::BlockContext,
    automation: &[plughost_core::AutomationEvent],
    events: &[Event],
    responder: &Responder,
    completion: &mut shared::Completion<'_>,
) -> io::Result<()> {
    let Some(config) = &pipeline.audio_config else {
        return responder.fail(
            None,
            Failure::new(FailureKind::NotPrepared, CHAIN_NOT_PREPARED),
        );
    };
    if pipeline.resync {
        return responder.fail(
            None,
            Failure::new(FailureKind::Processing, crate::errors::EVENT_RESYNC),
        );
    }
    let validation = match input {
        AudioSamples::F32(input) => validate(config, input, context, automation, events),
        AudioSamples::F64(input) => validate(config, input, context, automation, events),
    };
    if let Err(error) = validation {
        return responder.send(&Response::Rejected { slot: None, error });
    }
    for event in automation {
        let result = pipeline.parameters[event.slot]
            .iter()
            .find(|info| info.id == event.change.id)
            .ok_or(InputError::UnknownParameter {
                id: event.change.id,
            })
            .and_then(|info| info.automation_value(event.change.value));
        if let Err(error) = result {
            return responder.send(&Response::Rejected {
                slot: Some(event.slot),
                error,
            });
        }
    }
    let (Some(alignment), Some((events_plan, event_buffers))) =
        (&mut pipeline.alignment, &mut pipeline.events)
    else {
        return responder.fail(
            None,
            Failure::new(FailureKind::NotPrepared, CHAIN_NOT_PREPARED),
        );
    };
    match (input, &mut pipeline.prepared) {
        (AudioSamples::F32(input), Some(prepared::Prepared::F32(storage))) => run(
            AudioState {
                plan: &alignment.timing,
                processors: &pipeline.processors,
                calls: &pipeline.calls,
                changes: &mut pipeline.changes,
                storage,
                delays: alignment.delays32.as_mut().unwrap(),
                events_plan,
                event_buffers,
                resync: &mut pipeline.resync,
            },
            input,
            context,
            automation,
            events,
            responder,
            completion,
        ),
        (AudioSamples::F64(input), Some(prepared::Prepared::F64(storage))) => run(
            AudioState {
                plan: &alignment.timing,
                processors: &pipeline.processors,
                calls: &pipeline.calls,
                changes: &mut pipeline.changes,
                storage,
                delays: alignment.delays64.as_mut().unwrap(),
                events_plan,
                event_buffers,
                resync: &mut pipeline.resync,
            },
            input,
            context,
            automation,
            events,
            responder,
            completion,
        ),
        _ => responder.fail(
            None,
            Failure::new(FailureKind::NotPrepared, CHAIN_NOT_PREPARED),
        ),
    }
}
fn validate<S: WireSample>(
    config: &RoutedChainConfig,
    input: &[Vec<S>],
    context: &plughost_core::BlockContext,
    automation: &[plughost_core::AutomationEvent],
    events: &[Event],
) -> Result<(), InputError> {
    context.validate()?;
    plughost_core::validate_event_budget(automation.len(), events)?;
    if let Some(event) = events
        .iter()
        .find(|event| event.port >= config.event_inputs)
    {
        return Err(InputError::EventPort { port: event.port });
    }
    if let Some(transport) = context.transport {
        transport.validate_at_rate(config.sample_rate)?;
    }
    if S::FORMAT != config.sample_format {
        return Err(InputError::AudioSampleFormat);
    }
    if context.frames > config.max_block_size
        || input.len() != config.input_channels()
        || input.iter().any(|channel| channel.len() != context.frames)
        || !events_fit(events, context.frames)
    {
        return Err(InputError::AudioBuffers);
    }
    if automation.iter().any(|event| {
        event.slot >= config.slots.len()
            || !plughost_core::changes_fit(&[event.change], context.frames)
    }) || automation
        .windows(2)
        .any(|events| events[0].change.offset > events[1].change.offset)
    {
        return Err(InputError::Automation);
    }
    Ok(())
}
struct AudioState<'a, S: Sample> {
    plan: &'a audio_timing::AudioTiming,
    processors: &'a [Box<dyn BlockProcessor>],
    calls: &'a Calls,
    changes: &'a mut plughost_core::ChangeBuffer,
    storage: &'a mut prepared::Routed<S>,
    delays: &'a mut audio_timing::DelayBank<S>,
    events_plan: &'a events::EventPlan,
    event_buffers: &'a mut events::EventBuffers,
    resync: &'a mut bool,
}

/// A plugin that asked to be prepared again is refused before any slot advances. Other timing
/// changes surface when the timing is read once after the block, which also reports it.
fn run<S: WireSample>(
    state: AudioState<'_, S>,
    input: &[Vec<S>],
    context: &plughost_core::BlockContext,
    automation: &[plughost_core::AutomationEvent],
    events: &[Event],
    responder: &Responder,
    completion: &mut shared::Completion<'_>,
) -> io::Result<()> {
    if state
        .processors
        .iter()
        .any(|processor| processor.restart_required())
    {
        return responder.fail(
            None,
            Failure::new(
                FailureKind::RestartRequired,
                crate::errors::AUDIO_LATENCY_CHANGED,
            ),
        );
    }
    let event_buffers = &mut *state.event_buffers;
    for (slot, processor) in state.processors.iter().enumerate() {
        let (done, pending) = event_buffers.produced.split_at_mut(slot);
        let previous_events = done.last().map_or(&[][..], Vec::as_slice);
        let slot_events = &mut event_buffers.inputs[slot];
        if let Err(error) = state
            .events_plan
            .gather(slot, events, previous_events, slot_events)
        {
            // Earlier slots already advanced; the notes they forwarded may be held.
            *state.resync = true;
            return responder.fail(
                Some(slot),
                Failure::new(FailureKind::Processing, error.to_string()),
            );
        }
        let produced = &mut pending[0];
        let (past, next) = state.storage.slots.split_at_mut(slot);
        let buffers = &mut next[0];
        buffers.begin(slot, context.frames, automation);
        let previous = if slot == 0 {
            &[][..]
        } else {
            past[slot - 1].output.as_slice()
        };
        let result = {
            let _call = state.calls.enter(Caller::Processing, slot);
            buffers.process(
                input,
                previous,
                context.frames,
                state.delays,
                slot,
                |inputs, outputs, changes| {
                    S::call(
                        processor.as_ref(),
                        context,
                        inputs,
                        outputs,
                        changes,
                        slot_events,
                        produced,
                    )
                },
            )
        };
        if let Err(error) = result {
            if error.is_output_event_overflow() {
                *state.resync = true;
            }
            return responder.fail(Some(slot), error.failure());
        }
    }
    let event_buffers = &mut *state.event_buffers;
    state.events_plan.output(
        event_buffers.produced.last().unwrap(),
        &mut event_buffers.output,
    );
    for (slot, (processor, timing)) in state
        .processors
        .iter()
        .zip(&mut state.storage.timings)
        .enumerate()
    {
        let _call = state.calls.enter(Caller::Processing, slot);
        *timing = match audio_timing::plugin_timing(processor.as_ref()) {
            Ok(timing) => timing,
            Err(error) => return responder.fail(Some(slot), error.failure()),
        };
        state.changes.observe(slot, *timing);
    }
    let tail = match state.plan.tail(&state.storage.timings) {
        Ok(tail) => tail,
        Err(failure) => return responder.fail(None, failure),
    };
    S::respond(
        &state.storage.slots.last().unwrap().output,
        &state.event_buffers.output,
        state.plan.latency(),
        tail,
        completion,
    )
}
