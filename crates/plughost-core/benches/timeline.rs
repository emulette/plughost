//! Fixed offline scheduling workloads with output fingerprints, independent of native plugins.
use std::io::Write;
use std::time::Instant;

use plughost_core::render::{
    AutomationRamp, Process, RenderInput, RenderOptions, RenderSchedule, Tail, TailPolicy,
    render_stream,
};
use plughost_core::{AutomationEvent, BlockContext, Event, ParameterChange, RenderError};

const SEED: u64 = 0xcbf29ce484222325;

fn hash(hash: &mut u64, word: u64) {
    *hash = (*hash ^ word).wrapping_mul(0x100000001b3);
}

struct Probe {
    frames: usize,
    calls: usize,
    points: usize,
    value: f32,
    automation_hash: u64,
}

impl Process<f32> for Probe {
    type Error = RenderError;
    fn sample_rate(&self) -> f64 {
        48_000.0
    }
    fn max_block_size(&self) -> usize {
        512
    }
    fn input_channels(&self) -> usize {
        0
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn latency(&self) -> Result<u32, Self::Error> {
        Ok(0)
    }
    fn tail(&self) -> Result<Tail, Self::Error> {
        Ok(Tail::Samples(0))
    }
    fn validate_automation(&mut self, _: &[AutomationEvent]) -> Result<(), Self::Error> {
        Ok(())
    }
    fn process(
        &mut self,
        context: &BlockContext,
        _: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[AutomationEvent],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), Self::Error> {
        let mut offset = 0;
        for point in automation {
            output[0][offset..point.change.offset].fill(self.value);
            offset = point.change.offset;
            self.value = point.change.value as f32;
            for word in [
                point.slot as u64,
                point.change.id,
                (self.frames + offset) as u64,
                point.change.value.to_bits(),
            ] {
                hash(&mut self.automation_hash, word);
            }
        }
        output[0][offset..].fill(self.value);
        self.points += automation.len();
        self.frames += context.frames;
        self.calls += 1;
        produced.clear();
        produced.extend_from_slice(events);
        Ok(())
    }
}

fn run(
    name: &str,
    frames: usize,
    ramps: &[AutomationRamp],
    points: &[AutomationEvent],
    events: &[Event],
    expected: [u64; 3],
) -> Result<(), Box<dyn std::error::Error>> {
    let schedule = RenderSchedule {
        ramps,
        automation: points,
        transport: &[],
    };
    let mut probe = Probe {
        frames: 0,
        calls: 0,
        points: 0,
        value: 0.0,
        automation_hash: SEED,
    };
    let mut samples = 0;
    let mut audio_hash = SEED;
    let mut event_hash = SEED;
    let start = Instant::now();
    render_stream(
        &mut probe,
        RenderInput {
            audio: &[],
            frames,
            events,
            schedule,
        },
        &RenderOptions::new(TailPolicy::Reported, 0.0),
        |delivery| {
            for &sample in delivery.audio[0] {
                hash(&mut audio_hash, u64::from(sample.to_bits()));
                samples += 1;
            }
            for event in delivery.events {
                hash(&mut event_hash, event.offset as u64);
                for byte in event.data.to_midi().unwrap() {
                    hash(&mut event_hash, u64::from(byte));
                }
            }
        },
        || false,
    )?;
    let elapsed = start.elapsed();
    assert_eq!(samples, frames);
    assert_eq!(probe.frames, frames);
    assert_eq!([probe.automation_hash, audio_hash, event_hash], expected);
    writeln!(
        std::io::stdout(),
        "{name} frames={frames} ramps={} points={} blocks={} ms={:.3} automation={:016x} audio={audio_hash:016x} events={event_hash:016x}",
        ramps.len(),
        probe.points,
        probe.calls,
        elapsed.as_secs_f64() * 1000.0,
        probe.automation_hash
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ramps: Vec<_> = (0..5000)
        .map(|index| AutomationRamp {
            slot: 0,
            id: 0,
            start: index * 576,
            end: index * 576 + 575,
            from: 0.0,
            to: 1.0,
        })
        .collect();
    run(
        "successive",
        48_000 * 60,
        &ramps,
        &[],
        &[],
        [0x9c37586baa53bc45, 0xb7771fc047d8b275, SEED],
    )?;

    let ramps: Vec<_> = (0..5000)
        .rev()
        .map(|index| AutomationRamp {
            slot: 0,
            id: index as u64 % 500,
            start: index / 500 * 64,
            end: index / 500 * 64 + 31,
            from: 1.0,
            to: 0.0,
        })
        .collect();
    run(
        "overlap",
        640,
        &ramps,
        &[],
        &[],
        [0xedce44cd9847ece5, 0x037b62c199379f65, SEED],
    )?;

    let ramps: Vec<_> = (0..2048)
        .rev()
        .map(|index| AutomationRamp {
            slot: index % 4,
            id: index as u64 % 16,
            start: index * 512,
            end: index * 512 + 256,
            from: 0.0,
            to: 1.0,
        })
        .collect();
    let points: Vec<_> = (0..2048)
        .flat_map(|index| {
            [0.25, 0.75].map(move |value| AutomationEvent {
                slot: 0,
                change: ParameterChange {
                    id: 64,
                    offset: index * 512 + 100,
                    value,
                },
            })
        })
        .collect();
    let events: Vec<_> = (0..2048)
        .map(|index| Event::note_on(index * 512 + 100, 0, 60, 90))
        .collect();
    run(
        "mixed",
        2048 * 512,
        &ramps,
        &points,
        &events,
        [0x242905e0f4c5b325, 0x7f754fe73de22325, 0x03f3b39d94efe325],
    )
}
