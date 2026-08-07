use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;

use blake3::Hasher;
use cap_std::fs::{Dir, OpenOptions};
use fns_protocol::{
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspacePath,
};

use crate::{
    ApplyId, ApplyReceipt, ContentCache, FileFingerprint, FsError, MemoryHashCache, ObservedEntry,
    RootedWorkspace,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedEntry {
    Missing,
    Present {
        kind: WorkspaceEntryKind,
        content_hash: Option<WorkspaceContentHash>,
        fingerprint: FileFingerprint,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FsOperation {
    UpsertFile {
        path: WorkspacePath,
        content_hash: WorkspaceContentHash,
        metadata: WorkspaceFileMetadata,
        expected: ExpectedEntry,
    },
    Mkdir {
        path: WorkspacePath,
        metadata: WorkspaceFileMetadata,
        expected: ExpectedEntry,
    },
    UpsertSymlink {
        path: WorkspacePath,
        content_hash: WorkspaceContentHash,
        metadata: WorkspaceFileMetadata,
        expected: ExpectedEntry,
    },
    Delete {
        path: WorkspacePath,
        expected: ExpectedEntry,
    },
    Rename {
        path: WorkspacePath,
        new_path: WorkspacePath,
        content_hash: Option<WorkspaceContentHash>,
        metadata: WorkspaceFileMetadata,
        source_expected: ExpectedEntry,
        target_expected: ExpectedEntry,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyObservation {
    Preimage,
    Postimage,
    Diverged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyCheckpoint {
    TempSynced,
    FilesystemCommitted,
}

pub trait ApplyObserver: Send + Sync {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint);
}

#[derive(Clone, Copy)]
struct NoopObserver;

impl ApplyObserver for NoopObserver {
    fn checkpoint(&self, _checkpoint: ApplyCheckpoint) {}
}

pub struct AtomicWorkspaceWriter {
    root: RootedWorkspace,
    content: ContentCache,
    observer: Arc<dyn ApplyObserver>,
}

impl AtomicWorkspaceWriter {
    pub fn new(root: RootedWorkspace, content: ContentCache) -> Self {
        Self::with_observer(root, content, Box::new(NoopObserver))
    }

    pub fn with_observer(
        root: RootedWorkspace,
        content: ContentCache,
        observer: Box<dyn ApplyObserver>,
    ) -> Self {
        Self {
            root,
            content,
            observer: Arc::from(observer),
        }
    }

    pub fn apply(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<ApplyReceipt, FsError> {
        let mut cache = MemoryHashCache::default();
        self.validate_preimage(operation, &mut cache)?;
        match operation {
            FsOperation::UpsertFile {
                path,
                content_hash,
                metadata,
                expected,
            } => self.apply_file(apply_id, path, content_hash, metadata, expected),
            FsOperation::Mkdir { path, metadata, .. } => self.apply_mkdir(apply_id, path, metadata),
            FsOperation::UpsertSymlink {
                path,
                content_hash,
                metadata,
                expected,
            } => self.apply_symlink(apply_id, path, content_hash, metadata, expected),
            FsOperation::Delete { path, expected } => self.apply_delete(apply_id, path, expected),
            FsOperation::Rename { .. } => self.apply_rename(apply_id, operation),
        }
    }

    pub fn observe(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<ApplyObservation, FsError> {
        if self.has_unrelated_artifact(apply_id, operation)? {
            return Ok(ApplyObservation::Diverged);
        }
        let mut cache = MemoryHashCache::default();
        if self.matches_preimage(operation, &mut cache)? {
            return Ok(ApplyObservation::Preimage);
        }
        if self.matches_postimage(apply_id, operation, &mut cache)? {
            return Ok(ApplyObservation::Postimage);
        }
        Ok(ApplyObservation::Diverged)
    }

    pub fn finalize(&self, receipt: &ApplyReceipt) -> Result<(), FsError> {
        let Some(name) = receipt.cleanup_name.as_deref() else {
            return Ok(());
        };
        let cleanup_path = WorkspacePath::parse(name).map_err(|_| FsError::PathEscape)?;
        let leaf = cleanup_path.as_str().rsplit('/').next().unwrap_or_default();
        if !leaf.starts_with(".fns-delete-") {
            return Err(FsError::PathEscape);
        }
        let expected_leaf = format!(".fns-delete-{}", receipt.apply_id.0);
        if leaf != expected_leaf {
            return Err(FsError::PathEscape);
        }
        let Some((parent, name, metadata)) = self.root.open_entry(&cleanup_path)? else {
            return Ok(());
        };
        if metadata.is_dir() {
            parent.remove_dir_all(&name).map_err(|_| FsError::Io {
                operation: "finalize directory delete",
            })?;
        } else {
            parent.remove_file(&name).map_err(|_| FsError::Io {
                operation: "finalize delete",
            })?;
        }
        sync_parent(&parent)
    }

    fn validate_preimage(
        &self,
        operation: &FsOperation,
        cache: &mut MemoryHashCache,
    ) -> Result<(), FsError> {
        if !self.matches_preimage(operation, cache)? {
            return Err(FsError::ContentMismatch);
        }
        Ok(())
    }

    fn matches_preimage(
        &self,
        operation: &FsOperation,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        match operation {
            FsOperation::UpsertFile { path, expected, .. }
            | FsOperation::Mkdir { path, expected, .. }
            | FsOperation::UpsertSymlink { path, expected, .. }
            | FsOperation::Delete { path, expected } => {
                self.matches_expected(path, expected, true, cache)
            }
            FsOperation::Rename {
                path,
                new_path,
                source_expected,
                target_expected,
                ..
            } => Ok(self.matches_expected(path, source_expected, true, cache)?
                && self.matches_expected(new_path, target_expected, true, cache)?),
        }
    }

    fn matches_postimage(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        match operation {
            FsOperation::UpsertFile {
                path,
                content_hash,
                metadata,
                ..
            } => self.matches_live(
                path,
                WorkspaceEntryKind::File,
                Some(content_hash),
                Some(metadata),
                cache,
            ),
            FsOperation::Mkdir { path, metadata, .. } => self.matches_live(
                path,
                WorkspaceEntryKind::Directory,
                None,
                Some(metadata),
                cache,
            ),
            FsOperation::UpsertSymlink {
                path,
                content_hash,
                metadata,
                ..
            } => self.matches_live(
                path,
                WorkspaceEntryKind::Symlink,
                Some(content_hash),
                Some(metadata),
                cache,
            ),
            FsOperation::Delete { path, .. } => {
                let Some((parent, _)) = self.root.open_parent(path, false).ok() else {
                    return Ok(false);
                };
                let tomb = format!(".fns-delete-{}", apply_id.0);
                Ok(self.root.inspect(path)?.is_none() && parent.symlink_metadata(tomb).is_ok())
            }
            FsOperation::Rename {
                path,
                new_path,
                content_hash,
                metadata,
                ..
            } => {
                let Some(observed) = self.root.inspect(new_path)? else {
                    return Ok(false);
                };
                Ok(self.root.inspect(path)?.is_none()
                    && observed.metadata == *metadata
                    && (!content_hash.is_some()
                        || self.matches_hash(new_path, &observed, content_hash.as_ref(), cache)?))
            }
        }
    }

    fn matches_expected(
        &self,
        path: &WorkspacePath,
        expected: &ExpectedEntry,
        strict_fingerprint: bool,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(observed) = self.root.inspect(path)? else {
            return Ok(matches!(expected, ExpectedEntry::Missing));
        };
        let ExpectedEntry::Present {
            kind,
            content_hash,
            fingerprint,
        } = expected
        else {
            return Ok(false);
        };
        if observed.kind != *kind || (strict_fingerprint && observed.fingerprint != *fingerprint) {
            return Ok(false);
        }
        self.matches_hash(path, &observed, content_hash.as_ref(), cache)
    }

    fn matches_live(
        &self,
        path: &WorkspacePath,
        kind: WorkspaceEntryKind,
        content_hash: Option<&WorkspaceContentHash>,
        metadata: Option<&WorkspaceFileMetadata>,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(observed) = self.root.inspect(path)? else {
            return Ok(false);
        };
        Ok(observed.kind == kind
            && metadata.is_none_or(|metadata| observed.metadata == *metadata)
            && self.matches_hash(path, &observed, content_hash, cache)?)
    }

    fn matches_hash(
        &self,
        path: &WorkspacePath,
        observed: &ObservedEntry,
        expected: Option<&WorkspaceContentHash>,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(expected) = expected else {
            return Ok(true);
        };
        let _ = cache;
        Ok(self.root.content_hash(path, observed)?.as_ref() == Some(expected))
    }

    fn apply_file(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        content_hash: &WorkspaceContentHash,
        metadata: &WorkspaceFileMetadata,
        expected: &ExpectedEntry,
    ) -> Result<ApplyReceipt, FsError> {
        let (parent, leaf) = self.parent_path(path)?;
        let temporary = format!(".fns-tmp-{}", apply_id.0);
        self.copy_blob_to_temp(&parent, &temporary, content_hash, metadata)?;
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = parent.remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        self.rename_checked(
            &parent,
            &temporary,
            &leaf,
            !matches!(expected, ExpectedEntry::Missing),
            apply_id,
            path,
            expected,
        )?;
        sync_parent(&parent)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, Some(content_hash.clone()), None)
    }

    fn apply_mkdir(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        _metadata: &WorkspaceFileMetadata,
    ) -> Result<ApplyReceipt, FsError> {
        let (parent, leaf) = self.parent_path(path)?;
        if let Ok(metadata) = parent.symlink_metadata(&leaf) {
            if !metadata.is_dir() {
                return Err(FsError::ContentMismatch);
            }
        } else {
            parent.create_dir(&leaf).map_err(|_| FsError::Io {
                operation: "create workspace directory",
            })?;
        }
        sync_parent(&parent)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, None, None)
    }

    fn apply_symlink(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        content_hash: &WorkspaceContentHash,
        metadata: &WorkspaceFileMetadata,
        expected: &ExpectedEntry,
    ) -> Result<ApplyReceipt, FsError> {
        let (parent, leaf) = self.parent_path(path)?;
        let target = self.read_link_target(content_hash, metadata.size)?;
        validate_relative_target(path, &target)?;
        let temporary = format!(".fns-tmp-{}", apply_id.0);
        create_symlink(&parent, &target, &temporary)?;
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = parent.remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        self.rename_checked(
            &parent,
            &temporary,
            &leaf,
            !matches!(expected, ExpectedEntry::Missing),
            apply_id,
            path,
            expected,
        )?;
        sync_parent(&parent)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, Some(content_hash.clone()), None)
    }

    fn apply_delete(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        expected: &ExpectedEntry,
    ) -> Result<ApplyReceipt, FsError> {
        let (parent, name, _) = self
            .root
            .open_entry(path)?
            .ok_or(FsError::ContentMismatch)?;
        let tomb_name = format!(".fns-delete-{}", apply_id.0);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            return Err(FsError::ContentMismatch);
        }
        parent
            .rename(&name, &parent, &tomb_name)
            .map_err(|_| FsError::Io {
                operation: "stage workspace delete",
            })?;
        sync_parent(&parent)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        let cleanup_name = path.as_str().rsplit_once('/').map_or_else(
            || tomb_name.clone(),
            |(parent, _)| format!("{parent}/{tomb_name}"),
        );
        Ok(ApplyReceipt {
            apply_id,
            touched: vec![path.clone()],
            postimages: vec![None],
            postimage_hashes: vec![None],
            cleanup_name: Some(cleanup_name),
        })
    }

    fn apply_rename(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<ApplyReceipt, FsError> {
        let FsOperation::Rename {
            path,
            new_path,
            content_hash,
            source_expected,
            target_expected,
            ..
        } = operation
        else {
            return Err(FsError::ContentMismatch);
        };
        let (source_parent, source_name, _) = self
            .root
            .open_entry(path)?
            .ok_or(FsError::ContentMismatch)?;
        let (target_parent, target_leaf) = self.parent_path(new_path)?;
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, source_expected, true, &mut cache)?
            || !self.matches_expected(new_path, target_expected, true, &mut cache)?
        {
            return Err(FsError::ContentMismatch);
        }
        source_parent
            .rename(&source_name, &target_parent, &target_leaf)
            .map_err(|_| FsError::Io {
                operation: "rename workspace entry",
            })?;
        sync_parent(&source_parent)?;
        sync_parent(&target_parent)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        let observed = self.root.inspect(new_path)?;
        Ok(ApplyReceipt {
            apply_id,
            touched: vec![path.clone(), new_path.clone()],
            postimages: vec![None, observed],
            postimage_hashes: vec![None, content_hash.clone()],
            cleanup_name: None,
        })
    }

    fn receipt_single(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        content_hash: Option<WorkspaceContentHash>,
        cleanup_name: Option<String>,
    ) -> Result<ApplyReceipt, FsError> {
        Ok(ApplyReceipt {
            apply_id,
            touched: vec![path.clone()],
            postimages: vec![Some(
                self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?,
            )],
            postimage_hashes: vec![content_hash],
            cleanup_name,
        })
    }

    fn parent_path(&self, path: &WorkspacePath) -> Result<(Dir, String), FsError> {
        self.root.open_parent(path, true)
    }

    fn copy_blob_to_temp(
        &self,
        parent: &Dir,
        temporary: &str,
        expected: &WorkspaceContentHash,
        metadata: &WorkspaceFileMetadata,
    ) -> Result<(), FsError> {
        let mut source = self.content.open_blob(expected)?;
        let mut options = OpenOptions::new();
        options.write(true).read(true).create_new(true);
        let destination = parent
            .open_with(temporary, &options)
            .map_err(|_| FsError::Io {
                operation: "create workspace staging",
            })?;
        let mut destination = destination.into_std();
        let mut hasher = Hasher::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; crate::hash::HASH_BUFFER_BYTES];
        loop {
            let count = source.read(&mut buffer).map_err(|_| FsError::Io {
                operation: "read content cache",
            })?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(FsError::SizeMismatch)?;
            hasher.update(&buffer[..count]);
            destination
                .write_all(&buffer[..count])
                .map_err(|_| FsError::Io {
                    operation: "write workspace staging",
                })?;
        }
        let actual = WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
            .map_err(|_| FsError::ContentMismatch)?;
        if actual != *expected || total != metadata.size {
            let _ = parent.remove_file(temporary);
            return Err(FsError::ContentMismatch);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if metadata.executable { 0o755 } else { 0o644 };
            destination
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|_| FsError::Io {
                    operation: "set workspace permissions",
                })?;
        }
        let seconds = metadata.modified_at_ms / 1_000;
        let nanos = (metadata.modified_at_ms % 1_000) as u32 * 1_000_000;
        filetime::set_file_handle_times(
            &destination,
            None,
            Some(filetime::FileTime::from_unix_time(seconds, nanos)),
        )
        .map_err(|_| FsError::Io {
            operation: "set workspace modified time",
        })?;
        destination.flush().map_err(|_| FsError::Io {
            operation: "flush workspace staging",
        })?;
        destination.sync_all().map_err(|_| FsError::Io {
            operation: "sync workspace staging",
        })
    }

    fn read_link_target(
        &self,
        expected: &WorkspaceContentHash,
        size: u64,
    ) -> Result<String, FsError> {
        let file = self.content.open_blob(expected)?;
        let mut bytes = Vec::new();
        file.take(4_097)
            .read_to_end(&mut bytes)
            .map_err(|_| FsError::Io {
                operation: "read symlink content",
            })?;
        if bytes.len() as u64 != size || bytes.len() > 4_096 {
            return Err(FsError::SizeMismatch);
        }
        let actual =
            WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(&bytes).to_hex()))
                .map_err(|_| FsError::ContentMismatch)?;
        if actual != *expected {
            return Err(FsError::ContentMismatch);
        }
        let target = String::from_utf8(bytes).map_err(|_| FsError::ContentMismatch)?;
        if target.contains('\0') {
            return Err(FsError::ContentMismatch);
        }
        Ok(target)
    }

    #[allow(clippy::too_many_arguments)]
    fn rename_checked(
        &self,
        parent: &Dir,
        temporary: &str,
        destination: &str,
        replace_existing: bool,
        _apply_id: ApplyId,
        _path: &WorkspacePath,
        _expected: &ExpectedEntry,
    ) -> Result<(), FsError> {
        if parent.symlink_metadata(destination).is_ok() && !replace_existing {
            let _ = parent.remove_file(temporary);
            return Err(FsError::ContentMismatch);
        }
        #[cfg(windows)]
        if replace_existing {
            let backup = format!(".fns-delete-{}", _apply_id.0);
            if parent.symlink_metadata(&backup).is_ok() {
                let _ = parent.remove_file(temporary);
                return Err(FsError::ContentMismatch);
            }
            parent
                .rename(destination, parent, &backup)
                .map_err(|_| FsError::ContentMismatch)?;
            let backup_path = sibling_path(_path, &backup)?;
            let mut cache = MemoryHashCache::default();
            if !self.matches_expected(&backup_path, _expected, true, &mut cache)? {
                let _ = parent.rename(&backup, parent, destination);
                let _ = parent.remove_file(temporary);
                return Err(FsError::ContentMismatch);
            }
            if let Err(error) = parent.rename(temporary, parent, destination) {
                let _ = parent.rename(&backup, parent, destination);
                return Err(FsError::Io {
                    operation: if error.kind() == std::io::ErrorKind::AlreadyExists {
                        "commit workspace staging"
                    } else {
                        "commit workspace staging"
                    },
                });
            }
            parent.remove_file(&backup).map_err(|_| FsError::Io {
                operation: "remove workspace replacement backup",
            })?;
            return Ok(());
        }
        parent.rename(temporary, parent, destination).map_err(|_| {
            let _ = parent.remove_file(temporary);
            FsError::Io {
                operation: "commit workspace staging",
            }
        })
    }

    fn has_unrelated_artifact(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<bool, FsError> {
        let check = |path: &WorkspacePath| -> Result<bool, FsError> {
            let Some((parent, _)) = self.root.open_parent(path, false).ok() else {
                return Ok(false);
            };
            let expected_temp = format!(".fns-tmp-{}", apply_id.0);
            let expected_tomb = format!(".fns-delete-{}", apply_id.0);
            for entry in parent.entries().map_err(|_| FsError::Io {
                operation: "read directory",
            })? {
                let entry = entry.map_err(|_| FsError::Io {
                    operation: "read directory entry",
                })?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if (name.starts_with(".fns-tmp-") || name.starts_with(".fns-delete-"))
                    && name != expected_temp
                    && name != expected_tomb
                {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        match operation {
            FsOperation::Rename { path, new_path, .. } => Ok(check(path)? || check(new_path)?),
            FsOperation::UpsertFile { path, .. }
            | FsOperation::Mkdir { path, .. }
            | FsOperation::UpsertSymlink { path, .. }
            | FsOperation::Delete { path, .. } => check(path),
        }
    }
}

fn validate_relative_target(path: &WorkspacePath, target: &str) -> Result<(), FsError> {
    let parent_depth = path.as_str().split('/').count().saturating_sub(1);
    let mut depth = parent_depth;
    for component in Path::new(target).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return Err(FsError::PathEscape),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(FsError::PathEscape);
                }
                depth -= 1;
            }
            Component::Normal(_) => depth += 1,
        }
    }
    Ok(())
}

fn create_symlink(parent: &Dir, target: &str, temporary: &str) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        parent.symlink(target, temporary).map_err(|_| FsError::Io {
            operation: "create workspace symlink",
        })
    }
    #[cfg(windows)]
    {
        parent
            .symlink_file(target, temporary)
            .map_err(|_| FsError::Io {
                operation: "create workspace symlink",
            })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, target, temporary);
        Err(FsError::Io {
            operation: "create workspace symlink",
        })
    }
}

fn sync_parent(parent: &Dir) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        parent
            .try_clone()
            .map_err(|_| FsError::Io {
                operation: "clone workspace parent",
            })?
            .into_std_file()
            .sync_all()
            .map_err(|_| FsError::Io {
                operation: "sync workspace parent",
            })?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

#[cfg(windows)]
fn sibling_path(path: &WorkspacePath, leaf: &str) -> Result<WorkspacePath, FsError> {
    let value = path
        .as_str()
        .rsplit_once('/')
        .map_or_else(|| leaf.to_owned(), |(parent, _)| format!("{parent}/{leaf}"));
    WorkspacePath::parse(&value).map_err(|_| FsError::PathEscape)
}
