//! File tree browser and diff view backend.
//!
//! Reads the local workspace directory tree and provides it to the frontend.
//! Computes text diffs between local and remote content using diffy.
//!
//! File commands accept `projectId` only — the absolute `local_root` is loaded
//! from persisted `ProjectConfig`, so the frontend cannot pass arbitrary roots.

use crate::project::ProjectConfig;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// A file tree node for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FileTreeNode {
    Directory {
        name: String,
        path: String,
        children: Vec<FileTreeNode>,
    },
    File {
        name: String,
        path: String,
        size: u64,
    },
    Symlink {
        name: String,
        path: String,
        target: Option<String>,
    },
}

/// Read a file tree from the local workspace root.
/// Respects common exclude patterns (.git, node_modules, target, etc.).
pub fn read_file_tree(root: &Path, max_depth: usize) -> Result<FileTreeNode, std::io::Error> {
    fn read_dir(
        path: &Path,
        rel_path: &str,
        depth: usize,
        max_depth: usize,
    ) -> Result<Vec<FileTreeNode>, std::io::Error> {
        if depth >= max_depth {
            return Ok(Vec::new());
        }

        let mut entries: Vec<_> = std::fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        let mut nodes = Vec::new();
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let full_path = entry.path();
            let child_rel = if rel_path.is_empty() {
                name.clone()
            } else {
                format!("{rel_path}/{name}")
            };

            // Skip excluded directories.
            if is_excluded(&name) {
                continue;
            }

            let meta = entry.metadata()?;
            if meta.is_dir() {
                let children = read_dir(&full_path, &child_rel, depth + 1, max_depth)?;
                nodes.push(FileTreeNode::Directory {
                    name,
                    path: child_rel,
                    children,
                });
            } else if meta.is_symlink() {
                let target = std::fs::read_link(&full_path)
                    .ok()
                    .map(|t| t.to_string_lossy().to_string());
                nodes.push(FileTreeNode::Symlink {
                    name,
                    path: child_rel,
                    target,
                });
            } else {
                nodes.push(FileTreeNode::File {
                    name,
                    path: child_rel,
                    size: meta.len(),
                });
            }
        }
        Ok(nodes)
    }

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".into());
    let children = read_dir(root, "", 0, max_depth)?;
    Ok(FileTreeNode::Directory {
        name,
        path: "".into(),
        children,
    })
}

/// Check if a file/directory name matches common exclude patterns.
fn is_excluded(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | ".venv"
            | "venv"
            | "target"
            | "build"
            | "dist"
            | ".next"
            | ".cache"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".DS_Store"
            | ".fns_state.json"
    )
}

/// Compute a text diff between two strings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub has_conflicts: bool,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DiffLine {
    Context { content: String, line_number: u32 },
    Added { content: String, line_number: u32 },
    Removed { content: String, line_number: u32 },
}

/// Compute a unified diff between old and new text.
pub fn compute_text_diff(old: &str, new: &str) -> DiffResult {
    let patch = diffy::create_patch(old, new);
    let mut hunks = Vec::new();

    for hunk in patch.hunks() {
        let mut lines = Vec::new();
        let mut old_line = hunk.old_range().start() as u32;
        let mut new_line = hunk.new_range().start() as u32;

        for line in hunk.lines() {
            match line {
                diffy::Line::Context(s) => {
                    lines.push(DiffLine::Context {
                        content: s.to_string(),
                        line_number: old_line,
                    });
                    old_line += 1;
                    new_line += 1;
                }
                diffy::Line::Delete(s) => {
                    lines.push(DiffLine::Removed {
                        content: s.to_string(),
                        line_number: old_line,
                    });
                    old_line += 1;
                }
                diffy::Line::Insert(s) => {
                    lines.push(DiffLine::Added {
                        content: s.to_string(),
                        line_number: new_line,
                    });
                    new_line += 1;
                }
            }
        }

        hunks.push(DiffHunk {
            old_start: hunk.old_range().start() as u32,
            new_start: hunk.new_range().start() as u32,
            lines,
        });
    }

    DiffResult {
        has_conflicts: false,
        hunks,
    }
}

/// Resolve `relative` under `project_root`, rejecting absolute paths, `..`
/// components, and symlink escapes that would leave the project root.
///
/// For paths that do not yet exist, the nearest existing ancestor is
/// canonicalized and the remaining suffix is re-joined under a prefix check.
pub fn resolve_project_path(project_root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() {
        return Err("relative path must not be empty".into());
    }
    if relative.contains('\0') {
        return Err("relative path must not contain NUL".into());
    }

    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err("absolute paths are not allowed".into());
    }

    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err("path must not contain '..' components".into());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".into());
            }
        }
    }

    let root = project_root
        .canonicalize()
        .map_err(|e| format!("project root is not accessible: {e}"))?;
    if !root.is_dir() {
        return Err("project root is not a directory".into());
    }

    let candidate = root.join(rel);
    ensure_under_root(&root, &candidate)
}

fn ensure_under_root(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    // If the leaf exists, canonicalize it (follows final symlink) and require
    // the result stay under root.
    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|e| format!("failed to resolve path: {e}"))?;
        if !path_is_within(root, &canonical) {
            return Err("path escapes project root".into());
        }
        return Ok(canonical);
    }

    // Walk up to the nearest existing ancestor, canonicalize it, then re-apply
    // the non-existing suffix under a strict prefix check.
    let mut ancestor = candidate.to_path_buf();
    let mut missing_rev: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if ancestor.exists() {
            break;
        }
        let name = ancestor
            .file_name()
            .ok_or_else(|| "path escapes project root".to_string())?
            .to_os_string();
        missing_rev.push(name);
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "path escapes project root".to_string())?
            .to_path_buf();
    }

    let mut resolved = ancestor
        .canonicalize()
        .map_err(|e| format!("failed to resolve path: {e}"))?;
    if !path_is_within(root, &resolved) {
        return Err("path escapes project root".into());
    }

    for component in missing_rev.into_iter().rev() {
        resolved.push(component);
        // Intermediate symlinks that already exist are caught when we
        // re-canonicalize any existing prefix; non-existing components cannot
        // escape via symlink until they exist.
        if resolved.exists() {
            resolved = resolved
                .canonicalize()
                .map_err(|e| format!("failed to resolve path: {e}"))?;
            if !path_is_within(root, &resolved) {
                return Err("path escapes project root".into());
            }
        } else if !path_is_within(root, &resolved) {
            return Err("path escapes project root".into());
        }
    }

    Ok(resolved)
}

fn path_is_within(root: &Path, path: &Path) -> bool {
    if path == root {
        return true;
    }
    path.starts_with(root)
}

// --- Project root resolution ---

/// Resolve the configured `local_root` for a project id from persisted config.
/// Frontend callers must never supply absolute roots directly.
pub fn local_root_for_project_id(id: &str) -> Result<PathBuf, String> {
    let project =
        ProjectConfig::find_by_id(id).map_err(|e| format!("failed to load project {id}: {e}"))?;
    project_local_root_for(&project)
}

/// Expose `local_root` from an already-loaded project (unit-test helper).
pub fn project_local_root_for(project: &ProjectConfig) -> Result<PathBuf, String> {
    let trimmed = project.local_root.trim();
    if trimmed.is_empty() {
        return Err("project local_root is empty".into());
    }
    Ok(PathBuf::from(trimmed))
}

fn canonicalize_project_root(local_root: &Path) -> Result<PathBuf, String> {
    let root = local_root
        .canonicalize()
        .map_err(|e| format!("local_root is not accessible: {e}"))?;
    if !root.is_dir() {
        return Err("local_root is not a directory".into());
    }
    Ok(root)
}

// --- Tauri commands ---

#[tauri::command]
pub fn browse_files(project_id: String) -> Result<FileTreeNode, String> {
    let local_root = local_root_for_project_id(&project_id)?;
    let root = canonicalize_project_root(&local_root)?;
    read_file_tree(&root, 5).map_err(|e| e.to_string())
}

/// Read a project-relative file. Rejects `..` and symlink escapes outside the
/// project root loaded from `project_id`.
#[tauri::command]
pub fn read_file(project_id: String, relative_path: String) -> Result<String, String> {
    let local_root = local_root_for_project_id(&project_id)?;
    let full = resolve_project_path(&local_root, &relative_path)?;
    std::fs::read_to_string(&full).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn compute_diff(old_text: String, new_text: String) -> Result<DiffResult, String> {
    Ok(compute_text_diff(&old_text, &new_text))
}

/// Open a project-relative path in the system file manager (Finder on macOS).
/// When `relative_path` is `None` or empty, opens the project root.
#[tauri::command]
pub fn open_in_finder(project_id: String, relative_path: Option<String>) -> Result<(), String> {
    let local_root = local_root_for_project_id(&project_id)?;
    let root = canonicalize_project_root(&local_root)?;
    let full = match relative_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        None => root,
        Some(rel) => resolve_project_path(&root, rel)?,
    };

    if !full.exists() {
        return Err(format!("Path does not exist: {}", full.display()));
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&full)
            .spawn()
            .map_err(|e| format!("Failed to open in Finder: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&full)
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&full)
            .spawn()
            .map_err(|e| format!("Failed to open Explorer: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod security_path_tests {
    use super::resolve_project_path;
    use std::fs;

    #[test]
    fn rejects_path_escape_with_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("safe.txt"), "ok").unwrap();

        assert!(resolve_project_path(root, "../etc/passwd").is_err());
        assert!(resolve_project_path(root, "a/../../etc/passwd").is_err());
        assert!(resolve_project_path(root, "..").is_err());
        assert!(resolve_project_path(root, "/etc/passwd").is_err());
        assert!(resolve_project_path(root, "").is_err());
    }

    #[test]
    fn resolves_safe_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let resolved = resolve_project_path(root, "src/main.rs").unwrap();
        assert_eq!(resolved, root.join("src/main.rs").canonicalize().unwrap());

        // Non-existing file under an existing parent is allowed (for create-like UX).
        let missing = resolve_project_path(root, "src/new.rs").unwrap();
        assert!(missing.starts_with(root.canonicalize().unwrap()));
        assert!(missing.ends_with("new.rs"));
    }

    #[test]
    fn rejects_symlink_escape_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "classified").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("leak")).unwrap();
            let result = resolve_project_path(&root, "leak/secret.txt");
            assert!(
                result.is_err(),
                "symlink escape should be rejected, got {result:?}"
            );
        }

        #[cfg(not(unix))]
        {
            // Symlink escape coverage is Unix-oriented; still assert base rejection.
            assert!(resolve_project_path(&root, "../outside/secret.txt").is_err());
        }
    }

    #[test]
    fn rejects_absolute_and_nul_relative() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_project_path(dir.path(), "/tmp/x").is_err());
        assert!(resolve_project_path(dir.path(), "a\0b").is_err());
    }
}

#[cfg(test)]
mod project_id_path_tests {
    use super::{
        canonicalize_project_root, project_local_root_for, read_file_tree, resolve_project_path,
    };
    use crate::project::{ProjectConfig, SyncConfig, SyncMode};
    use std::collections::HashMap;
    use std::fs;

    fn sample_project(id: uuid::Uuid, local_root: &str) -> ProjectConfig {
        ProjectConfig {
            id,
            name: "demo".into(),
            ssh_host_alias: "devbox".into(),
            remote_root: "/home/dev/code/my-app".into(),
            local_root: local_root.into(),
            workspace_id: uuid::Uuid::new_v4(),
            tmux_session: "demo".into(),
            sync: SyncConfig {
                mode: SyncMode::TwoWaySafe,
                includes: vec!["**/*".into()],
                excludes: vec![],
                protect_secrets: true,
            },
        }
    }

    #[test]
    fn find_by_id_resolves_from_map_and_local_root_helper() {
        let id = uuid::Uuid::new_v4();
        let project = sample_project(id, "/Users/me/workspace/app");
        let mut map = HashMap::new();
        map.insert(id.to_string(), project.clone());

        let found = ProjectConfig::find_in_map(&map, &id.to_string()).unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.local_root, project.local_root);

        let missing = ProjectConfig::find_in_map(&map, "not-a-real-id");
        assert_eq!(missing.unwrap_err().kind(), std::io::ErrorKind::NotFound);

        let root = project_local_root_for(&found).unwrap();
        assert_eq!(root, std::path::PathBuf::from("/Users/me/workspace/app"));
    }

    #[test]
    fn browse_and_read_resolve_via_project_local_root() {
        let dir = tempfile::tempdir().unwrap();
        let local_root = dir.path();
        fs::create_dir_all(local_root.join("src")).unwrap();
        fs::write(local_root.join("src/hello.txt"), "hello-from-project").unwrap();

        let project = sample_project(uuid::Uuid::new_v4(), local_root.to_str().unwrap());
        let root_path = project_local_root_for(&project).unwrap();
        let root = canonicalize_project_root(&root_path).unwrap();

        let tree = read_file_tree(&root, 5).unwrap();
        match tree {
            super::FileTreeNode::Directory { children, .. } => {
                assert!(
                    children.iter().any(|child| matches!(
                        child,
                        super::FileTreeNode::Directory { name, .. } if name == "src"
                    )),
                    "expected src directory in tree: {children:?}"
                );
            }
            other => panic!("expected root directory, got {other:?}"),
        }

        let resolved = resolve_project_path(&root, "src/hello.txt").unwrap();
        let content = fs::read_to_string(resolved).unwrap();
        assert_eq!(content, "hello-from-project");

        // Escapes still rejected under the project root.
        assert!(resolve_project_path(&root, "../outside.txt").is_err());
    }
}
