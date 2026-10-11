use super::*;

#[test]
fn interactive_retry_uses_twenty_four_hours_independently_of_the_normal_deadline() {
    for configured in [
        Duration::ZERO,
        Duration::from_secs(30),
        Duration::from_secs(172_800),
    ] {
        assert_eq!(
            ScanMode::Retry.stall_timeout(configured),
            Duration::from_secs(86_400)
        );
        assert_eq!(ScanMode::Native.stall_timeout(configured), configured);
    }
}
