use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use blake3::Hasher;
use fns_protocol::{
    MAX_BLOB_BYTES, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspacePath,
};
use thiserror::Error;

use crate::{FileFingerprint, FsError, ObservedEntry, RootedWorkspace};

pub const HASH_BUFFER_BYTES: usize = 262_144;

struct CacheTempFile {
    file: File,
    _lock_file: File,
    path: PathBuf,
    lock_path: PathBuf,
    keep: bool,
}

impl CacheTempFile {
    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for CacheTempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
        let _ = fs::remove_file(&self.lock_path);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HashCacheError {
    #[error("hash cache I/O failed")]
    Io,
    #[error("hash cache entry is invalid")]
    Invalid,
}

pub trait HashCache {
    fn lookup(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
    ) -> Result<Option<WorkspaceContentHash>, HashCacheError>;
    fn store(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
        hash: &WorkspaceContentHash,
    ) -> Result<(), HashCacheError>;
    fn invalidate(&mut self, path: &WorkspacePath) -> Result<(), HashCacheError>;
}

#[derive(Default)]
pub struct MemoryHashCache {
    entries: HashMap<String, (FileFingerprint, WorkspaceContentHash)>,
    hit_count: usize,
}

impl MemoryHashCache {
    pub fn hits(&self) -> usize {
        self.hit_count
    }
}

impl HashCache for MemoryHashCache {
    fn lookup(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
    ) -> Result<Option<WorkspaceContentHash>, HashCacheError> {
        let Some((stored_fingerprint, hash)) = self.entries.get(path.as_str()) else {
            return Ok(None);
        };
        if stored_fingerprint == fingerprint {
            self.hit_count += 1;
            return Ok(Some(hash.clone()));
        }
        Ok(None)
    }

    fn store(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
        hash: &WorkspaceContentHash,
    ) -> Result<(), HashCacheError> {
        self.entries.insert(
            path.as_str().to_owned(),
            (fingerprint.clone(), hash.clone()),
        );
        Ok(())
    }

    fn invalidate(&mut self, path: &WorkspacePath) -> Result<(), HashCacheError> {
        self.entries.remove(path.as_str());
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentDescriptor {
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
    pub metadata: WorkspaceFileMetadata,
    pub fingerprint: FileFingerprint,
}

#[derive(Clone)]
pub struct ContentCache {
    blob_dir: PathBuf,
    temp_dir: PathBuf,
}

/// A crash-cleaned content-cache import that is not visible to readers until
/// it has been sealed and explicitly committed.
pub struct StagedContentImport {
    cache: ContentCache,
    temporary: CacheTempFile,
    expected: WorkspaceContentHash,
    expected_size: u64,
    hasher: Hasher,
    written: u64,
}

/// A fully written, fsynced, size/hash-verified import awaiting its commit
/// point. Dropping it abandons the staging file.
pub struct SealedContentImport {
    cache: ContentCache,
    temporary: CacheTempFile,
    expected: WorkspaceContentHash,
    size: u64,
}

impl StagedContentImport {
    pub fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), FsError> {
        let written = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or(FsError::SizeMismatch)?;
        if written > self.expected_size || written > MAX_BLOB_BYTES {
            return Err(FsError::SizeMismatch);
        }
        self.temporary
            .file
            .write_all(bytes)
            .map_err(|_| FsError::Io {
                operation: "write content staging",
            })?;
        self.hasher.update(bytes);
        self.written = written;
        Ok(())
    }

    pub fn seal(mut self) -> Result<SealedContentImport, FsError> {
        if self.written != self.expected_size {
            return Err(FsError::SizeMismatch);
        }
        let actual =
            WorkspaceContentHash::parse(&format!("blake3:{}", self.hasher.finalize().to_hex()))
                .map_err(|_| FsError::ContentMismatch)?;
        if actual != self.expected {
            return Err(FsError::ContentMismatch);
        }
        self.temporary.file.flush().map_err(|_| FsError::Io {
            operation: "flush content staging",
        })?;
        self.temporary.file.sync_all().map_err(|_| FsError::Io {
            operation: "sync content staging",
        })?;
        Ok(SealedContentImport {
            cache: self.cache,
            temporary: self.temporary,
            expected: self.expected,
            size: self.expected_size,
        })
    }
}

impl SealedContentImport {
    pub fn commit(self) -> Result<ContentDescriptor, FsError> {
        self.cache
            .commit_temporary(self.temporary, &self.expected, self.size)?;
        Ok(ContentDescriptor {
            content_hash: self.expected,
            size: self.size,
            metadata: WorkspaceFileMetadata {
                size: self.size,
                modified_at_ms: 0,
                executable: false,
            },
            fingerprint: synthetic_fingerprint(self.size),
        })
    }
}

impl ContentCache {
    pub fn open(state_dir: &Path) -> Result<Self, FsError> {
        let blob_dir = state_dir.join("blobs");
        let temp_dir = state_dir.join("tmp");
        fs::create_dir_all(&blob_dir).map_err(|_| FsError::Io {
            operation: "create blob cache",
        })?;
        fs::create_dir_all(&temp_dir).map_err(|_| FsError::Io {
            operation: "create blob staging",
        })?;
        if let Ok(entries) = fs::read_dir(&temp_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if !name.starts_with(".fns-tmp-") {
                    continue;
                }
                let base_name = name
                    .strip_suffix(".lock")
                    .or_else(|| name.strip_suffix(".staging"))
                    .unwrap_or(name);
                let base_path = temp_dir.join(base_name);
                let staging_path = temp_dir.join(format!("{base_name}.staging"));
                let lock_path = temp_dir.join(format!("{base_name}.lock"));
                let lock_available = match File::options().read(true).write(true).open(&lock_path) {
                    Ok(lock) => lock.try_lock().is_ok(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                if lock_available {
                    let _ = fs::remove_file(base_path);
                    let _ = fs::remove_file(staging_path);
                    let _ = fs::remove_file(lock_path);
                }
            }
        }
        Ok(Self { blob_dir, temp_dir })
    }

    pub fn stage_workspace_entry<C: HashCache>(
        &self,
        root: &RootedWorkspace,
        path: &WorkspacePath,
        cache: &mut C,
    ) -> Result<ContentDescriptor, FsError> {
        self.stage_workspace_entry_with_observer(root, path, cache, || {})
    }

    fn stage_workspace_entry_with_observer<C: HashCache, F: FnMut()>(
        &self,
        root: &RootedWorkspace,
        path: &WorkspacePath,
        cache: &mut C,
        mut observer: F,
    ) -> Result<ContentDescriptor, FsError> {
        let observed = root.inspect(path)?.ok_or(FsError::Io {
            operation: "stage missing entry",
        })?;
        if let Some(hash) = cache
            .lookup(path, &observed.fingerprint)
            .map_err(|_| FsError::Io {
                operation: "read hash cache",
            })?
        {
            if self.blob_matches(&hash, observed.metadata.size)? {
                return Ok(ContentDescriptor {
                    content_hash: hash,
                    size: observed.metadata.size,
                    metadata: observed.metadata,
                    fingerprint: observed.fingerprint,
                });
            }
            cache.invalidate(path).map_err(|_| FsError::Io {
                operation: "invalidate hash cache",
            })?;
        }

        for attempt in 0..2 {
            let before = root.inspect(path)?.ok_or(FsError::Io {
                operation: "stage missing entry",
            })?;
            let (temporary, hash, size) =
                self.stream_observed(root, path, &before, &mut observer)?;
            let after = root.inspect(path)?.ok_or(FsError::Io {
                operation: "stage missing entry",
            })?;
            if before.fingerprint != after.fingerprint {
                drop(temporary);
                if attempt == 0 {
                    continue;
                }
                return Err(FsError::UnstableFile { path: path.clone() });
            }
            let final_path = self.commit_temporary(temporary, &hash, size)?;
            let descriptor = ContentDescriptor {
                content_hash: hash.clone(),
                size,
                metadata: after.metadata,
                fingerprint: after.fingerprint.clone(),
            };
            cache
                .store(path, &after.fingerprint, &hash)
                .map_err(|_| FsError::Io {
                    operation: "write hash cache",
                })?;
            let _ = final_path;
            return Ok(descriptor);
        }
        Err(FsError::UnstableFile { path: path.clone() })
    }

    pub fn import<R: Read>(
        &self,
        expected: &WorkspaceContentHash,
        size: u64,
        mut reader: R,
    ) -> Result<ContentDescriptor, FsError> {
        let (temporary, hash, actual_size) = self.stream_reader(&mut reader, || {})?;
        if actual_size != size {
            drop(temporary);
            return Err(FsError::SizeMismatch);
        }
        if hash != *expected {
            drop(temporary);
            return Err(FsError::ContentMismatch);
        }
        self.commit_temporary(temporary, expected, size)?;
        let metadata = WorkspaceFileMetadata {
            size,
            modified_at_ms: 0,
            executable: false,
        };
        Ok(ContentDescriptor {
            content_hash: expected.clone(),
            size,
            metadata,
            fingerprint: synthetic_fingerprint(size),
        })
    }

    pub fn begin_staged_import(
        &self,
        expected: WorkspaceContentHash,
        size: u64,
    ) -> Result<StagedContentImport, FsError> {
        WorkspaceContentHash::parse(expected.as_str()).map_err(|_| FsError::ContentMismatch)?;
        if size > MAX_BLOB_BYTES {
            return Err(FsError::SizeMismatch);
        }
        Ok(StagedContentImport {
            cache: self.clone(),
            temporary: self.temp_file()?,
            expected,
            expected_size: size,
            hasher: Hasher::new(),
            written: 0,
        })
    }

    pub fn open_blob(&self, hash: &WorkspaceContentHash) -> Result<File, FsError> {
        let _ = WorkspaceContentHash::parse(hash.as_str()).map_err(|_| FsError::ContentMismatch)?;
        File::open(self.blob_path(hash)).map_err(|_| FsError::Io {
            operation: "open content cache",
        })
    }

    fn stream_observed<F: FnMut()>(
        &self,
        root: &RootedWorkspace,
        path: &WorkspacePath,
        observed: &ObservedEntry,
        observer: F,
    ) -> Result<(CacheTempFile, WorkspaceContentHash, u64), FsError> {
        if observed.kind == WorkspaceEntryKind::Symlink {
            let bytes = observed.symlink_target.as_deref().unwrap_or_default();
            return self.stream_bytes(bytes, observer);
        }
        let (parent, name, _) = root.open_entry(path)?.ok_or(FsError::Io {
            operation: "open workspace entry",
        })?;
        let mut file = parent.open(&name).map_err(|_| FsError::Io {
            operation: "open workspace entry",
        })?;
        self.stream_reader(&mut file, observer)
    }

    fn stream_reader<R: Read, F: FnMut()>(
        &self,
        reader: &mut R,
        mut observer: F,
    ) -> Result<(CacheTempFile, WorkspaceContentHash, u64), FsError> {
        let mut temporary = self.temp_file()?;
        let writer = &mut temporary.file;
        let mut hasher = Hasher::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; HASH_BUFFER_BYTES];
        loop {
            let count = reader.read(&mut buffer).map_err(|_| FsError::Io {
                operation: "read workspace content",
            })?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(FsError::SizeMismatch)?;
            if total > MAX_BLOB_BYTES {
                return Err(FsError::SizeMismatch);
            }
            hasher.update(&buffer[..count]);
            writer
                .write_all(&buffer[..count])
                .map_err(|_| FsError::Io {
                    operation: "write content cache",
                })?;
            observer();
        }
        writer.flush().map_err(|_| FsError::Io {
            operation: "flush content cache",
        })?;
        writer.sync_all().map_err(|_| FsError::Io {
            operation: "sync content cache",
        })?;
        let hash = WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
            .map_err(|_| FsError::ContentMismatch)?;
        Ok((temporary, hash, total))
    }

    fn stream_bytes<F: FnMut()>(
        &self,
        bytes: &[u8],
        observer: F,
    ) -> Result<(CacheTempFile, WorkspaceContentHash, u64), FsError> {
        self.stream_reader(&mut std::io::Cursor::new(bytes), observer)
    }

    fn temp_file(&self) -> Result<CacheTempFile, FsError> {
        for _ in 0..8 {
            let path = self
                .temp_dir
                .join(format!(".fns-tmp-{}", uuid::Uuid::new_v4()));
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
            ));
            let staging_path = path.with_file_name(format!(
                "{}.staging",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
            ));
            let lock_file = match File::options()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            };
            if lock_file.try_lock().is_err() {
                drop(lock_file);
                let _ = fs::remove_file(&lock_path);
                continue;
            }
            match File::options()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&staging_path)
            {
                Ok(file) => {
                    if fs::rename(&staging_path, &path).is_err() {
                        drop(file);
                        drop(lock_file);
                        let _ = fs::remove_file(&staging_path);
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                    return Ok(CacheTempFile {
                        file,
                        _lock_file: lock_file,
                        path,
                        lock_path,
                        keep: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    drop(lock_file);
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                Err(_) => {
                    drop(lock_file);
                    let _ = fs::remove_file(&lock_path);
                    break;
                }
            }
        }
        Err(FsError::Io {
            operation: "create content staging",
        })
    }

    fn commit_temporary(
        &self,
        mut temporary: CacheTempFile,
        hash: &WorkspaceContentHash,
        size: u64,
    ) -> Result<PathBuf, FsError> {
        let result = (|| {
            let final_path = self.blob_path(hash);
            if fs::metadata(&final_path).is_ok() {
                if !self.blob_matches(hash, size)? {
                    return Err(FsError::ContentMismatch);
                }
                let temporary_path = temporary.path.clone();
                temporary.keep();
                let _ = fs::remove_file(temporary_path);
                return Ok(final_path);
            }
            let temporary_path = temporary.path.clone();
            fs::rename(&temporary_path, &final_path).map_err(|_| FsError::Io {
                operation: "commit content cache",
            })?;
            temporary.keep();
            Ok(final_path)
        })();
        if result.is_err() {
            drop(temporary);
        }
        result
    }

    fn blob_matches(&self, hash: &WorkspaceContentHash, size: u64) -> Result<bool, FsError> {
        let metadata = match fs::metadata(self.blob_path(hash)) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => {
                return Err(FsError::Io {
                    operation: "stat content cache",
                });
            }
        };
        if metadata.len() != size {
            return Ok(false);
        }
        let mut file = File::open(self.blob_path(hash)).map_err(|_| FsError::Io {
            operation: "open content cache",
        })?;
        let mut hasher = Hasher::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; HASH_BUFFER_BYTES];
        loop {
            let count = file.read(&mut buffer).map_err(|_| FsError::Io {
                operation: "read content cache",
            })?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(FsError::SizeMismatch)?;
            hasher.update(&buffer[..count]);
        }
        let actual = WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
            .map_err(|_| FsError::ContentMismatch)?;
        Ok(total == size && actual == *hash)
    }

    fn blob_path(&self, hash: &WorkspaceContentHash) -> PathBuf {
        self.blob_dir
            .join(hash.as_str().trim_start_matches("blake3:"))
    }
}

fn synthetic_fingerprint(size: u64) -> FileFingerprint {
    #[cfg(unix)]
    let file_id = crate::NativeFileId::Unix {
        device: 0,
        inode: 0,
    };
    #[cfg(windows)]
    let file_id = crate::NativeFileId::Windows {
        volume_serial: 0,
        file_index: 0,
    };
    FileFingerprint {
        file_id,
        size,
        modified_at_ns: 0,
        changed_at_ns: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use fns_protocol::{WorkspaceContentHash, WorkspacePath};

    use super::{ContentCache, HASH_BUFFER_BYTES, MemoryHashCache};
    use crate::{FsError, RootedWorkspace};

    #[test]
    fn moving_file_is_never_cached() {
        let root_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let native_path = root_dir.path().join("moving");
        fs::write(&native_path, vec![b'a'; HASH_BUFFER_BYTES * 2]).unwrap();
        let root = RootedWorkspace::open(root_dir.path()).unwrap();
        let content = ContentCache::open(state_dir.path()).unwrap();
        let workspace_path = WorkspacePath::parse("moving").unwrap();
        let mut cache = MemoryHashCache::default();
        let mut reads = 0;
        let mut rewrites = 0;

        let result =
            content.stage_workspace_entry_with_observer(&root, &workspace_path, &mut cache, || {
                reads += 1;
                if reads == 1 || reads == 3 {
                    rewrites += 1;
                    fs::write(&native_path, vec![b'b'; HASH_BUFFER_BYTES * 2]).unwrap();
                }
            });

        assert!(matches!(result, Err(FsError::UnstableFile { .. })));
        assert_eq!(rewrites, 2);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn commit_failure_removes_temporary_staging() {
        let state_dir = tempfile::tempdir().unwrap();
        let content = ContentCache::open(state_dir.path()).unwrap();
        let hash = WorkspaceContentHash::parse(
            "blake3:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let mut temporary = content.temp_file().unwrap();
        temporary.file.write_all(b"x").unwrap();
        temporary.file.sync_all().unwrap();
        let temporary_path = temporary.path.clone();
        let final_path = content.blob_path(&hash);
        fs::create_dir(&final_path).unwrap();
        let size = fs::metadata(&final_path).unwrap().len();

        assert!(content.commit_temporary(temporary, &hash, size).is_err());
        assert!(!temporary_path.exists());
    }

    #[test]
    fn cache_temp_guard_survives_post_read_failure_and_open_sweep() {
        let root_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let native_path = root_dir.path().join("moving");
        fs::write(&native_path, vec![b'a'; HASH_BUFFER_BYTES * 2]).unwrap();
        let root = RootedWorkspace::open(root_dir.path()).unwrap();
        let content = ContentCache::open(state_dir.path()).unwrap();
        let workspace_path = WorkspacePath::parse("moving").unwrap();
        let mut cache = MemoryHashCache::default();
        let mut removed = false;

        let result =
            content.stage_workspace_entry_with_observer(&root, &workspace_path, &mut cache, || {
                if !removed {
                    fs::remove_file(&native_path).unwrap();
                    removed = true;
                }
            });
        assert!(matches!(result, Err(FsError::Io { .. })));
        let leaked = fs::read_dir(state_dir.path().join("tmp"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(".fns-tmp-"));

        let active = content.temp_file().unwrap();
        let active_path = active.path.clone();
        let _reopened = ContentCache::open(state_dir.path()).unwrap();
        let active_removed = !active_path.exists();

        assert!(!leaked, "post-read failure leaked a temporary staging file");
        assert!(!active_removed, "open sweep removed active staging");
    }
}
