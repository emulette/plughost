use super::{
    Delivery, Process, RenderInput, RenderOptions, RenderProgress, RenderSchedule, RenderStatus,
    Tail, TailPolicy, TransportChange, render_stream,
};
use crate::{AutomationEvent, BlockContext, MidiEvent, RenderError, Sample, Transport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Finished,
    Cancelled,
    Failed,
}

/// One continuous DSP render: push any number of input segments, then flush latency/tail once.
/// Construction does not reset the processor. Use a fresh/reset processor for an independent
/// render. After cancellation or a native failure, drop the session and reset before reuse.
pub struct RenderSession<'a, S: Sample, P: Process<S>> {
    processor: &'a mut P,
    latency: u32,
    skip: usize,
    transport: Option<Transport>,
    state: SessionState,
    marker: std::marker::PhantomData<S>,
}

impl<'a, S: Sample, P: Process<S>> RenderSession<'a, S, P> {
    pub fn new(processor: &'a mut P) -> Result<Self, P::Error> {
        let latency = processor.latency()?;
        Ok(Self {
            processor,
            latency,
            skip: latency as usize,
            transport: None,
            state: SessionState::Active,
            marker: std::marker::PhantomData,
        })
    }

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
        self.run(request, &options, None, consume, cancelled)
    }

    pub fn finish(
        &mut self,
        options: &RenderOptions,
        consume: impl FnMut(Delivery<'_, S>),
        cancelled: impl FnMut() -> bool,
    ) -> Result<RenderProgress, P::Error> {
        let request = RenderInput {
            audio: &[],
            frames: self.latency as usize,
            events: &[],
            schedule: RenderSchedule::default(),
        };
        // Allocated once per flush; every block borrows it as silent input.
        let zeros = vec![S::default(); self.processor.max_block_size()];
        let result = self.run(request, options, Some(zeros), consume, cancelled)?;
        if self.state == SessionState::Active {
            self.state = SessionState::Finished;
        }
        Ok(result)
    }

    fn run(
        &mut self,
        request: RenderInput<'_, S>,
        options: &RenderOptions,
        silent: Option<Vec<S>>,
        mut consume: impl FnMut(Delivery<'_, S>),
        cancelled: impl FnMut() -> bool,
    ) -> Result<RenderProgress, P::Error> {
        if self.state != SessionState::Active {
            return Err(RenderError::SessionClosed.into());
        }
        let mut transport = Vec::new();
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
            transport: &transport,
            ..request.schedule
        };
        let skip_before = self.skip;
        let mut emitted = 0;
        let mut adapter = Segment {
            processor: self.processor,
            latency: self.latency,
            silent,
        };
        let result = render_stream(
            &mut adapter,
            RenderInput {
                schedule,
                ..request
            },
            options,
            |delivery| {
                let count = delivery.audio.first().map_or(0, |channel| channel.len());
                let skip = self.skip.min(count);
                self.skip -= skip;
                if skip < count || !delivery.events.is_empty() {
                    let slices: Vec<_> = delivery
                        .audio
                        .iter()
                        .map(|channel| &channel[skip..])
                        .collect();
                    consume(Delivery {
                        audio: &slices,
                        events: delivery.events,
                    });
                    emitted += count - skip;
                }
            },
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
                if self.processor.output_channels() == 0 {
                    let skipped = skip_before.min(progress.output_frames);
                    self.skip = skip_before - skipped;
                    emitted = progress.output_frames - skipped;
                }
                progress.output_frames = emitted;
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
/// adapter checks the real native latency before any samples reach the output consumer.
struct Segment<'a, S, P> {
    processor: &'a mut P,
    latency: u32,
    /// Zero input for the final flush, at least one block long.
    silent: Option<Vec<S>>,
}

impl<S: Sample, P: Process<S>> Process<S> for Segment<'_, S, P> {
    type Error = P::Error;
    fn sample_rate(&self) -> f64 {
        self.processor.sample_rate()
    }
    fn max_block_size(&self) -> usize {
        self.processor.max_block_size()
    }
    fn input_channels(&self) -> usize {
        if self.silent.is_some() {
            0
        } else {
            self.processor.input_channels()
        }
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
        Ok(0)
    }
    fn tail(&self) -> Result<Tail, Self::Error> {
        self.processor.tail()
    }
    fn validate_automation(&mut self, automation: &[AutomationEvent]) -> Result<(), Self::Error> {
        self.processor.validate_automation(automation)
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[AutomationEvent],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<(), Self::Error> {
        if let Some(zeros) = &self.silent {
            let input = vec![&zeros[..context.frames]; self.processor.input_channels()];
            self.processor
                .process(context, &input, output, automation, events, produced)
        } else {
            self.processor
                .process(context, input, output, automation, events, produced)
        }
    }
}
