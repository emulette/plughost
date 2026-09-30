//! Scanning installed plugins in helper processes, one helper per bundle.

mod bundle;
mod cache;
mod control;
mod execute;
use control::Work;
pub use control::{ScanControl, ScanEvent, ScanItem, ScanStage, ScanTarget};
mod moduleinfo;
mod policy;
pub use policy::{ScanAction, ScanPolicy};
mod run;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use plughost_core::PluginRef;
use plughost_core::ipc::Request;
use plughost_core::{Failure, PluginInfo};
use serde::{Deserialize, Serialize};

use self::bundle::Inspection;
use crate::errors::Error;

/// The result of scanning one bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ScanOutcome {
    Found(Vec<PluginInfo>),
    /// Metadata discovery found a compatible binary, but no class metadata or cached result.
    /// Native inspection is required; this is not a plugin failure.
    NotInspected,
    /// The caller cancelled the operation. Never counted or cached as a plugin failure.
    Cancelled,
    /// The caller skipped this item. Never counted or cached as a plugin failure.
    Skipped,
    /// Excluded by the caller's policy, including during retry. Cached results are preserved.
    Blocked,
    /// The binary does not contain the helper's architecture; it was not loaded.
    UnsupportedArchitecture,
    /// There is no binary, for example the remains of an interrupted install.
    InvalidBundle,
    /// Scanning returned a plugin or host error.
    Failed(Failure),
    /// The helper process died while loading the module, with its exit status.
    Crashed(String),
    /// The helper made no progress within the stall timeout and was terminated.
    TimedOut,
}

impl ScanOutcome {
    fn is_failure(&self) -> bool {
        matches!(
            self,
            ScanOutcome::Failed(_) | ScanOutcome::Crashed(_) | ScanOutcome::TimedOut
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// Native inspection was selected; failure or interruption may occur before loading.
    Loaded,
    /// Read from `moduleinfo.json` without loading.
    ModuleInfo,
    /// Unchanged native result from an earlier scan.
    Cache,
    /// Inspected without loading (policy, architecture or missing binary).
    Inspected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleScan {
    pub path: PathBuf,
    pub outcome: ScanOutcome,
    pub source: Source,
    /// Consecutive failed scans of this unchanged bundle.
    pub failures: u32,
    /// Failed too often; skipped until [`Scanner::retry`].
    pub excluded: bool,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    /// Every VST3 and CLAP bundle found, in directory priority order.
    pub bundles: Vec<BundleScan>,
    /// Allowed Audio Units in the system registry (macOS), read fresh by a helper without loading.
    pub audio_units: ScanOutcome,
}

impl Catalog {
    /// Every discovered plugin class identity, without a guarantee of successful loading.
    /// When bundles of the same format share a class ID,
    /// the one found first (in the higher priority directory) wins.
    pub fn plugins(&self) -> Vec<(PluginRef, &PluginInfo)> {
        let mut seen = HashSet::new();
        let vst3 = self
            .bundles
            .iter()
            .filter_map(|scan| match &scan.outcome {
                ScanOutcome::Found(classes) => Some((scan.path.as_path(), classes)),
                _ => None,
            })
            .flat_map(|(path, classes)| classes.iter().map(move |class| (Some(path), class)));
        let audio_units = match &self.audio_units {
            ScanOutcome::Found(classes) => classes.as_slice(),
            _ => &[],
        };
        vst3.chain(audio_units.iter().map(|class| (None, class)))
            .filter(|(_, class)| seen.insert((class.format, class.class_id.clone())))
            .map(|(bundle, class)| {
                let plugin = PluginRef {
                    format: class.format,
                    bundle: bundle.map(Path::to_path_buf),
                    class_id: class.class_id.clone(),
                };
                (plugin, class)
            })
            .collect()
    }
}

/// The standard VST3 and CLAP folders of the platform, highest priority first, and the folders in
/// `CLAP_PATH`. Applications add their own plugin folders.
pub fn default_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        for format in ["VST3", "CLAP"] {
            if let Some(home) = &home {
                directories.push(home.join("Library/Audio/Plug-Ins").join(format));
            }
            directories.push(PathBuf::from("/Library/Audio/Plug-Ins").join(format));
        }
        directories.push(PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3"));
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let common = std::env::var_os("CommonProgramFiles").map(PathBuf::from);
        for format in ["VST3", "CLAP"] {
            if let Some(local) = &local {
                directories.push(local.join("Programs\\Common").join(format));
            }
            if let Some(common) = &common {
                directories.push(common.join(format));
            }
        }
    }
    if let Some(paths) = std::env::var_os("CLAP_PATH") {
        directories.extend(std::env::split_paths(&paths));
    }
    directories
}

pub struct Scanner {
    helper: PathBuf,
    cache: PathBuf,
    directories: Vec<PathBuf>,
    jobs: usize,
    stall_timeout: Duration,
    exclude_after: u32,
    policy: ScanPolicy,
}

/// A bundle's scan result and the cache entry to keep for it.
type Scanned = (BundleScan, Option<cache::Entry>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    Metadata,
    Native,
    Retry,
}

/// A wait long enough to answer a license dialog by hand.
const NO_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

impl Scanner {
    /// Scans the platform's standard folders with `helper`, keeping native results in `cache`.
    /// Native inspection is serial unless [`Self::jobs`] is explicitly set.
    pub fn new(helper: &Path, cache: &Path) -> Scanner {
        Scanner {
            helper: helper.to_path_buf(),
            cache: cache.to_path_buf(),
            directories: default_directories(),
            jobs: 1,
            stall_timeout: Duration::from_secs(30),
            exclude_after: 2,
            policy: ScanPolicy::default(),
        }
    }

    /// Folders to scan, highest priority first.
    pub fn directories(mut self, directories: Vec<PathBuf>) -> Scanner {
        self.directories = directories;
        self
    }

    /// Selects bundles and registered Audio Units. Blocked bundles remain visible with
    /// [`ScanOutcome::Blocked`]; blocked Audio Units are omitted from registry results.
    /// Rules apply to discovery, native scanning and retry, independently of failure exclusion.
    pub fn policy(mut self, policy: ScanPolicy) -> Scanner {
        self.policy = policy;
        self
    }

    /// Native inspection helpers to run at the same time (default: one). Zero selects one.
    /// Parallel inspection can open multiple plugin dialogs concurrently.
    pub fn jobs(mut self, jobs: usize) -> Scanner {
        self.jobs = jobs.max(1);
        self
    }

    /// How long a helper may go without progress (module loaded, each class) before it is
    /// terminated.
    pub fn stall_timeout(mut self, timeout: Duration) -> Scanner {
        self.stall_timeout = timeout;
        self
    }

    /// Discovers bundles without loading their plugin code. Uses unchanged native cache entries,
    /// static VST3 metadata and binary headers. Missing/malformed metadata yields
    /// [`ScanOutcome::NotInspected`], never an automatic native load. The cache is not written.
    /// Audio Unit registry enumeration uses a helper without instantiating Audio Units.
    /// Discovery does not prove that a plugin can be instantiated or process audio.
    pub fn discover(&self) -> Catalog {
        self.discover_controlled(&ScanControl::default(), &|_| {})
    }

    /// Metadata discovery with cancellation, per-item skipping and stage notifications.
    /// Filesystem traversal checks cancellation between entries; an OS filesystem call itself
    /// cannot be interrupted. Items not admitted before cancellation are absent from the catalog.
    pub fn discover_controlled(
        &self,
        control: &ScanControl,
        events: &(dyn Fn(&ScanEvent) + Sync),
    ) -> Catalog {
        let cache = cache::load(&self.cache);
        let paths = bundle::find(&self.directories, &|| control.is_cancelled());
        let mut bundles = Vec::new();
        for path in paths {
            let Some(work) = control.begin(ScanTarget::Bundle(path.clone()), events) else {
                break;
            };
            let (scan, _) = self.scan_bundle(
                &path,
                cache.bundles.get(&path),
                self.stall_timeout,
                ScanMode::Metadata,
                &work,
            );
            events(&ScanEvent::BundleFinished(scan.clone()));
            bundles.push(scan);
        }
        Catalog {
            bundles,
            audio_units: self.list_audio_units(control, events),
        }
    }

    /// Inspects native classes, calling `progress` as each bundle finishes. Reuses unchanged
    /// native cache results, but static module metadata does not replace native inspection.
    /// Plugins can show dialogs during loading. This does not instantiate every plugin class
    /// or verify DSP capabilities. Audio Units are listed from their registry only.
    /// Cache access can fail; each bundle's problems are in its outcome. Changed results are
    /// written to the cache before the completion callback, including when a later bundle is
    /// interrupted, and synced to stable storage when the scan ends.
    pub fn scan(&self, progress: &(dyn Fn(&BundleScan) + Sync)) -> Result<Catalog, Error> {
        self.scan_controlled(&ScanControl::default(), &|event| {
            if let ScanEvent::BundleFinished(scan) = event {
                progress(scan);
            }
        })
    }

    /// Reads the Audio Unit registry through a helper. There is none on other platforms.
    fn list_audio_units(
        &self,
        control: &ScanControl,
        events: &(dyn Fn(&ScanEvent) + Sync),
    ) -> ScanOutcome {
        let outcome = match control.begin(ScanTarget::AudioUnits, events) {
            None => ScanOutcome::Cancelled,
            Some(work) => {
                if let Some(outcome) = work.stage(ScanStage::Inspecting) {
                    outcome
                } else if cfg!(target_os = "macos") {
                    self.execute(Request::ListAudioUnits, self.stall_timeout, &work)
                } else {
                    ScanOutcome::Found(Vec::new())
                }
            }
        };
        events(&ScanEvent::AudioUnitsFinished(outcome.clone()));
        outcome
    }

    /// Scans one bundle again with a 24-hour stall timeout, even if excluded, so the user can answer
    /// a dialog the plugin shows. Updates the cache. Explicit policy blocks still apply.
    pub fn retry(&self, path: &Path) -> Result<BundleScan, Error> {
        self.retry_controlled(path, &ScanControl::default(), &|_| {})
    }

    /// Interactive retry with the same stop/stage contract as [`Self::scan_controlled`].
    pub fn retry_controlled(
        &self,
        path: &Path,
        control: &ScanControl,
        events: &(dyn Fn(&ScanEvent) + Sync),
    ) -> Result<BundleScan, Error> {
        if control.is_cancelled() {
            let cache = cache::load(&self.cache);
            let scan = self.interrupted(
                path,
                cache.bundles.get(path),
                ScanOutcome::Cancelled,
                Source::Inspected,
            );
            events(&ScanEvent::BundleFinished(scan.clone()));
            return Ok(scan);
        }
        let mut writer = cache::Writer::open(&self.cache).map_err(Error::Cache)?;
        let cached = writer.entry(path).cloned();
        let Some(work) = control.begin(ScanTarget::Bundle(path.to_path_buf()), events) else {
            let scan = self.interrupted(
                path,
                cached.as_ref(),
                ScanOutcome::Cancelled,
                Source::Inspected,
            );
            events(&ScanEvent::BundleFinished(scan.clone()));
            return Ok(scan);
        };
        let (scan, entry) =
            self.scan_bundle(path, cached.as_ref(), NO_TIMEOUT, ScanMode::Retry, &work);
        writer.update(path, entry).map_err(Error::Cache)?;
        writer.finish().map_err(Error::Cache)?;
        events(&ScanEvent::BundleFinished(scan.clone()));
        Ok(scan)
    }

    fn interrupted(
        &self,
        path: &Path,
        cached: Option<&cache::Entry>,
        outcome: ScanOutcome,
        source: Source,
    ) -> BundleScan {
        let failures = cached.map_or(0, |entry| entry.failures);
        BundleScan {
            path: path.to_path_buf(),
            outcome,
            source,
            failures,
            excluded: failures >= self.exclude_after,
        }
    }

    fn scan_bundle(
        &self,
        path: &Path,
        cached: Option<&cache::Entry>,
        stall_timeout: Duration,
        mode: ScanMode,
        work: &Work<'_>,
    ) -> Scanned {
        if let Some(outcome) = work.stage(ScanStage::Inspecting) {
            return (
                self.interrupted(path, cached, outcome, Source::Inspected),
                cached.cloned(),
            );
        }
        if !self.policy.allows_bundle(path) {
            return (
                self.interrupted(path, cached, ScanOutcome::Blocked, Source::Inspected),
                cached.cloned(),
            );
        }
        let scan = |outcome, source, failures, excluded| BundleScan {
            path: path.to_path_buf(),
            outcome,
            source,
            failures,
            excluded,
        };
        let executable = match bundle::inspect(path) {
            Inspection::Supported { executable } => executable,
            Inspection::UnsupportedArchitecture => {
                return (
                    scan(
                        ScanOutcome::UnsupportedArchitecture,
                        Source::Inspected,
                        0,
                        false,
                    ),
                    None,
                );
            }
            Inspection::Invalid => {
                return (
                    scan(ScanOutcome::InvalidBundle, Source::Inspected, 0, false),
                    None,
                );
            }
        };
        let fingerprint = bundle::fingerprint(path, &executable);
        let unchanged = cached.filter(|entry| entry.fingerprint == fingerprint);
        if let Some(entry) = unchanged
            && mode != ScanMode::Retry
        {
            let excluded = entry.failures >= self.exclude_after;
            if mode == ScanMode::Metadata || !entry.outcome.is_failure() || excluded {
                let result = scan(
                    entry.outcome.clone(),
                    Source::Cache,
                    entry.failures,
                    excluded,
                );
                return (result, Some(entry.clone()));
            }
        }

        if mode == ScanMode::Metadata {
            let (outcome, source) = match moduleinfo::read(path) {
                Some(classes) => (ScanOutcome::Found(classes), Source::ModuleInfo),
                None => (ScanOutcome::NotInspected, Source::Inspected),
            };
            return (scan(outcome, source, 0, false), None);
        }
        let outcome = self.execute(
            Request::Scan {
                bundle: path.to_path_buf(),
                interactive: mode == ScanMode::Retry,
            },
            stall_timeout,
            work,
        );
        if matches!(outcome, ScanOutcome::Cancelled | ScanOutcome::Skipped) {
            return (
                self.interrupted(path, cached, outcome, Source::Loaded),
                cached.cloned(),
            );
        }
        let failures = if outcome.is_failure() {
            unchanged.map_or(0, |entry| entry.failures) + 1
        } else {
            0
        };
        let entry = cache::Entry {
            fingerprint,
            outcome: outcome.clone(),
            failures,
        };
        let excluded = failures >= self.exclude_after;
        (
            scan(outcome, Source::Loaded, failures, excluded),
            Some(entry),
        )
    }
}

fn outcome_of(error: Error) -> ScanOutcome {
    match error {
        Error::Crashed { status, .. } => {
            ScanOutcome::Crashed(status.map(|status| status.to_string()).unwrap_or_default())
        }
        Error::TimedOut { .. } => ScanOutcome::TimedOut,
        Error::Operation { failure, .. } => ScanOutcome::Failed(failure),
        error => ScanOutcome::Failed(Failure::new(error.kind(), error.to_string())),
    }
}
