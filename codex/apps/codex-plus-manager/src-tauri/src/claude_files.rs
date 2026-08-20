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
    let binary = bytes.iter().take(512).any(|b| *b == 0);
    Ok(FilePreview {
        path: relative.into(),
        side: side.into(),
        content: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
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
