use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use cap_std::ambient_authority;
use fns_protocol::{
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspacePath,
};
use unicode_normalization::UnicodeNormalization;

use crate::{FsError, SyncRules, WorkspaceScan, scan::scan_workspace};

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum NativeFileId {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume_serial: u64, file_index: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FileFingerprint {
    pub file_id: NativeFileId,
    pub size: u64,
    pub modified_at_ns: i128,
    pub changed_at_ns: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedEntry {
    pub path: WorkspacePath,
    pub kind: WorkspaceEntryKind,
    pub metadata: WorkspaceFileMetadata,
    pub fingerprint: FileFingerprint,
    pub symlink_target: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaseSensitivity {
    Sensitive,
    Insensitive,
}

pub struct RootedWorkspace {
    canonical_root: PathBuf,
    capability: cap_std::fs::Dir,
    case_sensitivity: CaseSensitivity,
}

impl RootedWorkspace {
    pub fn open(root: &Path) -> Result<Self, FsError> {
        let metadata = fs::symlink_metadata(root).map_err(|_| FsError::Io {
            operation: "stat root",
        })?;
        if metadata.file_type().is_symlink() {
            return Err(FsError::RootSymlink);
        }
        if !metadata.is_dir() {
            return Err(FsError::RootNotDirectory);
        }
        let canonical_root = fs::canonicalize(root).map_err(|_| FsError::Io {
            operation: "canonicalize root",
        })?;
        let capability = cap_std::fs::Dir::open_ambient_dir(&canonical_root, ambient_authority())
            .map_err(|_| FsError::Io {
            operation: "open root",
        })?;
        Ok(Self {
            canonical_root,
            capability,
            case_sensitivity: if cfg!(windows) {
                CaseSensitivity::Insensitive
            } else {
                CaseSensitivity::Sensitive
            },
        })
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    pub fn inspect(&self, path: &WorkspacePath) -> Result<Option<ObservedEntry>, FsError> {
        let Some((parent, name, metadata)) = self.resolve_cap_entry(path)? else {
            return Ok(None);
        };
        Ok(Some(self.observed_cap(
            path.clone(),
            &parent,
            &name,
            metadata,
        )?))
    }

    pub fn scan(&self, rules: &SyncRules) -> Result<WorkspaceScan, FsError> {
        scan_workspace(self, rules)
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.canonical_root
    }

    pub(crate) fn native_path(&self, path: &WorkspacePath) -> Result<Option<PathBuf>, FsError> {
        self.resolve_native_path(path)
    }

    pub(crate) fn open_parent(
        &self,
        path: &WorkspacePath,
        create_missing: bool,
    ) -> Result<(cap_std::fs::Dir, String), FsError> {
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let leaf = components.last().ok_or(FsError::InvalidPath {
            reason: "empty_path".to_owned(),
        })?;
        let mut current = self.capability.try_clone().map_err(|_| FsError::Io {
            operation: "clone root capability",
        })?;
        for (index, component) in components[..components.len() - 1].iter().enumerate() {
            let prefix =
                WorkspacePath::parse(&components[..=index].join("/")).map_err(|error| {
                    FsError::InvalidPath {
                        reason: error.reason,
                    }
                })?;
            let actual = match self.find_cap_child(&current, component, &prefix)? {
                Some(actual) => actual,
                None if create_missing => {
                    current.create_dir(component).map_err(|_| FsError::Io {
                        operation: "create workspace parent",
                    })?;
                    (*component).to_owned()
                }
                None => return Err(FsError::ContentMismatch),
            };
            let metadata = current.symlink_metadata(&actual).map_err(|_| FsError::Io {
                operation: "stat workspace parent",
            })?;
            if metadata.file_type().is_symlink() {
                return Err(FsError::UnsupportedSymlink);
            }
            if !metadata.is_dir() {
                return Err(FsError::ContentMismatch);
            }
            current = current.open_dir(&actual).map_err(|_| FsError::Io {
                operation: "open workspace parent",
            })?;
        }
        Ok((current, (*leaf).to_owned()))
    }

    pub(crate) fn open_entry(
        &self,
        path: &WorkspacePath,
    ) -> Result<Option<(cap_std::fs::Dir, String, cap_std::fs::Metadata)>, FsError> {
        self.resolve_cap_entry(path)
    }

    pub(crate) fn content_hash(
        &self,
        path: &WorkspacePath,
        observed: &ObservedEntry,
    ) -> Result<Option<WorkspaceContentHash>, FsError> {
        match observed.kind {
            WorkspaceEntryKind::Directory => Ok(None),
            WorkspaceEntryKind::Symlink => {
                let bytes = observed
                    .symlink_target
                    .as_deref()
                    .ok_or(FsError::ContentMismatch)?;
                Ok(Some(hash_bytes(bytes)?))
            }
            WorkspaceEntryKind::File => {
                let Some((parent, name, _)) = self.resolve_cap_entry(path)? else {
                    return Ok(None);
                };
                let mut file = parent.open(&name).map_err(|_| FsError::Io {
                    operation: "open workspace file",
                })?;
                let mut hasher = blake3::Hasher::new();
                let mut buffer = [0_u8; crate::hash::HASH_BUFFER_BYTES];
                loop {
                    let count = file.read(&mut buffer).map_err(|_| FsError::Io {
                        operation: "read workspace file",
                    })?;
                    if count == 0 {
                        break;
                    }
                    hasher.update(&buffer[..count]);
                }
                WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
                    .map(Some)
                    .map_err(|_| FsError::ContentMismatch)
            }
            WorkspaceEntryKind::Tombstone => Err(FsError::ContentMismatch),
        }
    }

    pub(crate) fn observe_native(
        &self,
        path: WorkspacePath,
        native: PathBuf,
    ) -> Result<ObservedEntry, FsError> {
        let metadata = fs::symlink_metadata(&native).map_err(|_| FsError::Io {
            operation: "stat entry",
        })?;
        if metadata.file_type().is_symlink() {
            self.validate_symlink(&native)?;
        }
        self.observed(path, native, metadata)
    }

    pub(crate) fn resolve_child_name(
        &self,
        parent: &Path,
        name: &str,
        workspace_path: &WorkspacePath,
    ) -> Result<Option<PathBuf>, FsError> {
        let entries = fs::read_dir(parent).map_err(|_| FsError::Io {
            operation: "read directory",
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| FsError::Io {
                operation: "read directory entry",
            })?;
            let native_name = entry.file_name();
            let Some(native_name) = native_name.to_str() else {
                continue;
            };
            if native_name == name {
                return Ok(Some(parent.join(native_name)));
            }
            let normalized = native_name.nfc().collect::<String>();
            let normalized_match = normalized == name;
            let case_match = self.case_sensitivity == CaseSensitivity::Insensitive
                && normalized.eq_ignore_ascii_case(name);
            if normalized_match || case_match {
                return Err(FsError::PathCollision {
                    path: workspace_path.clone(),
                });
            }
        }
        Ok(None)
    }

    fn resolve_native_path(&self, path: &WorkspacePath) -> Result<Option<PathBuf>, FsError> {
        let mut current = self.canonical_root.clone();
        let components = path.as_str().split('/').collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let is_last = index + 1 == components.len();
            let workspace_prefix = components[..=index].join("/");
            let workspace_prefix =
                WorkspacePath::parse(&workspace_prefix).map_err(|error| FsError::InvalidPath {
                    reason: error.reason,
                })?;
            let Some(next) = self.resolve_child_name(&current, component, &workspace_prefix)?
            else {
                return Ok(None);
            };
            let metadata = fs::symlink_metadata(&next).map_err(|_| FsError::Io {
                operation: "stat path component",
            })?;
            if metadata.file_type().is_symlink() {
                self.validate_symlink(&next)?;
                if !is_last {
                    return Err(FsError::UnsupportedSymlink);
                }
            } else if !is_last && !metadata.is_dir() {
                return Ok(None);
            }
            current = next;
        }
        Ok(Some(current))
    }

    fn resolve_cap_entry(
        &self,
        path: &WorkspacePath,
    ) -> Result<Option<(cap_std::fs::Dir, String, cap_std::fs::Metadata)>, FsError> {
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let mut current = self.capability.try_clone().map_err(|_| FsError::Io {
            operation: "clone root capability",
        })?;
        for (index, component) in components.iter().enumerate() {
            let is_last = index + 1 == components.len();
            let prefix =
                WorkspacePath::parse(&components[..=index].join("/")).map_err(|error| {
                    FsError::InvalidPath {
                        reason: error.reason,
                    }
                })?;
            let Some(actual) = self.find_cap_child(&current, component, &prefix)? else {
                return Ok(None);
            };
            let metadata = current.symlink_metadata(&actual).map_err(|_| FsError::Io {
                operation: "stat entry",
            })?;
            if metadata.file_type().is_symlink() {
                self.validate_cap_symlink(&current, &actual, components[..index].join("/"))?;
                if !is_last {
                    return Err(FsError::UnsupportedSymlink);
                }
            } else if !is_last && !metadata.is_dir() {
                return Ok(None);
            }
            if is_last {
                return Ok(Some((current, actual, metadata)));
            }
            current = current.open_dir(&actual).map_err(|_| FsError::Io {
                operation: "open workspace directory",
            })?;
        }
        Ok(None)
    }

    fn find_cap_child(
        &self,
        parent: &cap_std::fs::Dir,
        name: &str,
        workspace_path: &WorkspacePath,
    ) -> Result<Option<String>, FsError> {
        let mut exact = None;
        let mut alias = None;
        for entry in parent.entries().map_err(|_| FsError::Io {
            operation: "read directory",
        })? {
            let entry = entry.map_err(|_| FsError::Io {
                operation: "read directory entry",
            })?;
            let native_name = entry.file_name();
            let Some(native_name) = native_name.to_str() else {
                continue;
            };
            if native_name == name {
                if exact.is_some() {
                    return Err(FsError::PathCollision {
                        path: workspace_path.clone(),
                    });
                }
                exact = Some(native_name.to_owned());
                continue;
            }
            let normalized = native_name.nfc().collect::<String>();
            let case_match = self.case_sensitivity == CaseSensitivity::Insensitive
                && normalized.eq_ignore_ascii_case(name);
            if normalized == name || case_match {
                if alias.is_some() || exact.is_some() {
                    return Err(FsError::PathCollision {
                        path: workspace_path.clone(),
                    });
                }
                alias = Some(native_name.to_owned());
            }
        }
        if alias.is_some() {
            return Err(FsError::PathCollision {
                path: workspace_path.clone(),
            });
        }
        Ok(exact)
    }

    fn validate_cap_symlink(
        &self,
        parent: &cap_std::fs::Dir,
        name: &str,
        parent_path: String,
    ) -> Result<(), FsError> {
        let target = parent.read_link_contents(name).map_err(|_| FsError::Io {
            operation: "read symlink",
        })?;
        let target = target.to_str().ok_or(FsError::PathEscape)?;
        validate_relative_target(&parent_path, target)?;
        match parent.canonicalize(name) {
            Ok(resolved) if resolved.is_absolute() => Err(FsError::PathEscape),
            Ok(resolved)
                if resolved.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                }) =>
            {
                Err(FsError::PathEscape)
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(FsError::PathEscape),
        }
    }

    fn observed_cap(
        &self,
        path: WorkspacePath,
        parent: &cap_std::fs::Dir,
        name: &str,
        metadata: cap_std::fs::Metadata,
    ) -> Result<ObservedEntry, FsError> {
        let file_type = metadata.file_type();
        let (kind, symlink_target) = if file_type.is_symlink() {
            let target = parent.read_link_contents(name).map_err(|_| FsError::Io {
                operation: "read symlink",
            })?;
            let target = target.to_str().ok_or(FsError::PathEscape)?;
            (
                WorkspaceEntryKind::Symlink,
                Some(target.as_bytes().to_vec()),
            )
        } else if metadata.is_dir() {
            (WorkspaceEntryKind::Directory, None)
        } else if metadata.is_file() {
            (WorkspaceEntryKind::File, None)
        } else {
            return Err(FsError::Io {
                operation: "classify entry",
            });
        };
        let size = if kind == WorkspaceEntryKind::Directory {
            0
        } else {
            metadata.len()
        };
        let modified_at_ms = metadata
            .modified()
            .ok()
            .and_then(|value| value.into_std().duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_millis().min(i64::MAX as u128) as i64);
        let fingerprint = fingerprint_cap(&metadata);
        let metadata = WorkspaceFileMetadata {
            size,
            modified_at_ms,
            executable: executable_cap(&metadata),
        };
        Ok(ObservedEntry {
            path,
            kind,
            metadata,
            fingerprint,
            symlink_target,
        })
    }

    fn validate_symlink(&self, path: &Path) -> Result<(), FsError> {
        let target = fs::read_link(path).map_err(|_| FsError::Io {
            operation: "read symlink",
        })?;
        if target.is_absolute() || target.to_str().is_none() {
            return Err(FsError::PathEscape);
        }
        let resolved = path.parent().unwrap_or(self.root_path()).join(target);
        let canonical = match fs::canonicalize(&resolved) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                lexical_normalize(&resolved)
            }
            Err(_) => return Err(FsError::PathEscape),
        };
        if !canonical.starts_with(self.root_path()) {
            return Err(FsError::PathEscape);
        }
        Ok(())
    }

    fn observed(
        &self,
        path: WorkspacePath,
        native: PathBuf,
        metadata: fs::Metadata,
    ) -> Result<ObservedEntry, FsError> {
        let file_type = metadata.file_type();
        let (kind, symlink_target) = if file_type.is_symlink() {
            let target = fs::read_link(&native).map_err(|_| FsError::Io {
                operation: "read symlink",
            })?;
            let target_bytes = target.to_string_lossy().as_bytes().to_vec();
            (WorkspaceEntryKind::Symlink, Some(target_bytes))
        } else if metadata.is_dir() {
            (WorkspaceEntryKind::Directory, None)
        } else if metadata.is_file() {
            (WorkspaceEntryKind::File, None)
        } else {
            return Err(FsError::Io {
                operation: "classify entry",
            });
        };
        let size = if kind == WorkspaceEntryKind::Directory {
            0
        } else {
            metadata.len()
        };
        let modified_at_ms = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_millis().min(i64::MAX as u128) as i64);
        let fingerprint = fingerprint(&metadata);
        let metadata = WorkspaceFileMetadata {
            size,
            modified_at_ms,
            executable: executable(&metadata),
        };
        Ok(ObservedEntry {
            path,
            kind,
            metadata,
            fingerprint,
            symlink_target,
        })
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::Prefix(value) => normalized.push(value.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn validate_relative_target(parent_path: &str, target: &str) -> Result<(), FsError> {
    let mut depth = if parent_path.is_empty() {
        0
    } else {
        parent_path.split('/').count()
    };
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

fn hash_bytes(bytes: &[u8]) -> Result<WorkspaceContentHash, FsError> {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex()))
        .map_err(|_| FsError::ContentMismatch)
}

fn executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn executable_cap(metadata: &cap_std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        metadata.mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

#[allow(clippy::needless_return)]
fn fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return FileFingerprint {
            file_id: NativeFileId::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            size: metadata.len(),
            modified_at_ns: metadata.mtime() as i128 * 1_000_000_000
                + metadata.mtime_nsec() as i128,
            changed_at_ns: metadata.ctime() as i128 * 1_000_000_000 + metadata.ctime_nsec() as i128,
        };
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return FileFingerprint {
            file_id: NativeFileId::Windows {
                volume_serial: metadata.volume_serial_number().unwrap_or_default() as u64,
                file_index: metadata.file_index().unwrap_or_default(),
            },
            size: metadata.file_size(),
            modified_at_ns: metadata.last_write_time() as i128 * 100,
            changed_at_ns: metadata.last_write_time() as i128 * 100,
        };
    }
    #[cfg(not(any(unix, windows)))]
    {
        FileFingerprint {
            file_id: NativeFileId::Unix {
                device: 0,
                inode: 0,
            },
            size: metadata.len(),
            modified_at_ns: 0,
            changed_at_ns: 0,
        }
    }
}

#[allow(clippy::needless_return)]
fn fingerprint_cap(metadata: &cap_std::fs::Metadata) -> FileFingerprint {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        return FileFingerprint {
            file_id: NativeFileId::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            size: metadata.len(),
            modified_at_ns: metadata.mtime() as i128 * 1_000_000_000
                + metadata.mtime_nsec() as i128,
            changed_at_ns: metadata.ctime() as i128 * 1_000_000_000 + metadata.ctime_nsec() as i128,
        };
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        return FileFingerprint {
            file_id: NativeFileId::Windows {
                volume_serial: metadata.volume_serial_number().unwrap_or_default() as u64,
                file_index: metadata.file_index().unwrap_or_default(),
            },
            size: metadata.file_size(),
            modified_at_ns: metadata.last_write_time() as i128 * 100,
            changed_at_ns: metadata.last_write_time() as i128 * 100,
        };
    }
    #[cfg(not(any(unix, windows)))]
    {
        FileFingerprint {
            file_id: NativeFileId::Unix {
                device: 0,
                inode: 0,
            },
            size: metadata.len(),
            modified_at_ns: 0,
            changed_at_ns: 0,
        }
    }
}
