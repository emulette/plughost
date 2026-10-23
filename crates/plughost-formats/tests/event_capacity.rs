//! Native processing rejects over-budget input before output or queued edits are consumed.
#![cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
use plughost_core::PluginRef;
use plughost_core::{
    BlockContext, Event, Events, HostIdentity, Layout, MAX_BLOCK_EVENTS, ParameterChange,
    PluginFormat, ProcessConfig, SampleFormat,
};
#[cfg(feature = "clap")]
use plughost_formats::HostedPlugin;
#[cfg(any(feature = "vst3", feature = "clap"))]
use std::path::Path;

fn check(reference: PluginRef) {
    let mut plugin = plughost_formats::load(&reference, &HostIdentity::default()).unwrap();
    plugin
        .prepare(&ProcessConfig::new(
            48_000.0,
            8,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
        .unwrap();
    let dry = if reference.id().format() == PluginFormat::AudioUnit {
        0.0
    } else {
        1.0
    };
    plugin.set_parameter(0, dry).unwrap();
    let processor = plugin.processor();
    let point = ParameterChange::new(0, 0, dry);
    let automation = vec![point; MAX_BLOCK_EVENTS + 1];
    let notes = vec![Event::note_off(0, 0, 60, 0); MAX_BLOCK_EVENTS + 1];
    let (mut left, mut right) = ([99.0f32; 8], [99.0f32; 8]);
    let input = [0.25f32; 8];
    for (changes, events) in [
        (automation.as_slice(), Events::new()),
        (&[][..], Events::from_iter(notes.clone())),
        (
            &automation[..MAX_BLOCK_EVENTS],
            Events::from_iter([notes[0]]),
        ),
    ] {
        let error = processor
            .process_audio_f32(
                &BlockContext::new(8),
                &[&input, &input],
                &mut [&mut left, &mut right],
                changes,
                &events,
                &mut Events::for_block(),
            )
            .unwrap_err();
        assert_eq!(
            error.failure().kind,
            plughost_core::FailureKind::InvalidInput
        );
        assert_eq!(left, [99.0; 8]);
        assert_eq!(right, [99.0; 8]);
        assert!(!processor.restart_required());
    }
    // The exact limit remains accepted, and previously pending controls still reach this block.
    processor
        .process_audio_f32(
            &BlockContext::new(8),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &automation[..MAX_BLOCK_EVENTS],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(left, input);
    assert_eq!(right, input);
}

#[cfg(any(feature = "vst3", feature = "clap"))]
#[test]
#[ignore = "needs routing VST3 and CLAP fixtures"]
fn native_effects_reject_combined_event_overflow_without_touching_output() {
    let check_native = |format: PluginFormat, extension: &str, class: &str| {
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-plugins/plughost-test-routing.{extension}"
        ));
        check(match format {
            PluginFormat::Vst3 => PluginRef::vst3(bundle, class),
            _ => PluginRef::clap(bundle, class),
        });
    };
    #[cfg(feature = "vst3")]
    check_native(
        PluginFormat::Vst3,
        "vst3",
        "706C7567686F7374526F7574696E6701",
    );
    #[cfg(feature = "clap")]
    check_native(
        PluginFormat::Clap,
        "clap",
        "com.studio.plughost.test-routing",
    );
}

#[cfg(all(target_os = "macos", feature = "au"))]
#[test]
fn apple_effect_rejects_combined_event_overflow_without_touching_output() {
    check(PluginRef::audio_unit("6175667864656C796170706C"));
}

#[cfg(feature = "clap")]
#[test]
#[ignore = "needs routing CLAP fixture"]
fn clap_pending_edits_are_bounded_and_survive_a_rejected_audio_block() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins/plughost-test-routing.clap");
    let mut plugin = plughost_formats::clap::Plugin::new(
        &path,
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap();
    plugin
        .prepare(&ProcessConfig::new(
            48_000.0,
            8,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
        .unwrap();
    for _ in 0..MAX_BLOCK_EVENTS {
        plugin.set_parameter(0, 0.25).unwrap();
    }
    assert_eq!(
        plugin.set_parameter(0, 0.75).unwrap_err().input_error(),
        Some(plughost_core::InputError::EventCapacity)
    );
    let processor = plugin.processor();
    let input = [1.0f32; 8];
    let (mut left, mut right) = ([99.0; 8], [99.0; 8]);
    let notes = Events::from_iter([Event::note_off(0, 0, 60, 0); MAX_BLOCK_EVENTS + 1]);
    assert!(
        processor
            .process_audio_f32(
                &BlockContext::new(8),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &notes,
                &mut Events::for_block(),
            )
            .is_err()
    );
    assert_eq!(left, [99.0; 8]);
    processor
        .process_audio_f32(
            &BlockContext::new(8),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(left, [0.25; 8]);
    assert_eq!(right, [0.25; 8]);
    plugin.set_parameter(0, 0.75).unwrap();
    processor
        .process_audio_f32(
            &BlockContext::new(8),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(left, [0.75; 8]);
}

#[cfg(feature = "clap")]
#[test]
#[ignore = "needs routing CLAP fixture"]
fn clap_pending_edits_survive_a_block_the_plugin_fails() {
    // The routing-faults bundle fails the block that brings it this message, before it takes the
    // block's edits.
    const FAIL: [u8; 4] = [0xF0, 0x7D, 0x7B, 0xF7];
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-plugins/plughost-test-routing-faults.clap");
    let mut plugin = plughost_formats::clap::Plugin::new(
        &path,
        "com.studio.plughost.test-routing",
        &HostIdentity::default(),
    )
    .unwrap();
    plugin
        .prepare(&ProcessConfig::new(
            48_000.0,
            8,
            SampleFormat::F32,
            Layout::Stereo,
            Layout::Stereo,
        ))
        .unwrap();
    plugin.set_parameter(0, 0.25).unwrap();
    let processor = plugin.processor();
    let input = [1.0f32; 8];
    let (mut left, mut right) = ([99.0; 8], [99.0; 8]);
    let error = processor
        .process_audio_f32(
            &BlockContext::new(8),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::from_iter([Event::sysex(0, &FAIL)]),
            &mut Events::for_block(),
        )
        .unwrap_err();
    assert_eq!(error.failure().kind, plughost_core::FailureKind::Processing);
    // The next block brings the edit again.
    processor
        .process_audio_f32(
            &BlockContext::new(8),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    assert_eq!(left, [0.25; 8]);
}
