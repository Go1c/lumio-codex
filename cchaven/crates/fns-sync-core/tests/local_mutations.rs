mod support;

use std::fs;

use fns_fs::{FsChange, SyncRuleConfig};
use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictResolvedRequest,
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceFileMetadata,
    WorkspaceMutation, WorkspaceMutationAcceptedMessage, WorkspaceMutationKind,
    WorkspaceMutationRejectReason, WorkspaceMutationRejectedMessage, WorkspaceRevision,
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
        SyncCommand::ResolveConflict(_)
        | SyncCommand::UploadBlob { .. }
        | SyncCommand::DownloadBlob { .. }
        | SyncCommand::SendAck(_)
        | SyncCommand::ResolveConflict(_) => panic!("expected mutation"),
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
fn duplicate_acceptance_after_outbox_removal_binds_the_full_result() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("accepted-identity.txt", b"new");
    let mutation = fixture.record_all_changes()[0].mutation().unwrap();
    let accepted = accepted_for(&fixture, &mutation, 7);
    fixture.engine.mutation_accepted(accepted.clone()).unwrap();

    assert!(
        fixture
            .engine
            .mutation_accepted(accepted.clone())
            .unwrap()
            .is_empty()
    );
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_outbox = fixture.engine.outbox().unwrap();
    let before_receipt = fixture
        .engine
        .state()
        .applied_operation(fixture.engine.state().client_id(), mutation.operation_id)
        .unwrap();
    let before_state = fixture
        .engine
        .state()
        .path_state("accepted-identity.txt")
        .unwrap();
    let before_bytes = fs::read(fixture.path("accepted-identity.txt")).unwrap();

    let mut changed = accepted;
    changed.path_state.metadata.modified_at_ms += 1;
    changed.validate().unwrap();
    assert_eq!(
        fixture.engine.mutation_accepted(changed).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(fixture.engine.outbox().unwrap(), before_outbox);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(fixture.engine.state().client_id(), mutation.operation_id)
            .unwrap(),
        before_receipt
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("accepted-identity.txt")
            .unwrap(),
        before_state
    );
    assert_eq!(
        fs::read(fixture.path("accepted-identity.txt")).unwrap(),
        before_bytes
    );
}

#[test]
fn duplicate_rename_acceptance_binds_both_rename_states() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("accepted-old.txt", 5, b"old");
    fixture.rename("accepted-old.txt", "accepted-new.txt");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let old_state = support::path_state(
        "accepted-old.txt",
        7,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Tombstone,
    );
    let new_state = file_state("accepted-new.txt", 7, b"old");
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: mutation.operation_id,
        revision: WorkspaceRevision::new(7),
        path_state: new_state.clone(),
        old_path_state: Some(old_state),
        new_path_state: Some(new_state),
    };
    fixture.engine.mutation_accepted(accepted.clone()).unwrap();

    assert!(
        fixture
            .engine
            .mutation_accepted(accepted.clone())
            .unwrap()
            .is_empty()
    );
    let before_cursor = fixture.engine.cursor().unwrap();
    let before_outbox = fixture.engine.outbox().unwrap();
    let before_receipt = fixture
        .engine
        .state()
        .applied_operation(fixture.engine.state().client_id(), mutation.operation_id)
        .unwrap();
    let before_old = fixture
        .engine
        .state()
        .path_state("accepted-old.txt")
        .unwrap();
    let before_new = fixture
        .engine
        .state()
        .path_state("accepted-new.txt")
        .unwrap();

    let mut changed_old = accepted.clone();
    changed_old.old_path_state = Some(support::path_state(
        "accepted-old.txt",
        7,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Directory,
    ));
    changed_old.validate().unwrap();
    assert_eq!(
        fixture.engine.mutation_accepted(changed_old).unwrap_err(),
        SyncError::OperationChanged
    );

    let mut changed_new = accepted;
    changed_new.path_state.metadata.modified_at_ms += 1;
    changed_new.new_path_state = Some(changed_new.path_state.clone());
    changed_new.validate().unwrap();
    assert_eq!(
        fixture.engine.mutation_accepted(changed_new).unwrap_err(),
        SyncError::OperationChanged
    );

    assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
    assert_eq!(fixture.engine.outbox().unwrap(), before_outbox);
    assert_eq!(
        fixture
            .engine
            .state()
            .applied_operation(fixture.engine.state().client_id(), mutation.operation_id)
            .unwrap(),
        before_receipt
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("accepted-old.txt")
            .unwrap(),
        before_old
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("accepted-new.txt")
            .unwrap(),
        before_new
    );
}

#[test]
fn accepted_inverse_directory_rename_then_stale_duplicate_settles_both_operations() {
    let mut fixture = support::EngineFixture::new();
    let source = support::workspace_path("local-renamed");
    let target = support::workspace_path("local-old");
    let source_state = support::path_state(
        source.as_str(),
        5,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Directory,
    );
    let target_tombstone = support::path_state(
        target.as_str(),
        5,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Tombstone,
    );
    fixture
        .engine
        .state_mut()
        .put_path_state(&source_state)
        .unwrap();
    fixture
        .engine
        .state_mut()
        .put_path_state(&target_tombstone)
        .unwrap();

    let first = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(1450),
        path: source.clone(),
        base_path_revision: WorkspaceRevision::new(5),
        kind: WorkspaceMutationKind::Rename,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
        new_path: Some(target.clone()),
        target_base_path_revision: Some(WorkspaceRevision::new(5)),
    };
    let mut second = first.clone();
    second.operation_id = support::operation_id(1451);
    fixture.engine.state_mut().put_outbox(&first).unwrap();
    fixture.engine.state_mut().put_outbox(&second).unwrap();
    assert_eq!(fixture.engine.pending_commands(2).unwrap().len(), 2);
    assert!(
        fixture
            .engine
            .state()
            .outbox()
            .unwrap()
            .iter()
            .all(|record| record.stage == OutboxStage::Dispatched)
    );

    let old_state = support::path_state(
        source.as_str(),
        6,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Tombstone,
    );
    let new_state = support::path_state(
        target.as_str(),
        6,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Directory,
    );
    fixture
        .engine
        .mutation_accepted(WorkspaceMutationAcceptedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: first.operation_id,
            revision: WorkspaceRevision::new(6),
            path_state: new_state.clone(),
            old_path_state: Some(old_state.clone()),
            new_path_state: Some(new_state),
        })
        .unwrap();
    assert_eq!(fixture.engine.state().outbox().unwrap().len(), 1);

    fixture
        .engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: second.operation_id,
            reason: WorkspaceMutationRejectReason::StaleBaseRevision,
            current_path_state: RequiredNullable::Value(old_state),
            conflict_id: RequiredNullable::Null,
            required_hash: RequiredNullable::Null,
        })
        .unwrap();

    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(fixture.engine.pending_commands(2).unwrap().is_empty());
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
fn accepted_rename_receipt_matches_own_event_and_binds_old_path_state() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("old.txt", 5, b"old");
    fixture.rename("old.txt", "new.txt");
    fixture.engine.scan_and_record().unwrap();
    let mutation = fixture.engine.pending_commands(1).unwrap()[0]
        .mutation()
        .unwrap();
    let old_state = support::path_state(
        "old.txt",
        7,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Tombstone,
    );
    let new_state = file_state("new.txt", 7, b"old");
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: mutation.operation_id,
        revision: WorkspaceRevision::new(7),
        path_state: new_state.clone(),
        old_path_state: Some(old_state.clone()),
        new_path_state: Some(new_state.clone()),
    };
    fixture.engine.mutation_accepted(accepted).unwrap();

    let event = WorkspaceEventMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        stream_id: fixture.stream_id(),
        index: 0,
        revision: WorkspaceRevision::new(7),
        operation_id: mutation.operation_id,
        origin_client_id: fixture.engine.state().client_id(),
        mutation,
        path_state: new_state.clone(),
        old_path_state: Some(old_state),
        new_path_state: Some(new_state),
    };
    event.validate().unwrap();
    fixture.engine.event(event.clone()).unwrap();

    let mut changed = event;
    changed.old_path_state = Some(support::path_state(
        "old.txt",
        7,
        RequiredNullable::Null,
        support::zero_metadata(),
        WorkspaceEntryKind::Directory,
    ));
    changed.validate().unwrap();
    assert_eq!(
        fixture.engine.event(changed).unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn duplicate_acceptance_path_identity_changes_are_operation_changed_for_all_shapes() {
    #[derive(Clone, Copy, Debug)]
    enum Case {
        UpsertFile,
        Delete,
        Mkdir,
        UpsertSymlink,
        FileRename,
        DirectoryRename,
    }

    for (index, case) in [
        Case::UpsertFile,
        Case::Delete,
        Case::Mkdir,
        Case::UpsertSymlink,
        Case::FileRename,
        Case::DirectoryRename,
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = support::EngineFixture::new();
        let (kind, path, content_hash, metadata, new_path, target_base_path_revision) = match case {
            Case::UpsertFile => (
                fns_protocol::WorkspaceMutationKind::UpsertFile,
                "duplicate-file",
                RequiredNullable::Value(hash(b"file")),
                support::file_metadata(4),
                None,
                None,
            ),
            Case::Delete => (
                fns_protocol::WorkspaceMutationKind::Delete,
                "duplicate-delete",
                RequiredNullable::Null,
                support::zero_metadata(),
                None,
                None,
            ),
            Case::Mkdir => (
                fns_protocol::WorkspaceMutationKind::Mkdir,
                "duplicate-directory",
                RequiredNullable::Null,
                support::zero_metadata(),
                None,
                None,
            ),
            Case::UpsertSymlink => (
                fns_protocol::WorkspaceMutationKind::UpsertSymlink,
                "duplicate-symlink",
                RequiredNullable::Value(hash(b"target")),
                support::file_metadata(6),
                None,
                None,
            ),
            Case::FileRename => (
                fns_protocol::WorkspaceMutationKind::Rename,
                "duplicate-old-file",
                RequiredNullable::Value(hash(b"file")),
                support::file_metadata(4),
                Some(support::workspace_path("duplicate-new-file")),
                Some(WorkspaceRevision::ZERO),
            ),
            Case::DirectoryRename => (
                fns_protocol::WorkspaceMutationKind::Rename,
                "duplicate-old-directory",
                RequiredNullable::Null,
                support::zero_metadata(),
                Some(support::workspace_path("duplicate-new-directory")),
                Some(WorkspaceRevision::ZERO),
            ),
        };
        let mutation = WorkspaceMutation {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: support::operation_id(360 + index as u32),
            path: support::workspace_path(path),
            base_path_revision: WorkspaceRevision::ZERO,
            kind,
            content_hash,
            metadata,
            new_path,
            target_base_path_revision,
        };
        mutation.validate().unwrap();
        fixture
            .engine
            .state_mut()
            .enqueue_mutation(&mutation)
            .unwrap();
        let event = support::self_event_from_mutation(&fixture, 0, 7, mutation.clone());
        let accepted = WorkspaceMutationAcceptedMessage {
            workspace_id: event.workspace_id,
            client_id: event.origin_client_id,
            operation_id: event.operation_id,
            revision: event.revision,
            path_state: event.path_state,
            old_path_state: event.old_path_state,
            new_path_state: event.new_path_state,
        };
        accepted.validate().unwrap();
        fixture.engine.mutation_accepted(accepted.clone()).unwrap();
        assert!(
            fixture
                .engine
                .mutation_accepted(accepted.clone())
                .unwrap()
                .is_empty(),
            "exact duplicate did not no-op for {case:?}"
        );

        let before_cursor = fixture.engine.cursor().unwrap();
        let before_counts = fixture.engine.state().row_counts().unwrap();
        let before_outbox = fixture.engine.outbox().unwrap();
        let before_states = fixture.engine.state().path_states().unwrap();
        let before_receipts = fixture.engine.state().applied_operations().unwrap();
        let before_tree = fs::read_dir(fixture.workspace.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let mut changed_revision = accepted.clone();
        changed_revision.revision = WorkspaceRevision::new(8);
        changed_revision.path_state.path_revision = WorkspaceRevision::new(8);
        if let Some(old) = &mut changed_revision.old_path_state {
            old.path_revision = WorkspaceRevision::new(8);
        }
        if let Some(new) = &mut changed_revision.new_path_state {
            new.path_revision = WorkspaceRevision::new(8);
            changed_revision.path_state = new.clone();
        }
        changed_revision.validate().unwrap();
        assert_eq!(
            fixture
                .engine
                .mutation_accepted(changed_revision)
                .unwrap_err(),
            SyncError::OperationChanged,
            "changed revision was not OperationChanged for {case:?}"
        );

        let mut changed_path = accepted.clone();
        let changed_path_value = support::workspace_path(&format!("changed-path-{index}"));
        changed_path.path_state.path = changed_path_value.clone();
        if let Some(new) = &mut changed_path.new_path_state {
            new.path = changed_path_value;
            changed_path.path_state = new.clone();
        }
        changed_path.validate().unwrap();
        assert_eq!(
            fixture.engine.mutation_accepted(changed_path).unwrap_err(),
            SyncError::OperationChanged,
            "changed path identity was not OperationChanged for {case:?}"
        );

        let mut changed_result = accepted.clone();
        match changed_result.path_state.kind {
            WorkspaceEntryKind::File | WorkspaceEntryKind::Symlink => {
                changed_result.path_state.metadata.modified_at_ms += 1;
            }
            WorkspaceEntryKind::Directory => {
                changed_result.path_state.kind = WorkspaceEntryKind::Tombstone;
                changed_result.path_state.tombstone = true;
            }
            WorkspaceEntryKind::Tombstone => {
                changed_result.path_state.kind = WorkspaceEntryKind::Directory;
                changed_result.path_state.tombstone = false;
            }
        }
        if let Some(new) = &mut changed_result.new_path_state {
            *new = changed_result.path_state.clone();
        }
        changed_result.validate().unwrap();
        assert_eq!(
            fixture
                .engine
                .mutation_accepted(changed_result)
                .unwrap_err(),
            SyncError::OperationChanged,
            "changed result metadata or kind was not OperationChanged for {case:?}"
        );

        if accepted.old_path_state.is_some() {
            let mut changed_old = accepted;
            changed_old.old_path_state.as_mut().unwrap().path =
                support::workspace_path(&format!("changed-old-path-{index}"));
            changed_old.validate().unwrap();
            assert_eq!(
                fixture.engine.mutation_accepted(changed_old).unwrap_err(),
                SyncError::OperationChanged,
                "changed oldPathState.path was not OperationChanged for {case:?}"
            );
        }

        assert_eq!(fixture.engine.cursor().unwrap(), before_cursor);
        assert_eq!(fixture.engine.state().row_counts().unwrap(), before_counts);
        assert_eq!(fixture.engine.outbox().unwrap(), before_outbox);
        assert_eq!(fixture.engine.state().path_states().unwrap(), before_states);
        assert_eq!(
            fixture.engine.state().applied_operations().unwrap(),
            before_receipts
        );
        assert_eq!(
            fs::read_dir(fixture.workspace.path())
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len(),
            before_tree.len()
        );
    }
}

#[test]
fn first_time_acceptance_shape_mismatch_remains_a_protocol_error() {
    let mut fixture = support::EngineFixture::new();
    let mutation = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(390),
        path: support::workspace_path("first-time.txt"),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: fns_protocol::WorkspaceMutationKind::Mkdir,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
        new_path: None,
        target_base_path_revision: None,
    };
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&mutation)
        .unwrap();
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: mutation.workspace_id,
        client_id: mutation.client_id,
        operation_id: mutation.operation_id,
        revision: WorkspaceRevision::new(1),
        path_state: support::path_state(
            "unrelated.txt",
            1,
            RequiredNullable::Null,
            support::zero_metadata(),
            WorkspaceEntryKind::Directory,
        ),
        old_path_state: None,
        new_path_state: None,
    };
    assert_eq!(
        fixture.engine.mutation_accepted(accepted).unwrap_err(),
        SyncError::ProtocolInvariant {
            reason: "mutation_acceptance_path_mismatch"
        }
    );
    assert!(
        fixture
            .engine
            .state()
            .applied_operation(mutation.client_id, mutation.operation_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(mutation.operation_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn changed_duplicate_acceptance_identity_uses_the_durable_full_result() {
    let mut fixture = support::EngineFixture::new();
    let mutation = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(391),
        path: support::workspace_path("duplicate-identity.txt"),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: fns_protocol::WorkspaceMutationKind::Mkdir,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
        new_path: None,
        target_base_path_revision: None,
    };
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&mutation)
        .unwrap();
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: mutation.workspace_id,
        client_id: mutation.client_id,
        operation_id: mutation.operation_id,
        revision: WorkspaceRevision::new(1),
        path_state: support::path_state(
            mutation.path.as_str(),
            1,
            RequiredNullable::Null,
            support::zero_metadata(),
            WorkspaceEntryKind::Directory,
        ),
        old_path_state: None,
        new_path_state: None,
    };
    fixture.engine.mutation_accepted(accepted.clone()).unwrap();
    assert!(
        fixture
            .engine
            .mutation_accepted(accepted.clone())
            .unwrap()
            .is_empty()
    );

    let mut changed_workspace = accepted.clone();
    changed_workspace.workspace_id =
        fns_protocol::WorkspaceId::parse("20000000-0000-4000-8000-000000000001").unwrap();
    changed_workspace.validate().unwrap();
    assert_eq!(
        fixture
            .engine
            .mutation_accepted(changed_workspace)
            .unwrap_err(),
        SyncError::OperationChanged
    );

    let mut changed_client = accepted.clone();
    changed_client.client_id =
        fns_protocol::ClientId::parse("20000000-0000-4000-8000-000000000002").unwrap();
    changed_client.validate().unwrap();
    assert_eq!(
        fixture
            .engine
            .mutation_accepted(changed_client)
            .unwrap_err(),
        SyncError::OperationChanged
    );

    let mut changed_operation = accepted;
    changed_operation.operation_id = support::operation_id(392);
    changed_operation.validate().unwrap();
    assert_eq!(
        fixture
            .engine
            .mutation_accepted(changed_operation)
            .unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn accepted_and_own_event_share_canonical_identity_for_every_mutation_shape() {
    #[derive(Clone, Copy, Debug)]
    enum Case {
        UpsertFile,
        Delete,
        Mkdir,
        UpsertSymlink,
        FileRename,
        DirectoryRename,
    }

    for (index, case) in [
        Case::UpsertFile,
        Case::Delete,
        Case::Mkdir,
        Case::UpsertSymlink,
        Case::FileRename,
        Case::DirectoryRename,
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = support::EngineFixture::new();
        let (kind, path, content_hash, metadata, new_path, target_base_path_revision) = match case {
            Case::UpsertFile => (
                fns_protocol::WorkspaceMutationKind::UpsertFile,
                "identity-file",
                RequiredNullable::Value(hash(b"file")),
                support::file_metadata(4),
                None,
                None,
            ),
            Case::Delete => (
                fns_protocol::WorkspaceMutationKind::Delete,
                "identity-delete",
                RequiredNullable::Null,
                support::zero_metadata(),
                None,
                None,
            ),
            Case::Mkdir => (
                fns_protocol::WorkspaceMutationKind::Mkdir,
                "identity-directory",
                RequiredNullable::Null,
                support::zero_metadata(),
                None,
                None,
            ),
            Case::UpsertSymlink => (
                fns_protocol::WorkspaceMutationKind::UpsertSymlink,
                "identity-symlink",
                RequiredNullable::Value(hash(b"target")),
                support::file_metadata(6),
                None,
                None,
            ),
            Case::FileRename => (
                fns_protocol::WorkspaceMutationKind::Rename,
                "identity-old-file",
                RequiredNullable::Value(hash(b"file")),
                support::file_metadata(4),
                Some(support::workspace_path("identity-new-file")),
                Some(WorkspaceRevision::ZERO),
            ),
            Case::DirectoryRename => (
                fns_protocol::WorkspaceMutationKind::Rename,
                "identity-old-directory",
                RequiredNullable::Null,
                support::zero_metadata(),
                Some(support::workspace_path("identity-new-directory")),
                Some(WorkspaceRevision::ZERO),
            ),
        };
        let mutation = WorkspaceMutation {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: fixture.engine.state().client_id(),
            operation_id: support::operation_id(280 + index as u32),
            path: support::workspace_path(path),
            base_path_revision: WorkspaceRevision::ZERO,
            kind,
            content_hash,
            metadata,
            new_path,
            target_base_path_revision,
        };
        mutation.validate().unwrap();
        fixture
            .engine
            .state_mut()
            .enqueue_mutation(&mutation)
            .unwrap();
        let mut event = support::self_event_from_mutation(&fixture, 17, 7, mutation.clone());
        let accepted = WorkspaceMutationAcceptedMessage {
            workspace_id: event.workspace_id,
            client_id: event.origin_client_id,
            operation_id: event.operation_id,
            revision: event.revision,
            path_state: event.path_state.clone(),
            old_path_state: event.old_path_state.clone(),
            new_path_state: event.new_path_state.clone(),
        };
        fixture.engine.mutation_accepted(accepted).unwrap();

        event.stream_id =
            fns_protocol::StreamId::parse(&format!("10000000-0000-4000-8000-{:012}", 300 + index))
                .unwrap();
        event.index = 99;
        assert!(fixture.engine.event(event.clone()).unwrap().is_empty());
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

        let mut changed_post = event.clone();
        changed_post.path_state.metadata.modified_at_ms += 1;
        if changed_post.new_path_state.is_some() {
            changed_post.new_path_state = Some(changed_post.path_state.clone());
        }
        changed_post.validate().unwrap();
        assert_eq!(
            fixture.engine.event(changed_post).unwrap_err(),
            SyncError::OperationChanged,
            "changed pathState was accepted for {case:?}"
        );

        let mut changed_mutation = event.clone();
        changed_mutation.mutation.base_path_revision = WorkspaceRevision::new(1);
        changed_mutation.validate().unwrap();
        assert_eq!(
            fixture.engine.event(changed_mutation).unwrap_err(),
            SyncError::OperationChanged,
            "changed mutation was accepted for {case:?}"
        );

        if event.old_path_state.is_some() {
            let mut changed_old = event;
            changed_old.old_path_state.as_mut().unwrap().kind = WorkspaceEntryKind::Directory;
            changed_old.old_path_state.as_mut().unwrap().tombstone = false;
            changed_old.validate().unwrap();
            assert_eq!(
                fixture.engine.event(changed_old).unwrap_err(),
                SyncError::OperationChanged,
                "changed oldPathState was accepted for {case:?}"
            );
        }
    }
}

#[test]
fn conflict_resolution_rows_replay_as_resolution_commands_not_mutations() {
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

    assert_eq!(commands.len(), 2);
    assert!(commands.iter().any(
        |command| matches!(command, SyncCommand::ResolveConflict(body) if body == &resolution)
    ));
    assert!(commands.iter().any(|command| matches!(
        command,
        SyncCommand::Mutation(mutation)
            if mutation.path == support::workspace_path("mutation.txt")
    )));
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(resolution.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::Dispatched
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
fn completed_blob_replays_the_original_mutation() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("blob-retry.txt", b"blob");
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

    assert!(matches!(
        fixture.engine.pending_commands(1).unwrap().as_slice(),
        [SyncCommand::UploadBlob { .. }]
    ));

    fixture.engine.blob_uploaded(mutation.operation_id).unwrap();
    let replay = fixture.engine.pending_commands(1).unwrap();
    assert_eq!(replay, vec![SyncCommand::Mutation(mutation.clone())]);
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(mutation.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::Dispatched
    );
}

#[test]
fn connection_attempt_replays_awaiting_blob_as_original_mutation() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("blob-reconnect.txt", b"blob reconnect");
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

    assert_eq!(fixture.engine.prepare_connection_attempt().unwrap(), 1);
    assert_eq!(
        fixture.engine.pending_commands(1).unwrap(),
        vec![SyncCommand::Mutation(mutation.clone())]
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(mutation.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::Dispatched
    );
    assert_eq!(fixture.engine.prepare_connection_attempt().unwrap(), 0);
}

#[test]
fn reopened_engine_replays_awaiting_blob_as_original_mutation() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let workspace_id =
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000301").unwrap();
    let client_id = fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000302").unwrap();
    std::fs::write(workspace.path().join("restart.txt"), b"restart").unwrap();
    let config = fns_sync_core::SyncEngineConfig::new(
        workspace_id,
        client_id,
        workspace.path(),
        state.path(),
    );
    let mut engine = fns_sync_core::SyncEngine::open(config.clone()).unwrap();
    engine.scan_and_record().unwrap();
    let mutation = engine.pending_commands(1).unwrap()[0].mutation().unwrap();
    engine
        .mutation_rejected(WorkspaceMutationRejectedMessage {
            workspace_id,
            client_id,
            operation_id: mutation.operation_id,
            reason: WorkspaceMutationRejectReason::BlobRequired,
            current_path_state: RequiredNullable::Null,
            conflict_id: RequiredNullable::Null,
            required_hash: mutation.content_hash.clone(),
        })
        .unwrap();
    engine.close().unwrap();
    drop(engine);

    let mut reopened = fns_sync_core::SyncEngine::open(config).unwrap();
    assert_eq!(reopened.prepare_connection_attempt().unwrap(), 1);
    assert_eq!(
        reopened.pending_commands(1).unwrap(),
        vec![SyncCommand::Mutation(mutation.clone())]
    );
    assert_eq!(
        reopened
            .state()
            .outbox_entry(mutation.operation_id)
            .unwrap()
            .unwrap()
            .body_digest,
        fns_sync_core::body_digest(&fns_sync_core::canonical_json(&mutation).unwrap())
    );
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
fn nested_directory_rename_matches_the_whole_subtree() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_directory("old", 5);
    fixture.seed_remote_directory("old/two", 5);
    fixture.seed_remote_file("old/two/value.txt", 5, b"child");
    fixture.rename("old", "renamed-old");

    let changes = fixture.engine.scan_changes().unwrap();

    assert_eq!(
        changes,
        vec![FsChange::Rename {
            from: support::workspace_path("old"),
            to: support::workspace_path("renamed-old"),
        }]
    );
}

#[test]
fn rescan_does_not_pair_a_deleted_directory_tree_with_an_unrelated_new_child() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_directory("old", 5);
    fixture.seed_remote_directory("old/two", 5);
    fixture.seed_remote_file("old/two/value.txt", 5, b"child");
    fixture.seed_remote_directory("renamed-old", 6);

    fixture.remove("old");
    fs::create_dir(fixture.path("renamed-old/two")).unwrap();

    let changes = fixture.engine.scan_changes().unwrap();

    assert!(!changes.iter().any(|change| matches!(
        change,
        FsChange::Rename { from, to }
            if from == &support::workspace_path("old")
                && to == &support::workspace_path("renamed-old/two")
    )));
    assert!(changes.contains(&FsChange::Create(support::workspace_path(
        "renamed-old/two"
    ))));
    assert!(changes.contains(&FsChange::Delete(support::workspace_path("old"))));
}

#[test]
fn rescan_does_not_guess_between_two_identical_deleted_directory_trees() {
    let mut fixture = support::EngineFixture::new();
    for root in ["old-a", "old-b"] {
        fixture.seed_remote_directory(root, 5);
        fixture.seed_remote_directory(&format!("{root}/two"), 5);
        fixture.seed_remote_file(&format!("{root}/two/value.txt"), 5, b"same");
        fixture.remove(root);
    }
    fs::create_dir_all(fixture.path("new/two")).unwrap();
    fs::write(fixture.path("new/two/value.txt"), b"same").unwrap();

    let changes = fixture.engine.scan_changes().unwrap();

    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, FsChange::Rename { .. })),
        "ambiguous directory trees must remain create/delete changes: {changes:?}"
    );
    assert!(changes.contains(&FsChange::Create(support::workspace_path("new"))));
    assert!(changes.contains(&FsChange::Delete(support::workspace_path("old-a"))));
    assert!(changes.contains(&FsChange::Delete(support::workspace_path("old-b"))));
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

#[test]
fn configured_sync_rules_govern_full_scans() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("private")).unwrap();
    fs::create_dir_all(workspace.path().join("target")).unwrap();
    fs::write(workspace.path().join("visible.txt"), b"visible").unwrap();
    fs::write(workspace.path().join("private/hidden.txt"), b"hidden").unwrap();
    fs::write(workspace.path().join("target/keep.txt"), b"keep").unwrap();

    let rules = SyncRuleConfig {
        includes: vec!["target/keep.txt".into()],
        excludes: vec!["private/**".into()],
        protect_secrets: true,
    };
    let config = SyncEngineConfig::new(
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
        fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap(),
        workspace.path(),
        state.path(),
    )
    .with_sync_rules(rules);
    let mut engine = SyncEngine::open(config).unwrap();

    let changes = engine.scan_changes().unwrap();

    assert!(changes.contains(&FsChange::Create(support::workspace_path("visible.txt"))));
    assert!(changes.contains(&FsChange::Create(support::workspace_path(
        "target/keep.txt"
    ))));
    assert!(!changes.iter().any(|change| match change {
        FsChange::Create(path) | FsChange::Update(path) | FsChange::Delete(path) => {
            path.as_str().starts_with("private")
        }
        FsChange::Rename { from, to } => {
            from.as_str().starts_with("private") || to.as_str().starts_with("private")
        }
        FsChange::RescanRequired => false,
    }));
}
