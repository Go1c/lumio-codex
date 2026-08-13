use crate::{io_error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntry {
    pub path: String,
    pub kind: PathKind,
    pub size: u64,
    pub mode: u32,
    pub blake3: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
    pub digest: String,
}

impl Manifest {
    pub fn sync_equivalent(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| {
                    left.path == right.path
                        && left.kind == right.kind
                        && left.size == right.size
                        && left.blake3 == right.blake3
                        && (left.kind != PathKind::File
                            || (left.mode & 0o111 != 0) == (right.mode & 0o111 != 0))
                })
    }
}

pub fn build_manifest(root: &Path) -> Result<Manifest> {
    let root = root.canonicalize().map_err(|error| io_error(root, error))?;
    let mut entries = Vec::new();
    visit_directory(&root, &root, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let encoded = serde_json::to_vec(&entries)?;
    Ok(Manifest {
        entries,
        digest: blake3::hash(&encoded).to_hex().to_string(),
    })
}

fn visit_directory(root: &Path, directory: &Path, entries: &mut Vec<ManifestEntry>) -> Result<()> {
    let mut children = fs::read_dir(directory)
        .map_err(|error| io_error(directory, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| io_error(directory, error))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        let relative = relative_path(root, &path)?;
        let mode = portable_mode(&metadata);
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|error| io_error(&path, error))?;
            let target_bytes = path_bytes(&target);
            entries.push(ManifestEntry {
                path: relative,
                kind: PathKind::Symlink,
                size: target_bytes.len() as u64,
                mode,
                blake3: Some(blake3::hash(&target_bytes).to_hex().to_string()),
            });
        } else if metadata.is_dir() {
            entries.push(ManifestEntry {
                path: relative,
                kind: PathKind::Directory,
                size: 0,
                mode,
                blake3: None,
            });
            visit_directory(root, &path, entries)?;
        } else if metadata.is_file() {
            entries.push(ManifestEntry {
                path: relative,
                kind: PathKind::File,
                size: metadata.len(),
                mode,
                blake3: Some(hash_file(&path)?),
            });
        } else {
            return Err(crate::HarnessError::InvalidConfiguration(
                "workspace contains an unsupported filesystem node",
            ));
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| crate::HarnessError::InvalidConfiguration("manifest path escaped root"))?;
    let components = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or(crate::HarnessError::InvalidConfiguration(
                    "manifest path is not valid UTF-8",
                ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(components.join("/"))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(unix)]
fn portable_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn portable_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}
