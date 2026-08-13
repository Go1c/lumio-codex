//! Bridge between the sync engine's conflict control surface and the
//! product-level conflict page (交互设计 5.5「冲突」).
//!
//! The engine speaks in revisions, hashes and four machine choices; the page
//! speaks in file contents and three human answers (保留本地 / 保留远端 /
//! 两者都保留). This module is the only place that translates between them.
//!
//! The agent is a **local** sidecar: its state directory, and with it the
//! content cache holding both sides of every conflict, sits on this Mac. That
//! is what makes 「两者都保留」 possible — the incoming bytes can be written next
//! to the local file without asking the server for them again.

use std::io::Read;
use std::path::{Path, PathBuf};

use fns_agent::ConflictView;
use fns_protocol::{WorkspaceConflictChoice, WorkspaceConflictKind};
use fns_sync_core::ConflictSideView;

use crate::conflicts::{Conflict, ConflictKind, ConflictSide, Resolution};
use crate::files::MAX_PREVIEW_BYTES;

/// Blobs are stored one file per hash, named by the hash without its algorithm
/// prefix (see `fns_fs::ContentCache`).
pub fn blob_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("blobs")
}

/// Read a conflict side's content out of the local content cache.
///
/// Anything the viewer cannot show — missing blob, oversized, binary — reads as
/// empty, exactly as the in-process engine used to behave. The conflict is
/// still listed; only its preview is blank.
fn blob_text(blob_dir: &Path, hash: &fns_protocol::WorkspaceContentHash) -> String {
    let path = blob_dir.join(hash.as_str().trim_start_matches("blake3:"));
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::new();
    if file
        .take(MAX_PREVIEW_BYTES + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return String::new();
    }
    if buf.len() as u64 > MAX_PREVIEW_BYTES || buf.contains(&0) {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

fn conflict_side(blob_dir: &Path, side: &ConflictSideView) -> ConflictSide {
    if side.tombstone {
        return ConflictSide {
            content: String::new(),
            modified_ms: side.modified_at_ms,
            deleted: true,
        };
    }
    ConflictSide {
        content: side
            .content_hash
            .as_ref()
            .map(|hash| blob_text(blob_dir, hash))
            .unwrap_or_default(),
        modified_ms: side.modified_at_ms,
        deleted: false,
    }
}

/// Which of the three human answers a conflict is really asking for.
pub fn conflict_kind(view: &ConflictView) -> ConflictKind {
    match view.kind {
        WorkspaceConflictKind::DeleteModify if view.current.tombstone => ConflictKind::LocalDeleted,
        WorkspaceConflictKind::DeleteModify => ConflictKind::RemoteDeleted,
        _ => ConflictKind::BothModified,
    }
}

/// Project one engine conflict onto the shape the conflict page renders.
pub fn conflict_from_view(state_dir: &Path, view: &ConflictView) -> Conflict {
    let blobs = blob_dir(state_dir);
    let kind = conflict_kind(view);
    Conflict {
        id: view.conflict_id.to_string(),
        path: view.path.as_str().to_string(),
        kind,
        kind_label: kind.label().to_string(),
        detected_at_ms: view
            .current
            .modified_at_ms
            .max(view.incoming.modified_at_ms),
        local: conflict_side(&blobs, &view.current),
        remote: conflict_side(&blobs, &view.incoming),
        can_resolve: view.can_resolve,
        pending_resolution: view
            .pending_resolution
            .as_ref()
            .map(|pending| choice_label(pending.choice).to_string()),
    }
}

/// zh-CN wording for a choice already queued on the server.
fn choice_label(choice: WorkspaceConflictChoice) -> &'static str {
    match choice {
        WorkspaceConflictChoice::Current => "保留本地",
        WorkspaceConflictChoice::Incoming => "保留远端",
        WorkspaceConflictChoice::Merged => "已合并",
        WorkspaceConflictChoice::Delete => "删除",
    }
}

/// Translate a user's answer into the choice the engine understands.
///
/// 「两者都保留」 keeps the local file as the winning revision — the server copy
/// has already been written beside it as a *new* local file, which the engine
/// then syncs outwards as an ordinary create.
pub fn engine_choice(resolution: Resolution, remote_deleted: bool) -> WorkspaceConflictChoice {
    match resolution {
        Resolution::KeepLocal | Resolution::KeepBoth => WorkspaceConflictChoice::Current,
        Resolution::KeepRemote if remote_deleted => WorkspaceConflictChoice::Delete,
        Resolution::KeepRemote => WorkspaceConflictChoice::Incoming,
    }
}

/// The engine conflict matching a product-level conflict id, if it is still open.
pub fn find_view<'a>(views: &'a [ConflictView], conflict_id: &str) -> Option<&'a ConflictView> {
    views
        .iter()
        .find(|view| view.conflict_id.to_string() == conflict_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_blob(dir: &Path, content: &[u8]) -> fns_protocol::WorkspaceContentHash {
        let hash = blake3::hash(content);
        let hash = fns_protocol::WorkspaceContentHash::parse(&format!("blake3:{}", hash.to_hex()))
            .expect("hash");
        let blobs = blob_dir(dir);
        std::fs::create_dir_all(&blobs).expect("mkdir");
        std::fs::write(
            blobs.join(hash.as_str().trim_start_matches("blake3:")),
            content,
        )
        .expect("write");
        hash
    }

    #[test]
    fn a_side_reads_its_bytes_out_of_the_local_content_cache() {
        let dir = tempfile::tempdir().expect("dir");
        let hash = write_blob(dir.path(), b"server version\n");
        let side = ConflictSideView {
            path: None,
            path_revision: fns_protocol::WorkspaceRevision::ZERO,
            content_hash: Some(hash),
            size: 15,
            modified_at_ms: 42,
            executable: false,
            tombstone: false,
        };

        let mapped = conflict_side(&blob_dir(dir.path()), &side);
        assert_eq!(mapped.content, "server version\n");
        assert_eq!(mapped.modified_ms, 42);
        assert!(!mapped.deleted);
    }

    #[test]
    fn a_tombstone_side_is_a_deletion_rather_than_empty_content() {
        let dir = tempfile::tempdir().expect("dir");
        let side = ConflictSideView {
            path: None,
            path_revision: fns_protocol::WorkspaceRevision::ZERO,
            content_hash: None,
            size: 0,
            modified_at_ms: 7,
            executable: false,
            tombstone: true,
        };

        let mapped = conflict_side(&blob_dir(dir.path()), &side);
        assert!(mapped.deleted);
        assert_eq!(mapped.modified_ms, 7);
    }

    #[test]
    fn binary_and_oversized_blobs_preview_as_empty_without_failing() {
        let dir = tempfile::tempdir().expect("dir");
        let binary = write_blob(dir.path(), &[0x00, 0xff, 0x00]);
        let oversized = write_blob(dir.path(), &vec![b'a'; (MAX_PREVIEW_BYTES + 1) as usize]);
        let blobs = blob_dir(dir.path());

        assert!(blob_text(&blobs, &binary).is_empty());
        assert!(blob_text(&blobs, &oversized).is_empty());
    }

    #[test]
    fn a_missing_blob_does_not_lose_the_conflict() {
        let dir = tempfile::tempdir().expect("dir");
        let absent =
            fns_protocol::WorkspaceContentHash::parse(&format!("blake3:{}", "0".repeat(64)))
                .expect("hash");
        assert!(blob_text(&blob_dir(dir.path()), &absent).is_empty());
    }

    #[test]
    fn keep_remote_becomes_a_delete_when_the_server_removed_the_file() {
        assert_eq!(
            engine_choice(Resolution::KeepRemote, true),
            WorkspaceConflictChoice::Delete
        );
        assert_eq!(
            engine_choice(Resolution::KeepRemote, false),
            WorkspaceConflictChoice::Incoming
        );
    }

    #[test]
    fn keeping_both_keeps_the_local_revision_as_the_winner() {
        // The server copy lands as a separate new file; the conflict itself is
        // answered with "mine", otherwise the engine would overwrite the local
        // file we just told the user we kept.
        assert_eq!(
            engine_choice(Resolution::KeepBoth, false),
            WorkspaceConflictChoice::Current
        );
        assert_eq!(
            engine_choice(Resolution::KeepLocal, false),
            WorkspaceConflictChoice::Current
        );
    }
}
