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
fn typed_transaction_rolls_back_cursor_path_outbox_stream_apply_conflict_and_receipt() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000038").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        })
        .unwrap();
    let conflict_message = fixture.conflict_created("tx-conflict.txt");
    let conflict = fns_sync_core::ConflictRecord {
        conflict_id: conflict_message.conflict_id,
        workspace_id: conflict_message.workspace_id,
        conflict_revision: conflict_message.conflict_revision,
        created_json: serde_json::to_vec(&conflict_message).unwrap(),
        status: fns_sync_core::ConflictStatus::Manual,
        candidate_hash: None,
        resolution_json: None,
        resolution_digest: None,
    };
    let apply_id = fns_sync_core::ApplyId(
        uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000039").unwrap(),
    );
    let journal = fns_sync_core::ApplyJournalRecord {
        apply_id,
        workspace_id: fixture.workspace_id(),
        stream_id,
        item_kind: fns_sync_core::ApplyItemKind::Entry,
        item_key: "tx-entry.txt".to_owned(),
        operation_json: b"op".to_vec(),
        preimage_json: b"pre".to_vec(),
        postimage_json: b"post".to_vec(),
        stage: fns_sync_core::ApplyStage::Prepared,
    };
    let active_state = state.stream_state().unwrap().unwrap();
    let before = state.row_counts().unwrap();
    let result: Result<(), SyncError> = state.transaction(|tx| {
        tx.set_pending_ack(WorkspaceRevision::new(1))?;
        tx.put_path_state(&fixture.path_state("tx-path.txt", 1))?;
        tx.put_local_intent("tx-path.txt", b"intent", 1)?;
        tx.enqueue_mutation(&fixture.mutation("tx-outbox.txt"))?;
        tx.put_stream_state(&active_state)?;
        tx.put_apply_journal(&journal)?;
        tx.put_conflict(&conflict)?;
        tx.record_applied_operation(
            fixture.client_id(),
            fixture.mutation("tx-receipt.txt").operation_id,
            WorkspaceRevision::new(1),
            [7; 32],
        )?;
        Err(SyncError::InvalidConfiguration { reason: "injected" })
    });
    assert!(result.is_err());
    assert_eq!(state.row_counts().unwrap(), before);
    assert!(state.apply_journal(apply_id).unwrap().is_none());
    assert!(state.conflict(conflict.conflict_id).unwrap().is_none());
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
fn duplicate_begin_preserves_progress_and_end_state() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let begin = WorkspaceSnapshotBeginMessage {
        workspace_id: fixture.workspace_id(),
        stream_id: fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000022").unwrap(),
        mode: WorkspaceSnapshotMode::Incremental,
        from_revision: WorkspaceRevision::ZERO,
        final_revision: WorkspaceRevision::new(2),
        entry_count: 0,
        event_count: 2,
        conflict_count: 0,
    };
    state.begin_stream(&begin).unwrap();
    state.advance_stream_index(1).unwrap();
    state.set_stream_end_received(true).unwrap();

    let resumed = state.begin_stream(&begin).unwrap();
    assert_eq!(resumed.next_event_index, 1);
    assert!(resumed.end_received);
}

#[test]
fn raw_revision_item_rejects_non_dto_bytes() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000023").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 1,
            conflict_count: 0,
        })
        .unwrap();

    let body = b"not-a-workspace-event".to_vec();
    let error = state
        .put_stream_revision_item(
            stream_id,
            WorkspaceRevision::new(1),
            fns_sync_core::StreamRevisionItemKind::Event,
            Some(0),
            body.clone(),
            fns_sync_core::body_digest(&body),
            fns_sync_core::StreamItemStatus::Received,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        SyncError::ProtocolInvariant { .. } | SyncError::StreamInvariant { .. }
    ));
}

#[test]
fn revision_item_replay_rejects_event_index_change() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000024").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 1,
            conflict_count: 0,
        })
        .unwrap();
    let event = fixture.event(stream_id, 0, 1, "event.txt");
    state
        .put_stream_event(&event, fns_sync_core::StreamItemStatus::Received)
        .unwrap();
    let body = serde_json::to_vec(&event).unwrap();

    let error = state
        .put_stream_revision_item(
            stream_id,
            event.revision,
            fns_sync_core::StreamRevisionItemKind::Event,
            Some(1),
            body.clone(),
            fns_sync_core::body_digest(&body),
            fns_sync_core::StreamItemStatus::Ready,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        SyncError::OperationChanged | SyncError::ProtocolInvariant { .. }
    ));
}

#[test]
fn stream_entry_requires_active_stream_and_ordered_snapshot_paths() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000025").unwrap();
    let entry = fixture.snapshot_entry(stream_id, 0, "b.txt", 1);
    assert!(matches!(
        state.put_stream_entry(&entry, fns_sync_core::StreamItemStatus::Received),
        Err(SyncError::StreamInvariant { .. }) | Err(SyncError::ProtocolInvariant { .. })
    ));

    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 2,
            event_count: 0,
            conflict_count: 0,
        })
        .unwrap();
    let first = fixture.snapshot_entry(stream_id, 0, "b.txt", 1);
    state
        .put_stream_entry(&first, fns_sync_core::StreamItemStatus::Received)
        .unwrap();
    let out_of_order = fixture.snapshot_entry(stream_id, 1, "a.txt", 1);
    assert!(matches!(
        state.put_stream_entry(&out_of_order, fns_sync_core::StreamItemStatus::Received),
        Err(SyncError::StreamInvariant { .. })
    ));
}

#[test]
fn stream_events_enforce_mode_count_index_and_revision_order() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let snapshot_stream =
        fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000026").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id: snapshot_stream,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        })
        .unwrap();
    let snapshot_event = fixture.event(snapshot_stream, 0, 1, "snapshot-event.txt");
    assert!(matches!(
        state.put_stream_event(&snapshot_event, fns_sync_core::StreamItemStatus::Received),
        Err(SyncError::StreamInvariant { .. })
    ));

    state.clear_stream().unwrap();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000027").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(2),
            entry_count: 0,
            event_count: 2,
            conflict_count: 0,
        })
        .unwrap();
    let gap = fixture.event(stream_id, 1, 1, "gap.txt");
    assert!(matches!(
        state.put_stream_event(&gap, fns_sync_core::StreamItemStatus::Received),
        Err(SyncError::StreamInvariant { .. })
    ));
    let first = fixture.event(stream_id, 0, 2, "first.txt");
    state
        .put_stream_event(&first, fns_sync_core::StreamItemStatus::Received)
        .unwrap();
    let regressed_revision = fixture.event(stream_id, 1, 1, "second.txt");
    assert!(matches!(
        state.put_stream_event(
            &regressed_revision,
            fns_sync_core::StreamItemStatus::Received
        ),
        Err(SyncError::StreamInvariant { .. })
    ));
}

#[test]
fn stream_conflicts_require_active_stream_and_expected_count() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000028").unwrap();
    let conflict = fixture.conflict_created("conflict.txt");
    assert!(matches!(
        state.put_stream_conflict(
            &conflict,
            fns_sync_core::StreamConflictStatus::Received,
            stream_id,
        ),
        Err(SyncError::StreamInvariant { .. })
    ));

    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 0,
            conflict_count: 1,
        })
        .unwrap();
    let other_stream =
        fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000029").unwrap();
    assert!(matches!(
        state.put_stream_conflict(
            &conflict,
            fns_sync_core::StreamConflictStatus::Received,
            other_stream,
        ),
        Err(SyncError::StreamInvariant { .. })
    ));
    state
        .put_stream_conflict(
            &conflict,
            fns_sync_core::StreamConflictStatus::Received,
            stream_id,
        )
        .unwrap();
    let second = fixture
        .conflict_created_with_id("conflict-two.txt", "10000000-0000-4000-8000-000000000031");
    assert!(matches!(
        state.put_stream_conflict(
            &second,
            fns_sync_core::StreamConflictStatus::Received,
            stream_id,
        ),
        Err(SyncError::StreamInvariant { .. })
    ));
}

#[test]
fn conflict_resolved_revision_item_never_has_stream_index() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000032").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 0,
            event_count: 1,
            conflict_count: 0,
        })
        .unwrap();
    let message = fixture.conflict_resolved(1, "resolved.txt");
    let record = state
        .put_stream_conflict_resolved(
            &message,
            Some(99),
            fns_sync_core::StreamItemStatus::Received,
        )
        .unwrap();
    assert_eq!(record.event_index, None);
    assert_eq!(
        state.stream_revision_items(stream_id).unwrap()[0].event_index,
        None
    );
}

#[test]
fn stream_state_upsert_and_progress_setters_reject_conflicting_regressions() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000033").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(2),
            entry_count: 0,
            event_count: 2,
            conflict_count: 0,
        })
        .unwrap();
    let mut progressed = state.stream_state().unwrap().unwrap();
    progressed.next_event_index = 1;
    state.put_stream_state(&progressed).unwrap();
    let mut regressed = progressed.clone();
    regressed.next_event_index = 0;
    assert!(matches!(
        state.put_stream_state(&regressed),
        Err(SyncError::StreamInvariant { .. })
    ));
    let mut conflicting = progressed.clone();
    conflicting.stream_id =
        fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000042").unwrap();
    assert!(matches!(
        state.put_stream_state(&conflicting),
        Err(SyncError::StreamInvariant { .. })
    ));
    state.set_stream_end_received(true).unwrap();
    assert!(matches!(
        state.set_stream_end_received(false),
        Err(SyncError::StreamInvariant { .. })
    ));
    assert!(matches!(
        state.advance_stream_index(0),
        Err(SyncError::StreamInvariant { .. })
    ));
}

#[test]
fn clear_stream_removes_all_stream_children() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000034").unwrap();
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: fixture.workspace_id(),
            stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 1,
            event_count: 0,
            conflict_count: 0,
        })
        .unwrap();
    let entry = fixture.snapshot_entry(stream_id, 0, "clear.txt", 1);
    state
        .put_stream_entry(&entry, fns_sync_core::StreamItemStatus::Received)
        .unwrap();
    assert_eq!(state.stream_entries(stream_id).unwrap().len(), 1);
    state.clear_stream().unwrap();
    assert!(state.stream_state().unwrap().is_none());
    assert!(state.stream_entries(stream_id).unwrap().is_empty());
}

#[test]
fn cursor_and_outbox_stages_do_not_regress() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    state
        .set_last_ack_revision(WorkspaceRevision::new(5))
        .unwrap();
    assert!(matches!(
        state.set_last_ack_revision(WorkspaceRevision::new(4)),
        Err(SyncError::StreamInvariant { .. })
    ));
    state
        .set_last_applied_revision(WorkspaceRevision::new(5))
        .unwrap();
    assert!(matches!(
        state.set_last_applied_revision(WorkspaceRevision::new(4)),
        Err(SyncError::StreamInvariant { .. })
    ));
    state.set_pending_ack(WorkspaceRevision::new(5)).unwrap();
    assert!(matches!(
        state.set_pending_ack(WorkspaceRevision::new(4)),
        Err(SyncError::StreamInvariant { .. })
    ));

    let mutation = fixture.mutation("stage.txt");
    state.enqueue_mutation(&mutation).unwrap();
    state.mark_dispatched(mutation.operation_id).unwrap();
    assert!(matches!(
        state.set_outbox_stage(mutation.operation_id, fns_sync_core::OutboxStage::Queued),
        Err(SyncError::ProtocolInvariant { .. })
    ));
}

#[test]
fn apply_journal_rejects_reuse_with_different_identity() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000035").unwrap();
    let other_stream =
        fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000036").unwrap();
    let record = fns_sync_core::ApplyJournalRecord {
        apply_id: fns_sync_core::ApplyId(
            uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000037").unwrap(),
        ),
        workspace_id: fixture.workspace_id(),
        stream_id,
        item_kind: fns_sync_core::ApplyItemKind::Entry,
        item_key: "entry.txt".to_owned(),
        operation_json: b"operation".to_vec(),
        preimage_json: b"preimage".to_vec(),
        postimage_json: b"postimage".to_vec(),
        stage: fns_sync_core::ApplyStage::Prepared,
    };
    state.put_apply_journal(&record).unwrap();
    state
        .set_apply_stage(
            record.apply_id,
            fns_sync_core::ApplyStage::FilesystemStarted,
        )
        .unwrap();
    assert!(matches!(
        state.set_apply_stage(record.apply_id, fns_sync_core::ApplyStage::Prepared),
        Err(SyncError::ProtocolInvariant { .. })
    ));
    let mut changed_stream = record.clone();
    changed_stream.stream_id = other_stream;
    assert_eq!(
        state.put_apply_journal(&changed_stream).unwrap_err(),
        SyncError::OperationChanged
    );
    let mut changed_kind = record.clone();
    changed_kind.item_kind = fns_sync_core::ApplyItemKind::Event;
    assert_eq!(
        state.put_apply_journal(&changed_kind).unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn malformed_apply_id_is_rejected_when_read_from_disk() {
    let fixture = support::StateFixture::new();
    let mut state = fixture.open();
    let record = fns_sync_core::ApplyJournalRecord {
        apply_id: fns_sync_core::ApplyId(
            uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000040").unwrap(),
        ),
        workspace_id: fixture.workspace_id(),
        stream_id: fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000041").unwrap(),
        item_kind: fns_sync_core::ApplyItemKind::Entry,
        item_key: "malformed.txt".to_owned(),
        operation_json: b"op".to_vec(),
        preimage_json: b"pre".to_vec(),
        postimage_json: b"post".to_vec(),
        stage: fns_sync_core::ApplyStage::Prepared,
    };
    state.put_apply_journal(&record).unwrap();
    drop(state);
    let connection = rusqlite::Connection::open(fixture.db_path()).unwrap();
    connection
        .execute("UPDATE apply_journal SET apply_id = ?1", [&"not-a-uuid"])
        .unwrap();
    drop(connection);
    let state = fixture.open();
    assert!(matches!(
        state.apply_journals(),
        Err(SyncError::StorageUnavailable)
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
    drop(fixture.open());
    let connection = rusqlite::Connection::open(fixture.db_path()).unwrap();
    connection
        .execute(
            "UPDATE workspace_cursor SET last_ack_revision = ?1",
            [&"not-a-revision"],
        )
        .unwrap();
    drop(connection);
    let state = fixture.open();
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
