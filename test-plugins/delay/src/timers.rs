//! The CLAP side of the `timers` variant: two timers of one millisecond, registered as the plugin
//! is created. The first one's first tick removes the second, so a host that calls a removed timer
//! shows it in `Shared::removed_timer_calls`.

use std::cell::Cell;
use std::sync::atomic::Ordering;

use clack_extensions::timer::{HostTimer, PluginTimerImpl, TimerId};
use clack_plugin::prelude::*;

use crate::clap::MainThread;

pub struct Timers {
    first: TimerId,
    /// The second timer, until the first one removes it.
    second: Cell<Option<TimerId>>,
    removed: Cell<Option<TimerId>>,
}

impl Timers {
    pub fn register(host: &HostMainThreadHandle) -> Option<Timers> {
        let timer = host.get_extension::<HostTimer>()?;
        let first = timer.register_timer(host, 1).ok()?;
        let second = timer.register_timer(host, 1).ok()?;
        Some(Timers {
            first,
            second: Cell::new(Some(second)),
            removed: Cell::new(None),
        })
    }
}

impl PluginTimerImpl for MainThread<'_> {
    fn on_timer(&self, timer_id: TimerId) {
        let Some(timers) = &self.timers else {
            return;
        };
        if timer_id == timers.first
            && let (Some(second), Some(timer)) =
                (timers.second.take(), self.host.get_extension::<HostTimer>())
        {
            let _ = timer.unregister_timer(&self.host, second);
            timers.removed.set(Some(second));
        } else if Some(timer_id) == timers.removed.get() {
            self.shared
                .removed_timer_calls
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}
