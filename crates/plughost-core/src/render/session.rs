use super::{
    Delivery, Process, RenderInput, RenderOptions, RenderProgress, RenderSchedule, RenderStatus,
    Scratch, Tail, TailPolicy, TransportChange, render_reusing,
};
use crate::{AutomationEvent, BlockContext, Events, RenderError, Sample, Transport};

/// Lifecycle of a continuous render session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionState {
    /// Accepts more input segments or a final latency/tail flush.
    Active,
    /// The final latency/tail flush completed successfully.
    Finished,
    /// A cancellation callback stopped processing; reset the processor before independent reuse.
    Cancelled,
    /// A processing or render error ended the session.
    Failed,
}

/// One continuous DSP render: push any number of input segments, then flush latency/tail once.
/// Construction does not reset the processor. Use a fresh/reset processor for an independent
/// render. After cancellation or a native failure, drop the session and reset before reuse.
#[must_use = "a session renders nothing until segments are pushed and it is finished"]
pub struct RenderSession<'a, S: Sample, P: Process<S>> {
    processor: &'a mut P,
    latency: u32,
    skip: usize,
    transport: Option<Transport>,
    state: SessionState,
    /// Render buffers every segment reuses.
    scratch: Scratch<S>,
    /// The current segment's transport changes, reused likewise.
    transport_changes: Vec<TransportChange>,
}

impl<S: Sample, P: Process<S>> std::fmt::Debug for RenderSession<'_, S, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderSession")
            .field("state", &self.state)
            .field("latency", &self.latency)
            .field("skip", &self.skip)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl<'a, S: Sample, P: Process<S>> RenderSession<'a, S, P> {
    /// Begins a continuous render and captures latency without resetting the processor.
    pub fn new(processor: &'a mut P) -> Result<Self, P::Error> {
        processor.begin_render()?;
        let latency = processor.latency()?;
        Ok(Self {
            processor,
            latency,
            skip: latency as usize,
            transport: None,
            state: SessionState::Active,
            scratch: Scratch::default(),
            transport_changes: Vec::new(),
        })
    }

    /// Current lifecycle state; only `Active` accepts further input.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Output event offsets count from the start of this segment, like its input events.
    pub fn push(
        &mut self,
        request: RenderInput<'_, S>,
        consume: impl FnMut(Delivery<'_, S>),
        cancelled: impl FnMut() -> bool,
    ) -> Result<RenderProgress, P::Error> {
        let options = RenderOptions {
            tail: TailPolicy::Reported,
            max_tail_seconds: 0.0,
        };
        self.run(request, &options, false, consume, cancelled)
    }

    /// Flushes the remaining latency and tail once, then closes the session.
    /// Output event offsets are relative to this flush; cancellation or errors close the session too.
    pub fn finish(
        &mut self,
        options: &RenderOptions,
        consume: impl FnMut(Delivery<'_, S>),
        cancelled: impl FnMut() -> bool,
    ) -> Result<RenderProgress, P::Error> {
        // No input frames: the renderer plays silence through the latency and the tail.
        let audio = vec![&[][..]; self.processor.input_channels()];
        let request = RenderInput {
            audio: &audio,
            frames: 0,
            events: &Events::new(),
            schedule: RenderSchedule::default(),
        };
        let result = self.run(request, options, true, consume, cancelled)?;
        if self.state == SessionState::Active {
            self.state = SessionState::Finished;
        }
        Ok(result)
    }

    fn run(
        &mut self,
        request: RenderInput<'_, S>,
        options: &RenderOptions,
        flush: bool,
        consume: impl FnMut(Delivery<'_, S>),
        cancelled: impl FnMut() -> bool,
    ) -> Result<RenderProgress, P::Error> {
        if self.state != SessionState::Active {
            return Err(RenderError::SessionClosed.into());
        }
        let transport = &mut self.transport_changes;
        transport.clear();
        if self.transport.is_some()
            && request
                .schedule
                .transport
                .first()
                .is_none_or(|point| point.offset != 0)
        {
            transport.push(TransportChange {
                offset: 0,
                transport: self.transport,
            });
        }
        transport.extend_from_slice(request.schedule.transport);
        let schedule = RenderSchedule {
            transport,
            ..request.schedule
        };
        let mut adapter = Segment {
            processor: self.processor,
            latency: self.latency,
            flush,
        };
        // The latency is trimmed once, from the start of the session's output.
        let result = render_reusing(
            &mut adapter,
            RenderInput {
                schedule,
                ..request
            },
            options,
            Some(self.skip),
            &mut self.scratch,
            consume,
            cancelled,
        );
        match result {
            Ok(mut progress) => {
                self.transport = schedule
                    .transport_at(progress.processed_frames, self.processor.sample_rate())
                    .map_err(RenderError::Input)?;
                if progress.status == RenderStatus::Cancelled {
                    self.state = SessionState::Cancelled;
                }
                self.skip -= self.skip.min(progress.processed_frames);
                progress.latency = self.latency;
                Ok(progress)
            }
            Err(error) => {
                self.state = SessionState::Failed;
                Err(error)
            }
        }
    }
}

/// Remove per-call alignment from the shared renderer; the session trims only once, and this
/// adapter checks the real native latency before any samples reach the output consumer. The
/// final flush reports the latency, so the renderer processes it after its empty input.
struct Segment<'a, P> {
    processor: &'a mut P,
    latency: u32,
    flush: bool,
}

impl<S: Sample, P: Process<S>> Process<S> for Segment<'_, P> {
    type Error = P::Error;
    fn sample_rate(&self) -> f64 {
        self.processor.sample_rate()
    }
    fn max_block_size(&self) -> usize {
        self.processor.max_block_size()
    }
    fn input_channels(&self) -> usize {
        self.processor.input_channels()
    }
    fn output_channels(&self) -> usize {
        self.processor.output_channels()
    }
    fn latency(&self) -> Result<u32, Self::Error> {
        let now = self.processor.latency()?;
        if now != self.latency {
            return Err(RenderError::LatencyChanged {
                before: self.latency,
                after: now,
            }
            .into());
        }
        Ok(if self.flush { self.latency } else { 0 })
    }
    fn tail(&self) -> Result<Tail, Self::Error> {
        self.processor.tail()
    }
    fn validate_automation(&mut self, automation: &[AutomationEvent]) -> Result<(), Self::Error> {
        self.processor.validate_automation(automation)
    }
    fn validate_events(&mut self, events: &Events) -> Result<(), Self::Error> {
        self.processor.validate_events(events)
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[AutomationEvent],
        events: &Events,
        produced: &mut Events,
    ) -> Result<(), Self::Error> {
        self.processor
            .process(context, input, output, automation, events, produced)
    }
}
