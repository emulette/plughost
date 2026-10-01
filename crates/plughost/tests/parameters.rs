use plughost::*;

mod support;

use support::{block, delay as reference, prepared_delay as chain};

#[test]
#[ignore = "needs helper and test plugins"]
fn native_and_helper_parameter_conversions_reach_output_and_saved_state() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut native =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        let mut chain = chain(format);
        let expected_plain = if format == PluginFormat::Vst3 {
            25.0
        } else {
            0.5
        };
        let text = chain.parameter_text(0, 0, 0.5).unwrap();
        assert_eq!(native.parameter_text(0, 0.5).unwrap(), text);
        assert_eq!(chain.parameter_to_plain(0, 0, 0.5).unwrap(), expected_plain);
        assert_eq!(native.parameter_to_plain(0, 0.5).unwrap(), expected_plain);
        assert_eq!(
            chain.parameter_to_normalized(0, 0, expected_plain).unwrap(),
            0.5
        );
        assert_eq!(
            native.parameter_to_normalized(0, expected_plain).unwrap(),
            0.5
        );
        assert_eq!(native.parameter_from_text(0, &text).unwrap(), 0.5);
        assert_eq!(chain.parameter_from_text(0, 0, &text).unwrap(), 0.5);
        chain.set_parameter_text(0, 0, &text).unwrap();
        let state = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        chain = chain.recover(&[]).unwrap();
        chain
            .restore_state(0, &state, plughost::StatePurpose::Project)
            .unwrap();
        let input = [1.0f32; 700];
        let rendered = render(
            &mut chain,
            &[&input, &input],
            input.len(),
            &[],
            &RenderOptions::new(TailPolicy::Reported, 0.0),
        )
        .unwrap();
        assert!(rendered.channels.iter().flatten().all(|v| *v == 0.5));
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn invalid_parameter_text_and_values_preserve_state() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut native =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        let mut chain = chain(format);
        chain.set_parameter(0, 0, 0.75).unwrap();
        let state = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        for text in ["not a value", "0.2\0ignored", &"x".repeat(4097)] {
            assert_eq!(
                chain.set_parameter_text(0, 0, text).unwrap_err().kind(),
                FailureKind::InvalidInput
            );
            assert_eq!(
                native.parameter_from_text(0, text).unwrap_err().kind(),
                FailureKind::InvalidInput
            );
        }
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert_eq!(
                chain.parameter_text(0, 0, value).unwrap_err().kind(),
                FailureKind::InvalidInput
            );
            assert_eq!(
                chain.parameter_to_plain(0, 0, value).unwrap_err().kind(),
                FailureKind::InvalidInput
            );
        }
        assert_eq!(
            chain
                .parameter_to_normalized(0, 0, f64::NAN)
                .unwrap_err()
                .kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            chain.parameter_text(0, u64::MAX, 0.5).unwrap_err().kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            chain.parameter_text(2, 0, 0.5).unwrap_err().kind(),
            FailureKind::InvalidInput
        );
        if format == PluginFormat::Vst3 {
            assert_eq!(
                chain.parameter_text(0, 3, 0.5).unwrap_err().kind(),
                FailureKind::Unsupported
            );
        }
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            state
        );
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn rejected_partial_state_preserves_pending_edits_and_dsp_history() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut candidate = chain(format);
        let mut control = chain(format);
        let mut broken = candidate
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        broken.component = (-1.0f64).to_le_bytes().to_vec();
        // Prove this payload reaches a native loader that mutates before returning an error.
        let mut native =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        assert_eq!(
            native
                .restore_state(&broken, plughost::StatePurpose::Project)
                .unwrap_err()
                .kind(),
            FailureKind::State
        );
        assert_eq!(
            native
                .save_state(plughost::StatePurpose::Project)
                .unwrap()
                .component,
            broken.component
        );
        for c in [&mut candidate, &mut control] {
            c.set_parameter(0, 0, 0.75).unwrap();
            assert_eq!(block(c, 101, 1.0), vec![0.0; 101]);
            c.set_parameter(0, 0, 0.25).unwrap();
        }
        assert_eq!(
            candidate
                .restore_state(0, &broken, plughost::StatePurpose::Project)
                .unwrap_err()
                .kind(),
            FailureKind::State
        );
        assert_eq!(
            block(&mut candidate, 512, 0.0),
            block(&mut control, 512, 0.0)
        );
        assert_eq!(
            candidate
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            control
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap()
        );
        let good = control
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        let mut wrong = good.clone();
        wrong.class_id = "wrong class".into();
        assert_eq!(
            candidate
                .restore_state(0, &wrong, plughost::StatePurpose::Project)
                .unwrap_err()
                .kind(),
            FailureKind::StateMismatch
        );
        assert_eq!(
            candidate
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            good
        );
        if format == PluginFormat::Vst3 {
            // A rejection by the edit controller is not reported as a successful restore either.
            let mut rejected = good.clone();
            rejected.component = 0.75f64.to_le_bytes().to_vec();
            rejected.controller = (-1.0f64).to_le_bytes().to_vec();
            assert_eq!(
                candidate
                    .restore_state(0, &rejected, plughost::StatePurpose::Project)
                    .unwrap_err()
                    .kind(),
                FailureKind::State
            );
            assert_eq!(
                candidate
                    .save_state(0, plughost::StatePurpose::Project)
                    .unwrap(),
                good
            );
        }
        candidate
            .restore_state(0, &good, plughost::StatePurpose::Project)
            .unwrap();
        assert_eq!(
            candidate
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            good
        );
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn candidate_prepare_failure_preserves_original_output() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = chain(format);
        chain.set_parameter(0, 0, 0.25).unwrap();
        let original = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        let mut rejected = original.clone();
        rejected.component = 42.0f64.to_le_bytes().to_vec();
        assert_eq!(
            chain
                .restore_state(0, &rejected, plughost::StatePurpose::Project)
                .unwrap_err()
                .kind(),
            FailureKind::Configuration
        );
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            original
        );
        let output = block(&mut chain, 512, 1.0);
        assert_eq!(&output[480..], &[0.25; 32]);
    }
}
