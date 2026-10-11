//! Periodic native timing reads have a cadence and never wait for a busy processing engine.
use plughost_core::{PluginFormat, PluginTiming};
use std::time::{Duration, Instant};

use super::{Caller, Host};

const INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Default)]
pub(super) struct Poll {
    next: Option<Instant>,
}

impl Poll {
    pub(super) fn retry(&mut self) {
        self.next = None;
    }

    fn run(
        &mut self,
        now: Instant,
        format: PluginFormat,
        read: impl FnOnce() -> Option<PluginTiming>,
        submit: impl FnOnce(PluginTiming) -> bool,
    ) {
        if format != PluginFormat::Clap && self.next.is_some_and(|next| now < next) {
            return;
        }
        if let Some(timing) = read()
            && submit(timing)
        {
            self.next = Some(now + INTERVAL);
        }
    }
}

impl Host {
    pub(super) fn observe_timing(&mut self) {
        if self.audio_config.is_none() {
            return;
        }
        let now = Instant::now();
        for (slot, (plugin, poll)) in self.slots.iter().zip(&mut self.timing_polls).enumerate() {
            poll.run(
                now,
                plugin.info().format,
                || {
                    let _call = self.calls.enter(Caller::Main, slot);
                    plugin.poll_timing()
                },
                |timing| self.processing.observe(slot, timing),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plughost_core::render::Tail;
    use std::cell::Cell;

    #[test]
    fn periodic_reads_have_a_cadence_but_clap_notifications_are_checked_each_tick() {
        for format in [
            PluginFormat::Vst3,
            PluginFormat::AudioUnit,
            PluginFormat::Clap,
        ] {
            let mut poll = Poll::default();
            let start = Instant::now();
            let reads = Cell::new(0);
            for ms in 0..=100 {
                poll.run(
                    start + Duration::from_millis(ms),
                    format,
                    || {
                        reads.set(reads.get() + 1);
                        Some(PluginTiming::new(0, Tail::Samples(0)))
                    },
                    |_| true,
                );
            }
            assert_eq!(
                reads.get(),
                if format == PluginFormat::Clap { 101 } else { 2 }
            );
        }
    }

    #[test]
    fn busy_native_reads_and_full_worker_queues_are_retried_on_the_next_tick() {
        let mut poll = Poll::default();
        let start = Instant::now();
        let reads = Cell::new(0);
        let submissions = Cell::new(0);
        for ms in [0, 10, 20, 30, 120] {
            poll.run(
                start + Duration::from_millis(ms),
                PluginFormat::Vst3,
                || {
                    reads.set(reads.get() + 1);
                    (ms != 0).then(|| PluginTiming::new(0, Tail::Samples(0)))
                },
                |_| {
                    submissions.set(submissions.get() + 1);
                    ms != 10
                },
            );
        }
        assert_eq!(reads.get(), 4);
        assert_eq!(submissions.get(), 3);
    }

    #[test]
    fn an_unpublished_explicit_read_retries_before_the_periodic_deadline() {
        let mut poll = Poll::default();
        let now = Instant::now();
        let timing = PluginTiming::new(0, Tail::Samples(0));
        poll.run(now, PluginFormat::Vst3, || Some(timing), |_| true);
        poll.retry();
        let submitted = Cell::new(false);
        poll.run(
            now,
            PluginFormat::Vst3,
            || Some(timing),
            |_| {
                submitted.set(true);
                true
            },
        );
        assert!(submitted.get());
    }
}
