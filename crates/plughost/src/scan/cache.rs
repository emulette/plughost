//! Native scan snapshots. One writer owns a cache path while discovery reads atomic snapshots.
//!
//! Each changed result is published with an atomic rename before its completion is announced, so
//! a scanner process that exits abruptly keeps the results it announced. Snapshots reach stable
//! storage once, when the writer finishes: plugin code runs in helpers, so a per-result sync would
//! only guard against power loss, at the cost of one sync per bundle. After a power loss the file
//! can be missing or malformed; it then reads as empty and the next scan rebuilds it.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ScanOutcome;
use super::bundle::FileStamp;

#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    schema: u32,
    library: String,
    architecture: String,
    pub bundles: BTreeMap<PathBuf, Entry>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub fingerprint: Vec<FileStamp>,
    pub outcome: ScanOutcome,
    /// Consecutive failed scans of this unchanged bundle.
    pub failures: u32,
}

fn current() -> (String, String) {
    (
        env!("CARGO_PKG_VERSION").to_owned(),
        std::env::consts::ARCH.to_owned(),
    )
}

/// Discovery tolerates a missing, unreadable, malformed or outdated cache.
pub fn load(path: &Path) -> Cache {
    read(path).unwrap_or_default()
}

/// Writers must not replace an unreadable existing file with an empty cache.
fn read(path: &Path) -> io::Result<Cache> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Cache::default()),
        Err(error) => return Err(error),
    };
    let (library, architecture) = current();
    Ok(serde_json::from_slice::<Cache>(&bytes)
        .ok()
        .filter(|cache| {
            cache.schema == 2 && cache.library == library && cache.architecture == architecture
        })
        .unwrap_or_default())
}

pub(super) struct Writer {
    path: PathBuf,
    parent: PathBuf,
    cache: Cache,
    /// A snapshot was published since the last sync.
    unsynced: bool,
    // Keep the stable sidecar inode: unlinking it would let another writer lock a different file.
    _lock: File,
}
impl Writer {
    /// Contention fails immediately with WouldBlock. The lock is released by closing the file,
    /// including when the scanning process exits abnormally. Readers need no lock.
    pub fn open(path: &Path) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock().map_err(io::Error::from)?;
        let (library, architecture) = current();
        let cache = Cache {
            schema: 2,
            library,
            architecture,
            bundles: read(path)?.bundles,
        };
        Ok(Self {
            path: path.to_path_buf(),
            parent: parent.to_path_buf(),
            cache,
            unsynced: false,
            _lock: lock,
        })
    }

    pub fn entry(&self, path: &Path) -> Option<&Entry> {
        self.cache.bundles.get(path)
    }

    /// Publishes a complete snapshot before completion is announced. On error the scanner must
    /// stop using this writer.
    pub fn update(&mut self, path: &Path, entry: Option<Entry>) -> io::Result<()> {
        if self.entry(path) == entry.as_ref() {
            return Ok(());
        }
        match entry {
            Some(entry) => self.cache.bundles.insert(path.to_path_buf(), entry),
            None => self.cache.bundles.remove(path),
        };
        let mut temporary = tempfile::Builder::new()
            .prefix(".plughost-cache-")
            .tempfile_in(&self.parent)?;
        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            serde_json::to_writer(&mut writer, &self.cache).map_err(io::Error::other)?;
            writer.flush()?;
        }
        // Unlike tempfile's persist, std's rename replaces a snapshot a reader holds open on Windows.
        // Dropping the renamed temporary path then finds nothing to delete.
        let temporary = temporary.into_temp_path();
        fs::rename(&temporary, &self.path)?;
        self.unsynced = true;
        Ok(())
    }

    /// Syncs the last published snapshot and its directory entry to stable storage.
    pub fn finish(self) -> io::Result<()> {
        if self.unsynced {
            // Windows flushes only through a handle with write access.
            File::options().write(true).open(&self.path)?.sync_all()?;
            #[cfg(unix)]
            File::open(&self.parent)?.sync_all()?;
        }
        Ok(())
    }
}
