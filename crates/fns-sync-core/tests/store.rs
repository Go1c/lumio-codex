mod support;

use fns_fs::{FileFingerprint, HashCache, NativeFileId};
use fns_protocol::{WorkspaceRevision, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotMode};
use fns_sync_core::SyncError;

#[test]
fn opens_exact_schema_and_round_trips_max_revision() {
    let fixture = support::StateFixture::new();
    let mut state = fns_sync_core::SqliteState::open(
        fixture.db_path(),
        fixture.workspace_id(),
        fixture.client_id(),
    )
    .unwrap();
    state
        .set_pending_ack(WorkspaceRevision::new(u64::MAX))
        .unwrap();
    let cursor = state.cursor().unwrap();
    assert_eq!(cursor.pending_ack_revision.unwrap().get(), u64::MAX);
    assert_eq!(state.user_version().unwrap(), 1);
    assert_eq!(state.pragma("journal_mode").unwrap(), "wal");
    assert_eq!(state.pragma("synchronous").unwrap(), "2");
    assert_eq!(state.pragma("foreign_keys").unwrap(), "1");
}

#[test]
fn reopen_preserves_cursor_and_rows() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    state.set_pending_ack(WorkspaceRevision::new(7)).unwrap();
    state
        .put_path_state(&fixture.path_state("notes.txt", 7))
        .unwrap();
    state
        .enqueue_mutation(&fixture.mutation("notes.txt"))
        .unwrap();
    drop(state);

    let state = fixture.open();
    assert_eq!(
        state.cursor().unwrap().pending_ack_revision.unwrap().get(),
        7
    );
    assert_eq!(
        state
            .path_state("notes.txt")
            .unwrap()
            .unwrap()
            .path
            .as_str(),
        "notes.txt"
    );
    assert_eq!(state.outbox().unwrap().len(), 1);
}

#[test]
fn identity_mismatch_is_rejected() {
    let fixture = support::StateFixture::new();
    let _state = fixture.open();
    let other_workspace =
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000011").unwrap();
    let error =
        fns_sync_core::SqliteState::open(fixture.db_path(), other_workspace, fixture.client_id())
            .unwrap_err();
    assert!(matches!(error, SyncError::InvalidConfiguration { .. }));
}

#[test]
fn failed_write_transaction_rolls_back_every_table() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let before = state.row_counts().unwrap();
    let result: Result<(), SyncError> = state.transaction(|tx| {
        tx.put_path_state(&fixture.path_state("rollback.txt", 1))?;
        tx.put_local_intent("rollback.txt", b"intent", 1)?;
        Err(SyncError::InvalidConfiguration { reason: "injected" })
    });
    assert!(result.is_err());
    assert_eq!(state.row_counts().unwrap(), before);
}

#[test]
fn dispatched_body_is_immutable() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let original = fixture.mutation("first.txt");
    state.enqueue_mutation(&original).unwrap();
    state.mark_dispatched(original.operation_id).unwrap();
    let mut changed = original.clone();
    changed.path = fns_protocol::WorkspacePath::parse("changed.txt").unwrap();
    let error = state.enqueue_mutation(&changed).unwrap_err();
    assert_eq!(error, SyncError::OperationChanged);
    let stored = state.outbox().unwrap().pop().unwrap();
    assert_eq!(stored.body_json, serde_json::to_vec(&original).unwrap());
}

#[test]
fn only_one_stream_can_be_active() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let first = WorkspaceSnapshotBeginMessage {
        workspace_id: fixture.workspace_id(),
        stream_id: fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000020").unwrap(),
        mode: WorkspaceSnapshotMode::Incremental,
        from_revision: WorkspaceRevision::ZERO,
        final_revision: WorkspaceRevision::new(1),
        entry_count: 0,
        event_count: 1,
        conflict_count: 0,
    };
    state.begin_stream(&first).unwrap();
    let mut second = first.clone();
    second.stream_id =
        fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000021").unwrap();
    assert!(matches!(
        state.begin_stream(&second),
        Err(SyncError::StreamInvariant { .. })
    ));
}

#[test]
fn applied_operation_receipt_is_permanent() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let mutation = fixture.mutation("receipt.txt");
    let digest = state::digest(&serde_json::to_vec(&mutation).unwrap());
    state
        .record_applied_operation(
            mutation.client_id,
            mutation.operation_id,
            WorkspaceRevision::new(4),
            digest,
        )
        .unwrap();
    state
        .record_applied_operation(
            mutation.client_id,
            mutation.operation_id,
            WorkspaceRevision::new(4),
            digest,
        )
        .unwrap();
    assert_eq!(state.applied_operations().unwrap().len(), 1);
}

#[test]
fn corrupt_revision_returns_safe_corrupt_state() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    state
        .transaction(|tx| {
            tx.execute(
                "UPDATE workspace_cursor SET last_ack_revision = ?1",
                [&"not-a-revision"],
            )?;
            Ok(())
        })
        .unwrap();
    let error = state.cursor().unwrap_err();
    let rendered = error.to_string();
    assert!(matches!(error, SyncError::CorruptState { .. }));
    assert!(!rendered.contains(fixture.db_path().to_string_lossy().as_ref()));
    assert!(!rendered.contains("UPDATE workspace_cursor"));
}

#[test]
fn hash_cache_survives_reopen_and_fingerprint_mismatch_is_a_miss() {
    let fixture = support::StateFixture::new();
    let path = fns_protocol::WorkspacePath::parse("cached.txt").unwrap();
    let hash = fns_protocol::WorkspaceContentHash::parse(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let fingerprint = test_fingerprint(3);
    let mut state = fixture.open();
    state.store(&path, &fingerprint, &hash).unwrap();
    drop(state);

    let mut reopened = fixture.open();
    assert_eq!(reopened.lookup(&path, &fingerprint).unwrap(), Some(hash));
    assert_eq!(reopened.lookup(&path, &test_fingerprint(4)).unwrap(), None);
}

#[cfg(unix)]
fn test_fingerprint(size: u64) -> FileFingerprint {
    FileFingerprint {
        file_id: NativeFileId::Unix {
            device: 1,
            inode: 2,
        },
        size,
        modified_at_ns: 3,
        changed_at_ns: 4,
    }
}

#[cfg(windows)]
fn test_fingerprint(size: u64) -> FileFingerprint {
    FileFingerprint {
        file_id: NativeFileId::Windows {
            volume_serial: 1,
            file_index: 2,
        },
        size,
        modified_at_ns: 3,
        changed_at_ns: 4,
    }
}

mod state {
    pub fn digest(bytes: &[u8]) -> [u8; 32] {
        *blake3::hash(bytes).as_bytes()
    }
}
