mod support;

use std::fs;

use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceEventMessage, WorkspaceMutation,
    WorkspaceMutationAcceptedMessage, WorkspaceMutationKind, WorkspaceRevision,
};
use fns_sync_core::{
    AppliedOperationReceiptKind, ConflictStatus, OutboxStage, StreamItemStatus, SyncCommand,
    SyncError,
};

#[derive(Clone, Copy, Debug)]
enum ReconnectBoundary {
    SnapshotBegin,
    SnapshotEntry,
    Event,
    EndBeforeAck,
}

fn blocked_rename_conflict(
    fixture: &support::EngineFixture,
    conflict_id: &str,
    mutation: &WorkspaceMutation,
) -> WorkspaceConflictCreatedMessage {
    let mut created = fixture.remote_conflict_created(conflict_id, "1", mutation.path.as_str());
    created.kind = WorkspaceConflictKind::Rename;
    created.created_by_operation_id = mutation.operation_id;
    created.ancestor.path_revision = mutation.base_path_revision;
    created.incoming.path = RequiredNullable::Value(
        mutation
            .new_path
            .clone()
            .expect("blocked rename target path"),
    );
    created.incoming.path_revision = mutation.target_base_path_revision.unwrap();
    created.incoming.content_hash = mutation.content_hash.clone();
    created.incoming.metadata = mutation.metadata.clone();
    if mutation.content_hash.is_null() {
        created.ancestor.content_hash = RequiredNullable::Null;
        created.ancestor.metadata = support::zero_metadata();
        created.current.content_hash = RequiredNullable::Null;
        created.current.metadata = support::zero_metadata();
    }
    created.validate().unwrap();
    created
}

fn fixture_with_blocked_file_rename(
    source: &str,
    target: &str,
    conflict_id: &str,
    bytes: &[u8],
) -> (
    support::EngineFixture,
    WorkspaceMutation,
    WorkspaceConflictCreatedMessage,
) {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file(source, 8, bytes);
    fixture.rename(source, target);
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path(source),
            to: support::workspace_path(target),
        })
        .unwrap();
    let blocked = fixture
        .engine
        .outbox()
        .unwrap()
        .pop()
        .expect("blocked file rename")
        .mutation()
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();
    let old = blocked_rename_conflict(&fixture, conflict_id, &blocked);
    fixture.engine.conflict_created(old.clone()).unwrap();
    (fixture, blocked, old)
}

#[test]
fn reconnect_replaces_partial_stream_without_losing_local_intents() {
    for (case, boundary) in [
        ReconnectBoundary::SnapshotBegin,
        ReconnectBoundary::SnapshotEntry,
        ReconnectBoundary::Event,
        ReconnectBoundary::EndBeforeAck,
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = support::EngineFixture::new();
        fixture.seed_remote_file("remote.txt", 0, b"base");
        fixture.write("local.txt", b"local-v1");
        fixture
            .engine
            .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
                "local.txt",
            )))
            .unwrap();
        assert!(
            fixture
                .engine
                .pending_commands(16)
                .unwrap()
                .iter()
                .any(|command| matches!(command, SyncCommand::Mutation(_)))
        );
        fixture.write("local.txt", b"local-v2");
        fixture
            .engine
            .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
                "local.txt",
            )))
            .unwrap();
        let local_intent = fixture
            .engine
            .state()
            .local_intent("local.txt")
            .unwrap()
            .expect("local intent");
        let local_outbox = fixture
            .engine
            .outbox()
            .unwrap()
            .pop()
            .expect("local outbox");
        let durable_conflict = fixture.remote_conflict_created(
            "10000000-0000-4000-8000-000000000039",
            "1",
            "conflict.txt",
        );
        fixture
            .engine
            .state_mut()
            .record_conflict(&durable_conflict, ConflictStatus::Manual)
            .unwrap();

        let old_stream_id = fixture.stream_id();
        let old_begin = match boundary {
            ReconnectBoundary::SnapshotEntry => fixture.snapshot_begin(1, 1, 0),
            ReconnectBoundary::SnapshotBegin | ReconnectBoundary::Event => {
                fixture.incremental_begin(0, 1, 1, 0)
            }
            ReconnectBoundary::EndBeforeAck => fixture.incremental_begin(0, 1, 1, 1),
        };
        fixture.engine.snapshot_begin(old_begin.clone()).unwrap();
        match boundary {
            ReconnectBoundary::SnapshotBegin => {}
            ReconnectBoundary::SnapshotEntry => {
                let commands = fixture
                    .engine
                    .snapshot_entry(fixture.snapshot_file_entry(0, 1, "remote.txt", b"server"))
                    .unwrap();
                assert!(commands.iter().any(support::is_download));
            }
            ReconnectBoundary::Event => {
                fixture
                    .engine
                    .stage_bytes(&support::hash(b"server"), b"server")
                    .unwrap();
                let commands = fixture
                    .engine
                    .workspace_event(fixture.remote_update_event(0, 1, "remote.txt", b"server"))
                    .unwrap();
                assert!(commands.is_empty());
                assert_eq!(fs::read(fixture.path("remote.txt")).unwrap(), b"server");
                assert!(
                    fixture
                        .engine
                        .state()
                        .applied_operation(support::remote_client_id(), support::operation_id(200))
                        .unwrap()
                        .is_some()
                );
            }
            ReconnectBoundary::EndBeforeAck => {
                fixture
                    .engine
                    .stage_bytes(&support::hash(b"server"), b"server")
                    .unwrap();
                fixture
                    .engine
                    .workspace_event(fixture.remote_update_event(0, 1, "remote.txt", b"server"))
                    .unwrap();
                fixture
                    .engine
                    .conflict_created(durable_conflict.clone())
                    .unwrap();
                fixture
                    .engine
                    .snapshot_end(fixture.incremental_end(1, 1, 1))
                    .unwrap();
                assert_eq!(
                    fixture.engine.cursor().unwrap().pending_ack_revision,
                    Some(WorkspaceRevision::new(1))
                );
                assert_eq!(fs::read(fixture.path("remote.txt")).unwrap(), b"server");
            }
        }

        let old_staging = fixture.engine.state().row_counts().unwrap();
        fixture.engine.snapshot_begin(old_begin.clone()).unwrap();
        assert_eq!(
            fixture.engine.state().row_counts().unwrap(),
            old_staging,
            "same Begin changed state at {boundary:?}"
        );
        let mut changed_begin = old_begin;
        changed_begin.final_revision = WorkspaceRevision::new(2);
        assert!(matches!(
            fixture.engine.snapshot_begin(changed_begin),
            Err(SyncError::StreamInvariant {
                reason: "stream_begin_changed"
            })
        ));

        let stale_live = fixture.remote_update_event(1, 2, "stale-live.txt", b"stale");
        let stale_hash = support::hash(b"stale");
        assert!(
            fixture
                .engine
                .event(stale_live)
                .unwrap()
                .iter()
                .any(|command| download_matches(command, &stale_hash))
        );

        let mut invalid_replacement = fixture.incremental_begin(1, 2, 1, 1);
        invalid_replacement.stream_id = reconnect_stream_id(70 + case as u32);
        let invalid_error = fixture
            .engine
            .snapshot_begin(invalid_replacement)
            .unwrap_err();
        assert!(
            matches!(
                invalid_error,
                SyncError::StreamInvariant {
                    reason: "stream_begin_not_at_ack"
                }
            ),
            "unexpected invalid replacement error: {invalid_error:?}"
        );
        assert_eq!(
            fixture
                .engine
                .state()
                .stream_state()
                .unwrap()
                .unwrap()
                .stream_id,
            old_stream_id
        );
        assert!(
            fixture
                .engine
                .pending_commands(16)
                .unwrap()
                .iter()
                .any(|command| download_matches(command, &stale_hash)),
            "rejected Begin cleared pending live state at {boundary:?}"
        );

        let new_stream_id = reconnect_stream_id(60 + case as u32);
        let mut replacement = fixture.incremental_begin(0, 1, 1, 1);
        replacement.stream_id = new_stream_id;
        fixture.engine.snapshot_begin(replacement.clone()).unwrap();
        let after_replacement = fixture.engine.state().row_counts().unwrap();
        fixture.engine.snapshot_begin(replacement).unwrap();
        assert_eq!(
            fixture.engine.state().row_counts().unwrap(),
            after_replacement,
            "duplicate replacement Begin changed state at {boundary:?}"
        );
        assert!(
            fixture
                .engine
                .state()
                .stream_entries(old_stream_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            fixture
                .engine
                .state()
                .stream_revision_items(old_stream_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            fixture
                .engine
                .state()
                .stream_conflicts(old_stream_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            !fixture
                .engine
                .pending_commands(16)
                .unwrap()
                .iter()
                .any(|command| download_matches(command, &stale_hash)),
            "replacement retained stale live state at {boundary:?}"
        );
        assert_eq!(
            fixture.engine.cursor().unwrap().last_ack_revision,
            WorkspaceRevision::ZERO
        );
        if matches!(boundary, ReconnectBoundary::EndBeforeAck) {
            assert_eq!(
                fixture.engine.cursor().unwrap().pending_ack_revision,
                Some(WorkspaceRevision::new(1))
            );
        }

        let mut replacement_event = fixture.remote_update_event(0, 1, "remote.txt", b"server");
        replacement_event.stream_id = new_stream_id;
        let replacement_commands = fixture.engine.workspace_event(replacement_event).unwrap();
        if matches!(
            boundary,
            ReconnectBoundary::Event | ReconnectBoundary::EndBeforeAck
        ) {
            assert!(replacement_commands.is_empty());
        }
        fixture
            .engine
            .conflict_created(durable_conflict.clone())
            .unwrap();
        fixture.provide_requested_blobs();
        let mut replacement_end = fixture.incremental_end(1, 1, 1);
        replacement_end.stream_id = new_stream_id;
        fixture.engine.snapshot_end(replacement_end).unwrap();

        assert_eq!(fs::read(fixture.path("remote.txt")).unwrap(), b"server");
        assert_eq!(fs::read(fixture.path("local.txt")).unwrap(), b"local-v2");
        assert!(
            fixture
                .engine
                .state()
                .applied_operation(support::remote_client_id(), support::operation_id(200))
                .unwrap()
                .is_some()
        );
        let commands = fixture.engine.pending_commands(16).unwrap();
        assert_eq!(support::ack_revisions(&commands), vec![1]);
        assert_eq!(
            fixture.engine.cursor().unwrap().pending_ack_revision,
            Some(WorkspaceRevision::new(1))
        );
        fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
        assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 1);
        assert!(fixture.engine.state().stream_state().unwrap().is_none());

        let retained_intent = fixture
            .engine
            .state()
            .local_intent("local.txt")
            .unwrap()
            .expect("retained local intent");
        assert_eq!(retained_intent.intent_json, local_intent.intent_json);
        let retained_outbox = fixture
            .engine
            .outbox()
            .unwrap()
            .into_iter()
            .find(|record| record.operation_id == local_outbox.operation_id)
            .expect("retained local outbox");
        assert_eq!(retained_outbox.body_digest, local_outbox.body_digest);
        assert!(
            fixture
                .engine
                .state()
                .conflict(durable_conflict.conflict_id)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn reconnect_fails_closed_while_apply_journal_is_unfinished() {
    let mut fixture = support::EngineFixture::new();
    let old_begin = fixture.incremental_begin(0, 1, 1, 0);
    fixture.engine.snapshot_begin(old_begin).unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "remote.txt", b"server"))
        .unwrap();
    let state = support::path_state(
        "journal.txt",
        1,
        fns_protocol::RequiredNullable::Null,
        support::file_metadata(0),
        fns_protocol::WorkspaceEntryKind::Directory,
    );
    let operation = fns_sync_core::model::RemoteApplyOperation::Upsert {
        state: state.clone(),
    };
    let journal = fns_sync_core::ApplyJournalRecord {
        apply_id: fns_sync_core::ApplyId(
            uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000042").unwrap(),
        ),
        workspace_id: fixture.engine.state().workspace_id(),
        stream_id: fixture.stream_id(),
        item_kind: fns_sync_core::ApplyItemKind::Entry,
        item_key: "journal.txt".to_owned(),
        apply_namespace: fns_sync_core::ApplyNamespace::SnapshotEntry,
        operation_body_digest: fns_sync_core::body_digest(
            &fns_sync_core::canonical_json(&operation).unwrap(),
        ),
        operation_json: fns_sync_core::canonical_json(&operation).unwrap(),
        filesystem_operation_json: b"{}".to_vec(),
        commit_json: b"{}".to_vec(),
        preimage_json: b"null".to_vec(),
        postimage_json: fns_sync_core::canonical_json(&vec![state]).unwrap(),
        filesystem_receipt_json: None,
        stage: fns_sync_core::ApplyStage::FilesystemStarted,
    };
    fixture
        .engine
        .state_mut()
        .put_apply_journal(&journal)
        .unwrap();
    let before = fixture.engine.state().row_counts().unwrap();

    let mut replacement = fixture.incremental_begin(0, 1, 1, 0);
    replacement.stream_id = reconnect_stream_id(90);
    assert!(matches!(
        fixture.engine.snapshot_begin(replacement),
        Err(SyncError::StreamInvariant {
            reason: "stream_apply_in_progress"
        })
    ));
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(
        fixture.engine.state().apply_journals().unwrap(),
        vec![journal]
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_state()
            .unwrap()
            .unwrap()
            .stream_id,
        fixture.stream_id()
    );
}

fn reconnect_stream_id(suffix: u32) -> fns_protocol::StreamId {
    fns_protocol::StreamId::parse(&format!("10000000-0000-4000-8000-{suffix:012}"))
        .expect("reconnect stream id")
}

#[test]
fn replacement_replay_of_remote_receipts_only_settles_new_staging() {
    let mut fixture = support::EngineFixture::new();
    let first_bytes = b"revision-one";
    let second_bytes = b"revision-two";
    fixture
        .engine
        .stage_bytes(&support::hash(first_bytes), first_bytes)
        .unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(second_bytes), second_bytes)
        .unwrap();
    let mut first = fixture.remote_update_event(0, 1, "same.txt", first_bytes);
    let mut second = fixture.remote_update_event(1, 2, "same.txt", second_bytes);

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 2, 0))
        .unwrap();
    fixture.engine.workspace_event(first.clone()).unwrap();
    fixture.engine.workspace_event(second.clone()).unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), second_bytes);

    let new_stream_id = reconnect_stream_id(91);
    let mut replacement = fixture.incremental_begin(0, 2, 2, 0);
    replacement.stream_id = new_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();
    first.stream_id = new_stream_id;
    second.stream_id = new_stream_id;

    assert!(
        fixture
            .engine
            .workspace_event(first.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), second_bytes);
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("same.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_revision_item(new_stream_id, WorkspaceRevision::new(1))
            .unwrap()
            .unwrap()
            .status,
        StreamItemStatus::Applied
    );

    let mut changed = first;
    changed.mutation.metadata.modified_at_ms += 1;
    changed.path_state.metadata.modified_at_ms += 1;
    assert_eq!(
        fixture.engine.workspace_event(changed).unwrap_err(),
        SyncError::OperationChanged
    );

    fixture.engine.workspace_event(second).unwrap();
    let mut end = fixture.incremental_end(2, 2, 0);
    end.stream_id = new_stream_id;
    fixture.engine.snapshot_end(end).unwrap();
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![2]
    );
}

#[test]
fn replacement_replay_rejects_changed_path_state_before_applied_or_acked() {
    let mut fixture = support::EngineFixture::new();
    let bytes = b"durable";
    fixture
        .engine
        .stage_bytes(&support::hash(bytes), bytes)
        .unwrap();
    let original = fixture.remote_update_event(0, 1, "receipt.txt", bytes);

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture.engine.workspace_event(original.clone()).unwrap();

    let replacement_stream_id = reconnect_stream_id(94);
    let mut replacement = fixture.incremental_begin(0, 1, 1, 0);
    replacement.stream_id = replacement_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();

    let mut changed = original.clone();
    changed.stream_id = replacement_stream_id;
    changed.path_state.metadata.modified_at_ms += 1;
    changed
        .validate()
        .expect("path-state-only replay remains valid");

    let replay_result = fixture.engine.workspace_event(changed);
    let staged = fixture
        .engine
        .state()
        .stream_revision_item(replacement_stream_id, WorkspaceRevision::new(1))
        .unwrap();
    let mut end = fixture.incremental_end(1, 1, 0);
    end.stream_id = replacement_stream_id;
    let end_result = fixture.engine.snapshot_end(end);
    let acks = support::ack_revisions(&fixture.engine.pending_commands(16).unwrap());
    let durable = fixture
        .engine
        .state()
        .path_state("receipt.txt")
        .unwrap()
        .unwrap()
        .state;

    assert_eq!(replay_result.unwrap_err(), SyncError::OperationChanged);
    assert!(staged.is_none());
    assert!(end_result.is_ok());
    assert!(acks.is_empty());
    assert_eq!(durable, original.path_state);
    assert_eq!(fs::read(fixture.path("receipt.txt")).unwrap(), bytes);
}

#[test]
fn legacy_mutation_receipt_upgrades_only_after_authoritative_replay_validation() {
    let mut fixture = support::EngineFixture::new();
    let authoritative_bytes = b"authoritative";
    let local_bytes = b"preserved-local";
    fixture
        .engine
        .stage_bytes(&support::hash(authoritative_bytes), authoritative_bytes)
        .unwrap();
    let event = fixture.remote_update_event(0, 1, "legacy.txt", authoritative_bytes);
    fixture.engine.event(event.clone()).unwrap();
    assert_eq!(
        fs::read(fixture.path("legacy.txt")).unwrap(),
        authoritative_bytes
    );

    fixture.write("legacy.txt", local_bytes);
    let mutation_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&event.mutation).expect("canonical mutation"),
    );
    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    if user_version == 1 {
        connection
            .execute(
                "UPDATE applied_operations SET body_digest = ?1 WHERE origin_client_id = ?2 AND operation_id = ?3",
                rusqlite::params![
                    mutation_digest.as_slice(),
                    event.origin_client_id.to_string(),
                    event.operation_id.to_string(),
                ],
            )
            .unwrap();
    } else {
        assert_eq!(user_version, 5);
        connection
            .execute(
                "UPDATE applied_operations SET body_digest = ?1, receipt_kind = 'legacy', mutation_json = NULL WHERE origin_client_id = ?2 AND operation_id = ?3",
                rusqlite::params![
                    mutation_digest.as_slice(),
                    event.origin_client_id.to_string(),
                    event.operation_id.to_string(),
                ],
            )
            .unwrap();
    }
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.event(event.clone()).unwrap();
    assert_eq!(fs::read(fixture.path("legacy.txt")).unwrap(), local_bytes);
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("legacy.txt")
            .unwrap()
            .unwrap()
            .state,
        event.path_state
    );
    let queued = fixture.engine.outbox().unwrap();
    assert_eq!(queued.len(), 1);
    let queued_mutation = queued[0].mutation().unwrap();
    assert_eq!(queued_mutation.path, event.mutation.path);
    assert_eq!(
        queued_mutation.content_hash,
        RequiredNullable::Value(support::hash(local_bytes))
    );
    assert_eq!(queued_mutation.base_path_revision, event.revision);
    let upgraded = fixture
        .engine
        .state()
        .applied_operation(event.origin_client_id, event.operation_id)
        .unwrap()
        .unwrap();
    assert_ne!(upgraded.body_digest, mutation_digest);

    let connection = rusqlite::Connection::open(&database).unwrap();
    let (kind, retained_mutation): (String, Option<Vec<u8>>) = connection
        .query_row(
            "SELECT receipt_kind, mutation_json FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
            rusqlite::params![event.origin_client_id.to_string(), event.operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "mutation_result");
    assert_eq!(
        retained_mutation,
        Some(fns_sync_core::canonical_json(&event.mutation).unwrap())
    );
    drop(connection);

    let mut fixture = fixture.reopen();
    assert!(fixture.engine.event(event.clone()).unwrap().is_empty());
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_outbox = fixture.engine.outbox().unwrap();
    let before_receipt = fixture
        .engine
        .state()
        .applied_operation(event.origin_client_id, event.operation_id)
        .unwrap();
    let before_state = fixture.engine.state().path_state("legacy.txt").unwrap();
    let before_bytes = fs::read(fixture.path("legacy.txt")).unwrap();

    let mut changed = event;
    changed.path_state.metadata.modified_at_ms += 1;
    changed.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(fixture.engine.outbox().unwrap(), before_outbox);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(upgraded.origin_client_id, upgraded.operation_id)
            .unwrap(),
        before_receipt
    );
    assert_eq!(
        fixture.engine.state().path_state("legacy.txt").unwrap(),
        before_state
    );
    assert_eq!(fs::read(fixture.path("legacy.txt")).unwrap(), before_bytes);
}

#[test]
fn own_historical_event_recovers_a_legacy_receipt_without_losing_local_changes() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("own-legacy.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "own-legacy.txt",
        )))
        .unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, mutation.clone());
    let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    };
    fixture.engine.mutation_accepted(accepted.clone()).unwrap();
    fixture.write("own-legacy.txt", b"new-local");
    fixture.engine.close().unwrap();

    let mutation_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&mutation).expect("canonical mutation"),
    );
    let connection = rusqlite::Connection::open(fixture.state.path().join("state.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE applied_operations SET body_digest = ?1, receipt_kind = 'legacy', mutation_json = NULL WHERE origin_client_id = ?2 AND operation_id = ?3",
            rusqlite::params![
                mutation_digest.as_slice(),
                event.origin_client_id.to_string(),
                event.operation_id.to_string(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&mutation)
        .unwrap();
    let legacy_receipt = fixture
        .engine
        .state()
        .applied_operation(event.origin_client_id, event.operation_id)
        .unwrap()
        .unwrap();
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_state = fixture.engine.state().path_state("own-legacy.txt").unwrap();
    let before_bytes = fs::read(fixture.path("own-legacy.txt")).unwrap();
    assert!(fixture.engine.mutation_accepted(accepted.clone()).is_ok());
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(event.origin_client_id, event.operation_id)
            .unwrap()
            .unwrap(),
        legacy_receipt
    );
    assert_eq!(
        fixture.engine.state().path_state("own-legacy.txt").unwrap(),
        before_state
    );
    assert_eq!(
        fs::read(fixture.path("own-legacy.txt")).unwrap(),
        before_bytes
    );
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);

    let mut fixture = fixture.reopen();
    assert!(fixture.engine.mutation_accepted(accepted).is_ok());
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(event.origin_client_id, event.operation_id)
            .unwrap()
            .unwrap(),
        legacy_receipt
    );
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);
    let mut changed_event = event.clone();
    changed_event.mutation.base_path_revision = WorkspaceRevision::new(9);
    changed_event.validate().unwrap();
    assert_eq!(
        fixture.engine.workspace_event(changed_event).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(event.origin_client_id, event.operation_id)
            .unwrap()
            .unwrap(),
        legacy_receipt
    );
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture.engine.workspace_event(event.clone()).unwrap();
    assert_eq!(
        fs::read(fixture.path("own-legacy.txt")).unwrap(),
        b"new-local"
    );
    let queued = fixture.engine.outbox().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(
        queued[0].mutation().unwrap().content_hash,
        RequiredNullable::Value(support::hash(b"new-local"))
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_revision_item(fixture.stream_id(), WorkspaceRevision::new(1))
            .unwrap()
            .unwrap()
            .status,
        StreamItemStatus::Preserved
    );
    let receipt = fixture
        .engine
        .state()
        .applied_operation(event.origin_client_id, event.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::MutationResult
    );
    assert_eq!(
        receipt.mutation_json,
        Some(fns_sync_core::canonical_json(&mutation).unwrap())
    );
}

#[test]
fn legacy_acceptance_is_provisional_until_exact_live_event_and_survives_restart() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("provisional.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "provisional.txt",
        )))
        .unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, mutation.clone());
    let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    };
    let mutation_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&mutation).expect("canonical mutation"),
    );
    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                mutation.client_id.to_string(),
                mutation.operation_id.to_string(),
                event.revision.to_string(),
                mutation_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_states = fixture.engine.state().path_states().unwrap();
    fixture.engine.mutation_accepted(accepted.clone()).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(fixture.engine.state().path_states().unwrap(), before_states);
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);
    fixture.engine.close().unwrap();

    let connection = rusqlite::Connection::open(&database).unwrap();
    let provisional: Vec<u8> = connection
        .query_row(
            "SELECT accepted_json FROM provisional_mutation_acceptances WHERE origin_client_id = ?1 AND operation_id = ?2",
            rusqlite::params![mutation.client_id.to_string(), mutation.operation_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        provisional,
        fns_sync_core::canonical_json(&accepted).unwrap()
    );
    drop(connection);

    let mut fixture = fixture.reopen();
    assert!(
        fixture
            .engine
            .mutation_accepted(accepted.clone())
            .unwrap()
            .is_empty()
    );
    for changed in [
        {
            let mut changed = accepted.clone();
            changed.workspace_id =
                fns_protocol::WorkspaceId::parse("20000000-0000-4000-8000-000000000001").unwrap();
            changed
        },
        {
            let mut changed = accepted.clone();
            changed.client_id =
                fns_protocol::ClientId::parse("20000000-0000-4000-8000-000000000002").unwrap();
            changed
        },
        {
            let mut changed = accepted.clone();
            changed.operation_id = support::operation_id(399);
            changed
        },
        {
            let mut changed = accepted.clone();
            changed.revision = WorkspaceRevision::new(2);
            changed.path_state.path_revision = WorkspaceRevision::new(2);
            changed
        },
    ] {
        changed.validate().unwrap();
        assert_eq!(
            fixture.engine.mutation_accepted(changed).unwrap_err(),
            SyncError::OperationChanged
        );
    }
    let mut changed_response = accepted.clone();
    changed_response.path_state.metadata.modified_at_ms += 1;
    changed_response.validate().unwrap();
    assert_eq!(
        fixture
            .engine
            .mutation_accepted(changed_response)
            .unwrap_err(),
        SyncError::OperationChanged
    );

    fixture.write("provisional.txt", b"newer-local");
    let mut changed_event = event.clone();
    changed_event.path_state.metadata.modified_at_ms += 1;
    changed_event.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed_event).unwrap_err(),
        SyncError::OperationChanged
    );
    let mut changed_event_client = event.clone();
    changed_event_client.origin_client_id =
        fns_protocol::ClientId::parse("20000000-0000-4000-8000-000000000003").unwrap();
    changed_event_client.mutation.client_id = changed_event_client.origin_client_id;
    changed_event_client.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed_event_client).unwrap_err(),
        SyncError::OperationChanged
    );
    let mut changed_event_operation = event.clone();
    changed_event_operation.operation_id = support::operation_id(398);
    changed_event_operation.mutation.operation_id = changed_event_operation.operation_id;
    changed_event_operation.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed_event_operation).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);

    fixture.engine.event(event.clone()).unwrap();
    assert_eq!(
        fs::read(fixture.path("provisional.txt")).unwrap(),
        b"newer-local"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("provisional.txt")
            .unwrap()
            .unwrap()
            .state,
        event.path_state
    );
    let queued = fixture.engine.outbox().unwrap();
    assert_eq!(queued.len(), 1);
    assert_ne!(queued[0].operation_id, mutation.operation_id);
    assert_eq!(
        queued[0].mutation().unwrap().content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );
    let cursor = fixture.engine.cursor().unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, Some(WorkspaceRevision::new(1)));
    let receipt = fixture
        .engine
        .state()
        .applied_operation(event.origin_client_id, event.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::MutationResult
    );

    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    let mut fixture = fixture.reopen();
    assert!(fixture.engine.event(event).unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision,
        WorkspaceRevision::new(1)
    );
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    let queued = fixture.engine.outbox().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(
        queued[0].mutation().unwrap().content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );
}

#[test]
fn preserved_legacy_replacement_is_atomic_when_enqueue_fails_after_stale_removal() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("atomic.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "atomic.txt",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    };
    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let mutation_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&stale).expect("canonical mutation"),
    );
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                stale.client_id.to_string(),
                stale.operation_id.to_string(),
                event.revision.to_string(),
                mutation_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("atomic.txt", b"newer-local");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER deterministic_replacement_fault BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'deterministic replacement fault'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(fixture.engine.workspace_event(event.clone()).is_err());
    let outbox = fixture.engine.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert!(
        outbox
            .iter()
            .any(|record| record.operation_id == stale.operation_id)
    );
    assert!(
        fixture
            .engine
            .state()
            .path_state("atomic.txt")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::Legacy
    );
    assert_eq!(
        fs::read(fixture.path("atomic.txt")).unwrap(),
        b"newer-local"
    );

    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER deterministic_replacement_fault;")
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    fixture.engine.workspace_event(event.clone()).unwrap();
    let replacement = fixture.engine.outbox().unwrap();
    assert_eq!(replacement.len(), 1);
    assert_ne!(replacement[0].operation_id, stale.operation_id);
    assert_eq!(
        replacement[0].mutation().unwrap().content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_revision_item(fixture.stream_id(), WorkspaceRevision::new(1))
            .unwrap()
            .unwrap()
            .status,
        StreamItemStatus::Preserved
    );
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    let fixture = fixture.reopen();
    let replacement = fixture.engine.outbox().unwrap();
    assert_eq!(replacement.len(), 1);
    assert_eq!(
        replacement[0].mutation().unwrap().content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision,
        WorkspaceRevision::new(1)
    );
}

#[test]
fn preserved_legacy_rename_rolls_back_all_replacements_when_the_second_enqueue_fails() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("rename-old.txt", 0, b"accepted");
    fixture.rename("rename-old.txt", "rename-new.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("rename-old.txt"),
            to: support::workspace_path("rename-new.txt"),
        })
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    };
    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let mutation_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&stale).expect("canonical mutation"),
    );
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                stale.client_id.to_string(),
                stale.operation_id.to_string(),
                event.revision.to_string(),
                mutation_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("rename-old.txt", b"resurrected-local");
    fixture.write("rename-new.txt", b"newer-local");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let before_states = fixture.engine.state().path_states().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER deterministic_second_replacement_fault BEFORE INSERT ON outbox WHEN (SELECT COUNT(*) FROM outbox) >= 1 BEGIN SELECT RAISE(ABORT, 'second replacement fault'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(fixture.engine.workspace_event(event.clone()).is_err());
    let outbox = fixture.engine.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].operation_id, stale.operation_id);
    assert_eq!(fixture.engine.state().path_states().unwrap(), before_states);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::Legacy
    );

    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER deterministic_second_replacement_fault;")
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    fixture.engine.workspace_event(event).unwrap();
    let replacements = fixture.engine.outbox().unwrap();
    assert_eq!(replacements.len(), 2);
    let replacement_paths = replacements
        .iter()
        .map(|record| record.mutation().unwrap().path)
        .collect::<Vec<_>>();
    assert!(replacement_paths.contains(&support::workspace_path("rename-old.txt")));
    assert!(replacement_paths.contains(&support::workspace_path("rename-new.txt")));
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    let fixture = fixture.reopen();
    assert_eq!(fixture.engine.outbox().unwrap().len(), 2);
    assert_eq!(
        fs::read(fixture.path("rename-old.txt")).unwrap(),
        b"resurrected-local"
    );
    assert_eq!(
        fs::read(fixture.path("rename-new.txt")).unwrap(),
        b"newer-local"
    );
}

#[test]
fn superseded_legacy_live_receipt_upgrades_without_rewinding_the_tree() {
    let mut fixture = support::EngineFixture::new();
    let first = fixture.remote_update_event(0, 1, "legacy-history.txt", b"first");
    let second = fixture.remote_update_event(1, 2, "legacy-history.txt", b"second");
    fixture
        .engine
        .stage_bytes(&support::hash(b"first"), b"first")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"second"), b"second")
        .unwrap();
    fixture.engine.event(first.clone()).unwrap();
    fixture.engine.event(second.clone()).unwrap();
    assert_eq!(
        fs::read(fixture.path("legacy-history.txt")).unwrap(),
        b"second"
    );
    fixture.engine.close().unwrap();

    let connection = rusqlite::Connection::open(fixture.state.path().join("state.sqlite")).unwrap();
    for event in [&first, &second] {
        let mutation_digest = fns_sync_core::body_digest(
            &fns_sync_core::canonical_json(&event.mutation).expect("canonical mutation"),
        );
        connection
            .execute(
                "UPDATE applied_operations SET body_digest = ?1, receipt_kind = 'legacy', mutation_json = NULL WHERE origin_client_id = ?2 AND operation_id = ?3",
                rusqlite::params![
                    mutation_digest.as_slice(),
                    event.origin_client_id.to_string(),
                    event.operation_id.to_string(),
                ],
            )
            .unwrap();
    }
    drop(connection);

    let mut fixture = fixture.reopen();
    let before_cursor = fixture.engine.cursor().unwrap();
    fixture.engine.event(first.clone()).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fs::read(fixture.path("legacy-history.txt")).unwrap(),
        b"second"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("legacy-history.txt")
            .unwrap()
            .unwrap()
            .state,
        second.path_state
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(first.origin_client_id, first.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::MutationResult
    );

    let mut changed_first = first.clone();
    changed_first.path_state.metadata.modified_at_ms += 1;
    changed_first.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed_first).unwrap_err(),
        SyncError::OperationChanged
    );
    fixture.engine.event(second.clone()).unwrap();
    assert_eq!(
        fs::read(fixture.path("legacy-history.txt")).unwrap(),
        b"second"
    );

    let mut fixture = fixture.reopen();
    assert!(fixture.engine.event(first).unwrap().is_empty());
    assert!(fixture.engine.event(second).unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("legacy-history.txt")).unwrap(),
        b"second"
    );
}

#[test]
fn superseded_own_legacy_event_settles_stale_outbox_and_preserves_newer_edit() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("superseded-own.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "superseded-own.txt",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let own_event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: own_event.workspace_id,
        client_id: own_event.origin_client_id,
        operation_id: own_event.operation_id,
        revision: own_event.revision,
        path_state: own_event.path_state.clone(),
        old_path_state: own_event.old_path_state.clone(),
        new_path_state: own_event.new_path_state.clone(),
    };
    let legacy_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&stale).expect("canonical mutation"),
    );

    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                stale.client_id.to_string(),
                stale.operation_id.to_string(),
                own_event.revision.to_string(),
                legacy_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("superseded-own.txt", b"newer-local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
            "superseded-own.txt",
        )))
        .unwrap();
    fixture.write("newer-operation.txt", b"keep-newer");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "newer-operation.txt",
        )))
        .unwrap();
    let newer_operation = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .find(|row| row.operation_id != stale.operation_id)
        .expect("newer outbox operation");

    // Advance last_applied past the delayed authoritative own Event while no
    // snapshot stream is active, then deliver that historical Event.
    fixture
        .engine
        .event(fixture.remote_mkdir_event(1, 2, "later"))
        .unwrap();
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_path_states = fixture.engine.state().path_states().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER superseded_live_replacement_fault BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'superseded live replacement fault'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(fixture.engine.event(own_event.clone()).is_err());
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fixture.engine.state().path_states().unwrap(),
        before_path_states
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        AppliedOperationReceiptKind::Legacy
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        1
    );
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    let outbox = fixture.engine.outbox().unwrap();
    assert_eq!(outbox.len(), 2);
    assert!(
        outbox
            .iter()
            .any(|row| row.operation_id == stale.operation_id)
    );
    assert!(outbox.iter().any(|row| {
        row.operation_id == newer_operation.operation_id
            && row.body_digest == newer_operation.body_digest
    }));
    assert_eq!(
        fs::read(fixture.path("superseded-own.txt")).unwrap(),
        b"newer-local"
    );
    println!(
        "superseded live rollback: cursor={:?} receipt=legacy provisional=1 intents=1 outbox={:?} filesystem_hash={}",
        fixture.engine.cursor().unwrap(),
        outbox
            .iter()
            .map(|row| (row.operation_id, digest_hex(&row.body_digest)))
            .collect::<Vec<_>>(),
        support::hash(b"newer-local")
    );

    fixture.engine.close().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER superseded_live_replacement_fault;")
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    fixture.engine.event(own_event.clone()).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fs::read(fixture.path("superseded-own.txt")).unwrap(),
        b"newer-local"
    );
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    let mut fixture = fixture.reopen();

    let receipt = fixture
        .engine
        .state()
        .applied_operation(stale.client_id, stale.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt_kind,
        AppliedOperationReceiptKind::MutationResult
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("superseded-own.txt")
            .unwrap()
            .unwrap()
            .state,
        own_event.path_state
    );
    let outbox = fixture.engine.outbox().unwrap();
    println!(
        "superseded live restart: cursor={:?} receipt={:?} provisional=0 intents=0 outbox={:?} filesystem_hash={}",
        fixture.engine.cursor().unwrap(),
        receipt.receipt_kind,
        outbox
            .iter()
            .map(|row| (
                row.operation_id,
                digest_hex(&row.body_digest),
                row.mutation().unwrap().content_hash,
            ))
            .collect::<Vec<_>>(),
        support::hash(b"newer-local")
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        0
    );
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("superseded-own.txt")).unwrap(),
        b"newer-local"
    );
    assert!(
        outbox
            .iter()
            .all(|row| row.operation_id != stale.operation_id),
        "authoritative own Event left the settled legacy operation in outbox"
    );
    assert!(outbox.iter().any(|row| {
        row.operation_id == newer_operation.operation_id
            && row.body_digest == newer_operation.body_digest
    }));
    assert!(outbox.iter().any(|row| {
        row.mutation().unwrap().content_hash
            == RequiredNullable::Value(support::hash(b"newer-local"))
    }));
    assert!(
        fixture
            .engine
            .pending_commands(16)
            .unwrap()
            .iter()
            .all(|command| !matches!(command, SyncCommand::SendAck(_)))
    );
}

#[test]
fn replayed_superseded_own_legacy_event_settles_stale_outbox() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("superseded-replay.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "superseded-replay.txt",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let own_event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: own_event.workspace_id,
        client_id: own_event.origin_client_id,
        operation_id: own_event.operation_id,
        revision: own_event.revision,
        path_state: own_event.path_state.clone(),
        old_path_state: own_event.old_path_state.clone(),
        new_path_state: own_event.new_path_state.clone(),
    };
    let legacy_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&stale).expect("canonical mutation"),
    );

    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                stale.client_id.to_string(),
                stale.operation_id.to_string(),
                own_event.revision.to_string(),
                legacy_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("superseded-replay.txt", b"newer-local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
            "superseded-replay.txt",
        )))
        .unwrap();
    let later_event = fixture.remote_mkdir_event(1, 2, "later-replay");
    fixture.engine.event(later_event.clone()).unwrap();

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 2, 0))
        .unwrap();
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_path_states = fixture.engine.state().path_states().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER superseded_stream_replacement_fault BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'superseded stream replacement fault'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(fixture.engine.workspace_event(own_event.clone()).is_err());
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fixture.engine.state().path_states().unwrap(),
        before_path_states
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        AppliedOperationReceiptKind::Legacy
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        1
    );
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    assert_eq!(
        fixture.engine.outbox().unwrap()[0].operation_id,
        stale.operation_id
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_revision_item(fixture.stream_id(), own_event.revision)
            .unwrap()
            .unwrap()
            .status,
        StreamItemStatus::Ready
    );
    assert_eq!(
        fs::read(fixture.path("superseded-replay.txt")).unwrap(),
        b"newer-local"
    );
    println!(
        "superseded stream rollback: cursor={:?} receipt=legacy provisional=1 intents=1 outbox=[({}, {})] stream_status=ready filesystem_hash={}",
        fixture.engine.cursor().unwrap(),
        stale.operation_id,
        digest_hex(&fixture.engine.outbox().unwrap()[0].body_digest),
        support::hash(b"newer-local")
    );

    fixture.engine.close().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER superseded_stream_replacement_fault;")
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    fixture.engine.workspace_event(own_event.clone()).unwrap();
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_revision_item(fixture.stream_id(), own_event.revision)
            .unwrap()
            .unwrap()
            .status,
        StreamItemStatus::Preserved
    );
    fixture.engine.workspace_event(later_event).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(2, 2, 0))
        .unwrap();
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    let fixture = fixture.reopen();

    let receipt = fixture
        .engine
        .state()
        .applied_operation(stale.client_id, stale.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt_kind,
        AppliedOperationReceiptKind::MutationResult
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("superseded-replay.txt")
            .unwrap()
            .unwrap()
            .state,
        own_event.path_state
    );
    let outbox = fixture.engine.outbox().unwrap();
    println!(
        "superseded stream restart: cursor={:?} receipt={:?} provisional=0 intents=0 outbox={:?} stream_state={:?} filesystem_hash={}",
        fixture.engine.cursor().unwrap(),
        receipt.receipt_kind,
        outbox
            .iter()
            .map(|row| (
                row.operation_id,
                digest_hex(&row.body_digest),
                row.mutation().unwrap().content_hash,
            ))
            .collect::<Vec<_>>(),
        fixture.engine.state().stream_state().unwrap(),
        support::hash(b"newer-local")
    );
    assert!(
        outbox
            .iter()
            .all(|row| row.operation_id != stale.operation_id),
        "historical stream replay left the settled legacy operation in outbox"
    );
    assert!(outbox.iter().any(|row| {
        row.mutation().unwrap().content_hash
            == RequiredNullable::Value(support::hash(b"newer-local"))
    }));
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        0
    );
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("superseded-replay.txt")).unwrap(),
        b"newer-local"
    );
}

#[test]
fn superseded_own_legacy_event_preserves_complete_rename_intent() {
    run_superseded_own_legacy_event_preserves_complete_rename_intent(false, false);
}

#[test]
fn replayed_superseded_own_legacy_event_preserves_complete_rename_intent() {
    run_superseded_own_legacy_event_preserves_complete_rename_intent(true, false);
}

#[test]
fn superseded_own_legacy_event_preserves_compacted_rename_chain() {
    run_superseded_own_legacy_event_preserves_complete_rename_intent(false, true);
}

fn run_superseded_own_legacy_event_preserves_complete_rename_intent(replayed: bool, chained: bool) {
    let mut fixture = support::EngineFixture::new();
    fixture.write("rename-chain-a.txt", b"accepted");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "rename-chain-a.txt",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let own_event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: own_event.workspace_id,
        client_id: own_event.origin_client_id,
        operation_id: own_event.operation_id,
        revision: own_event.revision,
        path_state: own_event.path_state.clone(),
        old_path_state: own_event.old_path_state.clone(),
        new_path_state: own_event.new_path_state.clone(),
    };
    let legacy_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&stale).expect("canonical mutation"),
    );

    fixture.engine.close().unwrap();
    let database = fixture.state.path().join("state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                stale.client_id.to_string(),
                stale.operation_id.to_string(),
                own_event.revision.to_string(),
                legacy_digest.as_slice(),
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.rename("rename-chain-a.txt", "rename-chain-b.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("rename-chain-a.txt"),
            to: support::workspace_path("rename-chain-b.txt"),
        })
        .unwrap();
    fixture.write("rename-chain-b.txt", b"newer-local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
            "rename-chain-b.txt",
        )))
        .unwrap();
    let target = if chained {
        fixture.rename("rename-chain-b.txt", "rename-chain-c.txt");
        fixture
            .engine
            .record_local_change(fns_fs::FsChange::Rename {
                from: support::workspace_path("rename-chain-b.txt"),
                to: support::workspace_path("rename-chain-c.txt"),
            })
            .unwrap();
        "rename-chain-c.txt"
    } else {
        "rename-chain-b.txt"
    };
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 2);

    fixture.write("rename-chain-unrelated.txt", b"unrelated");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "rename-chain-unrelated.txt",
        )))
        .unwrap();
    let unrelated = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .find(|row| row.operation_id != stale.operation_id)
        .expect("unrelated outbox work");

    let later_event = fixture.remote_mkdir_event(1, 2, "rename-chain-later");
    fixture.engine.event(later_event.clone()).unwrap();
    if replayed {
        fixture
            .engine
            .snapshot_begin(fixture.incremental_begin(0, 2, 2, 0))
            .unwrap();
    }

    let before_cursor = fixture.engine.cursor().unwrap();
    let before_path_states = fixture.engine.state().path_states().unwrap();
    let before_intents = fixture.engine.state().local_intents().unwrap();
    let before_outbox = fixture.engine.outbox().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER superseded_rename_chain_fault BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'superseded rename chain fault'); END;",
        )
        .unwrap();
    drop(connection);

    let failed = if replayed {
        fixture.engine.workspace_event(own_event.clone())
    } else {
        fixture.engine.event(own_event.clone())
    };
    assert!(failed.is_err());
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(
        fixture.engine.state().path_states().unwrap(),
        before_path_states
    );
    assert_eq!(
        fixture.engine.state().local_intents().unwrap(),
        before_intents
    );
    assert_eq!(fixture.engine.outbox().unwrap(), before_outbox);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        AppliedOperationReceiptKind::Legacy
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        1
    );
    if replayed {
        assert_eq!(
            fixture
                .engine
                .state()
                .stream_revision_item(fixture.stream_id(), own_event.revision)
                .unwrap()
                .unwrap()
                .status,
            StreamItemStatus::Ready
        );
    }

    fixture.engine.close().unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER superseded_rename_chain_fault;")
        .unwrap();
    drop(connection);
    let mut fixture = fixture.reopen();
    if replayed {
        fixture.engine.workspace_event(own_event.clone()).unwrap();
        fixture.engine.workspace_event(own_event.clone()).unwrap();
        fixture.engine.workspace_event(later_event).unwrap();
        fixture
            .engine
            .snapshot_end(fixture.incremental_end(2, 2, 0))
            .unwrap();
    } else {
        fixture.engine.event(own_event.clone()).unwrap();
        fixture.engine.event(own_event.clone()).unwrap();
    }
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    let mut fixture = fixture.reopen();

    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .state()
            .row_count("provisional_mutation_acceptances")
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(stale.client_id, stale.operation_id)
            .unwrap()
            .unwrap()
            .receipt_kind,
        AppliedOperationReceiptKind::MutationResult
    );
    assert_eq!(fs::read(fixture.path(target)).unwrap(), b"newer-local");
    let outbox = fixture.engine.outbox().unwrap();
    assert_eq!(outbox.len(), 2);
    assert!(
        outbox
            .iter()
            .all(|row| row.operation_id != stale.operation_id)
    );
    assert!(outbox.iter().any(|row| {
        row.operation_id == unrelated.operation_id && row.body_digest == unrelated.body_digest
    }));
    let replacement = outbox
        .into_iter()
        .find(|row| row.operation_id != unrelated.operation_id)
        .expect("rename replacement")
        .mutation()
        .unwrap();
    assert_eq!(replacement.kind, WorkspaceMutationKind::Rename);
    assert_eq!(
        replacement.path,
        support::workspace_path("rename-chain-a.txt")
    );
    assert_eq!(replacement.new_path, Some(support::workspace_path(target)));
    assert_eq!(
        replacement.content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );

    let replacement_event = support::self_event_from_mutation(&fixture, 2, 3, replacement);
    fixture
        .engine
        .mutation_accepted(WorkspaceMutationAcceptedMessage {
            workspace_id: replacement_event.workspace_id,
            client_id: replacement_event.origin_client_id,
            operation_id: replacement_event.operation_id,
            revision: replacement_event.revision,
            path_state: replacement_event.path_state.clone(),
            old_path_state: replacement_event.old_path_state.clone(),
            new_path_state: replacement_event.new_path_state.clone(),
        })
        .unwrap();
    let before_duplicate_cursor = fixture.engine.cursor().unwrap();
    let before_duplicate_states = fixture.engine.state().path_states().unwrap();
    fixture.engine.event(own_event).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap(), before_duplicate_cursor);
    assert_eq!(
        fixture.engine.state().path_states().unwrap(),
        before_duplicate_states
    );

    let fixture = fixture.reopen();
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(fixture.engine.outbox().unwrap().len(), 1);
    assert_eq!(
        fixture.engine.outbox().unwrap()[0].operation_id,
        unrelated.operation_id
    );
    assert_eq!(fs::read(fixture.path(target)).unwrap(), b"newer-local");
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("rename-chain-a.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(3)
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state(target)
            .unwrap()
            .unwrap()
            .state,
        replacement_event.path_state
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
}

#[test]
fn superseded_own_legacy_directory_event_preserves_rename_and_descendant_bytes() {
    let mut fixture = support::EngineFixture::new();
    fs::create_dir(fixture.path("legacy-tree")).unwrap();
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "legacy-tree",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let own_event = support::self_event_from_mutation(&fixture, 0, 1, stale.clone());
    let accepted = accepted_from_event(&own_event);
    install_legacy_receipt(&mut fixture, &stale, &own_event);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("legacy-tree/child.txt", b"descendant");
    fixture.rename("legacy-tree", "moved-tree");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("legacy-tree"),
            to: support::workspace_path("moved-tree"),
        })
        .unwrap();
    fixture
        .engine
        .event(fixture.remote_mkdir_event(1, 2, "directory-later"))
        .unwrap();
    fixture.engine.event(own_event).unwrap();
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    let mut fixture = fixture.reopen();

    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("moved-tree/child.txt")).unwrap(),
        b"descendant"
    );
    let replacement = fixture.engine.outbox().unwrap()[0].mutation().unwrap();
    assert_eq!(replacement.kind, WorkspaceMutationKind::Rename);
    assert_eq!(replacement.path, support::workspace_path("legacy-tree"));
    assert_eq!(
        replacement.new_path,
        Some(support::workspace_path("moved-tree"))
    );
    assert_eq!(replacement.content_hash, RequiredNullable::Null);

    let replacement_event = support::self_event_from_mutation(&fixture, 2, 3, replacement);
    fixture
        .engine
        .mutation_accepted(accepted_from_event(&replacement_event))
        .unwrap();
    let fixture = fixture.reopen();
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("moved-tree/child.txt")).unwrap(),
        b"descendant"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("moved-tree")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(3)
    );
}

#[test]
fn superseded_own_legacy_delete_event_recreates_renamed_target() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("deleted-then-renamed-a.txt", 1, b"remote");
    fixture.remove("deleted-then-renamed-a.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Delete(support::workspace_path(
            "deleted-then-renamed-a.txt",
        )))
        .unwrap();
    let stale = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let own_event = support::self_event_from_mutation(&fixture, 0, 2, stale.clone());
    let accepted = accepted_from_event(&own_event);
    install_legacy_receipt(&mut fixture, &stale, &own_event);

    let mut fixture = fixture.reopen();
    fixture.engine.mutation_accepted(accepted).unwrap();
    fixture.write("deleted-then-renamed-a.txt", b"newer-local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "deleted-then-renamed-a.txt",
        )))
        .unwrap();
    fixture.rename("deleted-then-renamed-a.txt", "deleted-then-renamed-b.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("deleted-then-renamed-a.txt"),
            to: support::workspace_path("deleted-then-renamed-b.txt"),
        })
        .unwrap();
    fixture
        .engine
        .event(fixture.remote_mkdir_event(1, 3, "delete-rename-later"))
        .unwrap();
    fixture.engine.event(own_event).unwrap();
    fixture.engine.ack_confirmed(fixture.ack(3)).unwrap();
    let mut fixture = fixture.reopen();

    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("deleted-then-renamed-b.txt")).unwrap(),
        b"newer-local"
    );
    let replacement = fixture.engine.outbox().unwrap()[0].mutation().unwrap();
    assert_eq!(replacement.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        replacement.path,
        support::workspace_path("deleted-then-renamed-b.txt")
    );
    assert_eq!(replacement.new_path, None);
    assert_eq!(
        replacement.content_hash,
        RequiredNullable::Value(support::hash(b"newer-local"))
    );

    let replacement_event = support::self_event_from_mutation(&fixture, 2, 4, replacement);
    fixture
        .engine
        .mutation_accepted(accepted_from_event(&replacement_event))
        .unwrap();
    let fixture = fixture.reopen();
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fs::read(fixture.path("deleted-then-renamed-b.txt")).unwrap(),
        b"newer-local"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("deleted-then-renamed-b.txt")
            .unwrap()
            .unwrap()
            .state,
        replacement_event.path_state
    );
}

fn accepted_from_event(event: &WorkspaceEventMessage) -> WorkspaceMutationAcceptedMessage {
    WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    }
}

fn install_legacy_receipt(
    fixture: &mut support::EngineFixture,
    mutation: &WorkspaceMutation,
    event: &WorkspaceEventMessage,
) {
    let legacy_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(mutation).expect("canonical mutation"),
    );
    fixture.engine.close().unwrap();
    let connection = rusqlite::Connection::open(fixture.state.path().join("state.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, 'legacy', NULL)",
            rusqlite::params![
                mutation.client_id.to_string(),
                mutation.operation_id.to_string(),
                event.revision.to_string(),
                legacy_digest.as_slice(),
            ],
        )
        .unwrap();
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn replacement_replay_of_own_receipt_does_not_rewrite_or_reopen_outbox() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("own.txt", b"local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path("own.txt")))
        .unwrap();
    let mutation = fixture.engine.outbox().unwrap()[0].mutation().unwrap();
    let mut own_event = support::self_event_from_mutation(&fixture, 0, 1, mutation);
    let mut later = fixture.remote_mkdir_event(1, 2, "later");

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 2, 0))
        .unwrap();
    fixture.engine.workspace_event(own_event.clone()).unwrap();
    fixture.engine.workspace_event(later.clone()).unwrap();
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );

    let new_stream_id = reconnect_stream_id(92);
    let mut replacement = fixture.incremental_begin(0, 2, 2, 0);
    replacement.stream_id = new_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();
    own_event.stream_id = new_stream_id;
    later.stream_id = new_stream_id;

    assert!(
        fixture
            .engine
            .workspace_event(own_event)
            .unwrap()
            .is_empty()
    );
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(fs::read(fixture.path("own.txt")).unwrap(), b"local");

    fixture.engine.workspace_event(later).unwrap();
    let mut end = fixture.incremental_end(2, 2, 0);
    end.stream_id = new_stream_id;
    fixture.engine.snapshot_end(end).unwrap();
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![2]
    );
}

#[test]
fn replacement_replay_of_conflict_receipt_is_staging_only_and_terminal_duplicate_is_exact() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("resolved.txt", 0, b"base");
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"later"), b"later")
        .unwrap();
    let resolved = fixture.remote_conflict_resolved(1, "resolved.txt");
    let mut later = fixture.remote_update_event(0, 2, "resolved.txt", b"later");

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 2, 0))
        .unwrap();
    fixture.engine.conflict_resolved(resolved.clone()).unwrap();
    fixture.engine.workspace_event(later.clone()).unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"later");

    let new_stream_id = reconnect_stream_id(93);
    let mut replacement = fixture.incremental_begin(0, 2, 2, 0);
    replacement.stream_id = new_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();
    later.stream_id = new_stream_id;

    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"later");
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("resolved.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(2)
    );
    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
            .unwrap()
            .is_empty()
    );
    let mut changed = resolved;
    changed.choice = fns_protocol::WorkspaceConflictChoice::Incoming;
    assert_eq!(
        fixture.engine.conflict_resolved(changed).unwrap_err(),
        SyncError::OperationChanged
    );

    fixture.engine.workspace_event(later).unwrap();
    let mut end = fixture.incremental_end(2, 2, 0);
    end.stream_id = new_stream_id;
    fixture.engine.snapshot_end(end).unwrap();
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![2]
    );
}

#[test]
fn legacy_conflict_receipt_upgrades_only_from_the_exact_full_message() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("legacy-resolved.txt", 0, b"base");
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let resolved = fixture.remote_conflict_resolved(1, "legacy-resolved.txt");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture.engine.conflict_resolved(resolved.clone()).unwrap();
    fixture.engine.close().unwrap();

    let connection = rusqlite::Connection::open(fixture.state.path().join("state.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE applied_operations SET receipt_kind = 'legacy' WHERE origin_client_id = ?1 AND operation_id = ?2",
            rusqlite::params![
                resolved.resolved_by_client_id.to_string(),
                resolved.operation_id.to_string()
            ],
        )
        .unwrap();
    drop(connection);

    let mut fixture = fixture.reopen();
    let replacement_stream_id = reconnect_stream_id(95);
    let mut replacement = fixture.incremental_begin(0, 1, 1, 0);
    replacement.stream_id = replacement_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();
    fixture.engine.conflict_resolved(resolved.clone()).unwrap();
    let receipt = fixture
        .engine
        .state()
        .applied_operation(resolved.resolved_by_client_id, resolved.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt_kind,
        fns_sync_core::AppliedOperationReceiptKind::ConflictResolution
    );
    assert_eq!(receipt.mutation_json, None);
    assert_eq!(
        receipt.body_digest,
        fns_sync_core::body_digest(&fns_sync_core::canonical_json(&resolved).unwrap())
    );

    let before = fixture.engine.state().row_counts().unwrap();
    let mut changed = resolved;
    changed.choice = fns_protocol::WorkspaceConflictChoice::Incoming;
    assert_eq!(
        fixture.engine.conflict_resolved(changed).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
}

#[test]
fn terminal_snapshot_entry_duplicates_are_exact_noops() {
    for (local_bytes, expected_status) in [
        (None, StreamItemStatus::Applied),
        (Some(b"local".as_slice()), StreamItemStatus::Preserved),
    ] {
        let mut fixture = support::EngineFixture::new();
        if let Some(bytes) = local_bytes {
            fixture.write("entry.txt", bytes);
        }
        fixture
            .engine
            .stage_bytes(&support::hash(b"server"), b"server")
            .unwrap();
        fixture
            .engine
            .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
            .unwrap();
        let entry = fixture.snapshot_file_entry(0, 1, "entry.txt", b"server");
        fixture.engine.snapshot_entry(entry.clone()).unwrap();
        assert_eq!(
            fixture
                .engine
                .state()
                .stream_entry(fixture.stream_id(), 0)
                .unwrap()
                .unwrap()
                .status,
            expected_status
        );

        let before = fixture.engine.state().row_counts().unwrap();
        assert!(
            fixture
                .engine
                .snapshot_entry(entry.clone())
                .unwrap()
                .is_empty()
        );
        assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
        assert_eq!(
            fixture
                .engine
                .state()
                .stream_entry(fixture.stream_id(), 0)
                .unwrap()
                .unwrap()
                .status,
            expected_status
        );

        let mut changed = entry;
        changed.entry.metadata.modified_at_ms += 1;
        assert_eq!(
            fixture.engine.snapshot_entry(changed).unwrap_err(),
            SyncError::OperationChanged
        );
        assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    }
}

#[test]
fn completed_stream_ack_requires_terminal_items_even_when_old_pending_matches_final() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(0, 1, "old-dir"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );

    let new_stream_id = reconnect_stream_id(94);
    let mut replacement = fixture.snapshot_begin(1, 1, 0);
    replacement.stream_id = new_stream_id;
    fixture.engine.snapshot_begin(replacement).unwrap();
    let mut entry = fixture.snapshot_file_entry(0, 1, "replacement.bin", b"replacement");
    entry.stream_id = new_stream_id;
    assert!(
        fixture
            .engine
            .snapshot_entry(entry)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    let mut end = fixture.snapshot_end(1, 1, 0);
    end.stream_id = new_stream_id;
    fixture.engine.snapshot_end(end).unwrap();
    assert_eq!(
        fixture.engine.completed_stream_ack_revision().unwrap(),
        None
    );

    fixture
        .engine
        .blob_available(
            support::hash(b"replacement"),
            b"replacement".len() as u64,
            std::io::Cursor::new(b"replacement"),
        )
        .unwrap();
    assert_eq!(
        fixture.engine.completed_stream_ack_revision().unwrap(),
        Some(WorkspaceRevision::new(1))
    );
    assert_eq!(
        fs::read(fixture.path("replacement.bin")).unwrap(),
        b"replacement"
    );
}

fn download_matches(command: &SyncCommand, expected: &fns_protocol::WorkspaceContentHash) -> bool {
    matches!(command, SyncCommand::DownloadBlob { content_hash, .. } if content_hash == expected)
}

#[test]
fn ack_is_emitted_only_after_every_remote_postimage_is_durable() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("a.txt", 10, b"old");
    fixture.seed_remote_file("b.txt", 10, b"gone");
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(10))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(10))
        .unwrap();
    let begin = fixture.incremental_begin(10, 12, 2, 0);
    let update = fixture.remote_update_event(0, 11, "a.txt", b"server");
    let delete = fixture.remote_delete_event(1, 12, "b.txt");

    fixture.engine.snapshot_begin(begin).unwrap();
    assert!(
        fixture
            .engine
            .workspace_event(update)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(fixture.engine.workspace_event(delete).unwrap().is_empty());
    assert!(
        fixture
            .engine
            .snapshot_end(fixture.incremental_end(12, 2, 0))
            .unwrap()
            .is_empty()
    );
    let before_blobs = fixture.engine.pending_commands(16).unwrap();
    assert!(before_blobs.iter().any(support::is_download));
    assert!(support::ack_revisions(&before_blobs).is_empty());
    fixture.provide_requested_blobs();

    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(support::ack_revisions(&commands), vec![12]);
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 10);
    fixture.engine.ack_confirmed(fixture.ack(12)).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 12);
}

#[test]
fn live_remote_file_event_downloads_and_applies_without_active_stream() {
    let mut fixture = support::EngineFixture::new();
    let event = fixture.remote_update_event(0, 1, "remote.txt", b"server");

    let commands = fixture.engine.event(event).unwrap();
    assert!(commands.iter().any(support::is_download));
    assert!(!fixture.path("remote.txt").exists());

    fixture.provide_requested_blobs();

    assert_eq!(fs::read(fixture.path("remote.txt")).unwrap(), b"server");
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(support::ack_revisions(&commands), vec![1]);
}

#[test]
fn begin_identity_mode_and_count_are_validated() {
    let mut fixture = support::EngineFixture::new();
    let mut invalid = fixture.incremental_begin(0, 1, 1, 0);
    invalid.entry_count = 1;
    assert!(matches!(
        fixture.engine.snapshot_begin(invalid),
        Err(SyncError::StreamInvariant { .. }) | Err(SyncError::ProtocolInvariant { .. })
    ));
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert!(!fixture.path("anything").exists());

    let mut mismatched = fixture.incremental_begin(0, 1, 0, 0);
    mismatched.workspace_id =
        fns_protocol::WorkspaceId::parse("20000000-0000-4000-8000-000000000001").unwrap();
    assert!(matches!(
        fixture.engine.snapshot_begin(mismatched),
        Err(SyncError::ProtocolInvariant { .. })
    ));
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
}

#[test]
fn event_indices_are_contiguous_only_within_event_items() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 3, 3, 0))
        .unwrap();
    let first = fixture.remote_update_event(0, 1, "first.txt", b"first");
    let resolved = fixture.remote_conflict_resolved(2, "resolved.txt");
    let second = fixture.remote_delete_event(1, 3, "second.txt");

    assert!(fixture.engine.workspace_event(first).is_ok());
    assert!(fixture.engine.conflict_resolved(resolved).is_ok());
    assert!(fixture.engine.workspace_event(second).is_ok());
}

#[test]
fn mixed_event_resolved_event_revision_order_is_strict() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 3, 3, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "first.txt", b"first"))
        .unwrap();
    fixture
        .engine
        .conflict_resolved(fixture.remote_conflict_resolved(2, "resolved.txt"))
        .unwrap();
    let regressed = fixture.remote_delete_event(1, 1, "second.txt");
    assert!(matches!(
        fixture.engine.workspace_event(regressed),
        Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );
}

#[test]
fn authoritative_conflicts_are_counted_and_replaced() {
    let mut fixture = support::EngineFixture::new();
    let old =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000031", "1", "old.txt");
    fixture
        .engine
        .state_mut()
        .record_conflict(&old, ConflictStatus::Manual)
        .unwrap();
    let absent =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000033", "1", "absent.txt");
    let resolution_json = br#"{"choice":"current"}"#.to_vec();
    fixture
        .engine
        .state_mut()
        .put_conflict(&fns_sync_core::ConflictRecord {
            conflict_id: absent.conflict_id,
            workspace_id: absent.workspace_id,
            conflict_revision: absent.conflict_revision,
            created_json: fns_sync_core::canonical_json(&absent).unwrap(),
            status: ConflictStatus::Resolving,
            candidate_hash: None,
            resolution_digest: Some(fns_sync_core::body_digest(&resolution_json)),
            resolution_json: Some(resolution_json.clone()),
        })
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(4, 5, 0, 2))
        .unwrap();
    let retained =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000031", "2", "old.txt");
    let replacement =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000032", "1", "new.txt");
    fixture.engine.conflict_created(retained.clone()).unwrap();
    fixture
        .engine
        .conflict_created(replacement.clone())
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 0, 2))
        .unwrap();

    let conflicts = fixture.engine.state().conflicts().unwrap();
    assert_eq!(conflicts.len(), 3);
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict.conflict_id == retained.conflict_id)
    );
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict.conflict_id == replacement.conflict_id)
    );
    let refreshed_absent = conflicts
        .iter()
        .find(|conflict| conflict.conflict_id == absent.conflict_id)
        .expect("absent pending conflict retained for dispatched resolution");
    assert_eq!(refreshed_absent.status, ConflictStatus::RefreshRequired);
    assert_eq!(
        refreshed_absent.resolution_json.as_deref(),
        Some(resolution_json.as_slice())
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .len(),
        2
    );
    assert!(
        !fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .iter()
            .any(|conflict| conflict.conflict_id == absent.conflict_id)
    );
}

#[test]
fn authoritative_new_conflict_generation_discards_only_stale_resolution_work() {
    let mut fixture = support::EngineFixture::new();
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000034",
        "1",
        "refreshed.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();

    let blocked_origin = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: old.created_by_operation_id,
        path: old.path.clone(),
        base_path_revision: old.ancestor.path_revision,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: old.incoming.content_hash.clone(),
        metadata: old.incoming.metadata.clone(),
        new_path: None,
        target_base_path_revision: None,
    };
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&blocked_origin)
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked_origin.operation_id, OutboxStage::BlockedConflict)
        .unwrap();

    let receipt = fixture
        .engine
        .resolve_conflict(
            old.conflict_id,
            old.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
        .unwrap();
    let resolution_operation_id = receipt.operation_id;
    fixture
        .engine
        .state_mut()
        .set_conflict_status(old.conflict_id, ConflictStatus::RefreshRequired)
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(resolution_operation_id)
            .unwrap()
            .is_some()
    );

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 1))
        .unwrap();
    let mut refreshed = old.clone();
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    fixture.engine.conflict_created(refreshed.clone()).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 1))
        .unwrap();

    let stored = fixture
        .engine
        .state()
        .conflict(old.conflict_id)
        .unwrap()
        .expect("refreshed conflict");
    assert_eq!(stored.conflict_revision, refreshed.conflict_revision);
    assert_eq!(stored.status, ConflictStatus::Manual);
    assert!(stored.resolution_json.is_none());
    assert!(stored.resolution_digest.is_none());
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(resolution_operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked_origin.operation_id)
            .unwrap()
            .expect("originating mutation retained")
            .stage,
        OutboxStage::BlockedConflict
    );
}

#[test]
fn authoritative_same_generation_reopens_an_unrepresentable_stale_conflict() {
    let mut fixture = support::EngineFixture::new();
    let conflict = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000035",
        "1",
        "same-generation.txt",
    );
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    let receipt = fixture
        .engine
        .resolve_conflict(
            conflict.conflict_id,
            conflict.conflict_revision,
            WorkspaceConflictChoice::Incoming,
        )
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_conflict_status(conflict.conflict_id, ConflictStatus::RefreshRequired)
        .unwrap();

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 1))
        .unwrap();
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 1))
        .unwrap();

    let reopened = fixture
        .engine
        .state()
        .conflict(conflict.conflict_id)
        .unwrap()
        .expect("authoritative pending conflict");
    assert_eq!(reopened.status, ConflictStatus::Manual);
    assert!(reopened.resolution_json.is_none());
    assert!(reopened.resolution_digest.is_none());
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(receipt.operation_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn authoritative_same_generation_preserves_merged_resolution_across_reconnects() {
    let mut fixture = support::EngineFixture::new();
    let conflict = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000043",
        "1",
        "merged-snapshot.txt",
    );
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    let merged = b"locally merged candidate";
    let merged_hash = support::hash(merged);
    fixture.write(conflict.path.as_str(), merged);
    let receipt = fixture
        .engine
        .resolve_conflict(
            conflict.conflict_id,
            conflict.conflict_revision,
            WorkspaceConflictChoice::Merged,
        )
        .unwrap();
    let durable_before = fixture
        .engine
        .state()
        .conflict(conflict.conflict_id)
        .unwrap()
        .expect("durable merged conflict");
    let outbox_before = fixture
        .engine
        .state()
        .outbox_entry(receipt.operation_id)
        .unwrap()
        .expect("durable merged resolution");
    assert_eq!(durable_before.status, ConflictStatus::Resolving);
    assert_eq!(
        durable_before.candidate_hash.as_deref(),
        Some(merged_hash.as_str())
    );

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 1))
        .unwrap();
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 1))
        .unwrap();

    let mut fixture = fixture.reopen();
    let after_first = fixture
        .engine
        .state()
        .conflict(conflict.conflict_id)
        .unwrap()
        .expect("merged conflict retained after first reopen");
    assert_eq!(after_first, durable_before);
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(receipt.operation_id)
            .unwrap()
            .expect("merged resolution retained after first reopen"),
        outbox_before
    );
    let first_replay = fixture
        .engine
        .pending_commands(16)
        .unwrap()
        .into_iter()
        .find_map(|command| match command {
            SyncCommand::ResolveConflict(request)
                if request.operation_id == receipt.operation_id =>
            {
                Some(request)
            }
            _ => None,
        })
        .expect("merged resolution replayed after first reopen");
    assert_eq!(
        fns_sync_core::canonical_json(&first_replay).unwrap(),
        outbox_before.body_json
    );
    let dispatched_outbox = fixture
        .engine
        .state()
        .outbox_entry(receipt.operation_id)
        .unwrap()
        .expect("merged resolution remains durable after replay");
    assert_eq!(dispatched_outbox.stage, OutboxStage::Dispatched);
    assert_eq!(dispatched_outbox.body_json, outbox_before.body_json);
    assert_eq!(dispatched_outbox.body_digest, outbox_before.body_digest);

    let second_stream_id = reconnect_stream_id(96);
    let mut second_begin = fixture.incremental_begin(0, 0, 0, 1);
    second_begin.stream_id = second_stream_id;
    fixture.engine.snapshot_begin(second_begin).unwrap();
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    let mut second_end = fixture.incremental_end(0, 0, 1);
    second_end.stream_id = second_stream_id;
    fixture.engine.snapshot_end(second_end).unwrap();

    let mut fixture = fixture.reopen();
    let after_second = fixture
        .engine
        .state()
        .conflict(conflict.conflict_id)
        .unwrap()
        .expect("merged conflict retained after second reopen");
    assert_eq!(after_second, durable_before);
    assert_eq!(after_second.status, ConflictStatus::Resolving);
    assert_eq!(
        after_second.candidate_hash.as_deref(),
        Some(merged_hash.as_str())
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(receipt.operation_id)
            .unwrap()
            .expect("merged resolution retained after second reopen"),
        dispatched_outbox
    );
    let view = fixture.engine.list_conflicts().unwrap().pop().unwrap();
    assert_eq!(
        view.pending_resolution
            .expect("merged pending resolution")
            .content_hash,
        Some(merged_hash)
    );
    let second_replay = fixture
        .engine
        .pending_commands(16)
        .unwrap()
        .into_iter()
        .find_map(|command| match command {
            SyncCommand::ResolveConflict(request)
                if request.operation_id == receipt.operation_id =>
            {
                Some(request)
            }
            _ => None,
        })
        .expect("merged resolution replayed after second reopen");
    assert_eq!(
        fns_sync_core::canonical_json(&second_replay).unwrap(),
        outbox_before.body_json
    );
}

#[test]
fn live_changed_origin_generation_preserves_newer_intent_without_materializing_it() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("changed-origin-live.txt", 0, b"base");
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000044",
        "1",
        "changed-origin-live.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();
    let blocked = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: old.created_by_operation_id,
        path: old.path.clone(),
        base_path_revision: old.ancestor.path_revision,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: old.incoming.content_hash.clone(),
        metadata: old.incoming.metadata.clone(),
        new_path: None,
        target_base_path_revision: None,
    };
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&blocked)
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();
    fixture.write("changed-origin-live.txt", b"newer local value");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
            "changed-origin-live.txt",
        )))
        .unwrap();
    let newer_intent = fixture
        .engine
        .state()
        .local_intent("changed-origin-live.txt")
        .unwrap()
        .expect("newer local intent");

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(281);
    refreshed.current.path_revision = WorkspaceRevision::new(2);
    refreshed.validate().unwrap();
    fixture.engine.conflict_created(refreshed.clone()).unwrap();

    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .local_intent("changed-origin-live.txt")
            .unwrap()
            .expect("newer local intent retained"),
        newer_intent
    );
    assert!(fixture.engine.pending_commands(16).unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .state()
            .local_intent("changed-origin-live.txt")
            .unwrap()
            .expect("intent still blocked by active conflict"),
        newer_intent
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(refreshed.conflict_id)
            .unwrap()
            .expect("new generation conflict")
            .conflict_revision,
        refreshed.conflict_revision
    );
}

#[test]
fn live_changed_origin_rename_preserves_both_local_endpoints() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("rename-source.txt", 8, b"base");
    fixture.rename("rename-source.txt", "rename-target.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("rename-source.txt"),
            to: support::workspace_path("rename-target.txt"),
        })
        .unwrap();
    let blocked = fixture
        .engine
        .outbox()
        .unwrap()
        .pop()
        .expect("local rename outbox")
        .mutation()
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();

    let old = blocked_rename_conflict(&fixture, "10000000-0000-4000-8000-000000000053", &blocked);
    fixture.engine.conflict_created(old.clone()).unwrap();

    fixture.write("rename-source.txt", b"new source");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "rename-source.txt",
        )))
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .local_intent("rename-source.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("rename-target.txt")
            .unwrap()
            .is_none()
    );

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(281);
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    refreshed.validate().unwrap();
    fixture.engine.conflict_created(refreshed).unwrap();

    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("rename-source.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("rename-target.txt")
            .unwrap()
            .is_some()
    );

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 0))
        .unwrap();

    let mutations = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .map(|record| record.mutation().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 2);
    let source = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "rename-source.txt")
        .expect("source upsert");
    assert_eq!(source.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        source.content_hash,
        RequiredNullable::Value(support::hash(b"new source"))
    );
    assert_eq!(source.metadata.size, b"new source".len() as u64);
    let target = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "rename-target.txt")
        .expect("target upsert");
    assert_eq!(target.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        target.content_hash,
        RequiredNullable::Value(support::hash(b"base"))
    );
    assert_eq!(target.metadata.size, b"base".len() as u64);
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn live_changed_origin_flattens_chained_file_rename_across_restart() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("chain-a.txt", 8, b"chain bytes");
    fixture.rename("chain-a.txt", "chain-b.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("chain-a.txt"),
            to: support::workspace_path("chain-b.txt"),
        })
        .unwrap();
    let blocked = fixture
        .engine
        .outbox()
        .unwrap()
        .pop()
        .expect("blocked first rename")
        .mutation()
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();
    let old = blocked_rename_conflict(&fixture, "10000000-0000-4000-8000-000000000054", &blocked);
    fixture.engine.conflict_created(old.clone()).unwrap();

    fixture.rename("chain-b.txt", "chain-c.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("chain-b.txt"),
            to: support::workspace_path("chain-c.txt"),
        })
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-b.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-c.txt")
            .unwrap()
            .is_some()
    );

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(282);
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    refreshed.validate().unwrap();
    fixture.engine.conflict_created(refreshed).unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-a.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-b.txt")
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-c.txt")
            .unwrap()
            .is_some()
    );

    let mut fixture = fixture.reopen();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-a.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("chain-c.txt")
            .unwrap()
            .is_some()
    );
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 0))
        .unwrap();
    let expected = fixture.engine.outbox().unwrap();
    assert_eq!(expected.len(), 2);

    let fixture = fixture.reopen();
    assert_eq!(fixture.engine.outbox().unwrap(), expected);
    let mutations = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .map(|record| record.mutation().unwrap())
        .collect::<Vec<_>>();
    let source = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "chain-a.txt")
        .expect("source delete");
    assert_eq!(source.kind, WorkspaceMutationKind::Delete);
    let target = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "chain-c.txt")
        .expect("final target upsert");
    assert_eq!(target.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        target.content_hash,
        RequiredNullable::Value(support::hash(b"chain bytes"))
    );
    assert!(
        mutations
            .iter()
            .all(|mutation| mutation.path.as_str() != "chain-b.txt")
    );
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn live_changed_origin_flattens_chained_directory_with_nested_files() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_directory("tree-a", 8);
    fixture.seed_remote_directory("tree-a/nested", 8);
    fixture.seed_remote_file("tree-a/nested/data.bin", 8, b"old nested bytes");
    fixture.rename("tree-a", "tree-b");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("tree-a"),
            to: support::workspace_path("tree-b"),
        })
        .unwrap();
    let blocked = fixture
        .engine
        .outbox()
        .unwrap()
        .pop()
        .expect("blocked directory rename")
        .mutation()
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();
    let old = blocked_rename_conflict(&fixture, "10000000-0000-4000-8000-000000000055", &blocked);
    fixture.engine.conflict_created(old.clone()).unwrap();

    fixture.rename("tree-b", "tree-c");
    fixture.write("tree-c/nested/data.bin", b"new nested bytes");
    fixture.write("tree-c/nested/empty.bin", b"");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Rename {
            from: support::workspace_path("tree-b"),
            to: support::workspace_path("tree-c"),
        })
        .unwrap();

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(283);
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    refreshed.validate().unwrap();
    fixture.engine.conflict_created(refreshed).unwrap();
    assert!(
        fixture
            .engine
            .state()
            .local_intent("tree-b")
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("tree-c/nested/data.bin")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("tree-c/nested/empty.bin")
            .unwrap()
            .is_some()
    );

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 0))
        .unwrap();
    let mutations = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .map(|record| record.mutation().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 7);
    for path in ["tree-a", "tree-a/nested", "tree-a/nested/data.bin"] {
        assert_eq!(
            mutations
                .iter()
                .find(|mutation| mutation.path.as_str() == path)
                .expect("directory source delete")
                .kind,
            WorkspaceMutationKind::Delete
        );
    }
    for path in ["tree-c", "tree-c/nested"] {
        assert_eq!(
            mutations
                .iter()
                .find(|mutation| mutation.path.as_str() == path)
                .expect("directory target mkdir")
                .kind,
            WorkspaceMutationKind::Mkdir
        );
    }
    let changed = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "tree-c/nested/data.bin")
        .expect("changed nested file");
    assert_eq!(changed.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        changed.content_hash,
        RequiredNullable::Value(support::hash(b"new nested bytes"))
    );
    assert_eq!(changed.metadata.size, b"new nested bytes".len() as u64);
    let empty = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "tree-c/nested/empty.bin")
        .expect("empty nested file");
    assert_eq!(empty.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        empty.content_hash,
        RequiredNullable::Value(support::hash(b""))
    );
    assert_eq!(empty.metadata.size, 0);
    assert!(
        mutations
            .iter()
            .all(|mutation| !mutation.path.as_str().starts_with("tree-b"))
    );
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn authoritative_changed_origin_preserves_target_only_edit() {
    let (mut fixture, blocked, old) = fixture_with_blocked_file_rename(
        "target-edit-a.txt",
        "target-edit-b.txt",
        "10000000-0000-4000-8000-000000000056",
        b"base target",
    );
    fixture.write("target-edit-b.txt", b"edited target");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Update(support::workspace_path(
            "target-edit-b.txt",
        )))
        .unwrap();

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(284);
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    refreshed.validate().unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 1))
        .unwrap();
    fixture.engine.conflict_created(refreshed).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 1))
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert!(
        fixture
            .engine
            .state()
            .local_intent("target-edit-a.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("target-edit-b.txt")
            .unwrap()
            .is_some()
    );

    let next_stream_id = reconnect_stream_id(98);
    let mut begin = fixture.incremental_begin(0, 0, 0, 0);
    begin.stream_id = next_stream_id;
    fixture.engine.snapshot_begin(begin).unwrap();
    let mut end = fixture.incremental_end(0, 0, 0);
    end.stream_id = next_stream_id;
    fixture.engine.snapshot_end(end).unwrap();

    let mutations = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .map(|record| record.mutation().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 2);
    assert_eq!(
        mutations
            .iter()
            .find(|mutation| mutation.path.as_str() == "target-edit-a.txt")
            .expect("source delete")
            .kind,
        WorkspaceMutationKind::Delete
    );
    let target = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "target-edit-b.txt")
        .expect("edited target upsert");
    assert_eq!(target.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        target.content_hash,
        RequiredNullable::Value(support::hash(b"edited target"))
    );
    assert_eq!(target.metadata.size, b"edited target".len() as u64);
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn live_changed_origin_preserves_delete_after_blocked_rename() {
    let (mut fixture, blocked, old) = fixture_with_blocked_file_rename(
        "delete-a.txt",
        "delete-b.txt",
        "10000000-0000-4000-8000-000000000057",
        b"delete me",
    );
    fixture.remove("delete-b.txt");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Delete(support::workspace_path(
            "delete-b.txt",
        )))
        .unwrap();

    let mut refreshed = old;
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.created_by_operation_id = support::operation_id(285);
    refreshed.current.path_revision = WorkspaceRevision::new(9);
    refreshed.validate().unwrap();
    fixture.engine.conflict_created(refreshed).unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("delete-a.txt")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .local_intent("delete-b.txt")
            .unwrap()
            .is_none()
    );

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 0))
        .unwrap();
    let mutations = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .map(|record| record.mutation().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 1);
    assert_eq!(mutations[0].path.as_str(), "delete-a.txt");
    assert_eq!(mutations[0].kind, WorkspaceMutationKind::Delete);
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn authoritative_absent_conflict_requeues_exact_blocked_desired_state() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("absent-conflict.txt", 0, b"base");
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000045",
        "1",
        "absent-conflict.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();
    let blocked = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: old.created_by_operation_id,
        path: old.path.clone(),
        base_path_revision: old.ancestor.path_revision,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: old.incoming.content_hash.clone(),
        metadata: old.incoming.metadata.clone(),
        new_path: None,
        target_base_path_revision: None,
    };
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&blocked)
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(0, 0, 0))
        .unwrap();

    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_none()
    );
    let replacement = fixture
        .engine
        .outbox()
        .unwrap()
        .into_iter()
        .next()
        .expect("replacement mutation")
        .mutation()
        .unwrap();
    assert_ne!(replacement.operation_id, blocked.operation_id);
    assert_eq!(replacement.path, blocked.path);
    assert_eq!(replacement.kind, blocked.kind);
    assert_eq!(replacement.content_hash, blocked.content_hash);
    assert_eq!(replacement.metadata, blocked.metadata);
    assert_eq!(replacement.base_path_revision, WorkspaceRevision::ZERO);
}

#[test]
fn authoritative_same_generation_rejects_changed_body_without_mutating_durable_state() {
    let mut fixture = support::EngineFixture::new();
    let original = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000042",
        "1",
        "changed-generation.txt",
    );
    fixture.engine.conflict_created(original.clone()).unwrap();
    let resolution = fixture
        .engine
        .resolve_conflict(
            original.conflict_id,
            original.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
        .unwrap();
    let durable_before = fixture
        .engine
        .state()
        .conflict(original.conflict_id)
        .unwrap()
        .expect("durable conflict");
    let outbox_before = fixture
        .engine
        .state()
        .outbox_entry(resolution.operation_id)
        .unwrap()
        .expect("durable resolution");
    let cursor_before = fixture.engine.cursor().unwrap();

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 0, 0, 1))
        .unwrap();
    let mut changed = original.clone();
    changed.incoming.metadata.modified_at_ms += 1;
    changed.validate().unwrap();
    fixture.engine.conflict_created(changed).unwrap();

    assert_eq!(
        fixture
            .engine
            .snapshot_end(fixture.incremental_end(0, 0, 1))
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(original.conflict_id)
            .unwrap()
            .expect("original conflict retained"),
        durable_before
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(resolution.operation_id)
            .unwrap()
            .expect("resolution retained"),
        outbox_before
    );
    assert_eq!(fixture.engine.cursor().unwrap(), cursor_before);
}

#[test]
fn conflict_created_has_no_stream_index_or_tree_revision() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 0, 1))
        .unwrap();
    let conflict = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000033",
        "99",
        "conflict.txt",
    );
    fixture.engine.conflict_created(conflict).unwrap();
    assert!(
        fixture
            .engine
            .state()
            .stream_revision_items(fixture.stream_id())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn conflict_only_reconnect_updates_conflicts_without_same_revision_ack() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(5))
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(5, 5, 0, 1))
        .unwrap();
    fixture
        .engine
        .conflict_created(fixture.remote_conflict_created(
            "10000000-0000-4000-8000-000000000034",
            "1",
            "conflict.txt",
        ))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 0, 1))
        .unwrap();
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert!(support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()).is_empty());
}

#[test]
fn conflict_resolved_materializes_remote_postimage() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("resolved.txt", 1, b"base");
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(1, 2, 1, 0))
        .unwrap();
    fixture
        .engine
        .conflict_resolved(fixture.remote_conflict_resolved(2, "resolved.txt"))
        .unwrap();

    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"current");
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("resolved.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(2)
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn end_must_match_begin() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 1, 0))
        .unwrap();
    let mut end = fixture.incremental_end(3, 1, 0);
    end.final_revision = WorkspaceRevision::new(3);
    assert!(matches!(
        fixture.engine.snapshot_end(end),
        Err(SyncError::ProtocolInvariant { .. }) | Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert!(!fixture.path("anything").exists());
    assert!(
        !fixture
            .engine
            .state()
            .stream_state()
            .unwrap()
            .unwrap()
            .end_received
    );
}

#[test]
fn duplicate_exact_event_is_noop() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("same.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let event = fixture.remote_update_event(0, 1, "same.txt", b"new");
    assert!(fixture.engine.workspace_event(event.clone()).is_ok());
    fixture.provide_requested_blobs();
    let before = fixture.engine.state().row_counts().unwrap();
    assert!(fixture.engine.workspace_event(event).unwrap().is_empty());
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
}

#[test]
fn duplicate_operation_with_new_digest_is_invariant_error() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("same.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let first = fixture.remote_update_event(0, 1, "same.txt", b"new");
    fixture.engine.workspace_event(first).unwrap();
    let changed = fixture.remote_update_event(0, 1, "same.txt", b"changed");
    let before = fixture.engine.state().row_counts().unwrap();
    assert!(matches!(
        fixture.engine.workspace_event(changed),
        Err(SyncError::OperationChanged) | Err(SyncError::ProtocolInvariant { .. })
    ));
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), b"old");
}

#[test]
fn self_event_settles_without_file_rewrite() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("self.txt", b"local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "self.txt",
        )))
        .unwrap();
    let mutation = fixture.engine.outbox().unwrap()[0].mutation().unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, mutation);
    let before = fs::read(fixture.path("self.txt")).unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture.engine.workspace_event(event).unwrap();
    assert_eq!(fs::read(fixture.path("self.txt")).unwrap(), before);
    assert!(fixture.engine.outbox().unwrap().is_empty());
}

#[test]
fn remote_file_directory_symlink_delete_and_rename_apply() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("delete.txt", 0, b"delete");
    fixture.write("old-dir/child.txt", b"child");
    fixture.seed_remote_directory("old-dir", 0);
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 5, 5, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "file.txt", b"file"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(1, 2, "dir"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_symlink_event(2, 3, "link", b"dir"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_delete_event(3, 4, "delete.txt"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_rename_event(4, 5, "old-dir", "new-dir"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 5, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert!(
        fixture
            .engine
            .pending_commands(16)
            .unwrap()
            .iter()
            .any(|command| {
                matches!(command, SyncCommand::SendAck(message) if message.revision.get() == 5)
            })
    );
    assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"file");
    assert!(fixture.path("dir").is_dir());
    assert!(fixture.path("link").is_symlink());
    assert!(!fixture.path("delete.txt").exists());
    assert!(fixture.path("new-dir/child.txt").exists());
}

#[test]
fn snapshot_paths_are_utf8_byte_sorted() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 2, 0))
        .unwrap();
    let first = fixture.snapshot_file_entry(0, 1, "é.txt", b"one");
    fixture.engine.snapshot_entry(first).unwrap();
    let out_of_order = fixture.snapshot_file_entry(1, 1, "z.txt", b"two");
    assert!(matches!(
        fixture.engine.snapshot_entry(out_of_order),
        Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(
        fixture.engine.state().row_counts().unwrap()["stream_entries"],
        1
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_entries(fixture.stream_id())
            .unwrap()[0]
            .entry_index,
        0
    );
}

#[test]
fn full_snapshot_path_revisions_need_not_be_sorted() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(5, 2, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 5, "a.txt", b"a"))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(1, 2, "b.txt", b"b"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(5, 2, 0))
        .unwrap();
    fixture.provide_requested_blobs();

    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        5
    );
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![5]
    );
}

#[test]
fn local_watcher_echo_during_snapshot_is_reconciled_as_remote_state() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("echo.txt", b"remote");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "echo.txt",
        )))
        .unwrap();
    let modified_at_ms = std::fs::metadata(fixture.path("echo.txt"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut entry = fixture.snapshot_file_entry(0, 1, "echo.txt", b"remote");
    entry.entry.metadata.modified_at_ms = modified_at_ms as i64;
    entry.validate().unwrap();
    fixture.engine.snapshot_entry(entry).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();

    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn initial_same_content_is_adopted() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("same.txt", b"same");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 1, "same.txt", b"same"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), b"same");
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("same.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision
            .get(),
        1
    );
}

#[test]
fn initial_difference_queues_base_zero_without_overwrite() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("different.txt", b"local");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 1, "different.txt", b"server"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert_eq!(fs::read(fixture.path("different.txt")).unwrap(), b"local");
    let commands = fixture.engine.pending_commands(16).unwrap();
    let mutations = commands
        .iter()
        .filter(|command| matches!(command, SyncCommand::Mutation(_)))
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 1);
    assert!(matches!(mutations[0], SyncCommand::Mutation(mutation)
        if mutation.kind == WorkspaceMutationKind::UpsertFile
            && mutation.base_path_revision == WorkspaceRevision::ZERO));
}

#[test]
fn local_edit_during_remote_update_is_preserved() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("edit.txt", 7, b"base");
    fixture.write("edit.txt", b"local");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(7, 8, 1, 0))
        .unwrap();
    let result = fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 8, "edit.txt", b"server"))
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(fs::read(fixture.path("edit.txt")).unwrap(), b"local");
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, SyncCommand::Mutation(mutation)
        if mutation.base_path_revision.get() == 8))
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );
}

#[test]
fn missing_blob_is_requested_again_after_reopen() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("reopen.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let event = fixture.remote_update_event(0, 1, "reopen.txt", b"new");
    assert!(
        fixture
            .engine
            .workspace_event(event)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    fixture.engine.close().unwrap();
    let mut reopened = fixture.reopen();
    let commands = reopened.engine.pending_commands(16).unwrap();
    assert_eq!(
        commands
            .iter()
            .filter(|command| support::is_download(command))
            .count(),
        1
    );
}

#[test]
fn prepared_apply_journal_is_removed_on_reopen() {
    let mut fixture = support::EngineFixture::new();
    let state = support::path_state(
        "journal.txt",
        1,
        fns_protocol::RequiredNullable::Null,
        support::file_metadata(0),
        fns_protocol::WorkspaceEntryKind::Directory,
    );
    let operation = fns_sync_core::model::RemoteApplyOperation::Upsert {
        state: state.clone(),
    };
    let apply_id = fns_sync_core::ApplyId(
        uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000041").unwrap(),
    );
    let workspace_id = fixture.engine.state().workspace_id();
    let stream_id = fixture.stream_id();
    let filesystem_operation = fns_fs::FsOperation::Mkdir {
        path: state.path.clone(),
        metadata: state.metadata.clone(),
        expected: fns_fs::ExpectedEntry::Missing,
    };
    let commit_plan = fns_sync_core::ApplyCommitPlan::SnapshotEntry {
        entry: fns_protocol::WorkspaceSnapshotEntryMessage {
            workspace_id,
            stream_id,
            index: 0,
            entry: state.clone(),
        },
    };
    let filesystem_operation_json = fns_sync_core::canonical_json(&filesystem_operation).unwrap();
    let mut journal = fns_sync_core::ApplyJournalRecord {
        apply_id,
        workspace_id,
        stream_id,
        item_kind: fns_sync_core::ApplyItemKind::Entry,
        item_key: "journal.txt".to_owned(),
        apply_namespace: fns_sync_core::ApplyNamespace::SnapshotEntry,
        operation_body_digest: [0; 32],
        operation_json: fns_sync_core::canonical_json(&operation).unwrap(),
        filesystem_operation_json: filesystem_operation_json.clone(),
        commit_json: fns_sync_core::canonical_json(&commit_plan).unwrap(),
        preimage_json: filesystem_operation_json,
        postimage_json: fns_sync_core::canonical_json(&vec![state]).unwrap(),
        filesystem_receipt_json: None,
        stage: fns_sync_core::ApplyStage::Prepared,
    };
    journal.operation_body_digest = fns_sync_core::apply_journal_immutable_digest(&journal);
    fixture
        .engine
        .state_mut()
        .put_apply_journal(&journal)
        .unwrap();
    assert_eq!(fixture.engine.state().apply_journals().unwrap().len(), 1);
    fixture.engine.close().unwrap();

    let reopened = fixture.reopen();
    assert!(reopened.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn ack_repeats_until_correlated_success() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("ack.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_delete_event(0, 1, "ack.txt"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![1]
    );
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![1]
    );
    assert!(fixture.engine.ack_confirmed(fixture.ack(0)).is_err());
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert!(support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()).is_empty());
}

#[test]
fn matching_watcher_echo_after_stream_completion_is_not_deferred() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(0, 1, "binary"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();

    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path("binary")))
        .unwrap();
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(fixture.engine.outbox().unwrap().is_empty());

    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();

    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(fixture.engine.state().pending_work_count().unwrap(), 0);
}

#[test]
fn pending_commands_recovers_matching_intent_after_stream_cleanup_and_restart() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(0, 1, "binary"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    let path = support::workspace_path("binary");
    let intent = fns_sync_core::LocalIntent::Desired {
        entry: fns_sync_core::LocalDesiredEntry {
            path: path.clone(),
            kind: fns_protocol::WorkspaceEntryKind::Directory,
            content_hash: RequiredNullable::Null,
            metadata: support::zero_metadata(),
        },
    };
    let body = fns_sync_core::canonical_json(&intent).unwrap();
    fixture
        .engine
        .state_mut()
        .put_local_intent(&path, &body, 1_800_000_000_000)
        .unwrap();
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    assert!(fixture.engine.state().stream_state().unwrap().is_none());

    let mut fixture = fixture.reopen();
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    assert!(fixture.engine.pending_commands(16).unwrap().is_empty());

    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(fixture.engine.state().pending_work_count().unwrap(), 0);
}

#[test]
fn deferred_live_event_precedes_orphaned_intent_materialization_after_ack() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(0, 1, "binary"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();

    let path = support::workspace_path("binary");
    let intent = fns_sync_core::LocalIntent::Desired {
        entry: fns_sync_core::LocalDesiredEntry {
            path: path.clone(),
            kind: fns_protocol::WorkspaceEntryKind::Directory,
            content_hash: RequiredNullable::Null,
            metadata: support::zero_metadata(),
        },
    };
    let body = fns_sync_core::canonical_json(&intent).unwrap();
    fixture
        .engine
        .state_mut()
        .put_local_intent(&path, &body, 1_800_000_000_000)
        .unwrap();

    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    assert!(fixture.engine.outbox().unwrap().is_empty());
    fixture
        .engine
        .event(fixture.remote_delete_event(1, 2, "binary"))
        .unwrap();

    let commands = fixture.engine.pending_commands(16).unwrap();
    let mutation = commands
        .iter()
        .find_map(|command| match command {
            SyncCommand::Mutation(mutation) => Some(mutation),
            _ => None,
        })
        .expect("orphaned intent mutation after deferred live event");
    assert_eq!(mutation.kind, WorkspaceMutationKind::Mkdir);
    assert_eq!(mutation.base_path_revision, WorkspaceRevision::new(2));
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(2))
    );
}

#[test]
fn blob_blocked_live_event_precedes_orphaned_intent_materialization() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "binary", b"old"))
        .unwrap();
    fixture.provide_requested_blobs();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();

    let path = support::workspace_path("binary");
    let initial = fixture
        .engine
        .state()
        .path_state("binary")
        .unwrap()
        .unwrap()
        .state;
    let intent = fns_sync_core::LocalIntent::Desired {
        entry: fns_sync_core::LocalDesiredEntry {
            path: path.clone(),
            kind: initial.kind,
            content_hash: initial.content_hash,
            metadata: initial.metadata,
        },
    };
    let body = fns_sync_core::canonical_json(&intent).unwrap();
    fixture
        .engine
        .state_mut()
        .put_local_intent(&path, &body, 1_800_000_000_000)
        .unwrap();
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();

    let commands = fixture
        .engine
        .event(fixture.remote_update_event(1, 2, "binary", b"remote-file"))
        .unwrap();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, SyncCommand::DownloadBlob { .. }))
    );
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("binary")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(1)
    );

    fixture.provide_requested_blobs();
    let commands = fixture.engine.pending_commands(16).unwrap();
    let mutation = commands
        .iter()
        .find_map(|command| match command {
            SyncCommand::Mutation(mutation) => Some(mutation),
            _ => None,
        })
        .expect("orphaned intent mutation after blob-backed live event");
    assert_eq!(mutation.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(mutation.base_path_revision, WorkspaceRevision::new(2));
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn exact_own_receipts_wait_behind_blob_blocked_live_revision() {
    let mut fixture = support::EngineFixture::new();
    for path in ["own-second.txt", "own-third.txt"] {
        fixture.write(path, path.as_bytes());
        fixture
            .engine
            .record_local_change(fns_fs::FsChange::Create(support::workspace_path(path)))
            .unwrap();
    }
    let mutations = fixture
        .engine
        .pending_commands(16)
        .unwrap()
        .into_iter()
        .filter_map(|command| match command {
            SyncCommand::Mutation(mutation) => Some(mutation),
            _ => None,
        })
        .collect::<Vec<_>>();
    let second_mutation = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "own-second.txt")
        .cloned()
        .expect("second mutation");
    let third_mutation = mutations
        .iter()
        .find(|mutation| mutation.path.as_str() == "own-third.txt")
        .cloned()
        .expect("third mutation");
    let second = support::self_event_from_mutation(&fixture, 1, 2, second_mutation);
    let third = support::self_event_from_mutation(&fixture, 2, 3, third_mutation);

    let blocked = fixture.remote_update_event(0, 1, "blocked.bin", b"remote-bytes");
    assert!(
        fixture
            .engine
            .event(blocked.clone())
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    fixture
        .engine
        .mutation_accepted(accepted_from_event(&second))
        .unwrap();
    fixture
        .engine
        .mutation_accepted(accepted_from_event(&third))
        .unwrap();

    assert!(
        fixture
            .engine
            .event(second.clone())
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(
        fixture
            .engine
            .event(third.clone())
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    let blocked_cursor = fixture.engine.cursor().unwrap();
    assert_eq!(
        blocked_cursor.last_applied_revision,
        WorkspaceRevision::new(0)
    );
    assert_eq!(blocked_cursor.pending_ack_revision, None);

    fixture
        .engine
        .blob_available(
            support::hash(b"remote-bytes"),
            b"remote-bytes".len() as u64,
            std::io::Cursor::new(b"remote-bytes"),
        )
        .unwrap();
    let applied_cursor = fixture.engine.cursor().unwrap();
    assert_eq!(
        applied_cursor.last_applied_revision,
        WorkspaceRevision::new(3)
    );
    assert_eq!(
        applied_cursor.pending_ack_revision,
        Some(WorkspaceRevision::new(3))
    );
    assert_eq!(
        fs::read(fixture.path("blocked.bin")).unwrap(),
        b"remote-bytes"
    );
    assert_eq!(
        fs::read(fixture.path("own-second.txt")).unwrap(),
        b"own-second.txt"
    );
    assert_eq!(
        fs::read(fixture.path("own-third.txt")).unwrap(),
        b"own-third.txt"
    );
    assert!(fixture.engine.outbox().unwrap().is_empty());

    assert!(fixture.engine.event(second.clone()).unwrap().is_empty());
    assert!(fixture.engine.event(third).unwrap().is_empty());
    assert_eq!(fixture.engine.cursor().unwrap(), applied_cursor);
    fixture.engine.ack_confirmed(fixture.ack(3)).unwrap();
    fixture.engine.ack_confirmed(fixture.ack(3)).unwrap();
    assert!(fixture.engine.event(second).unwrap().is_empty());
    let acknowledged = fixture.engine.cursor().unwrap();
    assert_eq!(acknowledged.last_ack_revision, WorkspaceRevision::new(3));
    assert_eq!(
        acknowledged.last_applied_revision,
        WorkspaceRevision::new(3)
    );
    assert_eq!(acknowledged.pending_ack_revision, None);
}

#[test]
fn older_correlated_ack_advances_without_clearing_newer_pending_ack() {
    let mut fixture = support::EngineFixture::new();

    fixture
        .engine
        .event(fixture.remote_mkdir_event(0, 1, "first"))
        .unwrap();
    let first_ack = fixture.ack(1);

    fixture
        .engine
        .event(fixture.remote_mkdir_event(1, 2, "second"))
        .unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(2))
    );

    fixture.engine.ack_confirmed(first_ack).unwrap();
    let after_first = fixture.engine.cursor().unwrap();
    assert_eq!(after_first.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(
        after_first.pending_ack_revision,
        Some(WorkspaceRevision::new(2))
    );
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![2]
    );

    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    let converged = fixture.engine.cursor().unwrap();
    assert_eq!(converged.last_ack_revision, WorkspaceRevision::new(2));
    assert_eq!(converged.pending_ack_revision, None);
}

#[test]
fn incremental_stream_advances_segment_ack_when_end_not_received() {
    // Reproduces a deadlock variant: when an incremental stream delivers every
    // expected event and they are all processed, but the SnapshotEnd frame is
    // lost (end_received=0), the already-applied events cannot be acknowledged.
    // last_ack stays at the pre-stream revision forever, so every reconnect
    // re-subscribes and re-applies the same events — a permanent stall.
    //
    // The fix lets the contiguous Applied prefix be acknowledged via a *segment*
    // ack that advances last_ack without clearing the active stream (the stream
    // is still waiting for End). This is safe because every expected event has
    // arrived and been fully processed — no blob download or deferred event is
    // in flight.
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(10))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(10))
        .unwrap();

    // Incremental stream: from=10, final=20, expects 5 events, 0 conflicts.
    // final_revision is intentionally higher than the last event to simulate a
    // stream that the server ended early or where End was lost mid-way.
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(10, 20, 5, 0))
        .unwrap();

    // Apply all 5 expected events (11-15). No blob required (mkdir).
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(0, 11, "dir-a"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(1, 12, "dir-b"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(2, 13, "dir-c"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(3, 14, "dir-d"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(4, 15, "dir-e"))
        .unwrap();

    // End has NOT been received — simulates a lost SnapshotEnd frame.
    assert!(
        !fixture
            .engine
            .state()
            .stream_state()
            .unwrap()
            .unwrap()
            .end_received
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(15)
    );

    // Before the fix: no SendAck would be produced and last_ack stayed at 10.
    // After the fix: a segment ack for revision 15 (all expected events
    // applied) is emitted.
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(
        support::ack_revisions(&commands),
        vec![15],
        "segment ack should be emitted when all expected events are applied"
    );

    // Confirming the segment ack advances last_ack to 15.
    fixture.engine.ack_confirmed(fixture.ack(15)).unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().last_ack_revision.get(),
        15,
        "segment ack should advance last_ack"
    );

    // The active stream must NOT be cleared — End has not arrived.
    assert!(
        fixture.engine.state().stream_state().unwrap().is_some(),
        "segment ack must not clear the active stream"
    );
    assert!(
        fixture
            .engine
            .cursor()
            .unwrap()
            .pending_ack_revision
            .is_none(),
        "segment ack must not set the terminal pending_ack"
    );

    // Delivering End should allow the terminal ack at the stream's final
    // revision to proceed normally after the segment ack.
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(20, 5, 0))
        .unwrap();

    let final_commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(
        support::ack_revisions(&final_commands),
        vec![20],
        "terminal ack should be emitted after stream completion"
    );
    fixture.engine.ack_confirmed(fixture.ack(20)).unwrap();
    let converged = fixture.engine.cursor().unwrap();
    assert_eq!(converged.last_ack_revision.get(), 20);
    assert_eq!(converged.pending_ack_revision, None);
    assert!(
        fixture.engine.state().stream_state().unwrap().is_none(),
        "terminal ack should clear the active stream"
    );
}

#[test]
fn incremental_stream_segment_ack_not_emitted_when_no_applied_prefix() {
    // When the stream has events that are still WaitingBlob (not yet Applied),
    // no segment ack should be emitted — there is no contiguous Applied prefix
    // to safely acknowledge.
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(10))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(10))
        .unwrap();

    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(10, 12, 2, 0))
        .unwrap();

    // This event needs a blob that is not yet available — it stalls in
    // WaitingBlob and last_applied does not advance.
    let commands = fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 11, "a.txt", b"server"))
        .unwrap();
    assert!(
        commands.iter().any(support::is_download),
        "blob-blocked event should request download"
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        10,
        "blob-blocked event must not advance last_applied"
    );

    // No segment ack — nothing Applied to acknowledge.
    let pending = fixture.engine.pending_commands(16).unwrap();
    assert!(
        support::ack_revisions(&pending).is_empty(),
        "no segment ack when no Applied prefix exists"
    );
}
