use std::time::Instant;

use plughost::{RenderOptions, TailPolicy};
use plughost_core::render::{Process, RenderSchedule, Tail, render_with_schedule};
use plughost_core::{AutomationEvent, BlockContext, Event};
use serde::Serialize;

use crate::cases::Case;
use crate::errors::Result;

#[derive(Serialize)]
pub struct Trial {
    pub seconds: f64,
    pub audio_seconds_per_second: f64,
    pub process_calls: usize,
    pub processed_frames: usize,
    /// Host heap allocations and reallocations during the render, across all application threads.
    pub allocations: u64,
}

#[derive(Serialize)]
pub struct Measurement {
    pub case: Case,
    pub lifecycle: crate::lifecycle::Lifecycle,
    pub trials: Vec<Trial>,
    pub median_render_seconds: f64,
    pub audio_seconds_per_second_at_median_duration: f64,
}

struct Counted<P> {
    processor: P,
    calls: usize,
    frames: usize,
}

impl<P: Process<f32>> Process<f32> for Counted<P> {
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
    fn latency(&self) -> std::result::Result<u32, Self::Error> {
        self.processor.latency()
    }
    fn tail(&self) -> std::result::Result<Tail, Self::Error> {
        self.processor.tail()
    }
    fn validate_automation(
        &mut self,
        events: &[AutomationEvent],
    ) -> std::result::Result<(), Self::Error> {
        self.processor.validate_automation(events)
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[AutomationEvent],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> std::result::Result<(), Self::Error> {
        self.calls += 1;
        self.frames += context.frames;
        self.processor
            .process(context, input, output, automation, events, produced)
    }
}

pub fn run<P: Process<f32>>(
    processor: P,
    case: Case,
    repetitions: usize,
    lifecycle: crate::lifecycle::Lifecycle,
) -> Result<Measurement>
where
    P::Error: Into<crate::errors::BenchError>,
{
    let input = case.input();
    let slices: Vec<_> = input.iter().map(Vec::as_slice).collect();
    let (points, ramps) = case.schedule();
    let schedule = RenderSchedule {
        transport: &[],
        automation: &points,
        ramps: &ramps,
    };
    let options = RenderOptions::new(TailPolicy::Reported, 0.0);
    let mut counted = Counted {
        processor,
        calls: 0,
        frames: 0,
    };
    let mut trials = Vec::new();
    // First full render is warm-up; every render is checked outside the timed interval.
    for trial in 0..=repetitions {
        counted.calls = 0;
        counted.frames = 0;
        let allocations = crate::allocations::count();
        let start = Instant::now();
        let output =
            render_with_schedule(&mut counted, &slices, case.frames, &[], &options, &schedule)
                .map_err(Into::into)?;
        let seconds = start.elapsed().as_secs_f64();
        let allocations = crate::allocations::count() - allocations;
        case.verify(&input, &output)?;
        std::hint::black_box(&output);
        if trial > 0 {
            trials.push(Trial {
                seconds,
                audio_seconds_per_second: case.frames as f64
                    / f64::from(case.sample_rate)
                    / seconds,
                process_calls: counted.calls,
                processed_frames: counted.frames,
                allocations,
            });
        }
    }
    let mut durations: Vec<_> = trials.iter().map(|trial| trial.seconds).collect();
    durations.sort_by(f64::total_cmp);
    let middle = durations.len() / 2;
    let median = if durations.len() % 2 == 0 {
        (durations[middle - 1] + durations[middle]) / 2.0
    } else {
        durations[middle]
    };
    Ok(Measurement {
        case,
        lifecycle,
        trials,
        median_render_seconds: median,
        audio_seconds_per_second_at_median_duration: case.frames as f64
            / f64::from(case.sample_rate)
            / median,
    })
}
