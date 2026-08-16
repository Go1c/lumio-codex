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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineConflictRecord {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub local: ConflictSide,
    pub remote: ConflictSide,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct EngineConflictFile {
    #[serde(default)]
    conflicts: Vec<EngineConflictRecord>,
}

fn parse_engine_kind(value: Option<&str>) -> ConflictKind {
    match value.unwrap_or_default() {
        "remoteDeleted" | "RemoteDeleted" | "remote_deleted" => ConflictKind::RemoteDeleted,
        "localDeleted" | "LocalDeleted" | "local_deleted" => ConflictKind::LocalDeleted,
        _ => ConflictKind::BothModified,
    }
}

pub fn records_to_conflicts(records: Vec<EngineConflictRecord>) -> Vec<Conflict> {
    records
        .into_iter()
        .map(|record| {
            let kind = parse_engine_kind(record.kind.as_deref());
            Conflict {
                id: record.id,
                path: record.path,
                kind,
                kind_label: kind.label().into(),
                local: record.local,
                remote: record.remote,
                can_resolve: true,
            }
        })
        .collect()
}

pub fn ingest_engine_conflicts(
    store: &ConflictStore,
    records: Vec<EngineConflictRecord>,
) -> Result<usize, String> {
    let conflicts = records_to_conflicts(records);
    let n = conflicts.len();
    store.replace(conflicts)?;
    Ok(n)
}

pub fn ingest_sidecar_conflicts(store: &ConflictStore, state_dir: &Path) -> Result<usize, String> {
    let path = state_dir.join("conflicts.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(0);
    };
    let file: EngineConflictFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("无法读取同步冲突：{e}"))?;
    ingest_engine_conflicts(store, file.conflicts)
}

pub fn write_sidecar_conflicts(
    state_dir: &Path,
    records: &[EngineConflictRecord],
) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|e| format!("无法写入同步冲突：{e}"))?;
    let bytes = serde_json::to_vec_pretty(&EngineConflictFile {
        conflicts: records.to_vec(),
    })
    .map_err(|e| format!("无法写入同步冲突：{e}"))?;
    std::fs::write(state_dir.join("conflicts.json"), bytes)
        .map_err(|e| format!("无法写入同步冲突：{e}"))
}

pub fn detect_content_conflicts(
    local_root: &Path,
    remote_files: &[(String, String)],
) -> Vec<EngineConflictRecord> {
    let mut found = Vec::new();
    for (path, remote_content) in remote_files {
        if path.is_empty() || path.contains("..") {
            continue;
        }
        let local_path = local_root.join(path);
        let Ok(local_content) = std::fs::read_to_string(&local_path) else {
            continue;
        };
        if local_content == *remote_content {
            continue;
        }
        found.push(EngineConflictRecord {
            id: format!("conflict-{}", path.replace(['/', '\\'], "-")),
            path: path.clone(),
            kind: Some("bothModified".into()),
            local: ConflictSide {
                content: local_content,
                deleted: false,
            },
            remote: ConflictSide {
                content: remote_content.clone(),
                deleted: false,
            },
        });
    }
    found
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

    #[test]
    fn engine_ingest_writes_conflicts_the_store_already_reads() {
        let records = tempfile::tempdir().expect("records");
        let store = ConflictStore::new(records.path(), "project-ingest").expect("store");
        assert!(store.list().is_empty());
        let ingested = ingest_engine_conflicts(
            &store,
            vec![EngineConflictRecord {
                id: "c-readme".into(),
                path: "README.md".into(),
                kind: Some("bothModified".into()),
                local: ConflictSide {
                    content: "local readme\n".into(),
                    deleted: false,
                },
                remote: ConflictSide {
                    content: "remote readme\n".into(),
                    deleted: false,
                },
            }],
        )
        .expect("ingest");
        assert_eq!(ingested, 1);
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "c-readme");
        assert_eq!(listed[0].path, "README.md");
        assert_eq!(listed[0].local.content, "local readme\n");
        assert_eq!(listed[0].remote.content, "remote readme\n");
    }

    #[test]
    fn ingest_from_sidecar_state_file_feeds_the_store() {
        let state = tempfile::tempdir().expect("state");
        let records = tempfile::tempdir().expect("records");
        std::fs::write(
            state.path().join("conflicts.json"),
            r#"{"conflicts":[{"id":"c-1","path":"src/a.rs","kind":"bothModified","local":{"content":"aaa\n","deleted":false},"remote":{"content":"bbb\n","deleted":false}}]}"#,
        )
        .expect("write");
        let store = ConflictStore::new(records.path(), "project-sidecar").expect("store");
        let n = ingest_sidecar_conflicts(&store, state.path()).expect("ingest");
        assert_eq!(n, 1);
        assert_eq!(store.list()[0].path, "src/a.rs");
    }

    #[test]
    fn ingested_keep_local_still_does_not_overwrite() {
        let root = tempfile::tempdir().expect("root");
        let records = tempfile::tempdir().expect("records");
        std::fs::write(root.path().join("notes.txt"), "keep me\n").expect("write");
        let store = ConflictStore::new(records.path(), "project-keep").expect("store");
        ingest_engine_conflicts(
            &store,
            vec![EngineConflictRecord {
                id: "c-notes".into(),
                path: "notes.txt".into(),
                kind: Some("bothModified".into()),
                local: ConflictSide {
                    content: "keep me\n".into(),
                    deleted: false,
                },
                remote: ConflictSide {
                    content: "server notes\n".into(),
                    deleted: false,
                },
            }],
        )
        .expect("ingest");
        store
            .resolve(root.path(), "c-notes", Resolution::KeepLocal)
            .expect("resolve");
        assert_eq!(
            std::fs::read_to_string(root.path().join("notes.txt")).expect("read"),
            "keep me\n"
        );
        assert!(!root.path().join("notes.服务器版本.txt").exists());
    }

    #[test]
    fn both_modified_pairs_become_engine_records() {
        let local = tempfile::tempdir().expect("local");
        std::fs::write(local.path().join("same.txt"), "local\n").expect("write");
        let found =
            detect_content_conflicts(local.path(), &[("same.txt".into(), "remote\n".into())]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "same.txt");
        assert_eq!(found[0].local.content, "local\n");
        assert_eq!(found[0].remote.content, "remote\n");
    }
}
