use std::io::Write;
use std::time::Instant;

use plughost::{PluginFormat, Tail};

mod support;

#[test]
#[ignore = "native control benchmark; run in release mode with --nocapture"]
fn control_timing_benchmark() {
    for operation in ["parameter", "text", "reset"] {
        let mut chain = support::prepared_delay(PluginFormat::Clap);
        let started = Instant::now();
        for index in 0..1_000 {
            match operation {
                "parameter" => chain
                    .set_parameter(0, 0, if index % 2 == 0 { 0.25 } else { 0.75 })
                    .unwrap(),
                "text" => chain
                    .set_parameter_text(0, 0, if index % 2 == 0 { "0.25" } else { "0.75" })
                    .unwrap(),
                _ => chain.reset().unwrap(),
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(chain.latency(), 480);
        assert_eq!(chain.tail(), Tail::Samples(0));
        let expected = if operation == "reset" { 1.0 } else { 0.75 };
        assert_eq!(chain.parameters(0).unwrap()[0].1, expected);
        writeln!(
            std::io::stdout(),
            "operation={operation} requests=1000 ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}
