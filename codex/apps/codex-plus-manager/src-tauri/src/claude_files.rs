//! Local / remote file trees confined to a project root.

use serde::Serialize;
use std::path::{Component, Path, PathBuf};

pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub side: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileNode>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub side: String,
    pub content: String,
    pub too_large: bool,
    pub binary: bool,
}

pub fn expand_local_root(local_root: &str) -> PathBuf {
    if let Some(rest) = local_root.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(rest)
    } else {
        PathBuf::from(local_root)
    }
}

pub fn resolve_for_write(root: &Path, relative: &str) -> Result<PathBuf, String> {
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
    if !parent_real.starts_with(&root_real) && parent_real != root_real {
        return Err("路径必须位于项目文件夹内。".into());
    }
    Ok(parent_real.join(target.file_name().unwrap_or_default()))
}

fn is_hidden_from_explorer(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | ".DS_Store" | ".fns_state.json"
    )
}

pub fn read_tree(
    root: &Path,
    side: &str,
    max_depth: usize,
) -> Result<Vec<FileNode>, std::io::Error> {
    read_children(root, "", side, 0, max_depth)
}

fn read_children(
    dir: &Path,
    rel: &str,
    side: &str,
    depth: usize,
    max_depth: usize,
) -> Result<Vec<FileNode>, std::io::Error> {
    if depth >= max_depth {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(_) => return Ok(nodes),
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden_from_explorer(&name) {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let meta = entry.metadata();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta
            .ok()
            .and_then(|m| if m.is_file() { Some(m.len()) } else { None });
        let children = if is_dir {
            Some(read_children(
                &entry.path(),
                &child_rel,
                side,
                depth + 1,
                max_depth,
            )?)
        } else {
            None
        };
        nodes.push(FileNode {
            name,
            path: child_rel,
            kind: if is_dir {
                "directory".into()
            } else {
                "file".into()
            },
            side: side.into(),
            size,
            fingerprint: None,
            children,
        });
    }
    nodes.sort_by(|a, b| match (a.kind.as_str(), b.kind.as_str()) {
        ("directory", "file") => std::cmp::Ordering::Less,
        ("file", "directory") => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(nodes)
}

pub fn preview_is_binary(path: &str, bytes: &[u8]) -> bool {
    if bytes.iter().take(512).any(|b| *b == 0) {
        return true;
    }
    if bytes.starts_with(b"%PDF")
        || bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"GIF8")
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"\x1f\x8b")
    {
        return true;
    }
    if let Some(ext) = path.rsplit('/').next().and_then(|name| {
        name.rfind('.')
            .map(|dot| name[dot + 1..].to_ascii_lowercase())
    }) {
        if matches!(
            ext.as_str(),
            "pdf"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "ico"
                | "bmp"
                | "zip"
                | "gz"
                | "tgz"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
                | "mp3"
                | "mp4"
                | "mov"
                | "wav"
                | "wasm"
                | "bin"
                | "dmg"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
        ) {
            return true;
        }
    }
    std::str::from_utf8(bytes).is_err()
}

pub fn read_preview(root: &Path, relative: &str, side: &str) -> Result<FilePreview, String> {
    let path = resolve_for_write(root, relative)?;
    let meta = std::fs::metadata(&path).map_err(|_| "文件不存在。".to_string())?;
    if meta.len() > MAX_PREVIEW_BYTES {
        return Ok(FilePreview {
            path: relative.into(),
            side: side.into(),
            content: String::new(),
            too_large: true,
            binary: false,
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读不了这个文件：{e}"))?;
    let binary = preview_is_binary(relative, &bytes);
    Ok(FilePreview {
        path: relative.into(),
        side: side.into(),
        content: if binary {
            String::new()
        } else {
            String::from_utf8(bytes).unwrap_or_default()
        },
        too_large: false,
        binary,
    })
}

pub fn parse_remote_listing(stdout: &str, side: &str) -> Vec<FileNode> {
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Node {
        is_dir: bool,
        children: BTreeMap<String, Node>,
    }

    let mut root = Node::default();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('.') {
            continue;
        }
        let is_dir = line.ends_with('/');
        let path = line.trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        let mut current = &mut root;
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            let child = current
                .children
                .entry((*part).to_string())
                .or_insert_with(Node::default);
            if !last || is_dir {
                child.is_dir = true;
            }
            current = child;
        }
    }

    fn flatten(nodes: &BTreeMap<String, Node>, prefix: &str, side: &str) -> Vec<FileNode> {
        nodes
            .iter()
            .map(|(name, node)| {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                FileNode {
                    name: name.clone(),
                    path: path.clone(),
                    kind: if node.is_dir {
                        "directory".into()
                    } else {
                        "file".into()
                    },
                    side: side.into(),
                    size: None,
                    fingerprint: None,
                    children: if node.is_dir {
                        Some(flatten(&node.children, &path, side))
                    } else {
                        None
                    },
                }
            })
            .collect()
    }

    flatten(&root.children, "", side)
}

pub fn content_fingerprint(bytes: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:x}:{}", hasher.finish(), bytes.len())
}

pub fn apply_fingerprints(
    nodes: &mut [FileNode],
    fingerprints: &std::collections::HashMap<String, String>,
) {
    for node in nodes {
        if let Some(fingerprint) = fingerprints.get(&node.path) {
            node.fingerprint = Some(fingerprint.clone());
        }
        if let Some(children) = node.children.as_mut() {
            apply_fingerprints(children, fingerprints);
        }
    }
}

pub fn relative_parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(at) => &path[..at],
        None => "",
    }
}

pub fn relative_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub fn target_dir(path: &str, is_dir: bool) -> String {
    if path.is_empty() {
        String::new()
    } else if is_dir {
        path.to_string()
    } else {
        relative_parent(path).to_string()
    }
}

pub fn sanitize_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err("FILE_NAME_INVALID".into());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err("FILE_NAME_INVALID".into());
    }
    Ok(trimmed.to_string())
}

pub fn split_stem_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (name[..dot].to_string(), name[dot..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

pub fn unique_relative(root: &Path, dir: &str, stem: &str, ext: &str) -> Result<String, String> {
    let first = relative_join(dir, &format!("{stem}{ext}"));
    if !root.join(&first).exists() {
        return Ok(first);
    }
    for n in 2..1000 {
        let candidate = relative_join(dir, &format!("{stem} {n}{ext}"));
        if !root.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err("FILE_EXISTS".into())
}

pub fn unique_named_relative(root: &Path, dir: &str, name: &str) -> Result<String, String> {
    let clean = sanitize_file_name(name)?;
    let (stem, ext) = split_stem_ext(&clean);
    unique_relative(root, dir, &stem, &ext)
}

pub fn resolve_existing(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() {
        return Err("FILE_MISSING".into());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err("PATH_OUTSIDE_PROJECT".into());
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err("PATH_OUTSIDE_PROJECT".into()),
        }
    }
    let root_real = root
        .canonicalize()
        .map_err(|_| "FILE_MISSING".to_string())?;
    let target = root_real.join(candidate);
    if !target.exists() {
        return Err("FILE_MISSING".into());
    }
    let real = target
        .canonicalize()
        .map_err(|_| "FILE_MISSING".to_string())?;
    if !real.starts_with(&root_real) {
        return Err("PATH_OUTSIDE_PROJECT".into());
    }
    Ok(real)
}

pub fn create_empty_file(root: &Path, relative: &str) -> Result<(), String> {
    let target = resolve_for_write(root, relative)?;
    if target.exists() {
        return Err("FILE_EXISTS".into());
    }
    std::fs::write(&target, b"").map_err(|_| "FILE_WRITE_FAILED".to_string())
}

pub fn create_folder(root: &Path, relative: &str) -> Result<(), String> {
    let target = resolve_for_write(root, relative)?;
    if target.exists() {
        return Err("FILE_EXISTS".into());
    }
    std::fs::create_dir(&target).map_err(|_| "FILE_WRITE_FAILED".to_string())
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(from).map_err(|_| "FILE_MISSING".to_string())?;
    if meta.is_dir() {
        std::fs::create_dir_all(to).map_err(|_| "FILE_WRITE_FAILED".to_string())?;
        for entry in std::fs::read_dir(from).map_err(|_| "FILE_WRITE_FAILED".to_string())? {
            let entry = entry.map_err(|_| "FILE_WRITE_FAILED".to_string())?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|_| "FILE_WRITE_FAILED".to_string())?;
        }
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|_| "FILE_WRITE_FAILED".to_string())
    }
}

pub fn duplicate_entry(root: &Path, relative: &str) -> Result<String, String> {
    let source = resolve_existing(root, relative)?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "FILE_NAME_INVALID".to_string())?;
    let (stem, ext) = split_stem_ext(name);
    let dir = relative_parent(relative);
    let dest_rel = unique_relative(root, dir, &format!("{stem} copy"), &ext)?;
    let dest = resolve_for_write(root, &dest_rel)?;
    copy_recursive(&source, &dest)?;
    Ok(dest_rel)
}

pub fn rename_entry(root: &Path, relative: &str, new_name: &str) -> Result<String, String> {
    let name = sanitize_file_name(new_name)?;
    let source = resolve_existing(root, relative)?;
    let dest_rel = relative_join(relative_parent(relative), &name);
    let dest = resolve_for_write(root, &dest_rel)?;
    if dest.exists() && dest != source {
        return Err("FILE_EXISTS".into());
    }
    std::fs::rename(&source, &dest).map_err(|_| "FILE_WRITE_FAILED".to_string())?;
    Ok(dest_rel)
}

pub fn delete_entry(root: &Path, relative: &str) -> Result<(), String> {
    if relative.is_empty() {
        return Err("PATH_OUTSIDE_PROJECT".into());
    }
    let target = resolve_existing(root, relative)?;
    let meta = std::fs::metadata(&target).map_err(|_| "FILE_MISSING".to_string())?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|_| "FILE_WRITE_FAILED".to_string())
    } else {
        std::fs::remove_file(&target).map_err(|_| "FILE_WRITE_FAILED".to_string())
    }
}

pub fn reveal_invocation(path: &Path) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        ("open".into(), vec!["-R".into(), path.display().to_string()])
    }
    #[cfg(target_os = "windows")]
    {
        (
            "explorer".into(),
            vec![format!("/select,{}", path.display())],
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let parent = path.parent().unwrap_or(path).display().to_string();
        ("xdg-open".into(), vec![parent])
    }
}

pub fn open_path_invocation(path: &Path) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        ("open".into(), vec![path.display().to_string()])
    }
    #[cfg(target_os = "windows")]
    {
        ("explorer".into(), vec![path.display().to_string()])
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        ("xdg-open".into(), vec![path.display().to_string()])
    }
}

pub fn flatten_file_paths(nodes: &[FileNode]) -> Vec<String> {
    let mut paths = Vec::new();
    fn walk(nodes: &[FileNode], paths: &mut Vec<String>) {
        for node in nodes {
            if node.kind == "file" {
                paths.push(node.path.clone());
            }
            if let Some(children) = &node.children {
                walk(children, paths);
            }
        }
    }
    walk(nodes, &mut paths);
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tree_includes_nested_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.path().join("README.md"), "hi\n").unwrap();
        let tree = read_tree(root.path(), "local", 4).unwrap();
        assert!(tree.iter().any(|n| n.name == "README.md"));
        let src = tree.iter().find(|n| n.name == "src").expect("src");
        assert_eq!(src.kind, "directory");
        assert!(
            src.children
                .as_ref()
                .unwrap()
                .iter()
                .any(|n| n.path == "src/main.rs")
        );
    }

    #[test]
    fn write_paths_cannot_escape_the_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(resolve_for_write(root.path(), "../secret").is_err());
        assert!(resolve_for_write(root.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn target_dir_uses_the_folder_or_the_file_parent() {
        assert_eq!(target_dir("src", true), "src");
        assert_eq!(target_dir("src/app.ts", false), "src");
        assert_eq!(target_dir("README.md", false), "");
    }

    #[test]
    fn sanitize_file_name_rejects_path_pieces() {
        assert!(sanitize_file_name("ok.ts").is_ok());
        assert!(sanitize_file_name("../x").is_err());
        assert!(sanitize_file_name("a/b").is_err());
        assert!(sanitize_file_name("").is_err());
    }

    #[test]
    fn pdf_and_invalid_utf8_previews_are_binary_not_dumped() {
        let root = tempfile::tempdir().unwrap();
        let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\x80\x81\x82";
        std::fs::write(root.path().join("doc.pdf"), pdf).unwrap();
        let preview = read_preview(root.path(), "doc.pdf", "local").unwrap();
        assert!(preview.binary);
        assert!(preview.content.is_empty());
        std::fs::write(root.path().join("note.md"), "hello\n").unwrap();
        let text = read_preview(root.path(), "note.md", "local").unwrap();
        assert!(!text.binary);
        assert_eq!(text.content, "hello\n");
    }

    #[test]
    fn create_rename_duplicate_and_delete_stay_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        create_empty_file(root.path(), "src/app.ts").unwrap();
        assert!(root.path().join("src/app.ts").is_file());
        let copied = duplicate_entry(root.path(), "src/app.ts").unwrap();
        assert_eq!(copied, "src/app copy.ts");
        assert!(root.path().join("src/app copy.ts").is_file());
        let renamed = rename_entry(root.path(), "src/app copy.ts", "util.ts").unwrap();
        assert_eq!(renamed, "src/util.ts");
        delete_entry(root.path(), "src/util.ts").unwrap();
        assert!(!root.path().join("src/util.ts").exists());
        assert!(delete_entry(root.path(), "").is_err());
        assert!(create_empty_file(root.path(), "../escape.ts").is_err());
    }

    #[test]
    fn create_folder_and_unique_names() {
        let root = tempfile::tempdir().unwrap();
        create_folder(root.path(), "docs").unwrap();
        create_empty_file(root.path(), "untitled").unwrap();
        let second = unique_relative(root.path(), "", "untitled", "").unwrap();
        assert_eq!(second, "untitled 2");
        create_empty_file(root.path(), "note.md").unwrap();
        let named = unique_named_relative(root.path(), "", "note.md").unwrap();
        assert_eq!(named, "note 2.md");
        let (cmd, args) = reveal_invocation(root.path().join("docs").as_path());
        assert!(!cmd.is_empty());
        assert!(args.iter().any(|arg| arg.contains("docs")));
    }

    #[test]
    fn parse_remote_listing_nests_directories() {
        let tree = parse_remote_listing("src/\nsrc/main.rs\nREADME.md\n", "remote");
        assert!(
            tree.iter()
                .any(|node| node.name == "README.md" && node.kind == "file")
        );
        let src = tree.iter().find(|node| node.name == "src").expect("src");
        assert_eq!(src.kind, "directory");
        assert_eq!(src.side, "remote");
        assert!(
            src.children
                .as_ref()
                .unwrap()
                .iter()
                .any(|node| node.path == "src/main.rs" && node.kind == "file")
        );
        let remote_file = src
            .children
            .as_ref()
            .unwrap()
            .iter()
            .find(|node| node.path == "src/main.rs")
            .expect("src/main.rs");
        assert_eq!(remote_file.size, None);
        assert_eq!(remote_file.fingerprint, None);
    }

    #[test]
    fn content_pairs_stamp_comparable_fingerprints_when_remote_size_is_missing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "fn a() {}\n").unwrap();
        let mut local = read_tree(root.path(), "local", 4).unwrap();
        let mut remote = parse_remote_listing("src/\nsrc/lib.rs\n", "remote");
        let remote_file = remote
            .iter()
            .find(|node| node.name == "src")
            .and_then(|node| node.children.as_ref())
            .and_then(|children| children.iter().find(|node| node.path == "src/lib.rs"))
            .expect("remote src/lib.rs");
        assert_eq!(remote_file.size, None);

        let local_bytes = std::fs::read(root.path().join("src/lib.rs")).unwrap();
        let mut fingerprints_local = std::collections::HashMap::new();
        let mut fingerprints_remote = std::collections::HashMap::new();
        fingerprints_local.insert("src/lib.rs".to_string(), content_fingerprint(&local_bytes));
        fingerprints_remote.insert(
            "src/lib.rs".to_string(),
            content_fingerprint(b"fn b() {}\n"),
        );
        apply_fingerprints(&mut local, &fingerprints_local);
        apply_fingerprints(&mut remote, &fingerprints_remote);

        let local_fp = local
            .iter()
            .find(|node| node.name == "src")
            .and_then(|node| node.children.as_ref())
            .and_then(|children| children.iter().find(|node| node.path == "src/lib.rs"))
            .and_then(|node| node.fingerprint.clone());
        let remote_fp = remote
            .iter()
            .find(|node| node.name == "src")
            .and_then(|node| node.children.as_ref())
            .and_then(|children| children.iter().find(|node| node.path == "src/lib.rs"))
            .and_then(|node| node.fingerprint.clone());
        assert!(local_fp.is_some());
        assert!(remote_fp.is_some());
        assert_ne!(local_fp, remote_fp);
    }
}
