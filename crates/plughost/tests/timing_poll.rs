//! Repeated idle observations across native slots, including the real helper command queue.
use std::io::Write;
use std::time::Instant;

use plughost::{PluginFormat, Tail};

mod support;

#[test]
#[ignore = "native timing observation benchmark; use --release --nocapture"]
fn timing_poll_benchmark() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let reference = if format == PluginFormat::Clap {
            support::delay_variant(format, "thread-check", 0xf)
        } else {
            support::delay(format)
        };
        let mut chain = support::spawn(&vec![reference; 8]);
        support::prepare(&mut chain);
        chain.take_changes().unwrap();
        let reads = |chain: &mut plughost::Chain| -> f64 {
            if format == PluginFormat::Clap {
                chain
                    .parameters(0)
                    .unwrap()
                    .into_iter()
                    .find(|(info, _)| info.id == 15)
                    .unwrap()
                    .1
                    * 1_000_000.0
            } else {
                0.0
            }
        };
        let before = reads(&mut chain);
        let start = Instant::now();
        for _ in 0..10_000 {
            let batch = chain.take_changes().unwrap();
            assert!(batch.changes.is_empty());
            assert_eq!(batch.snapshot.len(), 8);
            for timing in batch.snapshot {
                let timing = timing.unwrap();
                assert_eq!(timing.latency, 480);
                assert_eq!(timing.tail, Tail::Samples(0));
            }
        }
        let elapsed = start.elapsed();
        let native_reads = reads(&mut chain) - before;
        writeln!(
            std::io::stdout(),
            "format={format:?} slots=8 polls=10000 ms={:.3} first_slot_tail_reads={native_reads}",
            elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}
