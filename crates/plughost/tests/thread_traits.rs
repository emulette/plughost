//! The same owning-thread API can be moved or shared on both supported platforms.
use plughost::{Chain, ChainCanceller, Error, HelperMonitor, RealtimeEndpoint, Scanner};

fn require<T: Send + Sync>() {}

#[test]
fn public_owners_and_errors_are_send_and_sync() {
    require::<Chain>();
    require::<Scanner>();
    require::<Error>();
}

#[test]
fn handles_used_from_other_threads_can_go_there() {
    // The endpoint moves to the audio callback's thread, which alone calls it.
    fn movable<T: Send>() {}
    movable::<RealtimeEndpoint>();
    // Cancellers and monitors are cloned to control threads and called there.
    require::<ChainCanceller>();
    require::<HelperMonitor>();
}
