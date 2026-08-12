//! Local sync folder explorer (交互设计 5.5「文件」).
//!
//! Every operation is expressed against the project's local sync folder and is
//! confined to it: paths are relative, `..` is rejected, and the resolved target
//! must still live under the root after symlinks are followed. What lands in the
//! folder is picked up by `fns-sync-core` and mirrored to the server.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Largest file we will render in the built-in viewer (5.5 五态).
pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

/// A node in the explorer tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    /// Path relative to the sync root; `""` is the root itself.
    pub path: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Unix milliseconds, so the frontend can render relative times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileNode>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Directory,
    File,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
    /// Empty when `tooLarge` or `binary` is set.
    pub content: String,
    pub too_large: bool,
    pub binary: bool,
}

/// Handle returned by a delete so the 10 秒「撤销」 toast can put it back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashTicket {
    pub token: String,
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Directory,
}

/// Resolve a root-relative path, refusing anything that escapes the root.
pub fn resolve_within(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim_start_matches('/');
    if relative.is_empty() {
        return Ok(root.to_path_buf());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err("路径必须位于项目文件夹内。".into());
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err("路径必须位于项目文件夹内。".into()),
        }
    }

    let joined = root.join(candidate);
    // Follow symlinks where the target already exists; new files are checked by
    // their (existing) parent instead.
    let checked = match joined.canonicalize() {
        Ok(resolved) => resolved,
        Err(_) => {
            let parent = joined.parent().unwrap_or(root);
            let resolved_parent = parent
                .canonicalize()
                .map_err(|_| "目标文件夹不存在。".to_string())?;
            resolved_parent.join(joined.file_name().unwrap_or_default())
        }
    };
    let root_real = root
        .canonicalize()
        .map_err(|_| "项目文件夹不存在，可能已被移动或删除。".to_string())?;
    if !checked.starts_with(&root_real) {
        return Err("路径必须位于项目文件夹内。".into());
    }
    Ok(checked)
}

/// Resolve a path that is about to be written, creating missing parents.
///
/// [`resolve_within`] needs the parent to exist already; conflict resolution can
/// target a file whose folder has not been synced down yet. The parent is
/// canonicalised after creation so a symlinked ancestor still cannot escape.
pub fn resolve_for_write(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim_start_matches('/');
    let candidate = Path::new(relative);
    if relative.is_empty() || candidate.is_absolute() {
        return Err("路径必须位于项目文件夹内。".into());
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err("路径必须位于项目文件夹内。".into()),
        }
    }

    let root_real = root
        .canonicalize()
        .or_else(|_| std::fs::create_dir_all(root).and_then(|()| root.canonicalize()))
        .map_err(|_| "项目文件夹不存在，可能已被移动或删除。".to_string())?;

    let target = root_real.join(candidate);
    let parent = target.parent().unwrap_or(&root_real).to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| format!("无法创建目录：{e}"))?;
    let parent_real = parent
        .canonicalize()
        .map_err(|e| format!("无法创建目录：{e}"))?;
    if !parent_real.starts_with(&root_real) {
        return Err("路径必须位于项目文件夹内。".into());
    }
    Ok(parent_real.join(target.file_name().unwrap_or_default()))
}

/// Reject names that would create a path rather than an entry.
pub fn validate_entry_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("请输入名称。".into());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("这个名称不可用，请换一个。".into());
    }
    if trimmed.contains('/') || trimmed.contains('\0') {
        return Err("名称中不能包含「/」。".into());
    }
    Ok(trimmed)
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// Names never shown in the explorer: build output and sync bookkeeping.
fn is_hidden_from_explorer(name: &str) -> bool {
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

/// Read the explorer tree. Folders sort before files, matching VS Code.
pub fn read_tree(root: &Path, max_depth: usize) -> Result<Vec<FileNode>, std::io::Error> {
    read_children(root, "", 0, max_depth)
}

fn read_children(
    dir: &Path,
    rel: &str,
    depth: usize,
    max_depth: usize,
) -> Result<Vec<FileNode>, std::io::Error> {
    if depth >= max_depth {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    for entry in std::fs::read_dir(dir)?.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden_from_explorer(&name) {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(link_meta) = entry.path().symlink_metadata() else {
            continue;
        };

        if link_meta.file_type().is_symlink() {
            nodes.push(FileNode {
                name,
                path: child_rel,
                kind: NodeKind::Symlink,
                size: None,
                modified_ms: modified_ms(&link_meta),
                children: None,
            });
        } else if meta.is_dir() {
            nodes.push(FileNode {
                name,
                path: child_rel.clone(),
                kind: NodeKind::Directory,
                size: None,
                modified_ms: modified_ms(&meta),
                children: Some(read_children(
                    &entry.path(),
                    &child_rel,
                    depth + 1,
                    max_depth,
                )?),
            });
        } else {
            nodes.push(FileNode {
                name,
                path: child_rel,
                kind: NodeKind::File,
                size: Some(meta.len()),
                modified_ms: modified_ms(&meta),
                children: None,
            });
        }
    }
    nodes.sort_by(|a, b| {
        let a_dir = a.kind == NodeKind::Directory;
        let b_dir = b.kind == NodeKind::Directory;
        b_dir.cmp(&a_dir).then_with(|| a.name.cmp(&b.name))
    });
    Ok(nodes)
}

/// Flattened list of recently modified files for the 「最近更新」 panel.
pub fn recent_files(root: &Path, limit: usize) -> Result<Vec<FileNode>, std::io::Error> {
    fn flatten(nodes: Vec<FileNode>, out: &mut Vec<FileNode>) {
        for mut node in nodes {
            if let Some(children) = node.children.take() {
                flatten(children, out);
            } else if node.kind == NodeKind::File {
                out.push(node);
            }
        }
    }
    let mut files = Vec::new();
    flatten(read_tree(root, 8)?, &mut files);
    files.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    files.truncate(limit);
    Ok(files)
}

/// Read a file for the built-in viewer.
pub fn read_preview(root: &Path, relative: &str) -> Result<FilePreview, String> {
    let path = resolve_within(root, relative)?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("无法读取文件：{e}"))?;
    let size = meta.len();
    if size > MAX_PREVIEW_BYTES {
        return Ok(FilePreview {
            path: relative.to_string(),
            size,
            modified_ms: modified_ms(&meta),
            content: String::new(),
            too_large: true,
            binary: false,
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("无法读取文件：{e}"))?;
    match String::from_utf8(bytes) {
        Ok(content) => Ok(FilePreview {
            path: relative.to_string(),
            size,
            modified_ms: modified_ms(&meta),
            content,
            too_large: false,
            binary: false,
        }),
        Err(_) => Ok(FilePreview {
            path: relative.to_string(),
            size,
            modified_ms: modified_ms(&meta),
            content: String::new(),
            too_large: false,
            binary: true,
        }),
    }
}

/// Create a file or folder; returns its root-relative path.
pub fn create_entry(
    root: &Path,
    parent: &str,
    name: &str,
    kind: EntryKind,
) -> Result<String, String> {
    let name = validate_entry_name(name)?;
    let parent_rel = parent.trim_matches('/');
    let relative = if parent_rel.is_empty() {
        name.to_string()
    } else {
        format!("{parent_rel}/{name}")
    };
    let target = resolve_within(root, &relative)?;
    if target.exists() {
        return Err(format!("「{name}」已存在，请换一个名称。"));
    }
    match kind {
        EntryKind::Directory => {
            std::fs::create_dir(&target).map_err(|e| format!("无法创建文件夹：{e}"))?
        }
        EntryKind::File => {
            std::fs::write(&target, b"").map_err(|e| format!("无法创建文件：{e}"))?
        }
    }
    Ok(relative)
}

/// Rename in place; returns the new root-relative path.
pub fn rename_entry(root: &Path, relative: &str, new_name: &str) -> Result<String, String> {
    let new_name = validate_entry_name(new_name)?;
    let source = resolve_within(root, relative)?;
    if !source.exists() {
        return Err("原文件已不存在，可能刚刚被同步删除。".into());
    }
    let parent_rel = relative
        .trim_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default();
    let new_relative = if parent_rel.is_empty() {
        new_name.to_string()
    } else {
        format!("{parent_rel}/{new_name}")
    };
    let target = resolve_within(root, &new_relative)?;
    if target.exists() {
        return Err(format!("「{new_name}」已存在，请换一个名称。"));
    }
    std::fs::rename(&source, &target).map_err(|e| format!("重命名失败：{e}"))?;
    Ok(new_relative)
}

/// Move an entry into the app's staging area so the delete can be undone.
pub fn delete_entry(root: &Path, relative: &str, staging: &Path) -> Result<TrashTicket, String> {
    let source = resolve_within(root, relative)?;
    if !source.exists() {
        return Err("文件已不存在。".into());
    }
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative.to_string());

    let token = uuid::Uuid::new_v4().to_string();
    let bucket = staging.join(&token);
    std::fs::create_dir_all(&bucket).map_err(|e| format!("无法准备回收暂存目录：{e}"))?;
    std::fs::rename(&source, bucket.join(&name)).map_err(|e| format!("删除失败：{e}"))?;
    std::fs::write(bucket.join(".origin"), relative)
        .map_err(|e| format!("无法记录撤销信息：{e}"))?;

    Ok(TrashTicket {
        token,
        path: relative.to_string(),
        name,
    })
}

/// Put a staged deletion back where it came from.
pub fn restore_entry(root: &Path, staging: &Path, token: &str) -> Result<String, String> {
    if token.is_empty() || token.contains('/') || token.contains("..") {
        return Err("撤销信息无效。".into());
    }
    let bucket = staging.join(token);
    let relative = std::fs::read_to_string(bucket.join(".origin"))
        .map_err(|_| "撤销已过期，无法恢复。".to_string())?;
    let source = std::fs::read_dir(&bucket)
        .map_err(|e| format!("撤销失败：{e}"))?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name() != std::ffi::OsStr::new(".origin"))
        .map(|entry| entry.path())
        .ok_or_else(|| "撤销已过期，无法恢复。".to_string())?;

    let target = resolve_within(root, &relative)?;
    if target.exists() {
        return Err("该位置已有同名文件，无法撤销。".into());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("撤销失败：{e}"))?;
    }
    std::fs::rename(&source, &target).map_err(|e| format!("撤销失败：{e}"))?;
    let _ = std::fs::remove_dir_all(&bucket);
    Ok(relative)
}

/// Drop a staged deletion once the undo window has closed.
pub fn purge_staged(staging: &Path, token: &str) {
    if token.is_empty() || token.contains('/') || token.contains("..") {
        return;
    }
    let _ = std::fs::remove_dir_all(staging.join(token));
}

// --- Diff (used by the conflicts view) ---

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
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

/// Compute a line diff between two texts.
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
                        content: s.trim_end_matches('\n').to_string(),
                        line_number: old_line,
                    });
                    old_line += 1;
                    new_line += 1;
                }
                diffy::Line::Delete(s) => {
                    lines.push(DiffLine::Removed {
                        content: s.trim_end_matches('\n').to_string(),
                        line_number: old_line,
                    });
                    old_line += 1;
                }
                diffy::Line::Insert(s) => {
                    lines.push(DiffLine::Added {
                        content: s.trim_end_matches('\n').to_string(),
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

    DiffResult { hunks }
}

/// Reveal a path in Finder.
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("无法在 Finder 中显示：{e}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        open::that_detached(path).map_err(|e| format!("无法打开：{e}"))
    }
}

/// Open a path with the user's default application.
pub fn open_default(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|e| format!("无法打开：{e}"))
}

/// Milliseconds since the epoch, for callers that need a timestamp.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn rejects_paths_that_leave_the_project_folder() {
        let root = temp_root();
        for attempt in ["../secrets", "/etc/passwd", "a/../../b", ".."] {
            assert!(
                resolve_within(root.path(), attempt).is_err(),
                "expected {attempt:?} to be rejected"
            );
        }
    }

    #[test]
    fn resolves_paths_inside_the_project_folder() {
        let root = temp_root();
        std::fs::create_dir(root.path().join("src")).expect("mkdir");
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("write");

        let resolved = resolve_within(root.path(), "src/main.rs").expect("resolve");
        assert!(resolved.ends_with("src/main.rs"));
        assert_eq!(resolve_within(root.path(), "").expect("root"), root.path());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_that_points_outside() {
        let root = temp_root();
        let outside = temp_root();
        std::fs::write(outside.path().join("secret.txt"), "s").expect("write");
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.path().join("link"))
            .expect("symlink");

        assert!(resolve_within(root.path(), "link").is_err());
    }

    #[test]
    fn entry_names_must_be_single_segments() {
        assert!(validate_entry_name("main.rs").is_ok());
        assert_eq!(
            validate_entry_name("  spaced.rs  ").expect("ok"),
            "spaced.rs"
        );
        for bad in ["", "   ", "..", ".", "a/b"] {
            assert!(
                validate_entry_name(bad).is_err(),
                "expected {bad:?} rejected"
            );
        }
    }

    #[test]
    fn tree_lists_folders_first_and_hides_build_output() {
        let root = temp_root();
        std::fs::create_dir(root.path().join("src")).expect("mkdir");
        std::fs::create_dir(root.path().join("node_modules")).expect("mkdir");
        std::fs::write(root.path().join("README.md"), "hi").expect("write");
        std::fs::write(root.path().join("src/lib.rs"), "x").expect("write");

        let tree = read_tree(root.path(), 5).expect("tree");
        let names: Vec<_> = tree.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["src", "README.md"]);
        assert_eq!(tree[0].children.as_ref().expect("children").len(), 1);
    }

    #[test]
    fn create_rename_and_delete_round_trip_with_undo() {
        let root = temp_root();
        let staging = temp_root();

        let created = create_entry(root.path(), "", "notes.md", EntryKind::File).expect("create");
        assert_eq!(created, "notes.md");
        assert!(root.path().join("notes.md").exists());

        // Creating the same name twice is a user error, not a silent overwrite.
        assert!(create_entry(root.path(), "", "notes.md", EntryKind::File).is_err());

        let renamed = rename_entry(root.path(), "notes.md", "readme.md").expect("rename");
        assert_eq!(renamed, "readme.md");
        assert!(!root.path().join("notes.md").exists());

        let ticket = delete_entry(root.path(), "readme.md", staging.path()).expect("delete");
        assert!(!root.path().join("readme.md").exists());

        let restored = restore_entry(root.path(), staging.path(), &ticket.token).expect("undo");
        assert_eq!(restored, "readme.md");
        assert!(root.path().join("readme.md").exists());
    }

    #[test]
    fn nested_entries_keep_their_parent_on_rename() {
        let root = temp_root();
        create_entry(root.path(), "", "src", EntryKind::Directory).expect("mkdir");
        create_entry(root.path(), "src", "a.rs", EntryKind::File).expect("create");

        let renamed = rename_entry(root.path(), "src/a.rs", "b.rs").expect("rename");
        assert_eq!(renamed, "src/b.rs");
        assert!(root.path().join("src/b.rs").exists());
    }

    #[test]
    fn purging_a_staged_delete_makes_undo_impossible() {
        let root = temp_root();
        let staging = temp_root();
        std::fs::write(root.path().join("gone.txt"), "x").expect("write");

        let ticket = delete_entry(root.path(), "gone.txt", staging.path()).expect("delete");
        purge_staged(staging.path(), &ticket.token);
        assert!(restore_entry(root.path(), staging.path(), &ticket.token).is_err());
    }

    #[test]
    fn oversized_files_are_reported_rather_than_read() {
        let root = temp_root();
        let big = vec![b'a'; (MAX_PREVIEW_BYTES + 1) as usize];
        std::fs::write(root.path().join("big.log"), big).expect("write");

        let preview = read_preview(root.path(), "big.log").expect("preview");
        assert!(preview.too_large);
        assert!(preview.content.is_empty());
    }

    #[test]
    fn binary_files_are_flagged_instead_of_mangled() {
        let root = temp_root();
        std::fs::write(root.path().join("blob.bin"), [0xff, 0xfe, 0x00]).expect("write");

        let preview = read_preview(root.path(), "blob.bin").expect("preview");
        assert!(preview.binary);
        assert!(!preview.too_large);
    }

    #[test]
    fn recent_files_are_ordered_by_modification_time() {
        let root = temp_root();
        std::fs::create_dir(root.path().join("src")).expect("mkdir");
        std::fs::write(root.path().join("old.txt"), "old").expect("write");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.path().join("src/new.txt"), "new").expect("write");

        let recent = recent_files(root.path(), 5).expect("recent");
        assert_eq!(recent.first().map(|n| n.path.as_str()), Some("src/new.txt"));
    }

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let diff = compute_text_diff("a\nb\n", "a\nc\n");
        let kinds: Vec<_> = diff.hunks[0]
            .lines
            .iter()
            .map(|line| match line {
                DiffLine::Context { .. } => "ctx",
                DiffLine::Added { .. } => "add",
                DiffLine::Removed { .. } => "del",
            })
            .collect();
        assert!(kinds.contains(&"add"));
        assert!(kinds.contains(&"del"));
    }
}
