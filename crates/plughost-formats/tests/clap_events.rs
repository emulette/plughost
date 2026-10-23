//! What CLAP hosting makes of events a plugin sends against the rules, with the
//! `plughost-test-routing-faults` bundle of the routing fixture, which sends them on request.
//! Build it with `scripts/build-test-plugins.sh`, then run
//! `cargo test -p plughost-formats -- --ignored`.
#![cfg(feature = "clap")]
use plughost_core::*;
use plughost_formats::clap::Plugin;
use plughost_formats::{Error, HostedPlugin};
use std::path::Path;

/// Makes the fixture send a note on and a parameter value whose headers give only the header's
/// size.
const MALFORMED: [u8; 4] = [0xF0, 0x7D, 0x7C, 0xF7];
const FRAMES: usize = 32;

fn prepared() -> Plugin {
    let mut plugin = Plugin::new(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-plugins/plughost-test-routing-faults.clap"),
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap();
    plugin
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: FRAMES,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    plugin
}

/// Processes one block of `events` and returns the events the plugin produced.
fn block(plugin: &Plugin, events: &[Event]) -> Result<Vec<Event>, Error> {
    let input = [0.0f32; FRAMES];
    let (mut left, mut right) = ([0.0f32; FRAMES], [0.0f32; FRAMES]);
    let mut produced = Vec::new();
    plugin.processor().process_audio_f32(
        &BlockContext::new(FRAMES),
        &[&input, &input],
        &mut [&mut left, &mut right],
        &[],
        events,
        &mut produced,
    )?;
    Ok(produced)
}

#[test]
#[ignore = "needs routing CLAP fixture"]
fn events_whose_header_gives_another_size_than_their_type_are_not_read() {
    let mut plugin = prepared();
    plugin.take_diagnostics();
    plugin.take_parameter_events();
    let produced = block(&plugin, &[Event::sysex(0, MALFORMED.to_vec())]).unwrap();
    assert_eq!(produced, []);
    assert_eq!(plugin.take_parameter_events().events, []);
    // Both count as events with no portable form.
    let diagnostics = plugin.take_diagnostics().records;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].message.ends_with(" 2"), "{diagnostics:?}");
}
