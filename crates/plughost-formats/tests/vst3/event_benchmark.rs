//! Input event staging through the native fixture, including blocks holding a full note table.

use std::io::Write;
use std::time::Instant;

use plughost_core::{BlockContext, EventData, MAX_BLOCK_EVENTS, Note};

use super::*;

#[test]
#[ignore = "native VST3 input benchmark; run in release mode with --nocapture"]
fn input_event_staging_benchmark() {
    let _serial = serial();
    for dense in [false, true] {
        let mut plugin = plugin(if dense {
            "plughost-test-delay"
        } else {
            "plughost-test-routing"
        });
        let config = ProcessConfig {
            max_block_size: 1,
            ..config(48_000.0, SampleFormat::F32)
        };
        plugin.prepare(&config).unwrap();
        let notes: Vec<_> = (0..MAX_BLOCK_EVENTS)
            .map(|id| {
                Event::new(
                    0,
                    EventData::NoteOn(Note::new(0, 60, 1.0).with_id(id as u32)),
                )
            })
            .collect();
        let events: Vec<_> = (0..MAX_BLOCK_EVENTS)
            .map(|index| Event::control_change(0, 0, 7, (index % 128) as u8))
            .collect();
        let input = [1.0f32];
        let (mut left, mut right) = ([0.0f32], [0.0f32]);
        let mut produced = Vec::with_capacity(MAX_BLOCK_EVENTS);
        let mut block = |events: &[Event]| {
            plugin
                .process(
                    &BlockContext::new(1),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    events,
                    &mut produced,
                )
                .unwrap();
            assert_eq!(left, right);
            (left[0], produced.len())
        };
        if !dense {
            // The routing fixture echoes each note plus a controller, within the output budget.
            for notes in notes.chunks(MAX_BLOCK_EVENTS / 2) {
                assert_eq!(block(notes).1, 2 * notes.len());
            }
        }
        // Fill the fixture's delay before timing; every subsequent sample is exactly one.
        for _ in 0..TEST_DELAY_LATENCY {
            block(&[]);
        }
        let iterations = if dense { 1_000 } else { 100_000 };
        let inputs = if dense { events.as_slice() } else { &[] };
        let started = Instant::now();
        let mut sum = 0.0f64;
        for _ in 0..iterations {
            let (sample, events) = block(std::hint::black_box(inputs));
            assert_eq!(events, 0);
            sum += f64::from(sample);
        }
        let elapsed = started.elapsed();
        assert_eq!(sum, f64::from(iterations));
        if !dense {
            // All 4,096 held note IDs still receive the wildcard note-off after empty blocks.
            assert_eq!(block(&[Event::note_off(0, 0, 60, 0)]).1, MAX_BLOCK_EVENTS);
        }
        writeln!(
            std::io::stdout(),
            "dense={dense} blocks={iterations} ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}
