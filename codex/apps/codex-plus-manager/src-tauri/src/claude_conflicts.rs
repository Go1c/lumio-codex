//! Conflict list and keepLocal / keepRemote / keepBoth resolution.
//!
//! Resolutions never silently overwrite the other side: keepLocal leaves the
//! local file, keepRemote replaces it from the recorded remote side, keepBoth
//! writes the remote copy beside the original.

use crate::claude_files;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    BothModified,
    RemoteDeleted,
    LocalDeleted,
}

impl ConflictKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::BothModified => "本地与远端同时修改",
            Self::RemoteDeleted => "远端已删除，本地已修改",
            Self::LocalDeleted => "本地已删除，远端已修改",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub content: String,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub id: String,
    pub path: String,
    pub kind: ConflictKind,
    pub kind_label: String,
    pub local: ConflictSide,
    pub remote: ConflictSide,
    #[serde(default = "default_true")]
    pub can_resolve: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    KeepLocal,
    KeepRemote,
    KeepBoth,
}

impl Resolution {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "keepLocal" => Ok(Self::KeepLocal),
            "keepRemote" => Ok(Self::KeepRemote),
            "keepBoth" => Ok(Self::KeepBoth),
            _ => Err("不认识这个处理方式。".into()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::KeepLocal => "保留本地",
            Self::KeepRemote => "保留远端",
            Self::KeepBoth => "两者都保留",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictView {
    pub id: String,
    pub path: String,
    pub kind_label: String,
    pub local_content: String,
    pub remote_content: String,
    pub can_resolve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionReceipt {
    pub conflict_id: String,
    pub path: String,
    pub resolution: Resolution,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_path: Option<String>,
    pub remaining: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ConflictFile {
    #[serde(default)]
    conflicts: Vec<Conflict>,
}

pub struct ConflictStore {
    path: PathBuf,
}

impl ConflictStore {
    pub fn new(dir: &Path, project_id: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法准备冲突记录目录：{e}"))?;
        Ok(Self {
            path: dir.join(format!("{project_id}.json")),
        })
    }

    fn load(&self) -> ConflictFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, file: &ConflictFile) -> Result<(), String> {
        let bytes =
            serde_json::to_vec_pretty(file).map_err(|e| format!("无法写入冲突记录：{e}"))?;
        std::fs::write(&self.path, bytes).map_err(|e| format!("无法写入冲突记录：{e}"))
    }

    pub fn list(&self) -> Vec<Conflict> {
        self.load().conflicts
    }

    pub fn replace(&self, conflicts: Vec<Conflict>) -> Result<(), String> {
        self.save(&ConflictFile { conflicts })
    }

    pub fn resolve(
        &self,
        local_root: &Path,
        conflict_id: &str,
        resolution: Resolution,
    ) -> Result<ResolutionReceipt, String> {
        let mut file = self.load();
        let index = file
            .conflicts
            .iter()
            .position(|c| c.id == conflict_id)
            .ok_or_else(|| "该冲突已被处理。".to_string())?;
        let conflict = file.conflicts.remove(index);

        let target = claude_files::resolve_for_write(local_root, &conflict.path)?;
        let mut copy_path = None;

        match resolution {
            Resolution::KeepLocal => {}
            Resolution::KeepRemote => {
                if conflict.remote.deleted {
                    if target.exists() {
                        std::fs::remove_file(&target).map_err(|e| format!("解决冲突失败：{e}"))?;
                    }
                } else {
                    write_file(&target, &conflict.remote.content)?;
                }
            }
            Resolution::KeepBoth => {
                let copy_relative = conflict_copy_path(&conflict.path);
                let copy_target = claude_files::resolve_for_write(local_root, &copy_relative)?;
                write_file(&copy_target, &conflict.remote.content)?;
                copy_path = Some(copy_relative);
            }
        }

        self.save(&file)?;
        Ok(ResolutionReceipt {
            conflict_id: conflict.id,
            path: conflict.path,
            resolution,
            label: resolution.label().to_string(),
            copy_path,
            remaining: file.conflicts.len(),
        })
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("解决冲突失败：{e}"))?;
    }
    std::fs::write(path, content).map_err(|e| format!("解决冲突失败：{e}"))
}

pub fn conflict_copy_path(path: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !stem.ends_with('/') => {
            format!("{stem}.服务器版本.{ext}")
        }
        _ => format!("{path}.服务器版本"),
    }
}

pub fn sample_conflicts() -> Vec<Conflict> {
    vec![
        Conflict {
            id: "conflict-engine".into(),
            path: "src/engine.rs".into(),
            kind: ConflictKind::BothModified,
            kind_label: ConflictKind::BothModified.label().into(),
            local: ConflictSide {
                content: "local version\n".into(),
                deleted: false,
            },
            remote: ConflictSide {
                content: "pub struct WriteBatcher {\n    queue: VecDeque<Mutation>,\n}\n".into(),
                deleted: false,
            },
            can_resolve: true,
        },
        Conflict {
            id: "conflict-cargo".into(),
            path: "Cargo.toml".into(),
            kind: ConflictKind::RemoteDeleted,
            kind_label: ConflictKind::RemoteDeleted.label().into(),
            local: ConflictSide {
                content: "local cargo\n".into(),
                deleted: false,
            },
            remote: ConflictSide {
                content: String::new(),
                deleted: true,
            },
            can_resolve: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _root: tempfile::TempDir,
        _records: tempfile::TempDir,
        root: PathBuf,
        store: ConflictStore,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().expect("root");
        let records = tempfile::tempdir().expect("records");
        let store = ConflictStore::new(records.path(), "project-1").expect("store");
        std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
        std::fs::write(root.path().join("src/engine.rs"), "local version\n").expect("write");
        std::fs::write(root.path().join("Cargo.toml"), "local cargo\n").expect("write");
        store.replace(sample_conflicts()).expect("seed");
        Fixture {
            root: root.path().to_path_buf(),
            _root: root,
            _records: records,
            store,
        }
    }

    #[test]
    fn keeping_local_leaves_the_file_untouched_and_clears_the_conflict() {
        let f = fixture();
        let receipt = f
            .store
            .resolve(&f.root, "conflict-engine", Resolution::KeepLocal)
            .expect("resolve");
        assert_eq!(receipt.remaining, 1);
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read"),
            "local version\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.root.join("Cargo.toml")).expect("read"),
            "local cargo\n"
        );
    }

    #[test]
    fn keeping_remote_overwrites_only_the_conflicted_file() {
        let f = fixture();
        f.store
            .resolve(&f.root, "conflict-engine", Resolution::KeepRemote)
            .expect("resolve");
        let after = std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read");
        assert!(after.contains("VecDeque"));
        assert_eq!(
            std::fs::read_to_string(f.root.join("Cargo.toml")).expect("read"),
            "local cargo\n"
        );
        assert!(f.store.list().iter().all(|c| c.id != "conflict-engine"));
    }

    #[test]
    fn keeping_both_writes_the_server_copy_beside_the_original() {
        let f = fixture();
        let receipt = f
            .store
            .resolve(&f.root, "conflict-engine", Resolution::KeepBoth)
            .expect("resolve");
        let copy = receipt.copy_path.expect("copy path");
        assert_eq!(copy, "src/engine.服务器版本.rs");
        assert!(f.root.join(&copy).exists());
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read"),
            "local version\n"
        );
        assert!(
            std::fs::read_to_string(f.root.join(&copy))
                .expect("read")
                .contains("VecDeque")
        );
    }
}
