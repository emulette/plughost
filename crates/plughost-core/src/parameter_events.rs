use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Maximum queued parameter notifications before the oldest records are dropped.
pub const PARAMETER_EVENT_CAPACITY: usize = 256;

/// Notifications reported by the plugin, in observed order. Values are normalized to 0..=1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ParameterEvent {
    /// A parameter’s normalized value changed.
    Value {
        /// Native parameter ID or Audio Unit address.
        id: u64,
        /// Finite normalized value in `0..=1`.
        normalized: f64,
    },
    /// A native parameter edit gesture began.
    BeginEdit {
        /// Native parameter ID or Audio Unit address.
        id: u64,
    },
    /// A native parameter edit gesture ended.
    EndEdit {
        /// Native parameter ID or Audio Unit address.
        id: u64,
    },
    /// Parameter metadata should be queried again.
    MetadataChanged,
    /// Current parameter values should be queried again.
    ValuesChanged,
    /// The plugin reported its persistent state’s dirty status.
    Dirty {
        /// Whether the plugin reports unsaved changes.
        dirty: bool,
    },
}

/// Overflow loses ordering continuity: cancel open gestures and requery metadata and values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParameterEventBatch {
    /// Notifications retained in observed order.
    pub events: Vec<ParameterEvent>,
    /// Cancel open gestures and requery metadata and values when true.
    pub resync_required: bool,
    /// Number of lost records since the previous drain, including upstream loss.
    pub dropped: u64,
}

struct Pending {
    events: VecDeque<ParameterEvent>,
    dropped: u64,
    resync_required: bool,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            events: VecDeque::with_capacity(PARAMETER_EVENT_CAPACITY),
            dropped: 0,
            resync_required: false,
        }
    }
}

/// A bounded producer queue shared by native callbacks and the owning main thread.
#[derive(Clone, Default)]
pub struct ParameterEventBuffer(Arc<Mutex<Pending>>);

impl ParameterEventBuffer {
    /// Merges a producer batch without hiding loss already reported by that producer.
    pub fn append(&self, batch: ParameterEventBatch) {
        let mut pending = self.0.lock().unwrap_or_else(|error| error.into_inner());
        pending.dropped = pending.dropped.saturating_add(batch.dropped);
        pending.resync_required |= batch.resync_required;
        for event in batch.events {
            if pending.events.len() == PARAMETER_EVENT_CAPACITY {
                pending.events.pop_front();
                pending.dropped = pending.dropped.saturating_add(1);
            }
            pending.events.push_back(event);
        }
    }

    /// Appends one notification, dropping the oldest on capacity overflow.
    pub fn record(&self, event: ParameterEvent) {
        let mut pending = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if pending.events.len() == PARAMETER_EVENT_CAPACITY {
            pending.events.pop_front();
            pending.dropped = pending.dropped.saturating_add(1);
        }
        pending.events.push_back(event);
    }

    /// Drains notifications and loss counters into an owned batch.
    pub fn take(&self) -> ParameterEventBatch {
        let mut pending = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let dropped = std::mem::take(&mut pending.dropped);
        ParameterEventBatch {
            events: pending.events.drain(..).collect(),
            resync_required: std::mem::take(&mut pending.resync_required) || dropped != 0,
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_queue_preserves_gestures_and_reports_lost_continuity() {
        let consumer = ParameterEventBuffer::default();
        let producer = consumer.clone();
        producer.record(ParameterEvent::BeginEdit { id: 3 });
        for id in 0..PARAMETER_EVENT_CAPACITY as u64 {
            producer.record(ParameterEvent::Value {
                id,
                normalized: 0.5,
            });
        }
        producer.record(ParameterEvent::EndEdit { id: 3 });
        let batch = consumer.take();
        assert_eq!(batch.events.len(), PARAMETER_EVENT_CAPACITY);
        assert_eq!(batch.dropped, 2);
        assert!(batch.resync_required);
        assert_eq!(
            batch.events.first(),
            Some(&ParameterEvent::Value {
                id: 1,
                normalized: 0.5
            })
        );
        assert_eq!(
            batch.events.last(),
            Some(&ParameterEvent::EndEdit { id: 3 })
        );
        assert_eq!(consumer.take(), ParameterEventBatch::default());
    }

    #[test]
    fn appending_preserves_upstream_loss_and_requery_requests() {
        let buffer = ParameterEventBuffer::default();
        buffer.append(ParameterEventBatch {
            events: vec![ParameterEvent::MetadataChanged],
            dropped: 4,
            resync_required: true,
        });
        buffer.append(ParameterEventBatch {
            events: vec![ParameterEvent::ValuesChanged],
            dropped: 0,
            resync_required: false,
        });
        let batch = buffer.take();
        assert_eq!(
            batch.events,
            vec![
                ParameterEvent::MetadataChanged,
                ParameterEvent::ValuesChanged
            ]
        );
        assert_eq!(batch.dropped, 4);
        assert!(batch.resync_required);
        buffer.append(ParameterEventBatch {
            events: Vec::new(),
            dropped: 0,
            resync_required: true,
        });
        assert!(buffer.take().resync_required);
    }
}
