use plughost::{HostIdentity, PluginFormat, StatePurpose, Timeouts};
use std::time::Duration;

mod support;

use support::{block, delay, prepare, spawn_with};

#[test]
fn default_operation_deadlines_allow_native_loading_and_bound_control_and_processing() {
    let timeouts = Timeouts::default();
    assert_eq!(timeouts.load, Duration::from_secs(300));
    assert_eq!(timeouts.control, Duration::from_secs(60));
    assert_eq!(timeouts.process, Duration::from_secs(10));
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn timeouts_too_long_for_a_deadline_wait_without_one() {
    let mut timeouts = Timeouts::default();
    timeouts.load = Duration::MAX;
    timeouts.control = Duration::MAX;
    timeouts.process = Duration::MAX;
    let mut chain = spawn_with(
        &[delay(PluginFormat::Vst3)],
        &HostIdentity::default(),
        timeouts,
    );
    prepare(&mut chain);
    block(&mut chain, 64, 0.5);
    chain.take_changes().unwrap();
    chain.save_state(0, StatePurpose::Project).unwrap();
}
