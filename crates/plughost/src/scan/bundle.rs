//! Finding VST3 and CLAP bundles and inspecting them without loading their code.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Plugin folders nest by vendor, but never this deep; the bound also stops symlink loops.
const MAX_DEPTH: usize = 12;
/// Enough for a Mach-O fat header with many slices, or a PE header.
const HEADER_BYTES: usize = 4096;

/// Finds `.vst3` and `.clap` bundles (and single-file modules on Windows) under each directory,
/// recursively, without descending into bundles. Directories keep their order; paths within one are sorted.
pub fn find(directories: &[PathBuf], cancelled: &dyn Fn() -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for directory in directories {
        if cancelled() {
            break;
        }
        let mut in_directory = Vec::new();
        walk(directory, 0, &mut in_directory, cancelled);
        in_directory.sort();
        for path in in_directory {
            if !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

fn walk(directory: &Path, depth: usize, found: &mut Vec<PathBuf>, cancelled: &dyn Fn() -> bool) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if cancelled() {
            break;
        }
        let path = entry.path();
        let is_module = path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("vst3") || extension.eq_ignore_ascii_case("clap")
        });
        if is_module {
            found.push(path);
        } else if depth < MAX_DEPTH && path.is_dir() {
            walk(&path, depth + 1, found, cancelled);
        }
    }
}

pub enum Inspection {
    Supported { executable: PathBuf },
    UnsupportedArchitecture,
    Invalid,
}

/// Checks that the bundle has a binary and that the binary contains this process's architecture.
pub fn inspect(bundle: &Path) -> Inspection {
    let Some(executable) = executable(bundle) else {
        return Inspection::Invalid;
    };
    let mut header = Vec::with_capacity(HEADER_BYTES);
    let read = fs::File::open(&executable)
        .and_then(|file| file.take(HEADER_BYTES as u64).read_to_end(&mut header));
    if read.is_err() {
        return Inspection::Invalid;
    }
    match architectures(&header) {
        None => Inspection::Invalid,
        Some(found) if found.contains(&HOST) => Inspection::Supported { executable },
        Some(_) => Inspection::UnsupportedArchitecture,
    }
}

#[cfg(target_os = "macos")]
fn executable(bundle: &Path) -> Option<PathBuf> {
    use objc2_core_foundation::{CFBundle, CFURL};
    // Creating a CFBundle reads Info.plist; it does not load the binary.
    let url = CFURL::from_directory_path(bundle)?;
    let path = CFBundle::new(None, Some(&url))?
        .executable_url()?
        .to_file_path()?;
    path.is_file().then_some(path)
}

#[cfg(target_os = "windows")]
fn executable(bundle: &Path) -> Option<PathBuf> {
    let path = match bundle.file_name() {
        Some(name) if bundle.is_dir() => bundle.join("Contents").join("x86_64-win").join(name),
        _ => bundle.to_path_buf(),
    };
    path.is_file().then_some(path)
}

/// Architectures are identified by their Mach-O CPU type or PE machine type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
    X86_64,
    Other,
}

#[cfg(target_arch = "aarch64")]
const HOST: Architecture = Architecture::Arm64;
#[cfg(target_arch = "x86_64")]
const HOST: Architecture = Architecture::X86_64;

const MACH_O_64: u32 = 0xFEED_FACF;
const FAT: u32 = 0xCAFE_BABE;
const FAT_64: u32 = 0xCAFE_BABF;
const CPU_ARM64: u32 = 0x0100_000C;
const CPU_X86_64: u32 = 0x0100_0007;
const PE_AMD64: u16 = 0x8664;
const PE_ARM64: u16 = 0xAA64;

/// The architectures in a Mach-O (thin or fat) or PE binary header, or `None` for anything else.
pub fn architectures(header: &[u8]) -> Option<Vec<Architecture>> {
    let be32 = |at: usize| Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?));
    let le32 = |at: usize| Some(u32::from_le_bytes(header.get(at..at + 4)?.try_into().ok()?));
    let mach = |cpu: u32| match cpu {
        CPU_ARM64 => Architecture::Arm64,
        CPU_X86_64 => Architecture::X86_64,
        _ => Architecture::Other,
    };
    match be32(0)? {
        magic @ (FAT | FAT_64) => {
            let entry = if magic == FAT { 20 } else { 32 };
            let count = be32(4)? as usize;
            (0..count).map(|i| be32(8 + i * entry).map(mach)).collect()
        }
        _ if le32(0)? == MACH_O_64 => Some(vec![mach(le32(4)?)]),
        _ if header.starts_with(b"MZ") => {
            let pe = le32(0x3C)? as usize;
            if header.get(pe..pe + 4)? != b"PE\0\0" {
                return None;
            }
            let machine = u16::from_le_bytes(header.get(pe + 4..pe + 6)?.try_into().ok()?);
            Some(vec![match machine {
                PE_AMD64 => Architecture::X86_64,
                PE_ARM64 => Architecture::Arm64,
                _ => Architecture::Other,
            }])
        }
        _ => None,
    }
}

/// Path, size, and modification time of a file whose change means the bundle must be scanned
/// again. The bundle folder's own time is not used: it can stay the same when files inside change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub path: PathBuf,
    pub size: u64,
    pub modified_nanos: u128,
}

pub fn fingerprint(bundle: &Path, executable: &Path) -> Vec<FileStamp> {
    let contents = bundle.join("Contents");
    [
        executable.to_path_buf(),
        contents.join("Info.plist"),
        contents.join("Resources").join("moduleinfo.json"),
    ]
    .into_iter()
    .filter_map(|path| {
        let metadata = fs::metadata(&path).ok()?;
        let modified_nanos = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(FileStamp {
            path,
            size: metadata.len(),
            modified_nanos,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat_mach_o_lists_every_slice() {
        let mut header = FAT.to_be_bytes().to_vec();
        header.extend(2u32.to_be_bytes());
        for cpu in [CPU_X86_64, CPU_ARM64] {
            header.extend(cpu.to_be_bytes());
            header.extend([0; 16]);
        }
        assert_eq!(
            architectures(&header),
            Some(vec![Architecture::X86_64, Architecture::Arm64])
        );
    }

    #[test]
    fn thin_mach_o_and_pe_are_recognized() {
        let mut thin = MACH_O_64.to_le_bytes().to_vec();
        thin.extend(CPU_X86_64.to_le_bytes());
        assert_eq!(architectures(&thin), Some(vec![Architecture::X86_64]));

        let mut pe = vec![0; 0x80];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&PE_AMD64.to_le_bytes());
        assert_eq!(architectures(&pe), Some(vec![Architecture::X86_64]));
    }

    #[test]
    fn other_files_are_not_binaries() {
        assert_eq!(architectures(b"#!/bin/sh\n"), None);
        assert_eq!(architectures(&[]), None);
    }
}
