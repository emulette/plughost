//! Bounded, pull-based diagnostics. Plugin callbacks never call application code or perform I/O.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::FailureKind;
use crate::plugin::PluginRef;

/// Maximum retained records per buffer and per helper snapshot.
pub const DIAGNOSTIC_CAPACITY: usize = 128;
/// Maximum UTF-8 bytes retained from one diagnostic message.
pub const DIAGNOSTIC_MESSAGE_BYTES: usize = 4096;

/// Severity reported by the plugin or host, including CLAP's two misbehavior categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DiagnosticSeverity {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
    HostMisbehaving,
    PluginMisbehaving,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Present for messages from a plugin; helper failures can identify only the slot.
    pub plugin: Option<PluginRef>,
    pub slot: Option<usize>,
    /// Present for host operation failures, absent for plugin-authored log messages.
    pub failure_kind: Option<FailureKind>,
    pub truncated: bool,
}

/// Records drained since the last snapshot. Loss is explicit; this is not an audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticBatch {
    pub records: Vec<Diagnostic>,
    /// Records lost to capacity, concurrent access, or snapshot aggregation.
    pub dropped: u64,
}

impl DiagnosticBatch {
    /// Merges a snapshot while keeping the aggregate bounded, preserving earlier records.
    pub fn append(&mut self, mut other: Self) {
        let available = DIAGNOSTIC_CAPACITY.saturating_sub(self.records.len());
        let overflow = other.records.len().saturating_sub(available);
        self.dropped = self
            .dropped
            .saturating_add(other.dropped)
            .saturating_add(overflow as u64);
        other.records.truncate(available);
        self.records.extend(other.records);
    }
}

#[derive(Default)]
struct Buffer {
    records: Mutex<VecDeque<Diagnostic>>,
    dropped: AtomicU64,
}

/// A clone shares storage. Insertion never waits for a reader and never invokes user code.
/// This bounds diagnostic storage; it is not a real-time, allocation-free logging API.
#[derive(Clone, Default)]
pub struct DiagnosticBuffer(Arc<Buffer>);

impl DiagnosticBuffer {
    pub fn record(
        &self,
        severity: DiagnosticSeverity,
        message: &str,
        plugin: Option<&PluginRef>,
        slot: Option<usize>,
        failure_kind: Option<FailureKind>,
    ) {
        let Ok(mut records) = self.0.records.try_lock() else {
            self.0.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if records.len() == DIAGNOSTIC_CAPACITY {
            self.0.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut end = message.len().min(DIAGNOSTIC_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        records.push_back(Diagnostic {
            severity,
            message: message[..end].to_owned(),
            plugin: plugin.cloned(),
            slot,
            failure_kind,
            truncated: end < message.len(),
        });
    }

    /// Drains the queue. Call from application/control code, never an audio callback.
    pub fn take(&self) -> DiagnosticBatch {
        let mut records = self.0.records.lock().unwrap_or_else(|p| p.into_inner());
        DiagnosticBatch {
            records: records.drain(..).collect(),
            dropped: self.0.dropped.swap(0, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_storage_and_reports_overflow_and_utf8_truncation() {
        let buffer = DiagnosticBuffer::default();
        let message = "한".repeat(DIAGNOSTIC_MESSAGE_BYTES);
        for _ in 0..DIAGNOSTIC_CAPACITY + 3 {
            buffer.record(DiagnosticSeverity::Warning, &message, None, None, None);
        }
        let batch = buffer.take();
        assert_eq!(batch.records.len(), DIAGNOSTIC_CAPACITY);
        assert_eq!(batch.dropped, 3);
        assert!(
            batch
                .records
                .iter()
                .all(|r| r.truncated && r.message.len() <= DIAGNOSTIC_MESSAGE_BYTES)
        );
        assert_eq!(buffer.take(), DiagnosticBatch::default());
    }

    #[test]
    fn contention_drops_without_waiting_and_snapshot_merge_counts_loss() {
        let buffer = DiagnosticBuffer::default();
        let guard = buffer.0.records.lock().unwrap();
        buffer.record(DiagnosticSeverity::Info, "busy", None, None, None);
        drop(guard);
        assert_eq!(buffer.take().dropped, 1);
        for _ in 0..DIAGNOSTIC_CAPACITY {
            buffer.record(DiagnosticSeverity::Info, "retained", None, None, None);
        }
        let mut batch = buffer.take();
        batch.append(batch.clone());
        assert_eq!(batch.records.len(), DIAGNOSTIC_CAPACITY);
        assert_eq!(batch.dropped, DIAGNOSTIC_CAPACITY as u64);
    }
}
