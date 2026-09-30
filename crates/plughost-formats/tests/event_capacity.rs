//! Native processing rejects over-budget input before output or queued edits are consumed.
#![cfg(any(
    feature = "vst3",
    feature = "clap",
    all(feature = "au", target_os = "macos")
))]
use plughost_core::PluginRef;
use plughost_core::{
    BlockContext, HostIdentity, Layout, MAX_BLOCK_EVENTS, MidiEvent, ParameterChange, PluginFormat,
    ProcessConfig, ProcessMode, SampleFormat,
};
use plughost_formats::{Error, HostedPlugin};
#[cfg(any(feature = "vst3", feature = "clap"))]
use std::path::Path;

fn check(reference: PluginRef) {
    let mut plugin = plughost_formats::load(&reference, &HostIdentity::default()).unwrap();
    plugin
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 8,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    let dry = if reference.format == PluginFormat::AudioUnit {
        0.0
    } else {
        1.0
    };
    plugin.set_parameter(0, dry).unwrap();
    let processor = plugin.processor();
    let point = ParameterChange {
        id: 0,
        offset: 0,
        value: dry,
    };
    let automation = vec![point; MAX_BLOCK_EVENTS + 1];
    let notes = vec![MidiEvent::note_off(0, 0, 60, 0); MAX_BLOCK_EVENTS + 1];
    let (mut left, mut right) = ([99.0f32; 8], [99.0f32; 8]);
    let input = [0.25f32; 8];
    for (changes, events) in [
        (automation.as_slice(), &[][..]),
        (&[][..], notes.as_slice()),
        (&automation[..MAX_BLOCK_EVENTS], &notes[..1]),
    ] {
        let error = processor
            .process_audio_f32(
                &BlockContext::new(8),
                &[&input, &input],
                &mut [&mut left, &mut right],
                changes,
                events,
                &mut Vec::new(),
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
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left, input);
    assert_eq!(right, input);
}

#[cfg(any(feature = "vst3", feature = "clap"))]
#[test]
#[ignore = "needs routing VST3 and CLAP fixtures"]
fn native_effects_reject_combined_event_overflow_without_touching_output() {
    for (format, extension, class) in [
        #[cfg(feature = "vst3")]
        (
            PluginFormat::Vst3,
            "vst3",
            "706C7567686F7374526F7574696E6701",
        ),
        #[cfg(feature = "clap")]
        (
            PluginFormat::Clap,
            "clap",
            "com.studio.plughost.test-routing",
        ),
    ] {
        check(PluginRef {
            format,
            bundle: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../target/test-plugins/plughost-test-routing.{extension}"
            ))),
            class_id: class.into(),
        });
    }
}

#[cfg(all(target_os = "macos", feature = "au"))]
#[test]
fn apple_effect_rejects_combined_event_overflow_without_touching_output() {
    check(PluginRef {
        format: PluginFormat::AudioUnit,
        bundle: None,
        class_id: "6175667864656C796170706C".into(),
    });
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
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 8,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    for _ in 0..MAX_BLOCK_EVENTS {
        plugin.set_parameter(0, 0.25).unwrap();
    }
    assert!(matches!(
        plugin.set_parameter(0, 0.75),
        Err(Error::Clap(plughost_formats::clap::ClapError::Input(
            plughost_core::InputError::EventCapacity
        )))
    ));
    let processor = plugin.processor();
    let input = [1.0f32; 8];
    let (mut left, mut right) = ([99.0; 8], [99.0; 8]);
    let notes = vec![MidiEvent::note_off(0, 0, 60, 0); MAX_BLOCK_EVENTS + 1];
    assert!(
        processor
            .process_audio_f32(
                &BlockContext::new(8),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &notes,
                &mut Vec::new(),
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
            &[],
            &mut Vec::new(),
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
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(left, [0.75; 8]);
}
