//! Standard preset file locations and filesystem discovery. This does not load plugins,
//! inspect preset contents, or execute CLAP preset providers.
//!
//! Locations follow [Steinberg's preset locations](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical+Documentation/Locations+Format/Preset+Locations.html)
//! and [Apple TN2157](https://developer.apple.com/library/archive/technotes/tn2157/_index.html).

mod errors;
pub use errors::PresetSearchError;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use plughost_core::PluginFormat;
use walkdir::WalkDir;

/// One standard or caller-selected search root. The directory need not exist yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetDirectory {
    pub format: PluginFormat,
    pub path: PathBuf,
}

/// A candidate with the format's file extension, not yet parsed or class-validated.
/// Paths retain the caller's root (relative roots produce relative paths).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetFile {
    pub format: PluginFormat,
    pub path: PathBuf,
}

/// Standard roots in format/priority order. No filesystem scan or directory creation occurs.
///
/// Windows uses the native Documents, RoamingAppData, and ProgramData known folders, preserving
/// redirected locations. macOS searches user, local, and network Audio/Presets for VST3 and AU.
/// An explicitly supplied host application directory adds its `VST3 Presets` folder last.
/// CLAP providers have their own discovery API; CLAP has no universal preset file extension.
/// Failure to resolve a standard root is returned, never hidden by a guessed fallback path.
pub fn standard_preset_directories(
    application_directory: Option<&Path>,
) -> io::Result<Vec<PresetDirectory>> {
    let mut directories = platform_directories()?;
    if let Some(application) = application_directory {
        directories.push(PresetDirectory {
            format: PluginFormat::Vst3,
            path: application.join("VST3 Presets"),
        });
    }
    Ok(directories)
}

#[cfg(target_os = "windows")]
fn platform_directories() -> io::Result<Vec<PresetDirectory>> {
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Documents, FOLDERID_ProgramData, FOLDERID_RoamingAppData,
    };
    [
        FOLDERID_Documents,
        FOLDERID_RoamingAppData,
        FOLDERID_ProgramData,
    ]
    .iter()
    .map(|folder| {
        Ok(PresetDirectory {
            format: PluginFormat::Vst3,
            path: known_folder(folder)?.join("VST3 Presets"),
        })
    })
    .collect()
}

#[cfg(target_os = "windows")]
fn known_folder(folder: &windows_sys::core::GUID) -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath};

    let mut raw = std::ptr::null_mut();
    // DONT_VERIFY discovers configured roots even when no presets were installed there.
    let result = unsafe {
        SHGetKnownFolderPath(
            folder,
            KF_FLAG_DONT_VERIFY as u32,
            std::ptr::null_mut(),
            &mut raw,
        )
    };
    if result < 0 {
        unsafe { CoTaskMemFree(raw.cast()) };
        return Err(errors::known_folder_error(result));
    }
    let mut length = 0;
    unsafe {
        while *raw.add(length) != 0 {
            length += 1;
        }
    }
    let path = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(raw, length)
    }));
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(path)
}

#[cfg(target_os = "macos")]
fn platform_directories() -> io::Result<Vec<PresetDirectory>> {
    let home = std::env::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, errors::HOME))?;
    let paths = [
        home.join("Library/Audio/Presets"),
        PathBuf::from("/Library/Audio/Presets"),
        PathBuf::from("/Network/Library/Audio/Presets"),
    ];
    Ok([PluginFormat::Vst3, PluginFormat::AudioUnit]
        .into_iter()
        .flat_map(|format| {
            paths.iter().map(move |path| PresetDirectory {
                format,
                path: path.clone(),
            })
        })
        .collect())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_directories() -> io::Result<Vec<PresetDirectory>> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Recursively finds `.vstpreset` or `.aupreset` files, sorted by their complete path.
/// Extension matching is ASCII case-insensitive; file contents are not read.
///
/// Links and Windows junctions are followed, including a linked root; a link back to one of its
/// own ancestors is not followed again. Entries that cannot be read, such as a folder without
/// permission or a broken link, are skipped. Only a root that cannot be read is an error; callers
/// decide which optional roots may be absent. Discovery does not snapshot the filesystem, so
/// files may change before the caller opens them.
pub fn discover_preset_files(root: &PresetDirectory) -> Result<Vec<PresetFile>, PresetSearchError> {
    let extension = match root.format {
        PluginFormat::Vst3 => "vstpreset",
        PluginFormat::AudioUnit => "aupreset",
        _ => return Err(PresetSearchError::UnsupportedFormat(root.format)),
    };
    fs::read_dir(&root.path).map_err(|source| PresetSearchError::Io {
        path: root.path.clone(),
        source,
    })?;
    let mut files: Vec<PresetFile> = WalkDir::new(&root.path)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        })
        .map(|entry| PresetFile {
            format: root.format,
            path: entry.into_path(),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[cfg(test)]
mod tests;
