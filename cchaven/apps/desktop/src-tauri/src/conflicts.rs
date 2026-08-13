//! Conflict list and resolution (交互设计 5.5「冲突」).
//!
//! `fns-sync-core` owns conflict detection; this module is the desktop-side
//! projection of it: a per-project record file the UI reads, plus the three
//! resolutions the user can pick. Whatever a resolution overwrites is staged in
//! a local recycle folder first, so the 10 秒「撤销」 toast can always put it back.
//!
//! Wiring the live engine stream into this store is tracked in
//! `docs/spec-gaps.md`; until then a project without a record file starts empty
//! (and the mock backend seeds a sample pair for UI work).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::files;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// Both sides edited the same file.
    BothModified,
    /// The server deleted a file the user had edited locally.
    RemoteDeleted,
    /// The user deleted a file the server had edited.
    LocalDeleted,
}

impl ConflictKind {
    /// zh-CN label shown under the path in the conflict list.
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
    pub modified_ms: i64,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub id: String,
    /// Root-relative path inside the project's local sync folder.
    pub path: String,
    pub kind: ConflictKind,
    pub kind_label: String,
    pub detected_at_ms: i64,
    pub local: ConflictSide,
    pub remote: ConflictSide,
    /// False while the engine is still settling this conflict; the buttons stay
    /// disabled rather than queueing an answer the server will reject.
    #[serde(default = "default_can_resolve")]
    pub can_resolve: bool,
    /// The choice already on its way to the server, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_resolution: Option<String>,
}

fn default_can_resolve() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    KeepLocal,
    KeepRemote,
    /// Keep local and write the server's version alongside it.
    KeepBoth,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Self::KeepLocal => "保留本地",
            Self::KeepRemote => "保留远端",
            Self::KeepBoth => "两者都保留",
        }
    }
}

/// Result of resolving one conflict, including everything undo needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionReceipt {
    pub conflict_id: String,
    pub path: String,
    pub resolution: Resolution,
    pub label: String,
    /// Extra file created by 「两者都保留」, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_path: Option<String>,
    /// Remaining unresolved conflicts, for the tab badge.
    pub remaining: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ConflictFile {
    #[serde(default)]
    conflicts: Vec<Conflict>,
    /// Conflicts resolved within the undo window, keyed by conflict id.
    #[serde(default)]
    undo: Vec<UndoRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoRecord {
    conflict: Conflict,
    resolution: Resolution,
    /// Content the resolution overwrote, if it overwrote anything.
    overwritten: Option<String>,
    copy_path: Option<String>,
}

/// Per-project conflict projection stored under the app config directory.
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

    /// Replace the conflict set, as the sync engine reports it.
    pub fn replace(&self, conflicts: Vec<Conflict>) -> Result<(), String> {
        let mut file = self.load();
        file.conflicts = conflicts;
        self.save(&file)
    }

    /// Apply a resolution to the local sync folder and drop the conflict.
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

        let target = files::resolve_for_write(local_root, &conflict.path)?;
        let overwritten = std::fs::read_to_string(&target).ok();
        let mut copy_path = None;

        match resolution {
            // The local file already holds what the user wants to keep; the
            // engine pushes it outwards on the next pass.
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
                let copy_target = files::resolve_for_write(local_root, &copy_relative)?;
                write_file(&copy_target, &conflict.remote.content)?;
                copy_path = Some(copy_relative);
            }
        }

        file.undo.push(UndoRecord {
            conflict: conflict.clone(),
            resolution,
            overwritten,
            copy_path: copy_path.clone(),
        });
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

    /// Undo a resolution: restore the overwritten bytes and re-list the conflict.
    pub fn undo(&self, local_root: &Path, conflict_id: &str) -> Result<Conflict, String> {
        let mut file = self.load();
        let index = file
            .undo
            .iter()
            .position(|record| record.conflict.id == conflict_id)
            .ok_or_else(|| "撤销已过期。".to_string())?;
        let record = file.undo.remove(index);

        let target = files::resolve_for_write(local_root, &record.conflict.path)?;
        match (&record.overwritten, record.resolution) {
            (Some(previous), Resolution::KeepRemote) => write_file(&target, previous)?,
            (None, Resolution::KeepRemote) => {
                if target.exists() {
                    std::fs::remove_file(&target).map_err(|e| format!("撤销失败：{e}"))?;
                }
            }
            _ => {}
        }
        if let Some(copy_relative) = &record.copy_path {
            let copy_target = files::resolve_for_write(local_root, copy_relative)?;
            if copy_target.exists() {
                std::fs::remove_file(&copy_target).map_err(|e| format!("撤销失败：{e}"))?;
            }
        }

        file.conflicts.insert(0, record.conflict.clone());
        self.save(&file)?;
        Ok(record.conflict)
    }

    /// Forget an undo record once its 10 second window has closed.
    pub fn forget_undo(&self, conflict_id: &str) {
        let mut file = self.load();
        file.undo.retain(|record| record.conflict.id != conflict_id);
        let _ = self.save(&file);
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("解决冲突失败：{e}"))?;
    }
    std::fs::write(path, content).map_err(|e| format!("解决冲突失败：{e}"))
}

/// `src/engine.rs` → `src/engine.服务器版本.rs`, so the copy sorts next to the
/// original and reads unambiguously in Finder.
pub fn conflict_copy_path(path: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !stem.ends_with('/') => {
            format!("{stem}.服务器版本.{ext}")
        }
        _ => format!("{path}.服务器版本"),
    }
}

/// Sample conflicts for mock mode, so the page can be exercised without a server.
pub fn sample_conflicts(now_ms: i64) -> Vec<Conflict> {
    vec![
        Conflict {
            id: "conflict-engine".into(),
            path: "src/engine.rs".into(),
            kind: ConflictKind::BothModified,
            kind_label: ConflictKind::BothModified.label().into(),
            detected_at_ms: now_ms - 2 * 60 * 1000,
            local: ConflictSide {
                content: "pub struct WriteBatcher {\n    pending: Vec<Mutation>,\n    max_batch: usize,\n    flushed_at: Instant,\n}\n".into(),
                modified_ms: now_ms - 2 * 60 * 1000,
                deleted: false,
            },
            remote: ConflictSide {
                content: "pub struct WriteBatcher {\n    queue: VecDeque<Mutation>,\n    flushed_at: Instant,\n}\n".into(),
                modified_ms: now_ms - 3 * 60 * 1000,
                deleted: false,
            },
            can_resolve: true,
            pending_resolution: None,
        },
        Conflict {
            id: "conflict-cargo".into(),
            path: "Cargo.toml".into(),
            kind: ConflictKind::RemoteDeleted,
            kind_label: ConflictKind::RemoteDeleted.label().into(),
            detected_at_ms: now_ms - 14 * 60 * 1000,
            local: ConflictSide {
                content: "[package]\nname = \"sync-engine\"\nversion = \"0.4.2\"\n".into(),
                modified_ms: now_ms - 14 * 60 * 1000,
                deleted: false,
            },
            remote: ConflictSide {
                content: String::new(),
                modified_ms: now_ms - 15 * 60 * 1000,
                deleted: true,
            },
            can_resolve: true,
            pending_resolution: None,
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
        store
            .replace(sample_conflicts(1_700_000_000_000))
            .expect("seed");

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
        assert_eq!(receipt.label, "保留本地");
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read"),
            "local version\n"
        );
    }

    #[test]
    fn keeping_remote_overwrites_locally_and_can_be_undone() {
        let f = fixture();
        f.store
            .resolve(&f.root, "conflict-engine", Resolution::KeepRemote)
            .expect("resolve");
        let after = std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read");
        assert!(after.contains("VecDeque"));
        assert!(f.store.list().iter().all(|c| c.id != "conflict-engine"));

        f.store.undo(&f.root, "conflict-engine").expect("undo");
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/engine.rs")).expect("read"),
            "local version\n"
        );
        assert_eq!(f.store.list().len(), 2);
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

        f.store.undo(&f.root, "conflict-engine").expect("undo");
        assert!(!f.root.join(&copy).exists());
    }

    #[test]
    fn keeping_remote_for_a_remote_deletion_removes_the_local_file() {
        let f = fixture();
        f.store
            .resolve(&f.root, "conflict-cargo", Resolution::KeepRemote)
            .expect("resolve");
        assert!(!f.root.join("Cargo.toml").exists());

        f.store.undo(&f.root, "conflict-cargo").expect("undo");
        assert_eq!(
            std::fs::read_to_string(f.root.join("Cargo.toml")).expect("read"),
            "local cargo\n"
        );
    }

    #[test]
    fn resolving_everything_empties_the_list() {
        let f = fixture();
        for id in ["conflict-engine", "conflict-cargo"] {
            f.store
                .resolve(&f.root, id, Resolution::KeepLocal)
                .expect("resolve");
        }
        assert!(f.store.list().is_empty());
    }

    #[test]
    fn a_conflict_cannot_be_resolved_twice() {
        let f = fixture();
        f.store
            .resolve(&f.root, "conflict-engine", Resolution::KeepLocal)
            .expect("resolve");
        assert!(
            f.store
                .resolve(&f.root, "conflict-engine", Resolution::KeepLocal)
                .is_err()
        );
    }

    #[test]
    fn undo_expires_once_forgotten() {
        let f = fixture();
        f.store
            .resolve(&f.root, "conflict-engine", Resolution::KeepRemote)
            .expect("resolve");
        f.store.forget_undo("conflict-engine");
        assert!(f.store.undo(&f.root, "conflict-engine").is_err());
    }

    #[test]
    fn resolving_works_before_the_folder_has_been_synced_down() {
        let root = tempfile::tempdir().expect("root");
        let records = tempfile::tempdir().expect("records");
        let store = ConflictStore::new(records.path(), "project-2").expect("store");
        store
            .replace(sample_conflicts(1_700_000_000_000))
            .expect("seed");

        // `src/` does not exist yet: keeping the remote side must still land.
        store
            .resolve(root.path(), "conflict-engine", Resolution::KeepRemote)
            .expect("resolve");
        assert!(
            std::fs::read_to_string(root.path().join("src/engine.rs"))
                .expect("read")
                .contains("VecDeque")
        );
    }

    #[test]
    fn copy_names_keep_the_original_extension() {
        assert_eq!(
            conflict_copy_path("src/engine.rs"),
            "src/engine.服务器版本.rs"
        );
        assert_eq!(conflict_copy_path("Makefile"), "Makefile.服务器版本");
        assert_eq!(conflict_copy_path("a/.env"), "a/.env.服务器版本");
    }

    #[test]
    fn resolution_labels_match_the_toast_copy() {
        assert_eq!(Resolution::KeepLocal.label(), "保留本地");
        assert_eq!(Resolution::KeepRemote.label(), "保留远端");
        assert_eq!(Resolution::KeepBoth.label(), "两者都保留");
        assert_eq!(ConflictKind::BothModified.label(), "本地与远端同时修改");
    }
}
