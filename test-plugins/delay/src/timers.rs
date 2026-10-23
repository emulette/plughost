//! The CLAP side of the `timers` variant: two timers of one millisecond, registered as the plugin
//! is created. The first one's first tick removes the second, so a host that calls a removed timer
//! shows it in `Shared::removed_timer_calls`. As it registers them, the plugin also registers and
//! unregisters a timer from another thread, which CLAP forbids, and counts the calls the host
//! accepted in `Timers::accepted_off_main_thread`.

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use clack_extensions::timer::{HostTimer, PluginTimerImpl, TimerId};
use clack_plugin::prelude::*;

use crate::clap::MainThread;

pub struct Timers {
    first: TimerId,
    /// The second timer, until the first one removes it.
    second: Cell<Option<TimerId>>,
    removed: Cell<Option<TimerId>>,
    pub(crate) accepted_off_main_thread: u32,
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
            accepted_off_main_thread: off_main_thread(host),
        })
    }
}

/// Registers a timer from another thread and unregisters it, or one that does not exist, and
/// returns how many of the two calls the host accepted.
fn off_main_thread(host: &HostMainThreadHandle) -> u32 {
    let address = NonNull::from(host.as_raw()).as_ptr() as usize;
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                // SAFETY: the host outlives the thread, which this call waits for; calling the
                // main-thread functions from it is the fault.
                let host = unsafe {
                    HostMainThreadHandle::from_raw(NonNull::new(address as *mut _).unwrap())
                };
                let Some(timer) = host.get_extension::<HostTimer>() else {
                    return 0;
                };
                let registered = timer.register_timer(&host, 1).ok();
                // The host numbers its timers from 1.
                let unregistered = timer
                    .unregister_timer(&host, registered.unwrap_or(TimerId(0)))
                    .is_ok();
                u32::from(registered.is_some()) + u32::from(unregistered)
            })
            .join()
            .unwrap()
    })
}

impl PluginTimerImpl for MainThread<'_> {
    fn on_timer(&self, timer_id: TimerId) {
        let Some(timers) = &self.timers else {
            return;
        };
        if timer_id == timers.first {
            self.shared.timer_calls.fetch_add(1, Ordering::Relaxed);
            if let (Some(second), Some(timer)) =
                (timers.second.take(), self.host.get_extension::<HostTimer>())
            {
                let _ = timer.unregister_timer(&self.host, second);
                timers.removed.set(Some(second));
            }
        } else if Some(timer_id) == timers.removed.get() {
            self.shared
                .removed_timer_calls
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}
