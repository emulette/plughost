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

#[cfg(test)]
mod tests;

/// Increment when the meaning or representation of native results changes.
const SCHEMA: u32 = 2;

#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    schema: u32,
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
    Ok(decode(&bytes))
}

pub(super) fn decode(bytes: &[u8]) -> Cache {
    serde_json::from_slice::<Cache>(bytes)
        .ok()
        .filter(|cache| cache.schema == SCHEMA && cache.architecture == std::env::consts::ARCH)
        .unwrap_or_default()
}

pub(super) struct Writer {
    path: PathBuf,
    parent: PathBuf,
    cache: Cache,
    /// A snapshot was published since the last sync.
    unsynced: bool,
    // Keep the stable sidecar inode: unlinking it would let another writer lock a different file.
    lock: File,
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing alone releases the lock only once no other reference to this open file
        // remains, and a process another thread is starting can briefly share it.
        let _ = self.lock.unlock();
    }
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
        let mut bundles = read(path)?.bundles;
        let cached = bundles.len();
        // An inaccessible bundle may still exist; prune only confirmed removals.
        bundles.retain(|bundle, _| !matches!(bundle.try_exists(), Ok(false)));
        let mut writer = Self {
            path: path.to_path_buf(),
            parent: parent.to_path_buf(),
            cache: Cache {
                schema: SCHEMA,
                architecture: std::env::consts::ARCH.to_owned(),
                bundles,
            },
            unsynced: false,
            lock,
        };
        if writer.cache.bundles.len() != cached {
            writer.publish()?;
        }
        Ok(writer)
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
        self.publish()
    }

    /// Replaces the snapshot on disk with this writer's cache.
    fn publish(&mut self) -> io::Result<()> {
        snapshot(&self.path, &self.parent, &self.cache)?;
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

/// Serialization completes before the old snapshot is replaced. Kept separate from the cache
/// model so interruption tests can pause the serializer while the temporary is being written.
fn snapshot(path: &Path, parent: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".plughost-cache-")
        .tempfile_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
        writer.flush()?;
    }
    // Unlike tempfile's persist, std's rename replaces a snapshot a reader holds open on Windows.
    // Dropping the renamed temporary path then finds nothing to delete.
    let temporary = temporary.into_temp_path();
    fs::rename(&temporary, path)
}
