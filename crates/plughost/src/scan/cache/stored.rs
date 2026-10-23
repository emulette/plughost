//! The cache's own form of scan outcomes, apart from the public types, which have no serde form,
//! and from the helper protocol. Changing it changes [`super::SCHEMA`].

use plughost_core::{Failure, FailureKind, PluginId, PluginInfo, PluginKind};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::super::ScanOutcome;

#[derive(Serialize, Deserialize)]
enum StoredOutcome {
    Found(Vec<StoredClass>),
    NotInspected,
    Cancelled,
    Skipped,
    Blocked,
    UnsupportedArchitecture,
    InvalidBundle,
    Failed { kind: StoredKind, message: String },
    Crashed(String),
    TimedOut,
}

#[derive(Serialize, Deserialize)]
struct StoredClass {
    id: PluginId,
    name: String,
    vendor: String,
    version: String,
    sdk_version: String,
    kind: StoredPluginKind,
    categories: Vec<String>,
}

#[derive(Serialize, Deserialize)]
enum StoredPluginKind {
    Effect,
    Instrument,
}

/// The failure categories, by name.
#[derive(Serialize, Deserialize)]
enum StoredKind {
    InvalidInput,
    Unsupported,
    NotFound,
    Load,
    Configuration,
    NotPrepared,
    Closed,
    RestartRequired,
    EditorOpen,
    StateMismatch,
    State,
    Processing,
    Editor,
    Host,
    Protocol,
    Crashed,
    TimedOut,
    Cancelled,
}

impl From<FailureKind> for StoredKind {
    fn from(kind: FailureKind) -> StoredKind {
        match kind {
            FailureKind::InvalidInput => StoredKind::InvalidInput,
            FailureKind::Unsupported => StoredKind::Unsupported,
            FailureKind::NotFound => StoredKind::NotFound,
            FailureKind::Load => StoredKind::Load,
            FailureKind::Configuration => StoredKind::Configuration,
            FailureKind::NotPrepared => StoredKind::NotPrepared,
            FailureKind::Closed => StoredKind::Closed,
            FailureKind::RestartRequired => StoredKind::RestartRequired,
            FailureKind::EditorOpen => StoredKind::EditorOpen,
            FailureKind::StateMismatch => StoredKind::StateMismatch,
            FailureKind::State => StoredKind::State,
            FailureKind::Processing => StoredKind::Processing,
            FailureKind::Editor => StoredKind::Editor,
            FailureKind::Host => StoredKind::Host,
            FailureKind::Protocol => StoredKind::Protocol,
            FailureKind::Crashed => StoredKind::Crashed,
            FailureKind::TimedOut => StoredKind::TimedOut,
            FailureKind::Cancelled => StoredKind::Cancelled,
            // The core crate's version is pinned to this one, which names all of its kinds.
            _ => StoredKind::Host,
        }
    }
}

impl From<StoredKind> for FailureKind {
    fn from(kind: StoredKind) -> FailureKind {
        match kind {
            StoredKind::InvalidInput => FailureKind::InvalidInput,
            StoredKind::Unsupported => FailureKind::Unsupported,
            StoredKind::NotFound => FailureKind::NotFound,
            StoredKind::Load => FailureKind::Load,
            StoredKind::Configuration => FailureKind::Configuration,
            StoredKind::NotPrepared => FailureKind::NotPrepared,
            StoredKind::Closed => FailureKind::Closed,
            StoredKind::RestartRequired => FailureKind::RestartRequired,
            StoredKind::EditorOpen => FailureKind::EditorOpen,
            StoredKind::StateMismatch => FailureKind::StateMismatch,
            StoredKind::State => FailureKind::State,
            StoredKind::Processing => FailureKind::Processing,
            StoredKind::Editor => FailureKind::Editor,
            StoredKind::Host => FailureKind::Host,
            StoredKind::Protocol => FailureKind::Protocol,
            StoredKind::Crashed => FailureKind::Crashed,
            StoredKind::TimedOut => FailureKind::TimedOut,
            StoredKind::Cancelled => FailureKind::Cancelled,
        }
    }
}

impl From<&PluginInfo> for StoredClass {
    fn from(info: &PluginInfo) -> StoredClass {
        StoredClass {
            id: info.id.clone(),
            name: info.name.clone(),
            vendor: info.vendor.clone(),
            version: info.version.clone(),
            sdk_version: info.sdk_version.clone(),
            kind: match info.kind {
                PluginKind::Instrument => StoredPluginKind::Instrument,
                // As for failures: the core crate's version names all of its kinds.
                _ => StoredPluginKind::Effect,
            },
            categories: info.categories.clone(),
        }
    }
}

impl From<StoredClass> for PluginInfo {
    fn from(class: StoredClass) -> PluginInfo {
        let kind = match class.kind {
            StoredPluginKind::Effect => PluginKind::Effect,
            StoredPluginKind::Instrument => PluginKind::Instrument,
        };
        let mut info = PluginInfo::new(class.id, class.name, kind);
        info.vendor = class.vendor;
        info.version = class.version;
        info.sdk_version = class.sdk_version;
        info.categories = class.categories;
        info
    }
}

pub fn serialize<S: Serializer>(outcome: &ScanOutcome, serializer: S) -> Result<S::Ok, S::Error> {
    let stored = match outcome {
        ScanOutcome::Found(classes) => {
            StoredOutcome::Found(classes.iter().map(StoredClass::from).collect())
        }
        ScanOutcome::NotInspected => StoredOutcome::NotInspected,
        ScanOutcome::Cancelled => StoredOutcome::Cancelled,
        ScanOutcome::Skipped => StoredOutcome::Skipped,
        ScanOutcome::Blocked => StoredOutcome::Blocked,
        ScanOutcome::UnsupportedArchitecture => StoredOutcome::UnsupportedArchitecture,
        ScanOutcome::InvalidBundle => StoredOutcome::InvalidBundle,
        ScanOutcome::Failed(failure) => StoredOutcome::Failed {
            kind: failure.kind.into(),
            message: failure.message.clone(),
        },
        ScanOutcome::Crashed(status) => StoredOutcome::Crashed(status.clone()),
        ScanOutcome::TimedOut => StoredOutcome::TimedOut,
    };
    stored.serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ScanOutcome, D::Error> {
    Ok(match StoredOutcome::deserialize(deserializer)? {
        StoredOutcome::Found(classes) => {
            ScanOutcome::Found(classes.into_iter().map(PluginInfo::from).collect())
        }
        StoredOutcome::NotInspected => ScanOutcome::NotInspected,
        StoredOutcome::Cancelled => ScanOutcome::Cancelled,
        StoredOutcome::Skipped => ScanOutcome::Skipped,
        StoredOutcome::Blocked => ScanOutcome::Blocked,
        StoredOutcome::UnsupportedArchitecture => ScanOutcome::UnsupportedArchitecture,
        StoredOutcome::InvalidBundle => ScanOutcome::InvalidBundle,
        StoredOutcome::Failed { kind, message } => {
            ScanOutcome::Failed(Failure::new(kind.into(), message))
        }
        StoredOutcome::Crashed(status) => ScanOutcome::Crashed(status),
        StoredOutcome::TimedOut => ScanOutcome::TimedOut,
    })
}
