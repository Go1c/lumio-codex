use std::collections::HashSet;

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
    let mut pending = vec![(None, String::new())];
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    let mut seen = HashSet::new();

    while let Some((parent, prefix)) = pending.pop() {
        let children = match root.read_dir_names(parent.as_ref()) {
            Ok(children) => children,
            Err(FsError::PathEscape | FsError::UnsupportedSymlink) => {
                issues.push(ScanIssue {
                    reason: "unsafe_symlink",
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        for name in children {
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
            let observed = match root.inspect(&path) {
                Ok(Some(observed)) => observed,
                Ok(None) => continue,
                Err(FsError::PathEscape | FsError::UnsupportedSymlink) => {
                    issues.push(ScanIssue {
                        reason: "unsafe_symlink",
                    });
                    continue;
                }
                Err(FsError::PathCollision { .. }) => {
                    issues.push(ScanIssue {
                        reason: "path_collision",
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let is_dir = observed.kind == fns_protocol::WorkspaceEntryKind::Directory;
            if is_dir && rules.should_descend(&path) {
                pending.push((Some(path.clone()), value.clone()));
            }
            if !rules.decide(&path, is_dir).included {
                continue;
            }
            let key = if root.case_sensitivity() == CaseSensitivity::Insensitive {
                value.to_lowercase()
            } else {
                value.clone()
            };
            if !seen.insert(key) {
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
