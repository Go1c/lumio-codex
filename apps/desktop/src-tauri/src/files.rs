//! File tree browser and diff view backend.
//!
//! Reads the local workspace directory tree and provides it to the frontend.
//! Computes text diffs between local and remote content using diffy.

use serde::{Deserialize, Serialize};
use std::path::Path;

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

        let mut entries: Vec<_> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .collect();
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
        ".git" | ".hg" | ".svn"
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

// --- Tauri commands ---

#[tauri::command]
pub fn browse_files(local_root: String) -> Result<FileTreeNode, String> {
    read_file_tree(Path::new(&local_root), 5).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_file(path: String, base_dir: String) -> Result<String, String> {
    let full = Path::new(&base_dir).join(&path);
    std::fs::read_to_string(&full).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn compute_diff(old_text: String, new_text: String) -> Result<DiffResult, String> {
    Ok(compute_text_diff(&old_text, &new_text))
}
