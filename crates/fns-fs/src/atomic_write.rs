#[cfg(unix)]
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;

use blake3::Hasher;
use cap_fs_ext::{DirExt, SystemTimeSpec};
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
    PreimageValidated,
    DestinationBackedUp,
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
        let preimage_matches = self.matches_preimage(operation, &mut cache)?;
        if !preimage_matches {
            let mut postimage_cache = MemoryHashCache::default();
            if self.matches_postimage(apply_id, operation, &mut postimage_cache)? {
                return self.receipt_for_postimage(apply_id, operation);
            }
            if !matches!(operation, FsOperation::Rename { .. }) {
                return Err(FsError::ContentMismatch);
            }
        }
        match operation {
            FsOperation::UpsertFile {
                path,
                content_hash,
                metadata,
                expected,
            } => self.apply_file(apply_id, path, content_hash, metadata, expected),
            FsOperation::Mkdir {
                path,
                metadata,
                expected,
            } => self.apply_mkdir(apply_id, path, metadata, expected),
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
                let tomb_path = sibling_path(path, &tomb)?;
                let FsOperation::Delete { expected, .. } = operation else {
                    unreachable!();
                };
                let mut tomb_cache = MemoryHashCache::default();
                Ok(self.root.inspect(path)?.is_none()
                    && parent.symlink_metadata(tomb).is_ok()
                    && self.matches_expected(&tomb_path, expected, false, &mut tomb_cache)?)
            }
            FsOperation::Rename {
                path,
                new_path,
                content_hash,
                metadata,
                source_expected,
                ..
            } => {
                let kind = match source_expected {
                    ExpectedEntry::Present { kind, .. } => *kind,
                    ExpectedEntry::Missing => return Ok(false),
                };
                Ok(self.root.inspect(path)?.is_none()
                    && self.matches_live(
                        new_path,
                        kind,
                        content_hash.as_ref(),
                        Some(metadata),
                        cache,
                    )?)
            }
        }
    }

    fn receipt_for_postimage(
        &self,
        apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<ApplyReceipt, FsError> {
        let tomb_name = format!(".fns-delete-{}", apply_id.0);
        match operation {
            FsOperation::UpsertFile {
                path,
                content_hash,
                expected,
                metadata,
                ..
            } => {
                let observed = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
                let mut cache = MemoryHashCache::default();
                if !self.matches_observed(
                    path,
                    &observed,
                    WorkspaceEntryKind::File,
                    Some(content_hash),
                    Some(metadata),
                    &mut cache,
                )? {
                    return Err(FsError::ContentMismatch);
                }
                Ok(ApplyReceipt {
                    apply_id,
                    touched: vec![path.clone()],
                    postimages: vec![Some(observed)],
                    postimage_hashes: vec![Some(content_hash.clone())],
                    cleanup_name: matches!(expected, ExpectedEntry::Present { .. })
                        .then(|| sibling_path(path, &tomb_name))
                        .transpose()?
                        .map(|path| path.as_str().to_owned()),
                })
            }
            FsOperation::UpsertSymlink {
                path,
                content_hash,
                expected,
                metadata,
                ..
            } => {
                let observed = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
                let mut cache = MemoryHashCache::default();
                if !self.matches_observed(
                    path,
                    &observed,
                    WorkspaceEntryKind::Symlink,
                    Some(content_hash),
                    Some(metadata),
                    &mut cache,
                )? {
                    return Err(FsError::ContentMismatch);
                }
                Ok(ApplyReceipt {
                    apply_id,
                    touched: vec![path.clone()],
                    postimages: vec![Some(observed)],
                    postimage_hashes: vec![Some(content_hash.clone())],
                    cleanup_name: matches!(expected, ExpectedEntry::Present { .. })
                        .then(|| sibling_path(path, &tomb_name))
                        .transpose()?
                        .map(|path| path.as_str().to_owned()),
                })
            }
            FsOperation::Mkdir { path, metadata, .. } => {
                let observed = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
                let mut cache = MemoryHashCache::default();
                if !self.matches_observed(
                    path,
                    &observed,
                    WorkspaceEntryKind::Directory,
                    None,
                    Some(metadata),
                    &mut cache,
                )? {
                    return Err(FsError::ContentMismatch);
                }
                Ok(ApplyReceipt {
                    apply_id,
                    touched: vec![path.clone()],
                    postimages: vec![Some(observed)],
                    postimage_hashes: vec![None],
                    cleanup_name: None,
                })
            }
            FsOperation::Delete { path, .. } => Ok(ApplyReceipt {
                apply_id,
                touched: vec![path.clone()],
                postimages: vec![None],
                postimage_hashes: vec![None],
                cleanup_name: Some(sibling_path(path, &tomb_name)?.as_str().to_owned()),
            }),
            FsOperation::Rename {
                path,
                new_path,
                content_hash,
                metadata,
                source_expected,
                target_expected,
            } => {
                let kind = match source_expected {
                    ExpectedEntry::Present { kind, .. } => *kind,
                    ExpectedEntry::Missing => return Err(FsError::ContentMismatch),
                };
                let observed = self
                    .root
                    .inspect(new_path)?
                    .ok_or(FsError::ContentMismatch)?;
                let mut cache = MemoryHashCache::default();
                if !self.matches_observed(
                    new_path,
                    &observed,
                    kind,
                    content_hash.as_ref(),
                    Some(metadata),
                    &mut cache,
                )? {
                    return Err(FsError::ContentMismatch);
                }
                Ok(ApplyReceipt {
                    apply_id,
                    touched: vec![path.clone(), new_path.clone()],
                    postimages: vec![None, Some(observed)],
                    postimage_hashes: vec![None, content_hash.clone()],
                    cleanup_name: matches!(target_expected, ExpectedEntry::Present { .. })
                        .then(|| sibling_path(new_path, &tomb_name))
                        .transpose()?
                        .map(|path| path.as_str().to_owned()),
                })
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
        let fingerprint_matches = if strict_fingerprint {
            observed.fingerprint == *fingerprint
        } else {
            observed.fingerprint.file_id == fingerprint.file_id
                && observed.fingerprint.size == fingerprint.size
                && observed.fingerprint.modified_at_ns == fingerprint.modified_at_ns
        };
        if observed.kind != *kind || !fingerprint_matches {
            return Ok(false);
        }
        self.matches_hash(path, &observed, content_hash.as_ref(), cache)
    }

    fn matches_staged_expected(
        &self,
        path: &WorkspacePath,
        expected: &ExpectedEntry,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(observed) = self.root.inspect(path)? else {
            return Ok(false);
        };
        let ExpectedEntry::Present {
            kind,
            content_hash,
            fingerprint,
        } = expected
        else {
            return Ok(false);
        };
        Ok(observed.kind == *kind
            && observed.fingerprint.file_id == fingerprint.file_id
            && observed.fingerprint.size == fingerprint.size
            && self.matches_hash(path, &observed, content_hash.as_ref(), cache)?)
    }

    fn snapshot_expected(
        &self,
        path: &WorkspacePath,
        expected: &ExpectedEntry,
        cache: &mut MemoryHashCache,
    ) -> Result<Option<ObservedEntry>, FsError> {
        if !self.matches_expected(path, expected, true, cache)? {
            return Err(FsError::ContentMismatch);
        }
        self.root.inspect(path)
    }

    fn matches_snapshot(
        &self,
        path: &WorkspacePath,
        snapshot: &ObservedEntry,
        content_hash: Option<&WorkspaceContentHash>,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(observed) = self.root.inspect(path)? else {
            return Ok(false);
        };
        Ok(observed.kind == snapshot.kind
            && observed.metadata == snapshot.metadata
            && observed.fingerprint.file_id == snapshot.fingerprint.file_id
            && observed.fingerprint.size == snapshot.fingerprint.size
            && observed.fingerprint.modified_at_ns == snapshot.fingerprint.modified_at_ns
            && observed.symlink_target == snapshot.symlink_target
            && self.matches_hash(path, &observed, content_hash, cache)?)
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

    fn matches_observed(
        &self,
        path: &WorkspacePath,
        observed: &ObservedEntry,
        kind: WorkspaceEntryKind,
        content_hash: Option<&WorkspaceContentHash>,
        metadata: Option<&WorkspaceFileMetadata>,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        Ok(observed.kind == kind
            && metadata.is_none_or(|metadata| observed.metadata == *metadata)
            && self.matches_hash(path, observed, content_hash, cache)?)
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
        remove_if_exists(&parent, &temporary)?;
        self.copy_blob_to_temp(&parent, &temporary, content_hash, metadata)?;
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = parent.remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        let preimage = self.root.inspect(path)?;
        let preimage_hash = match expected {
            ExpectedEntry::Present { content_hash, .. } => content_hash.as_ref(),
            ExpectedEntry::Missing => None,
        };
        self.observer.checkpoint(ApplyCheckpoint::PreimageValidated);
        self.rename_checked(
            &parent,
            &temporary,
            &leaf,
            !matches!(expected, ExpectedEntry::Missing),
            apply_id,
            path,
            expected,
            preimage.as_ref(),
            preimage_hash,
        )?;
        sync_parent(&parent)?;
        let postimage = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, postimage, Some(content_hash.clone()), None)
    }

    fn apply_mkdir(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        metadata: &WorkspaceFileMetadata,
        expected: &ExpectedEntry,
    ) -> Result<ApplyReceipt, FsError> {
        let (parent, leaf) = self.parent_path(path)?;
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            return Err(FsError::ContentMismatch);
        }
        self.observer.checkpoint(ApplyCheckpoint::PreimageValidated);
        if matches!(expected, ExpectedEntry::Missing) {
            match parent.create_dir(&leaf) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(FsError::ContentMismatch);
                }
                Err(_) => {
                    return Err(FsError::Io {
                        operation: "create workspace directory",
                    });
                }
            }
        } else {
            let entry_metadata = parent
                .symlink_metadata(&leaf)
                .map_err(|_| FsError::ContentMismatch)?;
            if !entry_metadata.is_dir() {
                return Err(FsError::ContentMismatch);
            }
        }
        set_entry_metadata(&parent, &leaf, WorkspaceEntryKind::Directory, metadata)?;
        sync_parent(&parent)?;
        let postimage = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, postimage, None, None)
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
        remove_if_exists(&parent, &temporary)?;
        create_symlink(&parent, &target, &temporary)?;
        if let Err(error) = self.root.validate_symlink_target(path, &target) {
            let _ = remove_if_exists(&parent, &temporary);
            return Err(error);
        }
        if let Err(error) = set_entry_mtime(&parent, &temporary, metadata.modified_at_ms, true) {
            let _ = remove_if_exists(&parent, &temporary);
            return Err(error);
        }
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = parent.remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        let preimage = self.root.inspect(path)?;
        let preimage_hash = match expected {
            ExpectedEntry::Present { content_hash, .. } => content_hash.as_ref(),
            ExpectedEntry::Missing => None,
        };
        self.observer.checkpoint(ApplyCheckpoint::PreimageValidated);
        self.rename_checked(
            &parent,
            &temporary,
            &leaf,
            !matches!(expected, ExpectedEntry::Missing),
            apply_id,
            path,
            expected,
            preimage.as_ref(),
            preimage_hash,
        )?;
        sync_parent(&parent)?;
        let postimage = self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        self.receipt_single(apply_id, path, postimage, Some(content_hash.clone()), None)
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
        let preimage = self
            .snapshot_expected(path, expected, &mut cache)?
            .ok_or(FsError::ContentMismatch)?;
        self.observer.checkpoint(ApplyCheckpoint::PreimageValidated);
        if !self.matches_expected(path, expected, true, &mut cache)? {
            return Err(FsError::ContentMismatch);
        }
        if parent.symlink_metadata(&tomb_name).is_ok() {
            return Err(FsError::ContentMismatch);
        }
        self.observer
            .checkpoint(ApplyCheckpoint::DestinationBackedUp);
        rename_noreplace(&parent, &name, &parent, &tomb_name).map_err(|_| FsError::Io {
            operation: "stage workspace delete",
        })?;
        sync_parent(&parent)?;
        let tomb_path = sibling_path(path, &tomb_name)?;
        let mut moved_cache = MemoryHashCache::default();
        let content_hash = match expected {
            ExpectedEntry::Present { content_hash, .. } => content_hash.as_ref(),
            ExpectedEntry::Missing => None,
        };
        if !self.matches_snapshot(&tomb_path, &preimage, content_hash, &mut moved_cache)? {
            restore_moved_if_missing(&parent, &tomb_name, &parent, &name);
            return Err(FsError::ContentMismatch);
        }
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
            metadata,
            source_expected,
            target_expected,
            ..
        } = operation
        else {
            return Err(FsError::ContentMismatch);
        };
        if path == new_path {
            return Err(FsError::ContentMismatch);
        }
        let (source_parent, source_name) = self.root.open_parent(path, false)?;
        let (target_parent, target_leaf) = self.parent_path(new_path)?;
        let target_backup = format!(".fns-delete-{}", apply_id.0);
        let source_backup = format!(".fns-rename-{}", apply_id.0);
        let source_backup_path = sibling_path(path, &source_backup)?;
        let target_backup_path = sibling_path(new_path, &target_backup)?;
        let source_staged = source_parent.symlink_metadata(&source_backup).is_ok();
        let target_staged = target_parent.symlink_metadata(&target_backup).is_ok();
        let source_hash = match source_expected {
            ExpectedEntry::Present { content_hash, .. } => content_hash.as_ref(),
            ExpectedEntry::Missing => None,
        };
        let target_hash = match target_expected {
            ExpectedEntry::Present { content_hash, .. } => content_hash.as_ref(),
            ExpectedEntry::Missing => None,
        };
        let source_kind = match source_expected {
            ExpectedEntry::Present { kind, .. } => *kind,
            ExpectedEntry::Missing => return Err(FsError::ContentMismatch),
        };

        // A crash after the source backup was moved to the destination leaves
        // no source entry to validate. Resume that same apply when the
        // destination still contains the expected source postimage.
        if !source_staged && self.root.inspect(path)?.is_none() {
            let destination = self.root.inspect(new_path)?;
            let mut destination_cache = MemoryHashCache::default();
            let destination_committed = destination.as_ref().is_some_and(|observed| {
                observed.kind == source_kind
                    && self
                        .matches_hash(
                            new_path,
                            observed,
                            content_hash.as_ref(),
                            &mut destination_cache,
                        )
                        .unwrap_or(false)
            });
            if destination_committed {
                set_entry_metadata(&target_parent, &target_leaf, source_kind, metadata)?;
                let mut postimage_cache = MemoryHashCache::default();
                if !self.matches_live(
                    new_path,
                    source_kind,
                    content_hash.as_ref(),
                    Some(metadata),
                    &mut postimage_cache,
                )? {
                    return Err(FsError::ContentMismatch);
                }
                let cleanup_name = if target_staged {
                    let cleanup_name = target_backup_path.as_str().to_owned();
                    if remove_entry(&target_parent, &target_backup).is_ok() {
                        None
                    } else {
                        Some(cleanup_name)
                    }
                } else {
                    None
                };
                sync_parent(&target_parent)?;
                let observed = self.root.inspect(new_path)?;
                self.observer
                    .checkpoint(ApplyCheckpoint::FilesystemCommitted);
                return Ok(ApplyReceipt {
                    apply_id,
                    touched: vec![path.clone(), new_path.clone()],
                    postimages: vec![None, observed],
                    postimage_hashes: vec![None, content_hash.clone()],
                    cleanup_name,
                });
            }
        }

        let source_preimage = if source_staged {
            let mut cache = MemoryHashCache::default();
            if !self.matches_staged_expected(&source_backup_path, source_expected, &mut cache)? {
                return Err(FsError::ContentMismatch);
            }
            self.root
                .inspect(&source_backup_path)?
                .ok_or(FsError::ContentMismatch)?
        } else {
            let mut cache = MemoryHashCache::default();
            if !self.matches_expected(path, source_expected, true, &mut cache)? {
                return Err(FsError::ContentMismatch);
            }
            self.root.inspect(path)?.ok_or(FsError::ContentMismatch)?
        };
        let target_preimage = if target_staged {
            let mut cache = MemoryHashCache::default();
            if !self.matches_staged_expected(&target_backup_path, target_expected, &mut cache)? {
                return Err(FsError::ContentMismatch);
            }
            Some(
                self.root
                    .inspect(&target_backup_path)?
                    .ok_or(FsError::ContentMismatch)?,
            )
        } else {
            let mut cache = MemoryHashCache::default();
            if !self.matches_expected(new_path, target_expected, true, &mut cache)? {
                return Err(FsError::ContentMismatch);
            }
            self.root.inspect(new_path)?
        };
        self.observer.checkpoint(ApplyCheckpoint::PreimageValidated);

        let mut live_cache = MemoryHashCache::default();
        if !source_staged && !self.matches_expected(path, source_expected, true, &mut live_cache)? {
            return Err(FsError::ContentMismatch);
        }
        if !target_staged
            && !self.matches_expected(new_path, target_expected, true, &mut live_cache)?
        {
            return Err(FsError::ContentMismatch);
        }

        let target_had_entry = target_preimage.is_some();
        if !target_staged && target_had_entry {
            rename_noreplace(&target_parent, &target_leaf, &target_parent, &target_backup)
                .map_err(|_| FsError::ContentMismatch)?;
            sync_parent(&target_parent)?;
            let mut target_cache = MemoryHashCache::default();
            let Some(target_preimage) = target_preimage.as_ref() else {
                restore_if_missing(&target_parent, &target_backup, &target_leaf);
                return Err(FsError::ContentMismatch);
            };
            if !self.matches_snapshot(
                &target_backup_path,
                target_preimage,
                target_hash,
                &mut target_cache,
            )? {
                restore_if_missing(&target_parent, &target_backup, &target_leaf);
                return Err(FsError::ContentMismatch);
            }
            let target_baseline = self
                .root
                .inspect(&target_backup_path)?
                .ok_or(FsError::ContentMismatch)?;
            self.observer
                .checkpoint(ApplyCheckpoint::DestinationBackedUp);
            if !self.matches_snapshot(
                &target_backup_path,
                &target_baseline,
                target_hash,
                &mut target_cache,
            )? {
                restore_if_missing(&target_parent, &target_backup, &target_leaf);
                return Err(FsError::ContentMismatch);
            }
        } else {
            let target_baseline = if target_staged {
                Some(
                    self.root
                        .inspect(&target_backup_path)?
                        .ok_or(FsError::ContentMismatch)?,
                )
            } else {
                None
            };
            self.observer
                .checkpoint(ApplyCheckpoint::DestinationBackedUp);
            if let Some(target_baseline) = target_baseline.as_ref() {
                let mut target_cache = MemoryHashCache::default();
                if !self.matches_snapshot(
                    &target_backup_path,
                    target_baseline,
                    target_hash,
                    &mut target_cache,
                )? {
                    restore_if_missing(&target_parent, &target_backup, &target_leaf);
                    return Err(FsError::ContentMismatch);
                }
            }
        }

        if !source_staged {
            rename_noreplace(&source_parent, &source_name, &source_parent, &source_backup)
                .map_err(|_| {
                    restore_if_missing(&target_parent, &target_backup, &target_leaf);
                    FsError::Io {
                        operation: "stage workspace rename source",
                    }
                })?;
            sync_parent(&source_parent)?;
            let mut source_cache = MemoryHashCache::default();
            if !self.matches_snapshot(
                &source_backup_path,
                &source_preimage,
                source_hash,
                &mut source_cache,
            )? {
                restore_source_if_missing(
                    &source_parent,
                    &source_backup,
                    &source_parent,
                    &source_name,
                    source_kind,
                    &source_preimage.metadata,
                );
                restore_if_missing(&target_parent, &target_backup, &target_leaf);
                return Err(FsError::ContentMismatch);
            }
        }

        if let Err(error) =
            set_entry_metadata(&source_parent, &source_backup, source_kind, metadata)
        {
            restore_source_if_missing(
                &source_parent,
                &source_backup,
                &source_parent,
                &source_name,
                source_kind,
                &source_preimage.metadata,
            );
            restore_if_missing(&target_parent, &target_backup, &target_leaf);
            return Err(error);
        }
        let mut source_postimage_cache = MemoryHashCache::default();
        if !self.matches_live(
            &source_backup_path,
            source_kind,
            content_hash.as_ref(),
            Some(metadata),
            &mut source_postimage_cache,
        )? {
            restore_source_if_missing(
                &source_parent,
                &source_backup,
                &source_parent,
                &source_name,
                source_kind,
                &source_preimage.metadata,
            );
            restore_if_missing(&target_parent, &target_backup, &target_leaf);
            return Err(FsError::ContentMismatch);
        }
        sync_parent(&source_parent)?;

        if rename_noreplace(&source_parent, &source_backup, &target_parent, &target_leaf).is_err() {
            restore_source_if_missing(
                &source_parent,
                &source_backup,
                &source_parent,
                &source_name,
                source_kind,
                &source_preimage.metadata,
            );
            restore_if_missing(&target_parent, &target_backup, &target_leaf);
            return Err(FsError::Io {
                operation: "rename workspace entry",
            });
        }
        sync_parent(&source_parent)?;
        sync_parent(&target_parent)?;
        let mut postimage_cache = MemoryHashCache::default();
        let postimage_matches = self.matches_live(
            new_path,
            source_kind,
            content_hash.as_ref(),
            Some(metadata),
            &mut postimage_cache,
        )?;
        if !postimage_matches {
            restore_source_if_missing(
                &target_parent,
                &target_leaf,
                &source_parent,
                &source_name,
                source_kind,
                &source_preimage.metadata,
            );
            restore_if_missing(&target_parent, &target_backup, &target_leaf);
            return Err(FsError::ContentMismatch);
        }
        let cleanup_name = if target_had_entry {
            let cleanup_name = target_backup_path.as_str().to_owned();
            if remove_entry(&target_parent, &target_backup).is_ok() {
                sync_parent(&target_parent)?;
                None
            } else {
                sync_parent(&target_parent)?;
                Some(cleanup_name)
            }
        } else {
            None
        };
        let observed = self
            .root
            .inspect(new_path)?
            .ok_or(FsError::ContentMismatch)?;
        self.observer
            .checkpoint(ApplyCheckpoint::FilesystemCommitted);
        Ok(ApplyReceipt {
            apply_id,
            touched: vec![path.clone(), new_path.clone()],
            postimages: vec![None, Some(observed)],
            postimage_hashes: vec![None, content_hash.clone()],
            cleanup_name,
        })
    }

    fn receipt_single(
        &self,
        apply_id: ApplyId,
        path: &WorkspacePath,
        postimage: ObservedEntry,
        content_hash: Option<WorkspaceContentHash>,
        cleanup_name: Option<String>,
    ) -> Result<ApplyReceipt, FsError> {
        Ok(ApplyReceipt {
            apply_id,
            touched: vec![path.clone()],
            postimages: vec![Some(postimage)],
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
        let result = self.copy_blob_to_temp_inner(parent, temporary, expected, metadata);
        if result.is_err() {
            let _ = remove_if_exists(parent, temporary);
        }
        result
    }

    fn copy_blob_to_temp_inner(
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
        preimage: Option<&ObservedEntry>,
        preimage_hash: Option<&WorkspaceContentHash>,
    ) -> Result<(), FsError> {
        if parent.symlink_metadata(destination).is_ok() && !replace_existing {
            let _ = parent.remove_file(temporary);
            return Err(FsError::ContentMismatch);
        }
        if replace_existing {
            let mut live_cache = MemoryHashCache::default();
            match self.matches_expected(_path, _expected, true, &mut live_cache) {
                Ok(true) => {}
                Ok(false) => {
                    let _ = parent.remove_file(temporary);
                    return Err(FsError::ContentMismatch);
                }
                Err(FsError::PathEscape) => {
                    let _ = remove_if_exists(parent, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::PathEscape);
                }
                Err(error) => {
                    let _ = remove_if_exists(parent, temporary);
                    return Err(error);
                }
            }
            let backup = format!(".fns-delete-{}", _apply_id.0);
            if parent.symlink_metadata(&backup).is_ok() {
                let _ = parent.remove_file(temporary);
                return Err(FsError::ContentMismatch);
            }
            rename_noreplace(parent, destination, parent, &backup)
                .map_err(|_| FsError::ContentMismatch)?;
            sync_parent(parent)?;
            let backup_path = sibling_path(_path, &backup)?;
            let mut cache = MemoryHashCache::default();
            let Some(preimage) = preimage else {
                restore_if_missing(parent, &backup, destination);
                let _ = parent.remove_file(temporary);
                return Err(FsError::ContentMismatch);
            };
            match self.matches_snapshot(&backup_path, preimage, preimage_hash, &mut cache) {
                Ok(true) => {}
                Ok(false) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::ContentMismatch);
                }
                Err(FsError::PathEscape) => {
                    let _ = remove_if_exists(parent, &backup);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::PathEscape);
                }
                Err(error) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(error);
                }
            }
            let backup_baseline = match self.root.inspect(&backup_path) {
                Ok(Some(observed)) => observed,
                Ok(None) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::ContentMismatch);
                }
                Err(FsError::PathEscape) => {
                    let _ = remove_if_exists(parent, &backup);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::PathEscape);
                }
                Err(error) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(error);
                }
            };
            self.observer
                .checkpoint(ApplyCheckpoint::DestinationBackedUp);
            match self.matches_snapshot(&backup_path, &backup_baseline, preimage_hash, &mut cache) {
                Ok(true) => {}
                Ok(false) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::ContentMismatch);
                }
                Err(FsError::PathEscape) => {
                    let _ = remove_if_exists(parent, &backup);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(FsError::PathEscape);
                }
                Err(error) => {
                    restore_if_missing(parent, &backup, destination);
                    let _ = remove_if_exists(parent, temporary);
                    return Err(error);
                }
            }
            if rename_noreplace(parent, temporary, parent, destination).is_err() {
                restore_if_missing(parent, &backup, destination);
                let _ = parent.remove_file(temporary);
                return Err(FsError::Io {
                    operation: "commit workspace staging",
                });
            }
            sync_parent(parent)?;
            remove_entry(parent, &backup)?;
            sync_parent(parent)?;
            return Ok(());
        }
        rename_noreplace(parent, temporary, parent, destination).map_err(|_| {
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
        let (allow_temp, allow_tomb, allow_rename) = match operation {
            FsOperation::UpsertFile { expected, .. }
            | FsOperation::UpsertSymlink { expected, .. } => (
                true,
                matches!(expected, ExpectedEntry::Present { .. }),
                false,
            ),
            FsOperation::Delete { .. } => (false, true, false),
            FsOperation::Rename {
                target_expected, ..
            } => (
                false,
                matches!(target_expected, ExpectedEntry::Present { .. }),
                true,
            ),
            FsOperation::Mkdir { .. } => (false, false, false),
        };
        let check = |path: &WorkspacePath| -> Result<bool, FsError> {
            let Some((parent, _)) = self.root.open_parent(path, false).ok() else {
                return Ok(false);
            };
            let expected_temp = format!(".fns-tmp-{}", apply_id.0);
            let expected_tomb = format!(".fns-delete-{}", apply_id.0);
            let expected_rename = format!(".fns-rename-{}", apply_id.0);
            for entry in parent.entries().map_err(|_| FsError::Io {
                operation: "read directory",
            })? {
                let entry = entry.map_err(|_| FsError::Io {
                    operation: "read directory entry",
                })?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let internal = name.starts_with(".fns-tmp-")
                    || name.starts_with(".fns-delete-")
                    || name.starts_with(".fns-rename-");
                let allowed = (allow_temp && name == expected_temp)
                    || (allow_tomb && name == expected_tomb)
                    || (allow_rename && name == expected_rename);
                if internal && !allowed {
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
    #[cfg(test)]
    run_sync_parent_test_hook(parent);
    Ok(())
}

#[cfg(test)]
type SyncParentTestHook = Box<dyn Fn(&Dir) + Send + Sync>;

#[cfg(test)]
static SYNC_PARENT_TEST_HOOK: std::sync::OnceLock<std::sync::Mutex<Option<SyncParentTestHook>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn run_sync_parent_test_hook(parent: &Dir) {
    if let Some(hook) = SYNC_PARENT_TEST_HOOK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("sync parent test hook is not poisoned")
        .as_ref()
    {
        hook(parent);
    }
}

fn sibling_path(path: &WorkspacePath, leaf: &str) -> Result<WorkspacePath, FsError> {
    let value = path
        .as_str()
        .rsplit_once('/')
        .map_or_else(|| leaf.to_owned(), |(parent, _)| format!("{parent}/{leaf}"));
    WorkspacePath::parse(&value).map_err(|_| FsError::PathEscape)
}

fn remove_entry(parent: &Dir, name: &str) -> Result<(), FsError> {
    let metadata = parent.symlink_metadata(name).map_err(|_| FsError::Io {
        operation: "stat workspace cleanup entry",
    })?;
    if metadata.is_dir() {
        parent.remove_dir_all(name).map_err(|_| FsError::Io {
            operation: "remove workspace replacement backup",
        })
    } else {
        parent.remove_file(name).map_err(|_| FsError::Io {
            operation: "remove workspace replacement backup",
        })
    }
}

fn rename_noreplace(
    old_parent: &Dir,
    old_name: &str,
    new_parent: &Dir,
    new_name: &str,
) -> std::io::Result<()> {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "redox"
    ))]
    {
        let old_parent = old_parent.try_clone()?.into_std_file();
        let new_parent = new_parent.try_clone()?.into_std_file();
        rustix::fs::renameat_with(
            &old_parent,
            old_name,
            &new_parent,
            new_name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        Ok(())
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "redox"
    )))]
    {
        old_parent.rename(old_name, new_parent, new_name)
    }
}

fn remove_if_exists(parent: &Dir, name: &str) -> Result<(), FsError> {
    if parent.symlink_metadata(name).is_err() {
        return Ok(());
    }
    remove_entry(parent, name)?;
    sync_parent(parent)
}

fn restore_if_missing(parent: &Dir, backup: &str, destination: &str) {
    if parent.symlink_metadata(destination).is_err()
        && parent.symlink_metadata(backup).is_ok()
        && rename_noreplace(parent, backup, parent, destination).is_ok()
    {
        let _ = sync_parent(parent);
    }
}

fn restore_moved_if_missing(from_parent: &Dir, from_name: &str, to_parent: &Dir, to_name: &str) {
    if from_parent.symlink_metadata(from_name).is_ok()
        && to_parent.symlink_metadata(to_name).is_err()
        && rename_noreplace(from_parent, from_name, to_parent, to_name).is_ok()
    {
        let _ = sync_parent(from_parent);
        let _ = sync_parent(to_parent);
    }
}

fn restore_source_if_missing(
    from_parent: &Dir,
    from_name: &str,
    to_parent: &Dir,
    to_name: &str,
    kind: WorkspaceEntryKind,
    metadata: &WorkspaceFileMetadata,
) {
    if from_parent.symlink_metadata(from_name).is_ok()
        && to_parent.symlink_metadata(to_name).is_err()
        && rename_noreplace(from_parent, from_name, to_parent, to_name).is_ok()
    {
        let _ = set_entry_metadata(to_parent, to_name, kind, metadata);
        let _ = sync_parent(from_parent);
        let _ = sync_parent(to_parent);
    }
}

fn set_entry_mtime(
    parent: &Dir,
    name: &str,
    modified_at_ms: i64,
    symlink: bool,
) -> Result<(), FsError> {
    let time = if modified_at_ms >= 0 {
        std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_millis(modified_at_ms as u64))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    } else {
        std::time::SystemTime::UNIX_EPOCH
            .checked_sub(std::time::Duration::from_millis(
                modified_at_ms.unsigned_abs(),
            ))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    };
    let time = SystemTimeSpec::Absolute(cap_std::time::SystemTime::from_std(time));
    let result = if symlink {
        parent.set_symlink_times(name, None, Some(time))
    } else {
        parent.set_times(name, None, Some(time))
    };
    result.map_err(|_| FsError::Io {
        operation: "set workspace modified time",
    })
}

fn set_entry_metadata(
    parent: &Dir,
    name: &str,
    kind: WorkspaceEntryKind,
    metadata: &WorkspaceFileMetadata,
) -> Result<(), FsError> {
    #[cfg(unix)]
    if kind == WorkspaceEntryKind::File {
        use std::os::unix::fs::PermissionsExt;

        parent
            .open(name)
            .map_err(|_| FsError::Io {
                operation: "open workspace file for metadata",
            })?
            .into_std()
            .set_permissions(fs::Permissions::from_mode(if metadata.executable {
                0o755
            } else {
                0o644
            }))
            .map_err(|_| FsError::Io {
                operation: "set workspace permissions",
            })?;
    }
    set_entry_mtime(
        parent,
        name,
        metadata.modified_at_ms,
        kind == WorkspaceEntryKind::Symlink,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn rename_cleanup_syncs_parent_after_backup_removal() {
        let root_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        fs::write(root_dir.path().join("old"), b"old").unwrap();
        fs::write(root_dir.path().join("new"), b"target").unwrap();
        let content = ContentCache::open(state_dir.path()).unwrap();
        let source_hash =
            WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(b"old").to_hex()))
                .unwrap();
        content
            .import(&source_hash, 3, std::io::Cursor::new(b"old"))
            .unwrap();
        let root = RootedWorkspace::open(root_dir.path()).unwrap();
        let source_path = WorkspacePath::parse("old").unwrap();
        let target_path = WorkspacePath::parse("new").unwrap();
        let source_observed = root.inspect(&source_path).unwrap().unwrap();
        let target_observed = root.inspect(&target_path).unwrap().unwrap();
        let source_expected = ExpectedEntry::Present {
            kind: source_observed.kind,
            content_hash: Some(source_hash.clone()),
            fingerprint: source_observed.fingerprint,
        };
        let target_expected = ExpectedEntry::Present {
            kind: target_observed.kind,
            content_hash: Some(
                WorkspaceContentHash::parse(&format!(
                    "blake3:{}",
                    blake3::hash(b"target").to_hex()
                ))
                .unwrap(),
            ),
            fingerprint: target_observed.fingerprint,
        };
        let apply_id = ApplyId(uuid::Uuid::new_v4());
        let backup = root_dir.path().join(format!(".fns-delete-{}", apply_id.0));
        let saw_post_cleanup_sync = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hook_flag = std::sync::Arc::clone(&saw_post_cleanup_sync);
        *SYNC_PARENT_TEST_HOOK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(Box::new(move |_| {
            if !backup.exists() {
                hook_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }));

        let writer = AtomicWorkspaceWriter::new(root, content);
        writer
            .apply(
                apply_id,
                &FsOperation::Rename {
                    path: source_path,
                    new_path: target_path,
                    content_hash: Some(source_hash),
                    metadata: WorkspaceFileMetadata {
                        size: 3,
                        modified_at_ms: 0,
                        executable: false,
                    },
                    source_expected,
                    target_expected,
                },
            )
            .unwrap();
        *SYNC_PARENT_TEST_HOOK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = None;

        assert!(saw_post_cleanup_sync.load(std::sync::atomic::Ordering::SeqCst));
    }
}
