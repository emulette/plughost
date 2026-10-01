use crate::render::Tail;
use serde::{Deserialize, Serialize};

pub const CHANGE_CAPACITY: usize = 128;

/// A plugin's timing as its owning thread last read it. The latency is the one the plugin was
/// activated with: it changes only when the plugin is prepared again or reset, so processing and
/// rendering keep one alignment in between. The tail follows the plugin's reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTiming {
    pub latency: u32,
    pub tail: Tail,
    /// The plugin asked to be prepared again; processing is refused until it is.
    pub restart_required: bool,
    /// The plugin reported a new latency, which takes effect when it is prepared again or reset.
    /// It keeps processing with `latency` until then.
    pub latency_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotChange {
    pub slot: usize,
    pub timing: PluginTiming,
}

/// Timing notifications coalesce by slot, retaining last-observed order. Intermediate values
/// are not an audit trail. On overflow use `snapshot` to replace the entire consumer view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBatch {
    pub changes: Vec<SlotChange>,
    pub resync_required: bool,
    pub snapshot: Vec<Option<PluginTiming>>,
}

/// No callbacks or I/O under the producer. The owning helper serializes access to this buffer.
pub struct ChangeBuffer {
    pending: Vec<SlotChange>,
    snapshot: Vec<Option<PluginTiming>>,
    overflow: bool,
}

impl Default for ChangeBuffer {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(CHANGE_CAPACITY),
            snapshot: Vec::new(),
            overflow: false,
        }
    }
}

impl ChangeBuffer {
    pub fn observe(&mut self, slot: usize, timing: PluginTiming) {
        if self.snapshot.len() <= slot {
            self.snapshot.resize(slot + 1, None);
        }
        if self.snapshot[slot] == Some(timing) {
            return;
        }
        self.snapshot[slot] = Some(timing);
        self.pending.retain(|event| event.slot != slot);
        if self.pending.len() == CHANGE_CAPACITY {
            self.pending.remove(0);
            self.overflow = true;
        }
        self.pending.push(SlotChange { slot, timing });
    }

    pub fn take(&mut self) -> ChangeBatch {
        ChangeBatch {
            changes: self.pending.drain(..).collect(),
            resync_required: std::mem::take(&mut self.overflow),
            snapshot: self.snapshot.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_notifications_coalesce_and_resynchronize_without_a_consumer() {
        let mut events = ChangeBuffer::default();
        let timing = |latency| PluginTiming {
            latency,
            tail: Tail::Samples(latency),
            restart_required: false,
            latency_changed: false,
        };
        for slot in 0..CHANGE_CAPACITY + 10 {
            events.observe(slot, timing(slot as u32));
        }
        events.observe(0, timing(999));
        let batch = events.take();
        assert!(batch.resync_required);
        assert_eq!(batch.changes.len(), CHANGE_CAPACITY);
        assert_eq!(batch.changes.last().unwrap().slot, 0);
        assert_eq!(batch.snapshot[0], Some(timing(999)));
        assert_eq!(batch.snapshot.len(), CHANGE_CAPACITY + 10);
        let empty = events.take();
        assert!(!empty.resync_required);
        assert!(empty.changes.is_empty());
        assert_eq!(empty.snapshot, batch.snapshot);
    }
}
