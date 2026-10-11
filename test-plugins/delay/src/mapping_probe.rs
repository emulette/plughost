//! A controller query waits for actual processing, proving the host releases its engine lock.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct State {
    queried: bool,
    waiting: bool,
    processed: bool,
}

#[derive(Default)]
pub struct Probe {
    state: Mutex<State>,
    changed: Condvar,
}

impl Probe {
    pub fn query(&self) {
        let mut state = crate::lock(&self.state);
        if state.queried {
            return;
        }
        state.queried = true;
        state.waiting = true;
        // The timeout lets a host that holds its lock finish and report a failed assertion.
        // Success comes from a process call signalling this condition, without a sleep.
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| state.waiting)
            .unwrap();
        state.waiting = false;
    }

    pub fn process(&self) {
        let mut state = crate::lock(&self.state);
        if state.waiting {
            state.processed = true;
            state.waiting = false;
            self.changed.notify_one();
        }
    }

    pub fn processed(&self) -> bool {
        crate::lock(&self.state).processed
    }
}
