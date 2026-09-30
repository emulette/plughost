use plughost::*;

mod support;

use support::{block, delay as reference, prepared_delay as chain, spawn};

#[test]
#[ignore = "needs helper and test plugins"]
fn native_metadata_and_helper_metadata_preserve_ranges_defaults_and_groups() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut native =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        let mut chain = chain(format);
        let details = native.parameter_details(0).unwrap();
        assert_eq!(chain.parameter_details(0, 0).unwrap(), details);
        assert_eq!(details.info.default_value, Some(1.0));
        assert_eq!(details.plain_at_zero, 0.0);
        assert_eq!(
            details.plain_at_one,
            if format == PluginFormat::Vst3 {
                100.0
            } else {
                1.0
            }
        );
        assert!(!details.info.flags.discrete);
        if format == PluginFormat::Vst3 {
            assert_eq!(details.info.units, "units");
            assert_eq!(chain.parameter_to_plain(0, 0, 0.5).unwrap(), 25.0);
        }
        chain.set_parameter(0, 0, 0.25).unwrap();
        let before = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        assert_eq!(
            chain.parameter_details(0, 0).unwrap().info.default_value,
            Some(1.0)
        );
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            before
        );
        assert_eq!(&block(&mut chain, 512, 1.0)[480..], &[0.25; 32]);
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn paged_native_choice_labels_do_not_edit_parameters() {
    for (format, id, names) in [
        (PluginFormat::Vst3, 2, ["Volume", "Expression"]),
        (PluginFormat::Clap, 1, ["Normal", "Double"]),
    ] {
        let mut native =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        let mut chain = chain(format);
        let original = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        for start in 0..2 {
            let page = chain.parameter_choices(0, id, start, 1).unwrap();
            assert_eq!(native.parameter_choices(id, start, 1).unwrap(), page);
            assert_eq!(page.total, 2);
            assert_eq!(
                page.choices,
                vec![ParameterChoice {
                    index: start,
                    normalized: start as f64,
                    plain: start as f64,
                    text: names[start as usize].into()
                }]
            );
        }
        assert!(
            chain
                .parameter_choices(0, id, 2, 1)
                .unwrap()
                .choices
                .is_empty()
        );
        for (start, count) in [(3, 1), (0, 0), (0, 257), (u64::MAX, 1)] {
            assert_eq!(
                chain
                    .parameter_choices(0, id, start, count)
                    .unwrap_err()
                    .kind(),
                FailureKind::InvalidInput
            );
            assert_eq!(
                native
                    .parameter_choices(id, start, count)
                    .unwrap_err()
                    .kind(),
                FailureKind::InvalidInput
            );
        }
        assert_eq!(
            chain.parameter_choices(0, 0, 0, 1).unwrap_err().kind(),
            FailureKind::Unsupported
        );
        assert_eq!(
            chain.parameter_details(0, u64::MAX).unwrap_err().kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            chain.parameter_details(5, 0).unwrap_err().kind(),
            FailureKind::InvalidInput
        );
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            original
        );
    }
}

#[test]
#[ignore = "needs helper and CLAP test plugin"]
fn constant_discrete_and_large_choice_lists_are_bounded_without_losing_values() {
    let mut chain = chain(PluginFormat::Clap);
    let details = chain.parameter_details(0, 13).unwrap();
    assert!(details.info.flags.discrete && details.info.flags.read_only && details.info.flags.list);
    assert_eq!(details.plain_at_zero, 5.0);
    assert_eq!(details.plain_at_one, 5.0);
    assert_eq!(details.info.default_value, Some(0.0));
    assert_eq!(details.groups, Some(vec![]));
    let page = chain.parameter_choices(0, 13, 0, 256).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(
        page.choices,
        vec![ParameterChoice {
            index: 0,
            normalized: 0.0,
            plain: 5.0,
            text: "5.000".into()
        }]
    );
    let mut collected = Vec::new();
    for start in [0, 256, 512, 768] {
        let page = chain.parameter_choices(0, 14, start, 256).unwrap();
        assert_eq!(page.total, 1000);
        assert!(page.choices.len() <= 256);
        collected.extend(page.choices);
    }
    assert_eq!(collected.len(), 1000);
    for (index, choice) in collected.iter().enumerate() {
        assert_eq!(choice.index, index as u64);
        assert_eq!(choice.plain, index as f64);
        assert_eq!(choice.text, format!("Item {index}"));
        assert_eq!(choice.normalized, index as f64 / 999.0);
    }
}

#[test]
#[ignore = "needs helper and CLAP test plugin"]
fn clap_metadata_refresh_reads_the_new_native_range_and_default() {
    let mut native =
        plughost_formats::load(&reference(PluginFormat::Clap), &HostIdentity::default()).unwrap();
    assert_eq!(native.parameter_details(0).unwrap().plain_at_one, 1.0);
    native.set_parameter(1, 1.0).unwrap();
    native.save_state(plughost::StatePurpose::Project).unwrap();
    let updated = native.parameter_details(0).unwrap();
    assert_eq!(updated.plain_at_one, 2.0);
    assert_eq!(updated.info.default_value, Some(0.5));
    // Inactive CLAP range changes are delivered by control flush, not an audio callback.
    let mut isolated = spawn(&[reference(PluginFormat::Clap)]);
    isolated.set_parameter(0, 1, 1.0).unwrap();
    isolated
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    assert_eq!(isolated.parameter_details(0, 0).unwrap(), updated);
}
