use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Native plugin ABI used for discovery and loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluginFormat {
    /// Steinberg VST 3.
    Vst3,
    /// Audio Unit (macOS), v2 or v3.
    AudioUnit,
    /// CLever Audio Plug-in (CLAP).
    Clap,
}

/// A plugin class identifier in its format's canonical form, so that two IDs of one class
/// compare equal however they were written. It comes with its format in a [`PluginId`]. For VST3 it is the 32-digit hexadecimal class ID in
/// the form `.vstpreset` files use, and for Audio Units the component type, subtype, and
/// manufacturer codes as 24 hexadecimal digits; both are kept in upper case. For CLAP it is the
/// plugin ID as given.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassId(String);

impl ClassId {
    /// The canonical form of `id` in `format`: VST3 and Audio Unit IDs in upper case, CLAP IDs as
    /// they are. The ID is not otherwise checked; one no plugin has fails when it is loaded.
    fn new(format: PluginFormat, id: impl Into<String>) -> ClassId {
        let mut id = id.into();
        match format {
            PluginFormat::Vst3 | PluginFormat::AudioUnit => id.make_ascii_uppercase(),
            PluginFormat::Clap => {}
        }
        ClassId(id)
    }

    /// The identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identity of a plugin class, independent of where it is installed: its format and class
/// ID. Store it, not a path, with the work that uses the plugin, and find the installed class
/// again with the scanner's `Catalog::resolve`.
///
/// It serializes with serde as a `format` (`"vst3"`, `"audio-unit"` or `"clap"`) and a
/// `class_id`; deserializing puts the ID in its canonical form.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PluginId {
    format: PluginFormat,
    class_id: ClassId,
}

impl PluginId {
    /// The class `class_id` of `format`, with the ID in its canonical form (see [`ClassId`]).
    pub fn new(format: PluginFormat, class_id: impl Into<String>) -> PluginId {
        PluginId {
            format,
            class_id: ClassId::new(format, class_id),
        }
    }

    /// The class's native format.
    pub fn format(&self) -> PluginFormat {
        self.format
    }

    /// The class's identifier within its format.
    pub fn class_id(&self) -> &ClassId {
        &self.class_id
    }
}

/// The persisted form of a [`PluginId`], with format names that do not depend on the Rust names
/// of [`PluginFormat`]'s variants.
#[derive(Serialize, Deserialize)]
#[serde(rename = "PluginId")]
struct PersistedId {
    format: PersistedFormat,
    class_id: String,
}

#[derive(Serialize, Deserialize)]
enum PersistedFormat {
    #[serde(rename = "vst3")]
    Vst3,
    #[serde(rename = "audio-unit")]
    AudioUnit,
    #[serde(rename = "clap")]
    Clap,
}

impl Serialize for PluginId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let format = match self.format {
            PluginFormat::Vst3 => PersistedFormat::Vst3,
            PluginFormat::AudioUnit => PersistedFormat::AudioUnit,
            PluginFormat::Clap => PersistedFormat::Clap,
        };
        PersistedId {
            format,
            class_id: self.class_id.0.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<PluginId, D::Error> {
        let persisted = PersistedId::deserialize(deserializer)?;
        let format = match persisted.format {
            PersistedFormat::Vst3 => PluginFormat::Vst3,
            PersistedFormat::AudioUnit => PluginFormat::AudioUnit,
            PersistedFormat::Clap => PluginFormat::Clap,
        };
        Ok(PluginId::new(format, persisted.class_id))
    }
}

/// A plugin class to load: its identity and where it is installed. VST3 and CLAP classes live in
/// a bundle; Audio Units have none, the system registry finds them. Build it with the format's
/// constructor, or take it from the scanner's catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRef {
    pub(crate) id: PluginId,
    pub(crate) bundle: Option<PathBuf>,
}

impl PluginRef {
    /// The VST3 class `class_id` in the `.vst3` bundle (or, on Windows, single-file module) at
    /// `bundle`.
    pub fn vst3(bundle: impl Into<PathBuf>, class_id: impl Into<String>) -> PluginRef {
        PluginRef {
            id: PluginId::new(PluginFormat::Vst3, class_id),
            bundle: Some(bundle.into()),
        }
    }

    /// The CLAP plugin `class_id` in the `.clap` bundle at `bundle`.
    pub fn clap(bundle: impl Into<PathBuf>, class_id: impl Into<String>) -> PluginRef {
        PluginRef {
            id: PluginId::new(PluginFormat::Clap, class_id),
            bundle: Some(bundle.into()),
        }
    }

    /// The registered Audio Unit `class_id`.
    pub fn audio_unit(class_id: impl Into<String>) -> PluginRef {
        PluginRef {
            id: PluginId::new(PluginFormat::AudioUnit, class_id),
            bundle: None,
        }
    }

    /// The class's identity.
    pub fn id(&self) -> &PluginId {
        &self.id
    }

    /// The bundle that contains a VST3 or CLAP class; `None` for an Audio Unit.
    pub fn bundle(&self) -> Option<&Path> {
        self.bundle.as_deref()
    }
}

/// Broad native classification used in discovery results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluginKind {
    /// An audio or MIDI effect.
    Effect,
    /// A sound-generating instrument.
    Instrument,
}

/// A plugin class as its format describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PluginInfo {
    /// The class's format and identifier.
    pub id: PluginId,
    /// Native plugin display name.
    pub name: String,
    /// Native vendor name, or empty when unavailable.
    pub vendor: String,
    /// Native plugin version string, or empty when unavailable.
    pub version: String,
    /// Native SDK version string, or empty when unavailable.
    pub sdk_version: String,
    /// Native effect or instrument classification.
    pub kind: PluginKind,
    /// Format sub-categories, for example `Fx` and `EQ`.
    pub categories: Vec<String>,
}

impl PluginInfo {
    /// A class with no vendor, version, SDK version or categories yet.
    pub fn new(id: PluginId, name: impl Into<String>, kind: PluginKind) -> PluginInfo {
        PluginInfo {
            id,
            name: name.into(),
            vendor: String::new(),
            version: String::new(),
            sdk_version: String::new(),
            kind,
            categories: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexadecimal_ids_compare_whatever_their_case() {
        let lower = PluginId::new(PluginFormat::Vst3, "abcdef019182faeb4163747a44784c76");
        let upper = PluginId::new(PluginFormat::Vst3, "ABCDEF019182FAEB4163747A44784C76");
        assert_eq!(lower, upper);
        assert_eq!(
            lower.class_id().as_str(),
            "ABCDEF019182FAEB4163747A44784C76"
        );
        assert_eq!(
            PluginId::new(PluginFormat::AudioUnit, "6175667864656c796170706c"),
            PluginId::new(PluginFormat::AudioUnit, "6175667864656C796170706C")
        );
        // CLAP IDs are case-sensitive.
        assert_ne!(
            PluginId::new(PluginFormat::Clap, "com.example.Delay"),
            PluginId::new(PluginFormat::Clap, "com.example.delay")
        );
    }

    #[test]
    fn ids_persist_with_their_format_names_and_read_back_canonical() {
        let id = PluginId::new(PluginFormat::AudioUnit, "6175667864656C796170706C");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            r#"{"format":"audio-unit","class_id":"6175667864656C796170706C"}"#
        );
        for (json, format) in [
            (
                r#"{"format":"vst3","class_id":"abcdef01"}"#,
                PluginFormat::Vst3,
            ),
            (
                r#"{"format":"audio-unit","class_id":"abcdef01"}"#,
                PluginFormat::AudioUnit,
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<PluginId>(json).unwrap(),
                PluginId::new(format, "ABCDEF01")
            );
        }
        let clap = PluginId::new(PluginFormat::Clap, "com.example.Delay");
        let json = serde_json::to_string(&clap).unwrap();
        assert_eq!(json, r#"{"format":"clap","class_id":"com.example.Delay"}"#);
        assert_eq!(serde_json::from_str::<PluginId>(&json).unwrap(), clap);
    }
}
