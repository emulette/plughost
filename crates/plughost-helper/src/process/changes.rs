//! The worker's notifications and the latest observation admitted to its queue.
use std::sync::{Arc, Mutex};

use plughost_core::{ChangeBatch, ChangeBuffer, PluginTiming};

pub(super) type Observations = Arc<Mutex<Vec<Option<PluginTiming>>>>;

#[derive(Default)]
pub(crate) struct Changes {
    buffer: ChangeBuffer,
    current: Vec<Option<PluginTiming>>,
    pub(super) observations: Observations,
}

impl Changes {
    pub(super) fn new(observations: Observations) -> Self {
        Self {
            buffer: ChangeBuffer::default(),
            current: Vec::new(),
            observations,
        }
    }
    pub fn observe(&mut self, slot: usize, timing: PluginTiming) {
        if self.current.get(slot) == Some(&Some(timing)) {
            return;
        }
        publish(&mut self.current, slot, timing);
        self.buffer.observe(slot, timing);
        let mut observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        publish(&mut observations, slot, timing);
    }

    pub fn take(&mut self) -> ChangeBatch {
        self.buffer.take()
    }
}

pub(super) fn publish(
    observations: &mut Vec<Option<PluginTiming>>,
    slot: usize,
    timing: PluginTiming,
) {
    if observations.len() <= slot {
        observations.resize(slot + 1, None);
    }
    observations[slot] = Some(timing);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Command, Pipeline, Processing, Work};
    use plughost_core::render::Tail;
    use std::sync::mpsc::{self, Receiver};

    fn setup() -> (Processing, Receiver<Work>, Changes) {
        let (work, queue) = mpsc::sync_channel(1);
        let (_, returned) = mpsc::sync_channel(1);
        let observations = Observations::default();
        (
            Processing {
                work,
                bell: Mutex::new(None),
                returned: Mutex::new(returned),
                thread: std::thread::current().id(),
                observations: Arc::clone(&observations),
            },
            queue,
            Changes::new(observations),
        )
    }

    fn timing(tail: u32) -> PluginTiming {
        PluginTiming::new(0, Tail::Samples(tail))
    }

    fn receive(queue: &Receiver<Work>, changes: &mut Changes, expected: PluginTiming) {
        match queue.try_recv().unwrap() {
            Work::Observe(slot, timing) => {
                assert_eq!((slot, timing), (0, expected));
                changes.observe(slot, timing);
            }
            _ => panic!("expected a timing observation"),
        }
    }

    #[test]
    fn unchanged_observations_do_not_wake_the_worker_but_native_changes_are_not_hidden() {
        let (processing, queue, mut changes) = setup();
        changes.observe(0, timing(1));
        for _ in 0..10 {
            assert!(processing.observe(0, timing(1)));
            assert!(queue.try_recv().is_err());
        }
        // The DSP thread saw a change between main-thread polls. Returning to the main
        // thread's previous value must still replace the worker's newer snapshot.
        changes.observe(0, timing(2));
        assert!(processing.observe(0, timing(1)));
        receive(&queue, &mut changes, timing(1));
        assert_eq!(changes.take().snapshot, [Some(timing(1))]);
    }

    #[test]
    fn a_full_queue_or_busy_cache_keeps_the_observation_retryable() {
        let (processing, queue, mut changes) = setup();
        changes.observe(0, timing(1));
        assert!(processing.observe(0, timing(2)));
        assert!(processing.observe(0, timing(2))); // Already queued; no second message.
        assert!(!processing.observe(0, timing(3)));
        receive(&queue, &mut changes, timing(2));
        assert!(queue.try_recv().is_err());
        let guard = processing.observations.lock().unwrap();
        assert!(!processing.observe(0, timing(3)));
        drop(guard);
        assert!(processing.observe(0, timing(3)));
        receive(&queue, &mut changes, timing(3));
        assert_eq!(changes.take().snapshot, [Some(timing(3))]);
    }

    #[test]
    fn loading_a_new_chain_discards_the_previous_timing_cache() {
        let (processing, queue, changes) = setup();
        let mut pipeline = Pipeline {
            changes,
            ..Default::default()
        };
        pipeline.changes.observe(0, timing(1));
        pipeline.apply(&mut Command::Load(Box::default()));
        assert!(processing.observe(0, timing(1)));
        receive(&queue, &mut pipeline.changes, timing(1));
        assert_eq!(pipeline.changes.take().changes.len(), 1);
    }
}
