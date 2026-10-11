//! The same owning-thread API can be moved or shared on both supported platforms.
use plughost::{Chain, Error, Scanner};

#[test]
fn public_owners_and_errors_are_send_and_sync() {
    fn require<T: Send + Sync>() {}
    require::<Chain>();
    require::<Scanner>();
    require::<Error>();
}
