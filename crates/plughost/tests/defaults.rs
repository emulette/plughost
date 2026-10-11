use plughost::Timeouts;
use std::time::Duration;

#[test]
fn default_operation_deadlines_allow_native_loading_and_bound_control_and_processing() {
    let timeouts = Timeouts::default();
    assert_eq!(timeouts.load, Duration::from_secs(300));
    assert_eq!(timeouts.control, Duration::from_secs(60));
    assert_eq!(timeouts.process, Duration::from_secs(10));
}
