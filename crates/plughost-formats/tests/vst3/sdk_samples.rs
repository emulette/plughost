//! SDK examples that take the optional factory and controller paths.
use super::*;
use plughost_core::StatePurpose;

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn classes_described_only_in_unicode_are_listed() {
    let _serial = serial();
    let classes = module("utf16-name").classes();
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].class_id, "04F0CA5C1A1650F69F2EE4F3C216D8B5");
    assert!(
        classes[0].name.starts_with("UTF16Name öüäéèê-やあ-"),
        "{}",
        classes[0].name
    );
    assert!(classes[0].vendor.contains("やあ"), "{}", classes[0].vendor);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn parameter_groups_end_at_a_root_unit_the_plugin_leaves_implicit() {
    let _serial = serial();
    // AGain lists only Unit1, a child of the root, and puts its gain there; AGainSimple lists
    // no units at all.
    let mut again = plugin("again");
    let gain = again.parameter_details(AGAIN_GAIN).unwrap().groups.unwrap();
    assert_eq!(
        gain.iter()
            .map(|group| (group.id.as_str(), group.name.as_str()))
            .collect::<Vec<_>>(),
        [("1", "Unit1")]
    );
    assert_eq!(
        again.parameter_details(AGAIN_BYPASS).unwrap().groups,
        Some(Vec::new())
    );
    let mut simple = plugin("again-simple");
    assert_eq!(
        simple.parameter_details(AGAIN_GAIN).unwrap().groups,
        Some(Vec::new())
    );
    let bypass = simple.parameter_choices(AGAIN_BYPASS, 0, 2).unwrap();
    assert_eq!(bypass.total, 2);
}

#[test]
#[ignore = "needs scripts/build-test-plugins.ps1 or .sh"]
fn state_restores_when_the_controller_is_the_component() {
    let _serial = serial();
    let mut original = plugin("again-simple");
    original
        .prepare(&config(48_000.0, SampleFormat::F32))
        .unwrap();
    original.set_parameter(AGAIN_GAIN, 0.25).unwrap();
    let saved = original.save_state(StatePurpose::Project).unwrap();

    let mut restored = plugin("again-simple");
    restored
        .restore_state(&saved, StatePurpose::Project)
        .unwrap();
    assert!((restored.parameter_value(AGAIN_GAIN) - 0.25).abs() < 1e-6);
}
