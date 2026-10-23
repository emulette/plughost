//! What CLAP hosting makes of events and parameters a plugin gives against the rules, with the
//! `plughost-test-routing-faults` bundle of the routing fixture, which gives them.
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
/// The parameter whose range is the whole real line.
const UNBOUNDED: u64 = 2;

fn faults() -> Plugin {
    Plugin::new(
        // SAFETY: the tests load only the repository's fixture and SDK sample plugins.
        &unsafe {
            plughost_formats::clap::Entry::load(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/test-plugins/plughost-test-routing-faults.clap"),
            )
        }
        .unwrap(),
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap()
}

fn prepared() -> Plugin {
    let mut plugin = faults();
    plugin
        .prepare(&ProcessConfig::new(
            48_000.0,
            FRAMES,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
        .unwrap();
    plugin
}

/// Processes one block of `events` and returns the events the plugin produced.
fn block(plugin: &Plugin, events: &Events) -> Result<Events, Error> {
    let input = [0.0f32; FRAMES];
    let (mut left, mut right) = ([0.0f32; FRAMES], [0.0f32; FRAMES]);
    let mut produced = Events::for_block();
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
    let produced = block(&plugin, &Events::from_iter([Event::sysex(0, &MALFORMED)])).unwrap();
    assert_eq!(produced, []);
    assert_eq!(plugin.take_parameter_events().events, []);
    // Both count as events with no portable form.
    let diagnostics = plugin.take_diagnostics().records;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].message.ends_with(" 2"), "{diagnostics:?}");
}

#[test]
#[ignore = "needs routing CLAP fixture"]
fn a_parameter_whose_range_is_not_finite_has_no_value_to_send_or_show() {
    let mut plugin = faults();
    assert_eq!(
        plugin.set_parameter(UNBOUNDED, 0.5).unwrap_err().kind(),
        FailureKind::Host
    );
    let (info, value) = plugin
        .parameters()
        .into_iter()
        .find(|(info, _)| info.id == UNBOUNDED)
        .unwrap();
    assert_eq!((info.default_value, value), (None, None));
}
