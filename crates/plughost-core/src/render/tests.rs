use std::collections::VecDeque;

use super::*;

const RATE: f64 = 48_000.0;

/// A stereo processor whose output is the input delayed by `latency`, plus an echo of it
/// `echo_delay` samples later at half level.
struct Echo {
    latency: u32,
    echo_delay: usize,
    reported_tail: Tail,
    history: Vec<VecDeque<f32>>,
    latency_after_blocks: Option<(usize, u32)>,
    tail_after_blocks: Option<(usize, Tail)>,
    blocks: usize,
    /// The events each block received, by block number.
    events: Vec<(usize, Vec<Event>)>,
}

impl Echo {
    fn new(latency: u32, echo_delay: usize, reported_tail: Tail) -> Echo {
        let depth = latency as usize + echo_delay + 1;
        Echo {
            latency,
            echo_delay,
            reported_tail,
            history: vec![VecDeque::from(vec![0.0; depth]); 2],
            latency_after_blocks: None,
            tail_after_blocks: None,
            blocks: 0,
            events: Vec::new(),
        }
    }
}

impl Process<f32> for Echo {
    type Error = RenderError;

    fn sample_rate(&self) -> f64 {
        RATE
    }

    fn max_block_size(&self) -> usize {
        256
    }

    fn input_channels(&self) -> usize {
        2
    }

    fn output_channels(&self) -> usize {
        2
    }

    fn latency(&self) -> Result<u32, RenderError> {
        Ok(match self.latency_after_blocks {
            Some((blocks, latency)) if self.blocks >= blocks => latency,
            _ => self.latency,
        })
    }

    fn tail(&self) -> Result<Tail, RenderError> {
        Ok(match self.tail_after_blocks {
            Some((blocks, tail)) if self.blocks >= blocks => tail,
            _ => self.reported_tail,
        })
    }

    fn process(
        &mut self,
        _context: &crate::BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        _automation: &[crate::AutomationEvent],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), RenderError> {
        self.events.push((self.blocks, events.to_vec()));
        // The events come back out on port 1, one sample later where the block allows.
        produced.clear();
        produced.extend(events.iter().map(|event| Event {
            offset: (event.offset + 1).min(input[0].len() - 1),
            port: 1,
            ..event.clone()
        }));
        let direct = self.latency as usize;
        let echo = direct + self.echo_delay;
        for ((history, source), destination) in self.history.iter_mut().zip(input).zip(output) {
            for (sample, out) in source.iter().zip(destination.iter_mut()) {
                history.push_front(*sample);
                history.pop_back();
                let echoed = if self.echo_delay > 0 {
                    0.5 * history[echo]
                } else {
                    0.0
                };
                *out = history[direct] + echoed;
            }
        }
        self.blocks += 1;
        Ok(())
    }
}

fn signal(frames: usize) -> Vec<Vec<f32>> {
    (0..2)
        .map(|ch| {
            (0..frames)
                .map(|i| ((i * (ch + 3)) % 17) as f32 / 17.0 + 0.1)
                .collect()
        })
        .collect()
}

fn run(
    processor: &mut Echo,
    input: &[Vec<f32>],
    options: RenderOptions,
) -> Result<Rendered<f32>, RenderError> {
    let slices: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    render(processor, &slices, input[0].len(), &[], &options)
}

fn reported(max_tail_seconds: f64) -> RenderOptions {
    RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds,
    }
}

#[test]
fn latency_is_removed_so_output_lines_up_with_input() {
    let input = signal(1000);
    let rendered = run(
        &mut Echo::new(300, 0, Tail::Samples(0)),
        &input,
        reported(1.0),
    )
    .unwrap();
    assert_eq!(rendered.latency, 300);
    assert_eq!(rendered.channels, input);
}

#[test]
fn reported_tail_is_rendered_after_the_input() {
    let input = signal(1000);
    let rendered = run(
        &mut Echo::new(64, 300, Tail::Samples(300)),
        &input,
        reported(1.0),
    )
    .unwrap();
    assert_eq!(rendered.tail, 300);
    for (out, source) in rendered.channels.iter().zip(&input) {
        assert_eq!(out.len(), 1300);
        for i in 1000..1300 {
            assert_eq!(out[i], 0.5 * source[i - 300]);
        }
    }
}

#[test]
fn infinite_tail_stops_at_the_maximum() {
    let input = signal(1000);
    let rendered = run(&mut Echo::new(0, 0, Tail::Infinite), &input, reported(0.01)).unwrap();
    assert_eq!(rendered.tail, 480);
    assert_eq!(rendered.channels[0].len(), 1480);
}

#[test]
fn until_silence_waits_through_gaps_shorter_than_the_hold() {
    let input = signal(1000);
    let options = RenderOptions {
        tail: TailPolicy::UntilSilence {
            threshold: 1e-6,
            hold_seconds: 0.1,
        },
        max_tail_seconds: 1.0,
    };
    // The echo starts 2000 samples after the input ends: a silent gap shorter than the hold.
    let rendered = run(&mut Echo::new(10, 3000, Tail::Samples(0)), &input, options).unwrap();
    assert_eq!(rendered.tail, 3000);
    assert_eq!(rendered.channels[1][3999], 0.5 * input[1][999]);
}

#[test]
fn until_silence_stops_at_the_maximum_tail() {
    let input = signal(1000);
    let options = RenderOptions {
        tail: TailPolicy::UntilSilence {
            threshold: 1e-6,
            hold_seconds: 0.1,
        },
        max_tail_seconds: 0.01,
    };
    let rendered = run(&mut Echo::new(0, 3000, Tail::Samples(0)), &input, options).unwrap();
    assert_eq!(rendered.tail, 0);
    assert_eq!(rendered.channels[0].len(), 1000);
}

#[test]
fn latency_change_during_rendering_is_an_error() {
    let input = signal(1000);
    let mut processor = Echo::new(10, 0, Tail::Samples(0));
    processor.latency_after_blocks = Some((2, 20));
    let error = run(&mut processor, &input, reported(1.0)).unwrap_err();
    assert_eq!(
        error,
        RenderError::LatencyChanged {
            before: 10,
            after: 20
        }
    );
}

#[test]
fn tail_change_fails_only_when_it_changes_the_reported_render_length() {
    let input = signal(1000);
    let changing = || {
        let mut processor = Echo::new(0, 0, Tail::Samples(2400));
        processor.tail_after_blocks = Some((1, Tail::Samples(4800)));
        processor
    };
    let rendered = run(&mut changing(), &input, RenderOptions::default()).unwrap();
    assert_eq!(rendered.channels, input);
    assert_eq!(
        run(&mut changing(), &input, reported(1.0)).unwrap_err(),
        RenderError::TailChanged
    );
    // Both tails exceed the maximum, so the planned length is unchanged.
    let rendered = run(&mut changing(), &input, reported(0.01)).unwrap();
    assert_eq!(rendered.tail, 480);
}

#[test]
fn input_must_match_the_processor() {
    let mut processor = Echo::new(0, 0, Tail::Samples(0));
    let mono = [vec![0.0f32; 10]];
    assert_eq!(
        run(&mut processor, &mono, reported(1.0)).unwrap_err(),
        RenderError::InputChannels {
            expected: 2,
            actual: 1
        }
    );
    let uneven: Vec<&[f32]> = vec![&[0.0; 10], &[0.0; 9]];
    assert_eq!(
        render(&mut processor, &uneven, 10, &[], &reported(1.0)).unwrap_err(),
        RenderError::InputLength
    );
}

#[test]
fn events_reach_the_block_they_fall_in_with_block_offsets() {
    let input = signal(1000);
    let slices: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut processor = Echo::new(0, 0, Tail::Samples(0));
    let on = Event::note_on(10, 0, 60, 100);
    let off = Event::note_off(300, 0, 60, 0);
    render(
        &mut processor,
        &slices,
        1000,
        &[on.clone(), off.clone()],
        &reported(1.0),
    )
    .unwrap();
    let received: Vec<(usize, Vec<Event>)> = processor
        .events
        .into_iter()
        .filter(|(_, events)| !events.is_empty())
        .collect();
    let moved = Event {
        offset: 44,
        ..off.clone()
    };
    assert_eq!(received, [(0, vec![on.clone()]), (1, vec![moved])]);
    // Output events count from the start of the input, whichever block produced them.
    let rendered = render(
        &mut Echo::new(0, 0, Tail::Samples(0)),
        &slices,
        1000,
        &[on.clone(), off.clone()],
        &reported(1.0),
    )
    .unwrap();
    assert_eq!(
        rendered.events,
        [on.clone().on_port(1), off.clone().on_port(1)].map(|event| Event {
            offset: event.offset + 1,
            ..event
        })
    );
    let unordered = render(
        &mut Echo::new(0, 0, Tail::Samples(0)),
        &slices,
        1000,
        &[off, on],
        &reported(1.0),
    );
    assert_eq!(unordered.unwrap_err(), RenderError::Events);
}
