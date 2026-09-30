//! Parallel native inspections share one serialized cache writer; callbacks run outside its lock.
use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use super::{
    BundleScan, Catalog, ScanControl, ScanEvent, ScanMode, ScanTarget, Scanner, bundle, cache,
};
use crate::errors::Error;

struct State {
    writer: cache::Writer,
    results: Vec<Option<BundleScan>>,
    error: Option<io::Error>,
}

impl Scanner {
    /// A cancellable native scan. Changed results are atomically published before BundleFinished
    /// and synced to stable storage once, when the scan ends.
    /// Cancelled/skipped items retain old cache entries/counts; unadmitted items are absent from
    /// the catalog. A cache error stops admissions, reaps active helpers and returns Error::Cache.
    /// One native scan/retry writer may use a cache path at a time; contention returns WouldBlock.
    pub fn scan_controlled(
        &self,
        control: &ScanControl,
        events: &(dyn Fn(&ScanEvent) + Sync),
    ) -> Result<Catalog, Error> {
        let paths = bundle::find(&self.directories, &|| control.is_cancelled());
        if paths.is_empty() || control.is_cancelled() {
            return Ok(Catalog {
                bundles: Vec::new(),
                audio_units: self.list_audio_units(control, events),
            });
        }
        let state = Mutex::new(State {
            writer: cache::Writer::open(&self.cache).map_err(Error::Cache)?,
            results: vec![None; paths.len()],
            error: None,
        });
        let next = AtomicUsize::new(0);
        let aborted = AtomicBool::new(false);
        thread::scope(|scope| {
            for _ in 0..self.jobs.min(paths.len()) {
                scope.spawn(|| {
                    loop {
                        if aborted.load(Ordering::Acquire) {
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(index) else {
                            break;
                        };
                        let Some(work) = control.begin(ScanTarget::Bundle(path.clone()), events)
                        else {
                            break;
                        };
                        let work = work.abort_on(&aborted);
                        let cached = state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .writer
                            .entry(path)
                            .cloned();
                        let (scan, entry) = self.scan_bundle(
                            path,
                            cached.as_ref(),
                            self.stall_timeout,
                            ScanMode::Native,
                            &work,
                        );
                        {
                            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                            if state.error.is_some() {
                                break;
                            }
                            if let Err(error) = state.writer.update(path, entry) {
                                state.error = Some(error);
                                aborted.store(true, Ordering::Release);
                                break;
                            }
                            state.results[index] = Some(scan.clone());
                        }
                        events(&ScanEvent::BundleFinished(scan));
                    }
                });
            }
        });
        let state = state.into_inner().unwrap_or_else(|e| e.into_inner());
        let synced = state.writer.finish();
        if let Some(error) = state.error {
            return Err(Error::Cache(error));
        }
        synced.map_err(Error::Cache)?;
        Ok(Catalog {
            bundles: state.results.into_iter().flatten().collect(),
            audio_units: self.list_audio_units(control, events),
        })
    }
}
