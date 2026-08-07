use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use blake3::Hasher;
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
        _apply_id: ApplyId,
        operation: &FsOperation,
    ) -> Result<ApplyObservation, FsError> {
        let mut cache = MemoryHashCache::default();
        if self.matches_preimage(operation, &mut cache)? {
            return Ok(ApplyObservation::Preimage);
        }
        if self.matches_postimage(operation, &mut cache)? {
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
        let Some(tomb) = self.root.native_path(&cleanup_path)? else {
            return Ok(());
        };
        let metadata = match fs::symlink_metadata(&tomb) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(FsError::Io {
                    operation: "stat delete staging",
                });
            }
        };
        if metadata.is_dir() {
            fs::remove_dir_all(&tomb).map_err(|_| FsError::Io {
                operation: "finalize directory delete",
            })?;
        } else {
            fs::remove_file(&tomb).map_err(|_| FsError::Io {
                operation: "finalize delete",
            })?;
        }
        sync_parent(tomb.parent().ok_or(FsError::PathEscape)?)
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
        operation: &FsOperation,
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        match operation {
            FsOperation::UpsertFile {
                path, content_hash, ..
            } => self.matches_live(path, WorkspaceEntryKind::File, Some(content_hash), cache),
            FsOperation::Mkdir { path, .. } => {
                self.matches_live(path, WorkspaceEntryKind::Directory, None, cache)
            }
            FsOperation::UpsertSymlink {
                path, content_hash, ..
            } => self.matches_live(path, WorkspaceEntryKind::Symlink, Some(content_hash), cache),
            FsOperation::Delete { path, .. } => Ok(self.root.inspect(path)?.is_none()),
            FsOperation::Rename {
                path,
                new_path,
                content_hash,
                ..
            } => {
                let Some(observed) = self.root.inspect(new_path)? else {
                    return Ok(false);
                };
                Ok(self.root.inspect(path)?.is_none()
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
        cache: &mut MemoryHashCache,
    ) -> Result<bool, FsError> {
        let Some(observed) = self.root.inspect(path)? else {
            return Ok(false);
        };
        Ok(observed.kind == kind && self.matches_hash(path, &observed, content_hash, cache)?)
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
        let descriptor = self
            .content
            .stage_workspace_entry(&self.root, path, cache)?;
        Ok(descriptor.content_hash == *expected
            && descriptor.metadata.size == observed.metadata.size)
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
        let temporary = parent.join(format!(".fns-tmp-{}", apply_id.0));
        self.copy_blob_to_temp(&temporary, content_hash, metadata)?;
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = fs::remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        self.rename_checked(
            &temporary,
            &parent.join(leaf),
            !matches!(expected, ExpectedEntry::Missing),
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
        let destination = parent.join(&leaf);
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if !metadata.is_dir() {
                return Err(FsError::ContentMismatch);
            }
        } else {
            fs::create_dir(&destination).map_err(|_| FsError::Io {
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
        let temporary = parent.join(format!(".fns-tmp-{}", apply_id.0));
        create_symlink(&target, &temporary)?;
        self.observer.checkpoint(ApplyCheckpoint::TempSynced);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            let _ = fs::remove_file(&temporary);
            return Err(FsError::ContentMismatch);
        }
        self.rename_checked(
            &temporary,
            &parent.join(leaf),
            !matches!(expected, ExpectedEntry::Missing),
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
        let native = self
            .root
            .native_path(path)?
            .ok_or(FsError::ContentMismatch)?;
        let parent = native.parent().ok_or(FsError::PathEscape)?;
        let tomb_name = format!(".fns-delete-{}", apply_id.0);
        let tomb = parent.join(&tomb_name);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, expected, true, &mut cache)? {
            return Err(FsError::ContentMismatch);
        }
        fs::rename(&native, &tomb).map_err(|_| FsError::Io {
            operation: "stage workspace delete",
        })?;
        sync_parent(parent)?;
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
        let source = self
            .root
            .native_path(path)?
            .ok_or(FsError::ContentMismatch)?;
        let (target_parent, target_leaf) = self.parent_path(new_path)?;
        let target = target_parent.join(target_leaf);
        let mut cache = MemoryHashCache::default();
        if !self.matches_expected(path, source_expected, true, &mut cache)?
            || !self.matches_expected(new_path, target_expected, true, &mut cache)?
        {
            return Err(FsError::ContentMismatch);
        }
        fs::rename(&source, &target).map_err(|_| FsError::Io {
            operation: "rename workspace entry",
        })?;
        sync_parent(source.parent().ok_or(FsError::PathEscape)?)?;
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

    fn parent_path(&self, path: &WorkspacePath) -> Result<(PathBuf, String), FsError> {
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let leaf = components.last().ok_or(FsError::InvalidPath {
            reason: "empty_path".to_owned(),
        })?;
        let mut current = self.root.root_path().to_path_buf();
        for (index, component) in components[..components.len() - 1].iter().enumerate() {
            let prefix =
                WorkspacePath::parse(&components[..=index].join("/")).map_err(|error| {
                    FsError::InvalidPath {
                        reason: error.reason,
                    }
                })?;
            let next = match self.root.resolve_child_name(&current, component, &prefix)? {
                Some(next) => next,
                None => {
                    let next = current.join(component);
                    fs::create_dir(&next).map_err(|_| FsError::Io {
                        operation: "create workspace parent",
                    })?;
                    next
                }
            };
            let metadata = fs::symlink_metadata(&next).map_err(|_| FsError::Io {
                operation: "stat workspace parent",
            })?;
            if metadata.file_type().is_symlink() {
                return Err(FsError::UnsupportedSymlink);
            }
            if !metadata.is_dir() {
                return Err(FsError::ContentMismatch);
            }
            current = next;
        }
        Ok((current, (*leaf).to_owned()))
    }

    fn copy_blob_to_temp(
        &self,
        temporary: &Path,
        expected: &WorkspaceContentHash,
        metadata: &WorkspaceFileMetadata,
    ) -> Result<(), FsError> {
        let mut source = self.content.open_blob(expected)?;
        let mut destination = File::options()
            .write(true)
            .read(true)
            .create_new(true)
            .open(temporary)
            .map_err(|_| FsError::Io {
                operation: "create workspace staging",
            })?;
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
            let _ = fs::remove_file(temporary);
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

    fn rename_checked(
        &self,
        temporary: &Path,
        destination: &Path,
        replace_existing: bool,
    ) -> Result<(), FsError> {
        if fs::symlink_metadata(destination).is_ok() && !replace_existing {
            let _ = fs::remove_file(temporary);
            return Err(FsError::ContentMismatch);
        }
        #[cfg(windows)]
        if replace_existing {
            let _ = fs::remove_file(destination);
        }
        fs::rename(temporary, destination).map_err(|_| {
            let _ = fs::remove_file(temporary);
            FsError::Io {
                operation: "commit workspace staging",
            }
        })
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

fn create_symlink(target: &str, temporary: &Path) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, temporary).map_err(|_| FsError::Io {
            operation: "create workspace symlink",
        })
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, temporary).map_err(|_| FsError::Io {
            operation: "create workspace symlink",
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, temporary);
        Err(FsError::Io {
            operation: "create workspace symlink",
        })
    }
}

fn sync_parent(parent: &Path) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| FsError::Io {
                operation: "sync workspace parent",
            })?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}
