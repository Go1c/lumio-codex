use std::collections::HashMap;
use std::fs;

use fns_protocol::WorkspacePath;
use unicode_normalization::UnicodeNormalization;

use crate::{CaseSensitivity, FsError, ObservedEntry, RootedWorkspace, SyncRules};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanIssue {
    pub reason: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceScan {
    pub entries: Vec<ObservedEntry>,
    pub issues: Vec<ScanIssue>,
}

pub(crate) fn scan_workspace(
    root: &RootedWorkspace,
    rules: &SyncRules,
) -> Result<WorkspaceScan, FsError> {
    let mut pending = vec![(root.root_path().to_path_buf(), String::new())];
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    while let Some((parent, prefix)) = pending.pop() {
        let children = fs::read_dir(&parent).map_err(|_| FsError::Io {
            operation: "scan directory",
        })?;
        for child in children {
            let child = child.map_err(|_| FsError::Io {
                operation: "scan directory entry",
            })?;
            let name = child.file_name();
            let Some(name) = name.to_str() else {
                issues.push(ScanIssue {
                    reason: "non_utf8_name",
                });
                continue;
            };
            let normalized_name = name.nfc().collect::<String>();
            let value = if prefix.is_empty() {
                normalized_name
            } else {
                format!("{prefix}/{normalized_name}")
            };
            let path = match WorkspacePath::parse(&value) {
                Ok(path) => path,
                Err(_) => {
                    issues.push(ScanIssue {
                        reason: "invalid_workspace_path",
                    });
                    continue;
                }
            };
            let metadata = fs::symlink_metadata(child.path()).map_err(|_| FsError::Io {
                operation: "scan entry",
            })?;
            let is_dir = metadata.is_dir() && !metadata.file_type().is_symlink();
            if is_dir && rules.should_descend(&path) {
                pending.push((child.path(), value.clone()));
            }
            if !rules.decide(&path, is_dir).included {
                continue;
            }
            let observed = match root.observe_native(path, child.path()) {
                Ok(observed) => observed,
                Err(FsError::PathEscape | FsError::UnsupportedSymlink) => {
                    issues.push(ScanIssue {
                        reason: "unsafe_symlink",
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let key = if root.case_sensitivity() == CaseSensitivity::Insensitive {
                value.to_lowercase()
            } else {
                value.clone()
            };
            if let Some(previous) = seen.insert(key, entries.len()) {
                entries.remove(previous);
                for index in seen.values_mut() {
                    if *index > previous {
                        *index -= 1;
                    }
                }
                issues.push(ScanIssue {
                    reason: "path_collision",
                });
                continue;
            }
            entries.push(observed);
        }
    }

    entries.sort_by(|left, right| {
        left.path
            .as_str()
            .as_bytes()
            .cmp(right.path.as_str().as_bytes())
    });
    Ok(WorkspaceScan { entries, issues })
}
