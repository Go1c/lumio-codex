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
    path: PathBuf,
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

pub struct ContentCache {
    blob_dir: PathBuf,
    temp_dir: PathBuf,
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
                let _ = fs::remove_file(&temporary);
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
            let _ = fs::remove_file(temporary);
            return Err(FsError::SizeMismatch);
        }
        if hash != *expected {
            let _ = fs::remove_file(temporary);
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
    ) -> Result<(PathBuf, WorkspaceContentHash, u64), FsError> {
        if observed.kind == WorkspaceEntryKind::Symlink {
            let bytes = observed.symlink_target.as_deref().unwrap_or_default();
            return self.stream_bytes(bytes, observer);
        }
        let native = root.native_path(path)?.ok_or(FsError::Io {
            operation: "open workspace entry",
        })?;
        let mut file = File::open(native).map_err(|_| FsError::Io {
            operation: "open workspace entry",
        })?;
        self.stream_reader(&mut file, observer)
    }

    fn stream_reader<R: Read, F: FnMut()>(
        &self,
        reader: &mut R,
        mut observer: F,
    ) -> Result<(PathBuf, WorkspaceContentHash, u64), FsError> {
        let temporary = self.temp_file()?;
        let temporary_path = temporary.path.clone();
        let mut writer = temporary.file;
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
                let _ = fs::remove_file(&temporary_path);
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
        Ok((temporary_path, hash, total))
    }

    fn stream_bytes<F: FnMut()>(
        &self,
        bytes: &[u8],
        observer: F,
    ) -> Result<(PathBuf, WorkspaceContentHash, u64), FsError> {
        self.stream_reader(&mut std::io::Cursor::new(bytes), observer)
    }

    fn temp_file(&self) -> Result<CacheTempFile, FsError> {
        for _ in 0..8 {
            let path = self
                .temp_dir
                .join(format!(".fns-tmp-{}", uuid::Uuid::new_v4()));
            match File::options()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok(CacheTempFile { file, path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
        Err(FsError::Io {
            operation: "create content staging",
        })
    }

    fn commit_temporary(
        &self,
        temporary: PathBuf,
        hash: &WorkspaceContentHash,
        size: u64,
    ) -> Result<PathBuf, FsError> {
        let final_path = self.blob_path(hash);
        if fs::metadata(&final_path).is_ok() {
            if !self.blob_matches(hash, size)? {
                let _ = fs::remove_file(&temporary);
                return Err(FsError::ContentMismatch);
            }
            let _ = fs::remove_file(&temporary);
            return Ok(final_path);
        }
        fs::rename(&temporary, &final_path).map_err(|_| FsError::Io {
            operation: "commit content cache",
        })?;
        Ok(final_path)
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

    use fns_protocol::WorkspacePath;

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
}
