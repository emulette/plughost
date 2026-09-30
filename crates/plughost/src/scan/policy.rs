//! Caller-owned selection policy, independent of cached plugin failures.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Whether scanning may include a bundle or registered Audio Unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanAction {
    #[default]
    Allow,
    Block,
}

/// Explicit scan selection. The default allows all items; individual rules override it.
/// Bundle paths must match the paths returned by discovery (no canonicalization is performed).
/// Audio Unit IDs are compared case-insensitively. The last rule for an identity wins.
/// This policy is not persisted in the native cache and does not restrict Chain loading.
#[derive(Clone, Debug, Default)]
pub struct ScanPolicy {
    default: ScanAction,
    bundles: BTreeMap<PathBuf, ScanAction>,
    audio_units: BTreeMap<String, ScanAction>,
}

impl ScanPolicy {
    pub fn new(default: ScanAction) -> Self {
        Self {
            default,
            ..Self::default()
        }
    }

    /// Sets a rule for one bundle, including explicit retries. Allow does not clear failures or
    /// bypass automatic exclusion; retry is the explicit way to recheck an excluded bundle.
    pub fn bundle(mut self, path: impl Into<PathBuf>, action: ScanAction) -> Self {
        self.bundles.insert(path.into(), action);
        self
    }

    /// Sets a rule for one Audio Unit class ID. Registry enumeration never instantiates it.
    pub fn audio_unit(mut self, class_id: impl Into<String>, action: ScanAction) -> Self {
        self.audio_units
            .insert(class_id.into().to_ascii_uppercase(), action);
        self
    }

    pub(super) fn allows_bundle(&self, path: &Path) -> bool {
        self.bundles.get(path).copied().unwrap_or(self.default) == ScanAction::Allow
    }

    pub(super) fn allows_audio_unit(&self, class_id: &str) -> bool {
        self.audio_units
            .get(&class_id.to_ascii_uppercase())
            .copied()
            .unwrap_or(self.default)
            == ScanAction::Allow
    }
}
