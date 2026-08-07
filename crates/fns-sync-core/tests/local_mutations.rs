mod support;

use std::fs;

use fns_fs::FsChange;
use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictResolvedRequest,
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceFileMetadata,
    WorkspaceMutation, WorkspaceMutationAcceptedMessage, WorkspaceMutationRejectReason,
    WorkspaceMutationRejectedMessage, WorkspaceRevision,
};
use fns_sync_core::{OutboxStage, SyncCommand, SyncEngine, SyncEngineConfig, SyncError};

fn hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex())).unwrap()
}

fn file_state(path: &str, revision: u64, bytes: &[u8]) -> fns_protocol::WorkspacePathState {
    support::path_state(
        path,
        revision,
        RequiredNullable::Value(hash(bytes)),
        support::file_metadata(bytes.len() as u64),
        WorkspaceEntryKind::File,
    )
}

fn accepted_for(
    fixture: &support::EngineFixture,
    mutation: &WorkspaceMutation,
    revision: u64,
) -> WorkspaceMutationAcceptedMessage {
    let state = file_state(mutation.path.as_str(), revision, b"new");
    WorkspaceMutationAcceptedMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: mutation.operation_id,
        revision: WorkspaceRevision::new(revision),
        path_state: state,
        old_path_state: None,
        new_path_state: None,
    }
}

#[test]
fn disconnected_changes_reopen_as_exact_create_update_delete_and_rename_commands() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("create.txt", b"one");
    fixture.seed_remote_file("update.txt", 3, b"old");
    fixture.write("update.txt", b"new");
    fixture.seed_remote_file("delete.txt", 4, b"gone");
    fixture.remove("delete.txt");
    fixture.seed_remote_file("old.txt", 5, b"move");
    fixture.rename("old.txt", "new.txt");

    let expected = fixture.record_all_changes_and_close();
    let mut reopened = fixture.reopen();
    let actual = reopened.engine.pending_commands(16).unwrap();

    assert_eq!(actual, expected);
    assert_eq!(
        support::mutation_kinds(&actual),
        vec![
            fns_protocol::WorkspaceMutationKind::UpsertFile,
            fns_protocol::WorkspaceMutationKind::UpsertFile,
            fns_protocol::WorkspaceMutationKind::Delete,
            fns_protocol::WorkspaceMutationKind::Rename,
        ]
    );
    assert_eq!(support::base_revision(&actual[1]), 3);
    assert_eq!(support::base_revision(&actual[2]), 4);
    assert_eq!(support::rename_revisions(&actual[3]), (5, 0));
}

#[test]
fn duplicate_watcher_event_is_noop() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("same.txt", b"same");
    let path = support::workspace_path("same.txt");
    fixture
        .engine
        .record_local_changes([FsChange::Create(path.clone())])
        .unwrap();
    fixture
        .engine
        .record_local_changes([FsChange::Create(path)])
        .unwrap();

    assert_eq!(fixture.engine.state().row_counts().unwrap()["outbox"], 1);
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 0);
}

#[test]
fn queued_mutation_is_replaced_before_dispatch() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("replace.txt", b"one");
    let path = support::workspace_path("replace.txt");
    fixture
        .engine
        .record_local_changes([FsChange::Create(path.clone())])
        .unwrap();
    let first = fixture.engine.state().outbox().unwrap().pop().unwrap();
    fixture.write("replace.txt", b"two");
    fixture
        .engine
        .record_local_changes([FsChange::Update(path)])
        .unwrap();
    let second = fixture.engine.state().outbox().unwrap().pop().unwrap();

    assert_ne!(first.operation_id, second.operation_id);
    assert_eq!(first.stage, OutboxStage::Queued);
    assert_eq!(second.stage, OutboxStage::Queued);
    assert_eq!(
        second.mutation().unwrap().content_hash,
        RequiredNullable::Value(hash(b"two"))
    );
}

#[test]
fn change_after_dispatch_becomes_deferred_intent() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("deferred.txt", b"one");
    let path = support::workspace_path("deferred.txt");
    fixture
        .engine
        .record_local_changes([FsChange::Create(path.clone())])
        .unwrap();
    let dispatched = fixture.engine.pending_commands(1).unwrap();
    fixture.write("deferred.txt", b"two");
    fixture
        .engine
        .record_local_changes([FsChange::Update(path)])
        .unwrap();

    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].stage, OutboxStage::Dispatched);
    assert_eq!(fixture.engine.state().local_intents().unwrap().len(), 1);
    let dispatched_body = match &dispatched[0] {
        SyncCommand::Mutation(mutation) => fixture.engine.canonical_body(mutation).unwrap(),
        SyncCommand::UploadBlob { .. } => panic!("expected mutation"),
    };
    assert_eq!(outbox[0].body(), dispatched_body.as_slice());
}

#[test]
fn dispatched_rename_merges_deferred_intents_on_both_paths() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("old.txt", 5, b"old");
    fixture.rename("old.txt", "new.txt");
    fixture.engine.scan_and_record().unwrap();
    let rename = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();

    fixture.write("new.txt", b"newer");
    fixture.write("old.txt", b"recreated");
    fixture
        .engine
        .record_local_changes([
            FsChange::Update(support::workspace_path("new.txt")),
            FsChange::Create(support::workspace_path("old.txt")),
        ])
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .state()
            .local_intents()
            .unwrap()
            .into_iter()
            .map(|record| record.path)
            .collect::<Vec<_>>(),
        vec![
            support::workspace_path("new.txt"),
            support::workspace_path("old.txt"),
        ]
    );

    let old_state = support::path_state(
        "old.txt",
        7,
        RequiredNullable::Null,
        support::file_metadata(0),
        WorkspaceEntryKind::Tombstone,
    );
    let new_state = file_state("new.txt", 7, b"old");
    fixture
        .engine
        .mutation_accepted(WorkspaceMutationAcceptedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: rename.operation_id,
            revision: WorkspaceRevision::new(7),
            path_state: new_state.clone(),
            old_path_state: Some(old_state),
            new_path_state: Some(new_state),
        })
        .unwrap();

    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 2);
    assert!(
        outbox
            .iter()
            .all(|record| record.stage == OutboxStage::Queued)
    );
    assert_eq!(
        outbox
            .iter()
            .map(|record| record.mutation().unwrap().path)
            .collect::<Vec<_>>(),
        vec![
            support::workspace_path("new.txt"),
            support::workspace_path("old.txt"),
        ]
    );
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn chained_deferred_rename_compacts_a_changed_destination() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("a.txt", 5, b"old");
    fixture.rename("a.txt", "b.txt");
    fixture.engine.scan_and_record().unwrap();
    let first = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();

    fixture.rename("b.txt", "c.txt");
    fixture
        .engine
        .record_local_changes([FsChange::Rename {
            from: support::workspace_path("b.txt"),
            to: support::workspace_path("c.txt"),
        }])
        .unwrap();
    fixture.write("c.txt", b"final");
    fixture
        .engine
        .record_local_changes([FsChange::Update(support::workspace_path("c.txt"))])
        .unwrap();

    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].stage, OutboxStage::Dispatched);
    assert_eq!(
        fixture
            .engine
            .state()
            .local_intents()
            .unwrap()
            .into_iter()
            .map(|record| record.path)
            .collect::<Vec<_>>(),
        vec![
            support::workspace_path("b.txt"),
            support::workspace_path("c.txt"),
        ]
    );

    let old_state = support::path_state(
        "a.txt",
        7,
        RequiredNullable::Null,
        support::file_metadata(0),
        WorkspaceEntryKind::Tombstone,
    );
    let new_state = file_state("b.txt", 7, b"old");
    fixture
        .engine
        .mutation_accepted(WorkspaceMutationAcceptedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: first.operation_id,
            revision: WorkspaceRevision::new(7),
            path_state: new_state.clone(),
            old_path_state: Some(old_state),
            new_path_state: Some(new_state),
        })
        .unwrap();

    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].stage, OutboxStage::Queued);
    let next = outbox[0].mutation().unwrap();
    assert_eq!(next.kind, fns_protocol::WorkspaceMutationKind::Rename);
    assert_eq!(next.path, support::workspace_path("b.txt"));
    assert_eq!(next.new_path, Some(support::workspace_path("c.txt")));
    assert_eq!(next.content_hash, RequiredNullable::Value(hash(b"final")));
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn reconnect_replays_same_operation_and_body() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("replay.txt", b"body");
    let first = fixture.record_all_changes_and_close();
    let mut reopened = fixture.reopen();
    let replay = reopened.engine.pending_commands(16).unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(replay.len(), 1);
    assert_eq!(first[0].operation_id(), replay[0].operation_id());
    assert_eq!(
        first[0].body_bytes().unwrap(),
        replay[0].body_bytes().unwrap()
    );
}

#[test]
fn record_all_changes_and_close_closes_engine() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("closed.txt", b"closed");
    let commands = fixture.record_all_changes_and_close();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        fixture.engine.pending_commands(16),
        Err(SyncError::ProtocolInvariant {
            reason: "engine_closed"
        })
    ));
    assert_eq!(
        fixture.engine.state().row_counts().unwrap_err(),
        SyncError::StorageUnavailable
    );
}

#[test]
fn accepted_updates_path_and_removes_outbox_atomically() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("accepted.txt", b"new");
    let commands = fixture.record_all_changes();
    let mutation = commands[0].mutation().unwrap();
    fixture
        .engine
        .mutation_accepted(accepted_for(&fixture, &mutation, 7))
        .unwrap();

    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("accepted.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(7)
    );
    assert!(
        fixture
            .engine
            .mutation_accepted(accepted_for(&fixture, &mutation, 7))
            .is_ok()
    );
    assert_eq!(
        fixture
            .engine
            .mutation_accepted(accepted_for(&fixture, &mutation, 8))
            .unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn accepted_reconciles_deferred_state_at_the_accepted_revision() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("deferred-accepted.txt", 3, b"old");
    fixture.write("deferred-accepted.txt", b"new");
    fixture.engine.scan_and_record().unwrap();
    let first = fixture.engine.pending_commands(1).unwrap();
    let first_mutation = first[0].mutation().unwrap();
    fixture.write("deferred-accepted.txt", b"newer");
    fixture
        .engine
        .record_local_changes([FsChange::Update(support::workspace_path(
            "deferred-accepted.txt",
        ))])
        .unwrap();

    fixture
        .engine
        .mutation_accepted(WorkspaceMutationAcceptedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: first_mutation.operation_id,
            revision: WorkspaceRevision::new(7),
            path_state: file_state("deferred-accepted.txt", 7, b"new"),
            old_path_state: None,
            new_path_state: None,
        })
        .unwrap();

    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].stage, OutboxStage::Queued);
    let next = outbox[0].mutation().unwrap();
    assert_eq!(next.base_path_revision, WorkspaceRevision::new(7));
    assert_eq!(next.content_hash, RequiredNullable::Value(hash(b"newer")));
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn stale_revision_requeues_cached_local_postimage_without_writing_workspace_bytes() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("stale.txt", 3, b"old");
    fixture.write("stale.txt", b"local");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    fixture
        .engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: mutation.operation_id,
            reason: WorkspaceMutationRejectReason::StaleBaseRevision,
            current_path_state: RequiredNullable::Value(file_state("stale.txt", 9, b"remote")),
            conflict_id: RequiredNullable::Null,
            required_hash: RequiredNullable::Null,
        })
        .unwrap();

    assert_eq!(fs::read(fixture.path("stale.txt")).unwrap(), b"local");
    let outbox = fixture.engine.state().outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].stage, OutboxStage::Queued);
    let next = outbox[0].mutation().unwrap();
    assert_eq!(next.base_path_revision, WorkspaceRevision::new(9));
    assert_eq!(next.content_hash, RequiredNullable::Value(hash(b"local")));
}

#[test]
fn self_event_before_response_settles_same_outbox() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("self.txt", b"new");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let state = file_state("self.txt", 7, b"new");
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000201").unwrap();
    let event = WorkspaceEventMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        stream_id,
        index: 0,
        revision: WorkspaceRevision::new(7),
        operation_id: mutation.operation_id,
        origin_client_id: fixture.engine.state().client_id(),
        mutation: mutation.clone(),
        path_state: state,
        old_path_state: None,
        new_path_state: None,
    };
    fixture.engine.event(event.clone()).unwrap();
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    fixture.engine.event(event.clone()).unwrap();

    let mut changed_revision = event.clone();
    changed_revision.revision = WorkspaceRevision::new(8);
    changed_revision.path_state.path_revision = WorkspaceRevision::new(8);
    assert_eq!(
        fixture.engine.event(changed_revision).unwrap_err(),
        SyncError::OperationChanged
    );

    let mut changed_body = event;
    changed_body.mutation.path = support::workspace_path("different.txt");
    assert_eq!(
        fixture.engine.event(changed_body).unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn conflict_resolution_rows_are_not_mutation_commands_or_replayed() {
    let mut fixture = support::EngineFixture::new();
    let resolution = WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: fns_protocol::OperationId::parse("10000000-0000-4000-8000-000000000200")
            .unwrap(),
        conflict_id: fns_protocol::ConflictId::parse("10000000-0000-4000-8000-000000000201")
            .unwrap(),
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        choice: WorkspaceConflictChoice::Current,
        path: support::workspace_path("conflict.txt"),
        content_hash: RequiredNullable::Null,
        metadata: support::file_metadata(0),
    };
    fixture
        .engine
        .state_mut()
        .enqueue_conflict_resolution(&resolution)
        .unwrap();
    fixture.write("mutation.txt", b"mutation");
    let commands = fixture
        .engine
        .scan_and_record()
        .and_then(|_| fixture.engine.pending_commands(16))
        .unwrap();

    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].mutation().unwrap().path,
        support::workspace_path("mutation.txt")
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(resolution.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::Queued
    );
}

#[test]
fn blob_required_emits_one_upload() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("blob.txt", b"blob");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    fixture
        .engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: mutation.operation_id,
            reason: WorkspaceMutationRejectReason::BlobRequired,
            current_path_state: RequiredNullable::Null,
            conflict_id: RequiredNullable::Null,
            required_hash: mutation.content_hash.clone(),
        })
        .unwrap();
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(commands.len(), 1);
    assert!(matches!(commands[0], SyncCommand::UploadBlob { .. }));
}

#[test]
fn conflict_created_blocks_mutation() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("conflict.txt", b"conflict");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let conflict_id =
        fns_protocol::ConflictId::parse("10000000-0000-4000-8000-000000000202").unwrap();
    fixture
        .engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: mutation.operation_id,
            reason: WorkspaceMutationRejectReason::ConflictCreated,
            current_path_state: RequiredNullable::Null,
            conflict_id: RequiredNullable::Value(conflict_id),
            required_hash: RequiredNullable::Null,
        })
        .unwrap();
    assert_eq!(
        fixture.engine.state().outbox().unwrap()[0].stage,
        OutboxStage::BlockedConflict
    );
    assert!(fixture.engine.pending_commands(16).unwrap().is_empty());
}

#[test]
fn operation_reused_is_protocol_invariant() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("reused.txt", b"reused");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let error = fixture
        .engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: mutation.operation_id,
            reason: WorkspaceMutationRejectReason::OperationReused,
            current_path_state: RequiredNullable::Null,
            conflict_id: RequiredNullable::Null,
            required_hash: RequiredNullable::Null,
        })
        .unwrap_err();
    assert!(matches!(error, SyncError::ProtocolInvariant { .. }));
}

#[test]
fn directory_rename_uses_null_hash_and_zero_metadata() {
    let mut fixture = support::EngineFixture::new();
    fs::create_dir(fixture.path("old")).unwrap();
    fixture.rename("old", "new");
    fixture
        .engine
        .record_local_changes([FsChange::Rename {
            from: support::workspace_path("old"),
            to: support::workspace_path("new"),
        }])
        .unwrap();
    let command = fixture.engine.pending_commands(1).unwrap().pop().unwrap();
    let mutation = command.mutation().unwrap();
    assert_eq!(mutation.kind, fns_protocol::WorkspaceMutationKind::Rename);
    assert_eq!(mutation.content_hash, RequiredNullable::Null);
    assert_eq!(
        mutation.metadata,
        WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false
        }
    );
}

#[test]
fn populated_directory_rename_suppresses_descendant_pairs() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("old/file.txt", 5, b"child");
    fixture
        .engine
        .state_mut()
        .put_path_state(&support::path_state(
            "old",
            5,
            RequiredNullable::Null,
            support::file_metadata(0),
            WorkspaceEntryKind::Directory,
        ))
        .unwrap();
    fixture.rename("old", "new");

    let changes = fixture.engine.scan_changes().unwrap();
    assert_eq!(
        changes,
        vec![FsChange::Rename {
            from: support::workspace_path("old"),
            to: support::workspace_path("new"),
        }]
    );
    fixture.engine.record_local_changes(changes).unwrap();
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].mutation().unwrap().new_path,
        Some(support::workspace_path("new"))
    );
}

#[test]
fn state_and_workspace_roots_must_not_overlap() {
    let workspace = tempfile::tempdir().unwrap();
    let state = workspace.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let config = SyncEngineConfig::new(
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
        fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap(),
        workspace.path(),
        &state,
    );
    assert!(matches!(
        SyncEngine::open(config),
        Err(SyncError::InvalidConfiguration { .. })
    ));
}

#[test]
fn nonexistent_nested_state_root_is_rejected_without_workspace_residue() {
    let workspace = tempfile::tempdir().unwrap();
    let state = workspace.path().join("nested").join("state");
    let config = SyncEngineConfig::new(
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
        fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap(),
        workspace.path(),
        &state,
    );
    assert!(matches!(
        SyncEngine::open(config),
        Err(SyncError::InvalidConfiguration { .. })
    ));
    assert!(!state.exists());
    assert!(!workspace.path().join("nested").exists());
}

#[cfg(unix)]
#[test]
fn incomplete_workspace_scan_never_emits_deletes() {
    use std::os::unix::fs::symlink;

    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("remote.txt", 3, b"remote");
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("hidden.txt"), b"hidden").unwrap();
    symlink(outside.path(), fixture.path("unsafe")).unwrap();

    assert_eq!(
        fixture.engine.scan_and_record().unwrap_err(),
        SyncError::ScanIncomplete
    );
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
}
