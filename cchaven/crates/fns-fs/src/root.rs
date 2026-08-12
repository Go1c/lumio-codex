use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
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
    #[serde(with = "serde_i128_string")]
    pub modified_at_ns: i128,
    #[serde(with = "serde_i128_string")]
    pub changed_at_ns: i128,
}

mod serde_i128_string {
    use serde::de::{Error, Visitor};

    pub fn serialize<S>(value: &i128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i128, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct I128Visitor;

        impl Visitor<'_> for I128Visitor {
            type Value = i128;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a signed 128-bit integer or decimal string")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(i128::from(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(i128::from(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                value.parse().map_err(Error::custom)
            }
        }

        deserializer.deserialize_any(I128Visitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ObservedEntry {
    pub path: WorkspacePath,
    pub kind: WorkspaceEntryKind,
    pub metadata: WorkspaceFileMetadata,
    pub fingerprint: FileFingerprint,
    pub symlink_target: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySnapshotEntry {
    pub relative_path: WorkspacePath,
    pub kind: WorkspaceEntryKind,
    pub content_hash: Option<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
    pub fingerprint: FileFingerprint,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySnapshot {
    pub digest: WorkspaceContentHash,
    pub entries: Vec<DirectorySnapshotEntry>,
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
        #[cfg(test)]
        run_open_test_hook(root);
        let opened_metadata = capability.symlink_metadata(".").map_err(|_| FsError::Io {
            operation: "stat opened root",
        })?;
        if !same_file_identity(&metadata, &opened_metadata) {
            return Err(FsError::Io {
                operation: "root changed while opening",
            });
        }
        let case_sensitivity = detect_case_sensitivity(&capability);
        Ok(Self {
            canonical_root,
            capability,
            case_sensitivity,
        })
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    pub(crate) fn clone_for_watcher(&self) -> Result<Self, FsError> {
        Ok(Self {
            canonical_root: self.canonical_root.clone(),
            capability: self.capability.try_clone().map_err(|_| FsError::Io {
                operation: "clone watcher root capability",
            })?,
            case_sensitivity: self.case_sensitivity,
        })
    }

    pub(crate) fn bound_path_is_current(&self) -> bool {
        let Ok(path_metadata) = fs::symlink_metadata(&self.canonical_root) else {
            return false;
        };
        if path_metadata.file_type().is_symlink() || !path_metadata.is_dir() {
            return false;
        }
        let Ok(capability_metadata) = self.capability.symlink_metadata(".") else {
            return false;
        };
        same_file_identity(&path_metadata, &capability_metadata)
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

    pub fn directory_digest(&self, path: &WorkspacePath) -> Result<WorkspaceContentHash, FsError> {
        Ok(self.directory_snapshot(path)?.digest)
    }

    pub fn directory_snapshot(&self, path: &WorkspacePath) -> Result<DirectorySnapshot, FsError> {
        if self
            .inspect(path)?
            .is_none_or(|observed| observed.kind != WorkspaceEntryKind::Directory)
        {
            return Err(FsError::ContentMismatch);
        }
        let mut hasher = blake3::Hasher::new();
        let mut entries = Vec::new();
        self.hash_directory_descendants(path, "", &mut hasher, &mut entries)?;
        let digest = WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
            .map_err(|_| FsError::ContentMismatch)?;
        Ok(DirectorySnapshot { digest, entries })
    }

    pub fn scan(&self, rules: &SyncRules) -> Result<WorkspaceScan, FsError> {
        scan_workspace(self, rules)
    }

    pub(crate) fn read_dir_names(
        &self,
        path: Option<&WorkspacePath>,
    ) -> Result<Vec<OsString>, FsError> {
        let directory = match path {
            Some(path) => {
                let Some((parent, name, metadata)) = self.resolve_cap_entry(path)? else {
                    return Ok(Vec::new());
                };
                if metadata.file_type().is_symlink() {
                    return Err(FsError::UnsupportedSymlink);
                }
                if !metadata.is_dir() {
                    return Ok(Vec::new());
                }
                parent.open_dir(&name).map_err(|_| FsError::Io {
                    operation: "open workspace directory",
                })?
            }
            None => self.capability.try_clone().map_err(|_| FsError::Io {
                operation: "clone root capability",
            })?,
        };
        directory
            .entries()
            .map_err(|_| FsError::Io {
                operation: "read directory",
            })?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|_| FsError::Io {
                        operation: "read directory entry",
                    })
            })
            .collect()
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

    fn hash_directory_descendants(
        &self,
        directory: &WorkspacePath,
        relative_prefix: &str,
        hasher: &mut blake3::Hasher,
        entries: &mut Vec<DirectorySnapshotEntry>,
    ) -> Result<(), FsError> {
        let names = self.normalized_directory_names(directory)?;
        for name in &names {
            let child = WorkspacePath::parse(&format!("{}/{name}", directory.as_str())).map_err(
                |error| FsError::InvalidPath {
                    reason: error.reason,
                },
            )?;
            let relative = if relative_prefix.is_empty() {
                name.clone()
            } else {
                format!("{relative_prefix}/{name}")
            };
            let before = self.inspect(&child)?.ok_or(FsError::ContentMismatch)?;
            let content_hash = self.content_hash(&child, &before)?;

            hash_tree_field(hasher, relative.as_bytes());
            hasher.update(&[match before.kind {
                WorkspaceEntryKind::File => 1,
                WorkspaceEntryKind::Directory => 2,
                WorkspaceEntryKind::Symlink => 3,
                WorkspaceEntryKind::Tombstone => return Err(FsError::ContentMismatch),
            }]);
            hasher.update(&before.metadata.size.to_le_bytes());
            hasher.update(&before.metadata.modified_at_ms.to_le_bytes());
            hasher.update(&[u8::from(before.metadata.executable)]);
            hash_tree_field(
                hasher,
                content_hash
                    .as_ref()
                    .map_or(&[], |hash| hash.as_str().as_bytes()),
            );
            entries.push(DirectorySnapshotEntry {
                relative_path: WorkspacePath::parse(&relative).map_err(|error| {
                    FsError::InvalidPath {
                        reason: error.reason,
                    }
                })?,
                kind: before.kind,
                content_hash: content_hash.clone(),
                metadata: before.metadata.clone(),
                fingerprint: before.fingerprint.clone(),
            });

            if before.kind == WorkspaceEntryKind::Directory {
                self.hash_directory_descendants(&child, &relative, hasher, entries)?;
            }
            if self.inspect(&child)?.as_ref() != Some(&before) {
                return Err(FsError::ContentMismatch);
            }
        }
        if self.normalized_directory_names(directory)? != names {
            return Err(FsError::ContentMismatch);
        }
        Ok(())
    }

    fn normalized_directory_names(
        &self,
        directory: &WorkspacePath,
    ) -> Result<Vec<String>, FsError> {
        let mut names = self
            .read_dir_names(Some(directory))?
            .into_iter()
            .map(|name| {
                name.to_str()
                    .map(|name| name.nfc().collect::<String>())
                    .ok_or(FsError::InvalidPath {
                        reason: "non_utf8_name".to_owned(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(FsError::PathCollision {
                path: directory.clone(),
            });
        }
        Ok(names)
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

    pub(crate) fn validate_symlink_target(
        &self,
        path: &WorkspacePath,
        target: &str,
    ) -> Result<(), FsError> {
        let parent = path
            .as_str()
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        validate_relative_target(parent, target)?;
        let mut components = if parent.is_empty() {
            Vec::new()
        } else {
            parent.split('/').map(str::to_owned).collect::<Vec<_>>()
        };
        for component in Path::new(target).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    components.pop().ok_or(FsError::PathEscape)?;
                }
                Component::Normal(component) => {
                    components.push(component.to_str().ok_or(FsError::PathEscape)?.to_owned());
                }
                Component::Prefix(_) | Component::RootDir => return Err(FsError::PathEscape),
            }
        }
        if !components.is_empty() {
            let resolved =
                WorkspacePath::parse(&components.join("/")).map_err(|_| FsError::PathEscape)?;
            let _ = self.inspect(&resolved)?;
        }
        Ok(())
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
            executable: kind == WorkspaceEntryKind::File && executable_cap(&metadata),
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

fn hash_tree_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn detect_case_sensitivity(root: &cap_std::fs::Dir) -> CaseSensitivity {
    let probe = format!(".fns-case-probe-aA-{}", uuid::Uuid::new_v4());
    let alternate = probe.to_uppercase();
    if root.create_dir(&probe).is_ok() {
        let insensitive = root.symlink_metadata(&alternate).is_ok();
        let _ = root.remove_dir(&probe);
        return if insensitive {
            CaseSensitivity::Insensitive
        } else {
            CaseSensitivity::Sensitive
        };
    }
    if cfg!(any(windows, target_os = "macos")) {
        CaseSensitivity::Insensitive
    } else {
        CaseSensitivity::Sensitive
    }
}

fn same_file_identity(left: &fs::Metadata, right: &cap_std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::unix::fs::MetadataExt as StdMetadataExt;

        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::windows::fs::MetadataExt as StdMetadataExt;

        left.creation_time() == right.creation_time()
            && left.last_write_time() == right.last_write_time()
            && left.file_size() == right.file_size()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        true
    }
}

#[cfg(test)]
fn run_open_test_hook(root: &Path) {
    if let Some(hook) = OPEN_TEST_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("root open test hook is not poisoned")
        .as_ref()
    {
        hook(root);
    }
}

#[cfg(test)]
type OpenTestHook = Box<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
static OPEN_TEST_HOOK: OnceLock<Mutex<Option<OpenTestHook>>> = OnceLock::new();

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
        use cap_fs_ext::MetadataExt;
        let modified_at_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.into_std().duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos().min(i128::MAX as u128) as i128);
        return FileFingerprint {
            file_id: NativeFileId::Windows {
                volume_serial: metadata.dev(),
                file_index: metadata.ino(),
            },
            size: metadata.len(),
            modified_at_ns,
            changed_at_ns: modified_at_ns,
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::{OPEN_TEST_HOOK, RootedWorkspace};
    use fns_protocol::WorkspacePath;

    #[test]
    fn root_capability_is_bound_before_path_swap_can_follow_a_symlink() {
        let area = tempfile::tempdir().unwrap();
        let root_path = area.path().join("root");
        let moved_path = area.path().join("moved");
        let outside_path = area.path().join("outside");
        std::fs::create_dir(&root_path).unwrap();
        std::fs::create_dir(&outside_path).unwrap();
        std::fs::write(root_path.join("entry"), b"original").unwrap();
        std::fs::write(outside_path.join("entry"), b"outside").unwrap();

        *OPEN_TEST_HOOK
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = Some(Box::new({
            let root_path = root_path.clone();
            let moved_path = moved_path.clone();
            let outside_path = outside_path.clone();
            move |path| {
                if path != root_path {
                    return;
                }
                std::fs::rename(path, &moved_path).unwrap();
                symlink(&outside_path, path).unwrap();
            }
        }));

        let rooted = RootedWorkspace::open(&root_path).unwrap();
        let entry = rooted
            .inspect(&WorkspacePath::parse("entry").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(entry.metadata.size, 8);
        assert_eq!(
            std::fs::read(outside_path.join("entry")).unwrap(),
            b"outside"
        );
        assert_eq!(
            std::fs::read(moved_path.join("entry")).unwrap(),
            b"original"
        );
        *OPEN_TEST_HOOK
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = None;
    }
}
