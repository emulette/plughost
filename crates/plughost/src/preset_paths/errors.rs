use std::fmt;
use std::io;
use std::path::PathBuf;

use plughost_core::PluginFormat;

const SEARCH_IO: &str = "could not search the preset directory";
const UNSUPPORTED: &str = "this format has no standard preset file extension";
#[cfg(target_os = "windows")]
const KNOWN_FOLDER: &str = "could not resolve the Windows known folder";
#[cfg(target_os = "macos")]
pub(super) const HOME: &str = "could not resolve the user's home directory";

/// Why a preset search root could not be searched.
#[derive(Debug)]
#[non_exhaustive]
pub enum PresetSearchError {
    /// The root could not be read. Includes missing optional roots: callers may ignore
    /// `NotFound` specifically.
    Io {
        path: PathBuf,
        source: io::Error,
    },
    UnsupportedFormat(PluginFormat),
}

impl fmt::Display for PresetSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{SEARCH_IO} ({}): {source}", path.display()),
            Self::UnsupportedFormat(format) => write!(f, "{UNSUPPORTED}: {format:?}"),
        }
    }
}

impl std::error::Error for PresetSearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn known_folder_error(result: i32) -> io::Error {
    io::Error::other(format!("{KNOWN_FOLDER}: 0x{result:08X}"))
}
