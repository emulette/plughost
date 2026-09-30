//! Cooperative scan admission and per-item cancellation.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use plughost_core::PluginInfo;

use super::{BundleScan, ScanOutcome};

/// A shared stop handle for scanner operations. Cancellation is permanent for this handle;
/// create a fresh one for a later independent scan. Clones control the same operation group.
#[derive(Clone, Default)]
pub struct ScanControl {
    cancelled: Arc<AtomicBool>,
}
impl ScanControl {
    /// Stops admissions and requests termination of all active helpers using this control.
    /// Returning does not wait for workers; the scanning call returns after helper cleanup.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub(super) fn begin<'a>(
        &'a self,
        target: ScanTarget,
        events: &'a (dyn Fn(&ScanEvent) + Sync),
    ) -> Option<Work<'a>> {
        if self.is_cancelled() {
            return None;
        }
        Some(Work {
            control: self,
            events,
            aborted: None,
            item: ScanItem {
                target,
                skipped: Arc::new(AtomicBool::new(false)),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanTarget {
    Bundle(PathBuf),
    AudioUnits,
}

/// One admitted item. Retain a clone to skip it from another thread. A late skip only affects
/// this item, never a future item at the same path or a different item in the worker.
#[derive(Clone, Debug)]
pub struct ScanItem {
    pub target: ScanTarget,
    skipped: Arc<AtomicBool>,
}
impl ScanItem {
    pub fn skip(&self) {
        self.skipped.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ScanStage {
    Inspecting,
    StartingHelper,
    Loading,
    ModuleLoaded,
    ClassFound(PluginInfo),
}

/// Callbacks run on scanner workers and may overlap with explicit parallelism. They must return
/// promptly for cancellation to proceed; dispatch UI work instead of blocking the scanner.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ScanEvent {
    Progress { item: ScanItem, stage: ScanStage },
    BundleFinished(BundleScan),
    AudioUnitsFinished(ScanOutcome),
}

pub(super) struct Work<'a> {
    control: &'a ScanControl,
    events: &'a (dyn Fn(&ScanEvent) + Sync),
    item: ScanItem,
    aborted: Option<&'a AtomicBool>,
}
impl<'a> Work<'a> {
    pub fn abort_on(mut self, aborted: &'a AtomicBool) -> Self {
        self.aborted = Some(aborted);
        self
    }
    pub fn stopped(&self) -> Option<ScanOutcome> {
        if self.control.is_cancelled()
            || self
                .aborted
                .is_some_and(|abort| abort.load(Ordering::Acquire))
        {
            Some(ScanOutcome::Cancelled)
        } else if self.item.skipped.load(Ordering::Acquire) {
            Some(ScanOutcome::Skipped)
        } else {
            None
        }
    }
    pub fn stage(&self, stage: ScanStage) -> Option<ScanOutcome> {
        if let Some(outcome) = self.stopped() {
            return Some(outcome);
        }
        (self.events)(&ScanEvent::Progress {
            item: self.item.clone(),
            stage,
        });
        self.stopped()
    }
}
