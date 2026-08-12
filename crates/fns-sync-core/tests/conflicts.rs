mod support;

use std::fs;

use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest,
    WorkspaceEntryKind, WorkspaceMutation, WorkspaceMutationKind, WorkspaceRevision,
    WorkspaceV2ErrorCode,
};
use fns_sync_core::{ConflictStatus, OutboxStage, StreamItemStatus, SyncCommand, SyncError};

fn current_resolution(
    fixture: &support::EngineFixture,
    created: &WorkspaceConflictCreatedMessage,
    operation: u32,
) -> WorkspaceConflictResolvedRequest {
    WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(operation),
        conflict_id: created.conflict_id,
        conflict_revision: created.conflict_revision,
        choice: WorkspaceConflictChoice::Current,
        path: created
            .current
            .path
            .clone()
            .into_option()
            .expect("current conflict path"),
        content_hash: created.current.content_hash.clone(),
        metadata: created.current.metadata.clone(),
    }
}

fn merged_resolution(
    fixture: &support::EngineFixture,
    created: &WorkspaceConflictCreatedMessage,
    operation: u32,
    bytes: &[u8],
) -> WorkspaceConflictResolvedRequest {
    WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(operation),
        conflict_id: created.conflict_id,
        conflict_revision: created.conflict_revision,
        choice: WorkspaceConflictChoice::Merged,
        path: created.path.clone(),
        content_hash: RequiredNullable::Value(support::hash(bytes)),
        metadata: support::file_metadata(bytes.len() as u64),
    }
}

fn resolved_from_request(
    request: &WorkspaceConflictResolvedRequest,
    revision: u64,
) -> WorkspaceConflictResolvedMessage {
    WorkspaceConflictResolvedMessage {
        workspace_id: request.workspace_id,
        conflict_id: request.conflict_id,
        conflict_revision: request.conflict_revision,
        operation_id: request.operation_id,
        revision: WorkspaceRevision::new(revision),
        choice: request.choice,
        path_state: fns_protocol::WorkspacePathState {
            path: request.path.clone(),
            path_revision: WorkspaceRevision::new(revision),
            kind: if request.choice == WorkspaceConflictChoice::Delete {
                WorkspaceEntryKind::Tombstone
            } else {
                WorkspaceEntryKind::File
            },
            content_hash: request.content_hash.clone(),
            metadata: request.metadata.clone(),
            tombstone: request.choice == WorkspaceConflictChoice::Delete,
        },
        resolved_by_client_id: request.client_id,
    }
}

fn blocked_origin_mutation(
    fixture: &support::EngineFixture,
    created: &WorkspaceConflictCreatedMessage,
) -> WorkspaceMutation {
    WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: created.created_by_operation_id,
        path: created.path.clone(),
        base_path_revision: created.ancestor.path_revision,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: created.incoming.content_hash.clone(),
        metadata: created.incoming.metadata.clone(),
        new_path: None,
        target_base_path_revision: None,
    }
}

fn only_resolution(commands: Vec<SyncCommand>) -> WorkspaceConflictResolvedRequest {
    assert_eq!(commands.len(), 1, "one durable resolution command");
    match commands.into_iter().next().unwrap() {
        SyncCommand::ResolveConflict(request) => request,
        other => panic!("expected conflict resolution, got {other:?}"),
    }
}

#[test]
fn live_conflict_created_persists_without_stream_and_does_not_advance_tree_revision() {
    let mut fixture = support::EngineFixture::new();
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000031",
        "1",
        "conflict.txt",
    );
    let before = fixture.engine.cursor().unwrap();

    assert!(
        fixture
            .engine
            .conflict_created(created.clone())
            .unwrap()
            .is_empty()
    );
    let stored = fixture
        .engine
        .state()
        .conflict(created.conflict_id)
        .unwrap()
        .expect("durable live conflict");
    assert_eq!(stored.status, ConflictStatus::Manual);
    assert_eq!(
        stored.created_json,
        fns_sync_core::canonical_json(&created).unwrap()
    );
    assert_eq!(fixture.engine.cursor().unwrap(), before);
    assert!(fixture.engine.state().stream_state().unwrap().is_none());

    assert!(
        fixture
            .engine
            .conflict_created(created.clone())
            .unwrap()
            .is_empty()
    );
    let row_counts = fixture.engine.state().row_counts().unwrap();
    let mut changed = created;
    changed.incoming.metadata.modified_at_ms += 1;
    changed.validate().unwrap();
    assert_eq!(
        fixture.engine.conflict_created(changed).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), row_counts);
}

#[test]
fn live_new_generation_replaces_manual_or_resolving_stale_generation() {
    for resolving in [false, true] {
        let mut fixture = support::EngineFixture::new();
        let old = fixture.remote_conflict_created(
            "10000000-0000-4000-8000-000000000042",
            "1",
            "refreshed-live.txt",
        );
        fixture.engine.conflict_created(old.clone()).unwrap();
        let blocked = blocked_origin_mutation(&fixture, &old);
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

        let stale_resolution = resolving.then(|| {
            let request = current_resolution(&fixture, &old, 270);
            fixture
                .engine
                .queue_conflict_resolution(request.clone())
                .unwrap();
            request
        });
        let mut refreshed = old.clone();
        refreshed.conflict_revision = support::conflict_revision("2");
        refreshed.current.path_revision = WorkspaceRevision::new(9);
        refreshed.current.metadata.modified_at_ms += 1;
        refreshed.validate().unwrap();

        fixture.engine.conflict_created(refreshed.clone()).unwrap();
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
        if let Some(request) = stale_resolution {
            assert!(
                fixture
                    .engine
                    .state()
                    .outbox_entry(request.operation_id)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(
            fixture
                .engine
                .state()
                .outbox_entry(blocked.operation_id)
                .unwrap()
                .expect("blocked origin retained")
                .stage,
            OutboxStage::BlockedConflict
        );

        let before = fixture.engine.state().row_counts().unwrap();
        let mut changed_same_generation = refreshed;
        changed_same_generation.incoming.metadata.modified_at_ms += 1;
        changed_same_generation.validate().unwrap();
        assert_eq!(
            fixture
                .engine
                .conflict_created(changed_same_generation)
                .unwrap_err(),
            SyncError::OperationChanged
        );
        assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    }
}

#[test]
fn resolution_outbox_replays_exact_request_after_engine_reopen() {
    let mut fixture = support::EngineFixture::new();
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000032", "4", "replay.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    let request = current_resolution(&fixture, &created, 250);

    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let conflict = fixture
        .engine
        .state()
        .conflict(created.conflict_id)
        .unwrap()
        .unwrap();
    assert_eq!(conflict.status, ConflictStatus::Resolving);
    assert_eq!(
        conflict.resolution_json.as_deref(),
        Some(fns_sync_core::canonical_json(&request).unwrap().as_slice())
    );
    assert_eq!(
        only_resolution(fixture.engine.pending_commands(1).unwrap()),
        request
    );

    let mut fixture = fixture.reopen();
    assert_eq!(
        only_resolution(fixture.engine.pending_commands(1).unwrap()),
        request
    );
    let outbox = fixture
        .engine
        .state()
        .outbox_entry(request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(outbox.stage, OutboxStage::Dispatched);
    assert_eq!(
        outbox.body_json,
        fns_sync_core::canonical_json(&request).unwrap()
    );

    let mut changed = request.clone();
    changed.operation_id = support::operation_id(251);
    changed.choice = WorkspaceConflictChoice::Incoming;
    changed.path = created.incoming.path.clone().into_option().unwrap();
    changed.content_hash = created.incoming.content_hash.clone();
    changed.metadata = created.incoming.metadata.clone();
    assert_eq!(
        fixture
            .engine
            .queue_conflict_resolution(changed)
            .unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn blob_required_resolution_stays_durable_and_replays_without_ephemeral_need() {
    let mut fixture = support::EngineFixture::new();
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000033", "1", "merged.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    let merged = b"merged candidate";
    fixture
        .engine
        .stage_bytes(&support::hash(merged), merged)
        .unwrap();
    let request = merged_resolution(&fixture, &created, 252, merged);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    assert_eq!(
        only_resolution(fixture.engine.pending_commands(1).unwrap()),
        request
    );

    let commands = fixture
        .engine
        .conflict_resolution_rejected(request.operation_id, WorkspaceV2ErrorCode::BlobRequired)
        .unwrap();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        SyncCommand::UploadBlob {
            operation_id,
            content_hash,
            size,
            ..
        } if *operation_id == request.operation_id
            && Some(content_hash.clone()) == request.content_hash.clone().into_option()
            && *size == merged.len() as u64
    ));
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::AwaitingBlob
    );

    assert_eq!(fixture.engine.prepare_connection_attempt().unwrap(), 1);
    assert_eq!(
        only_resolution(fixture.engine.pending_commands(1).unwrap()),
        request
    );
}

#[test]
fn stale_resolution_requires_refresh_and_never_reuses_old_operation() {
    let mut fixture = support::EngineFixture::new();
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000034", "1", "stale.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    let old = current_resolution(&fixture, &created, 253);
    fixture
        .engine
        .queue_conflict_resolution(old.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    fixture
        .engine
        .conflict_resolution_rejected(
            old.operation_id,
            WorkspaceV2ErrorCode::ConflictRevisionStale,
        )
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(old.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .unwrap()
            .unwrap()
            .status,
        ConflictStatus::RefreshRequired
    );
    assert!(fixture.engine.pending_commands(16).unwrap().is_empty());

    let mut refreshed = created.clone();
    refreshed.conflict_revision = support::conflict_revision("2");
    refreshed.incoming.metadata.modified_at_ms += 1;
    fixture.engine.conflict_created(refreshed.clone()).unwrap();
    let fresh = current_resolution(&fixture, &refreshed, 254);
    fixture
        .engine
        .queue_conflict_resolution(fresh.clone())
        .unwrap();
    assert_eq!(
        only_resolution(fixture.engine.pending_commands(1).unwrap()),
        fresh
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(old.operation_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn live_resolved_after_response_is_the_only_apply_and_clears_durable_conflict_state() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("resolved.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000035",
        "1",
        "resolved.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &created);
    fixture
        .engine
        .state_mut()
        .enqueue_mutation(&blocked)
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_outbox_stage(
            created.created_by_operation_id,
            OutboxStage::BlockedConflict,
        )
        .unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let request = current_resolution(&fixture, &created, 255);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();
    let resolved = resolved_from_request(&request, 1);

    fixture
        .engine
        .conflict_resolution_accepted(resolved.clone())
        .unwrap();
    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"base");
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );
    assert!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_none()
    );

    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"current");
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        1
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );
    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());

    let before = fixture.engine.state().row_counts().unwrap();
    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    let mut changed = resolved;
    changed.choice = WorkspaceConflictChoice::Incoming;
    assert_eq!(
        fixture.engine.conflict_resolved(changed).unwrap_err(),
        SyncError::OperationChanged
    );
}

#[test]
fn current_resolution_rejects_authoritative_tombstone_body_change() {
    let mut fixture = support::EngineFixture::new();
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000041",
        "1",
        "deleted-current.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    let request = current_resolution(&fixture, &created, 262);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();
    let mut resolved = resolved_from_request(&request, 1);
    resolved.path_state.kind = WorkspaceEntryKind::Tombstone;
    resolved.path_state.content_hash = RequiredNullable::Null;
    resolved.path_state.metadata = support::zero_metadata();
    resolved.path_state.tombstone = true;
    resolved.validate().unwrap();

    assert_eq!(
        fixture
            .engine
            .conflict_resolution_accepted(resolved)
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn rename_current_accepts_authoritative_tombstone_after_source_deleted() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("rename-source.txt", 0, b"base");
    let mut created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000052",
        "1",
        "rename-source.txt",
    );
    created.kind = WorkspaceConflictKind::Rename;
    created.incoming.path = RequiredNullable::Value(support::workspace_path("rename-target.txt"));
    created.validate().unwrap();
    fixture.engine.conflict_created(created.clone()).unwrap();
    let request = current_resolution(&fixture, &created, 282);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let mut resolved = resolved_from_request(&request, 1);
    resolved.path_state.kind = WorkspaceEntryKind::Tombstone;
    resolved.path_state.content_hash = RequiredNullable::Null;
    resolved.path_state.metadata = support::zero_metadata();
    resolved.path_state.tombstone = true;
    resolved.validate().unwrap();

    fixture
        .engine
        .conflict_resolution_accepted(resolved.clone())
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_none()
    );
    fixture.engine.conflict_resolved(resolved).unwrap();
    assert!(!fixture.path("rename-source.txt").exists());
    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );
}

#[test]
fn live_revision_queue_does_not_bypass_blob_blocked_event_with_conflict_resolution() {
    let mut fixture = support::EngineFixture::new();
    let event = fixture.remote_update_event(0, 1, "first.txt", b"first");
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000036", "1", "second.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let request = current_resolution(&fixture, &created, 256);
    let mut resolved = resolved_from_request(&request, 2);
    resolved.resolved_by_client_id = support::remote_client_id();

    assert!(
        fixture
            .engine
            .event(event.clone())
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(
        fixture
            .engine
            .conflict_resolved(resolved)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(!fixture.path("first.txt").exists());
    assert!(!fixture.path("second.txt").exists());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );

    fixture
        .engine
        .blob_available(
            support::hash(b"first"),
            b"first".len() as u64,
            std::io::Cursor::new(b"first"),
        )
        .unwrap();
    let _ = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(fs::read(fixture.path("first.txt")).unwrap(), b"first");
    assert_eq!(fs::read(fixture.path("second.txt")).unwrap(), b"current");
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        2
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(2))
    );
}

#[test]
fn exact_conflict_receipt_waits_behind_blob_blocked_live_revision() {
    let mut fixture = support::EngineFixture::new();
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000038",
        "1",
        "resolved.txt",
    );
    let request = current_resolution(&fixture, &created, 257);
    let resolved = resolved_from_request(&request, 2);
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let body_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&resolved).expect("canonical resolved message"),
    );
    fixture
        .engine
        .state_mut()
        .transaction(|tx| {
            tx.record_conflict_applied_operation(
                resolved.resolved_by_client_id,
                resolved.operation_id,
                resolved.revision,
                body_digest,
                None,
            )
        })
        .unwrap();

    let blocked = fixture.remote_update_event(0, 1, "blocked.bin", b"blocked");
    assert!(
        fixture
            .engine
            .event(blocked)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
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
            support::hash(b"blocked"),
            b"blocked".len() as u64,
            std::io::Cursor::new(b"blocked"),
        )
        .unwrap();
    let applied_cursor = fixture.engine.cursor().unwrap();
    assert_eq!(
        applied_cursor.last_applied_revision,
        WorkspaceRevision::new(2)
    );
    assert_eq!(
        applied_cursor.pending_ack_revision,
        Some(WorkspaceRevision::new(2))
    );
    assert_eq!(fs::read(fixture.path("blocked.bin")).unwrap(), b"blocked");

    assert!(
        fixture
            .engine
            .conflict_resolved(resolved.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.engine.cursor().unwrap(), applied_cursor);
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    fixture.engine.ack_confirmed(fixture.ack(2)).unwrap();
    assert!(
        fixture
            .engine
            .conflict_resolved(resolved)
            .unwrap()
            .is_empty()
    );
    let acknowledged = fixture.engine.cursor().unwrap();
    assert_eq!(acknowledged.last_ack_revision, WorkspaceRevision::new(2));
    assert_eq!(acknowledged.pending_ack_revision, None);
}

#[test]
fn local_unwitnessed_resolution_is_rejected_before_filesystem_apply() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("unwitnessed.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000048",
        "1",
        "unwitnessed.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let request = current_resolution(&fixture, &created, 276);
    let before = fixture.engine.state().row_counts().unwrap();

    assert_eq!(
        fixture
            .engine
            .conflict_resolved(resolved_from_request(&request, 1))
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(fs::read(fixture.path("unwitnessed.txt")).unwrap(), b"base");
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
}

#[test]
fn resolution_kind_change_is_rejected_before_filesystem_apply() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("kind-change.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000049",
        "1",
        "kind-change.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    let request = current_resolution(&fixture, &created, 277);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let mut changed = resolved_from_request(&request, 1);
    changed.path_state.kind = WorkspaceEntryKind::Symlink;
    changed.validate().unwrap();

    assert_eq!(
        fixture.engine.conflict_resolved(changed).unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fs::read(fixture.path("kind-change.txt")).unwrap(), b"base");
    assert!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_some()
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
}

#[test]
fn exact_old_generation_replay_preserves_new_generation_conflict() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("new-generation-active.txt", 0, b"base");
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000050",
        "1",
        "new-generation-active.txt",
    );
    let request = current_resolution(&fixture, &old, 278);
    let resolved = resolved_from_request(&request, 1);
    let body_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&resolved).expect("canonical resolved message"),
    );
    fixture
        .engine
        .state_mut()
        .transaction(|tx| {
            tx.record_conflict_applied_operation(
                resolved.resolved_by_client_id,
                resolved.operation_id,
                resolved.revision,
                body_digest,
                None,
            )
        })
        .unwrap();
    let mut active = old;
    active.conflict_revision = support::conflict_revision("2");
    active.created_by_operation_id = support::operation_id(279);
    active.current.path_revision = WorkspaceRevision::new(2);
    active.validate().unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();

    fixture.engine.conflict_resolved(resolved).unwrap();

    let retained = fixture
        .engine
        .state()
        .conflict(active.conflict_id)
        .unwrap()
        .expect("new generation retained");
    assert_eq!(retained.conflict_revision, active.conflict_revision);
    assert_eq!(
        retained.created_json,
        fns_sync_core::canonical_json(&active).unwrap()
    );
    assert_eq!(
        fs::read(fixture.path("new-generation-active.txt")).unwrap(),
        b"base"
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn stale_live_resolution_preserves_newer_generation_and_blocked_origin() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("stale-live-generation.txt", 5, b"current generation");
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(4))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(4))
        .unwrap();

    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000058",
        "1",
        "stale-live-generation.txt",
    );
    let mut active = old.clone();
    active.conflict_revision = support::conflict_revision("2");
    active.created_by_operation_id = support::operation_id(286);
    active.current.path_revision = WorkspaceRevision::new(5);
    active.validate().unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &active);
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

    let stale_request = WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: support::remote_client_id(),
        operation_id: support::operation_id(287),
        conflict_id: old.conflict_id,
        conflict_revision: old.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: old.path,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    let stale = resolved_from_request(&stale_request, 5);

    fixture.engine.conflict_resolved(stale.clone()).unwrap();
    fixture.engine.conflict_resolved(stale).unwrap();

    let retained = fixture
        .engine
        .state()
        .conflict(active.conflict_id)
        .unwrap()
        .expect("newer generation retained");
    assert_eq!(retained.conflict_revision, active.conflict_revision);
    assert_eq!(
        retained.created_json,
        fns_sync_core::canonical_json(&active).unwrap()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .expect("newer blocked origin retained")
            .stage,
        OutboxStage::BlockedConflict
    );
    assert_eq!(
        fs::read(fixture.path("stale-live-generation.txt")).unwrap(),
        b"current generation"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("stale-live-generation.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(5)
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(5)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(5))
    );
}

#[test]
fn stale_stream_resolution_after_reopen_preserves_authoritative_newer_generation() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("stale-stream-generation.txt", 5, b"authoritative current");
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(4))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(4))
        .unwrap();
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000059",
        "1",
        "stale-stream-generation.txt",
    );
    let mut active = old.clone();
    active.conflict_revision = support::conflict_revision("2");
    active.created_by_operation_id = support::operation_id(288);
    active.current.path_revision = WorkspaceRevision::new(5);
    active.validate().unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &active);
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

    let stale_request = WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: support::remote_client_id(),
        operation_id: support::operation_id(289),
        conflict_id: old.conflict_id,
        conflict_revision: old.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: old.path,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    let stale = resolved_from_request(&stale_request, 5);
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(4, 5, 1, 1))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .put_stream_conflict_resolved(&stale, None, StreamItemStatus::Ready)
        .unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();

    let mut fixture = fixture.reopen();
    fixture.engine.pending_commands(16).unwrap();

    let retained = fixture
        .engine
        .state()
        .conflict(active.conflict_id)
        .unwrap()
        .expect("newer generation retained after replay");
    assert_eq!(retained.conflict_revision, active.conflict_revision);
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .expect("newer blocked origin retained after replay")
            .stage,
        OutboxStage::BlockedConflict
    );
    assert_eq!(
        fs::read(fixture.path("stale-stream-generation.txt")).unwrap(),
        b"authoritative current"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("stale-stream-generation.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(5)
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(4)
    );
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);

    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 1, 1))
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .conflict(active.conflict_id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(5)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(5))
    );
}

#[test]
fn late_local_resolution_after_live_generation_replacement_is_superseded() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("late-local-live.txt", 7, b"newer generation");
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(6))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(6))
        .unwrap();
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000060",
        "1",
        "late-local-live.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &old);
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
    let request = WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(290),
        conflict_id: old.conflict_id,
        conflict_revision: old.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: old.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    let mut active = old.clone();
    active.conflict_revision = support::conflict_revision("2");
    active.current.path_revision = WorkspaceRevision::new(7);
    active.validate().unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();
    let active_request = WorkspaceConflictResolvedRequest {
        workspace_id: request.workspace_id,
        client_id: request.client_id,
        operation_id: support::operation_id(292),
        conflict_id: active.conflict_id,
        conflict_revision: active.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: active.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    fixture
        .engine
        .queue_conflict_resolution(active_request.clone())
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_none()
    );

    let resolved = resolved_from_request(&request, 7);
    fixture.engine.conflict_resolved(resolved.clone()).unwrap();
    fixture.engine.conflict_resolved(resolved.clone()).unwrap();

    let retained = fixture
        .engine
        .state()
        .conflict(active.conflict_id)
        .unwrap()
        .expect("newer generation retained");
    assert_eq!(retained.conflict_revision, active.conflict_revision);
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .expect("newer blocked origin retained")
            .stage,
        OutboxStage::BlockedConflict
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(active_request.operation_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fs::read(fixture.path("late-local-live.txt")).unwrap(),
        b"newer generation"
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(7)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(7))
    );
    assert!(
        fixture
            .engine
            .state()
            .applied_operation(resolved.resolved_by_client_id, resolved.operation_id)
            .unwrap()
            .is_some()
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn late_local_resolution_after_stream_generation_replacement_survives_reopen() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("late-local-stream.txt", 7, b"newer generation");
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(6))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(6))
        .unwrap();
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000061",
        "1",
        "late-local-stream.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &old);
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
    let request = WorkspaceConflictResolvedRequest {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(291),
        conflict_id: old.conflict_id,
        conflict_revision: old.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: old.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    let mut active = old.clone();
    active.conflict_revision = support::conflict_revision("2");
    active.current.path_revision = WorkspaceRevision::new(7);
    active.validate().unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();
    let active_request = WorkspaceConflictResolvedRequest {
        workspace_id: request.workspace_id,
        client_id: request.client_id,
        operation_id: support::operation_id(293),
        conflict_id: active.conflict_id,
        conflict_revision: active.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: active.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    fixture
        .engine
        .queue_conflict_resolution(active_request.clone())
        .unwrap();
    let resolved = resolved_from_request(&request, 7);
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(6, 7, 1, 1))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .put_stream_conflict_resolved(&resolved, None, StreamItemStatus::Ready)
        .unwrap();
    fixture.engine.conflict_created(active.clone()).unwrap();

    let mut fixture = fixture.reopen();
    fixture.engine.pending_commands(16).unwrap();
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(active.conflict_id)
            .unwrap()
            .expect("newer generation retained after replay")
            .conflict_revision,
        active.conflict_revision
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(blocked.operation_id)
            .unwrap()
            .expect("newer blocked origin retained after replay")
            .stage,
        OutboxStage::BlockedConflict
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(active_request.operation_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fs::read(fixture.path("late-local-stream.txt")).unwrap(),
        b"newer generation"
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert!(
        fixture
            .engine
            .state()
            .applied_operation(resolved.resolved_by_client_id, resolved.operation_id)
            .unwrap()
            .is_some()
    );
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(7, 1, 1))
        .unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(7)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(7))
    );
}

#[test]
fn exact_receipt_replay_cleans_lingering_same_generation_state_and_acks() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("exact-cleanup.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000051",
        "1",
        "exact-cleanup.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &created);
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
    let request = current_resolution(&fixture, &created, 280);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let resolved = resolved_from_request(&request, 1);
    let body_digest = fns_sync_core::body_digest(
        &fns_sync_core::canonical_json(&resolved).expect("canonical resolved message"),
    );
    fixture
        .engine
        .state_mut()
        .transaction(|tx| {
            tx.record_conflict_applied_operation(
                resolved.resolved_by_client_id,
                resolved.operation_id,
                resolved.revision,
                body_digest,
                None,
            )
        })
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();

    fixture.engine.conflict_resolved(resolved.clone()).unwrap();
    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::new(1)
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );
    assert!(fixture.engine.pending_commands(16).unwrap().iter().any(
        |command| matches!(command, SyncCommand::SendAck(ack) if ack.revision == resolved.revision)
    ));
    assert_eq!(
        fs::read(fixture.path("exact-cleanup.txt")).unwrap(),
        b"base"
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn remote_resolver_push_clears_origin_without_a_local_resolution_outbox() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("remote-resolved.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000037",
        "1",
        "remote-resolved.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    let blocked = blocked_origin_mutation(&fixture, &created);
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
    let resolved = WorkspaceConflictResolvedMessage {
        workspace_id: fixture.engine.state().workspace_id(),
        conflict_id: created.conflict_id,
        conflict_revision: created.conflict_revision,
        operation_id: support::operation_id(257),
        revision: WorkspaceRevision::new(1),
        choice: WorkspaceConflictChoice::Delete,
        path_state: fns_protocol::WorkspacePathState {
            path: created.path.clone(),
            path_revision: WorkspaceRevision::new(1),
            kind: WorkspaceEntryKind::Tombstone,
            content_hash: RequiredNullable::Null,
            metadata: support::zero_metadata(),
            tombstone: true,
        },
        resolved_by_client_id: support::remote_client_id(),
    };

    assert!(
        fixture
            .engine
            .conflict_resolved(resolved)
            .unwrap()
            .is_empty()
    );
    assert!(!fixture.path("remote-resolved.txt").exists());
    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );
}

#[test]
fn authoritative_generation_resolution_settles_stale_local_generation_and_rejects_operation_reuse()
{
    for streamed in [false, true] {
        let mut fixture = support::EngineFixture::new();
        let base_revision = if streamed { 364 } else { 0 };
        let resolution_revision = base_revision + 1;
        if streamed {
            fixture
                .engine
                .state_mut()
                .set_last_applied_revision(WorkspaceRevision::new(base_revision))
                .unwrap();
            fixture
                .engine
                .state_mut()
                .set_last_ack_revision(WorkspaceRevision::new(base_revision))
                .unwrap();
        }
        fixture.seed_remote_file("new-generation.txt", base_revision, b"base");
        let mut old = fixture.remote_conflict_created(
            "10000000-0000-4000-8000-000000000043",
            "1",
            "new-generation.txt",
        );
        old.current.path_revision = WorkspaceRevision::new(base_revision);
        old.validate().unwrap();
        fixture.engine.conflict_created(old.clone()).unwrap();
        let blocked = blocked_origin_mutation(&fixture, &old);
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
        let stale_resolution = current_resolution(&fixture, &old, 271);
        fixture
            .engine
            .queue_conflict_resolution(stale_resolution.clone())
            .unwrap();
        let _ = fixture.engine.pending_commands(1).unwrap();

        let remote_request = WorkspaceConflictResolvedRequest {
            workspace_id: fixture.engine.state().workspace_id(),
            client_id: support::remote_client_id(),
            operation_id: stale_resolution.operation_id,
            conflict_id: old.conflict_id,
            conflict_revision: support::conflict_revision("2"),
            choice: WorkspaceConflictChoice::Delete,
            path: old.path.clone(),
            content_hash: RequiredNullable::Null,
            metadata: support::zero_metadata(),
        };
        let resolved = resolved_from_request(&remote_request, resolution_revision);
        if streamed {
            fixture
                .engine
                .snapshot_begin(fixture.incremental_begin(base_revision, resolution_revision, 1, 0))
                .unwrap();
            fixture
                .engine
                .state_mut()
                .put_stream_conflict_resolved(&resolved, None, StreamItemStatus::Ready)
                .unwrap();
            fixture = fixture.reopen();
            fixture.engine.pending_commands(16).unwrap();
            assert!(fixture.engine.state().conflicts().unwrap().is_empty());
            assert_eq!(
                fixture.engine.cursor().unwrap().last_applied_revision,
                WorkspaceRevision::new(resolution_revision)
            );
            assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
            fixture
                .engine
                .snapshot_end(fixture.incremental_end(resolution_revision, 1, 0))
                .unwrap();
        } else {
            fixture.engine.conflict_resolved(resolved).unwrap();
        }

        assert!(!fixture.path("new-generation.txt").exists());
        assert!(fixture.engine.state().conflicts().unwrap().is_empty());
        assert!(fixture.engine.state().outbox().unwrap().is_empty());
        assert!(fixture.engine.state().local_intents().unwrap().is_empty());
        assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
        assert_eq!(
            fixture.engine.cursor().unwrap().pending_ack_revision,
            Some(WorkspaceRevision::new(resolution_revision))
        );
        assert!(
            fixture
                .engine
                .pending_commands(16)
                .unwrap()
                .iter()
                .any(|command| matches!(command, SyncCommand::SendAck(ack) if ack.revision == WorkspaceRevision::new(resolution_revision)))
        );
        fixture
            .engine
            .ack_confirmed(fixture.ack(resolution_revision))
            .unwrap();
        assert_eq!(
            fixture.engine.cursor().unwrap().last_ack_revision,
            WorkspaceRevision::new(resolution_revision)
        );
    }

    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("reused-operation.txt", 0, b"base");
    let old = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000044",
        "1",
        "reused-operation.txt",
    );
    fixture.engine.conflict_created(old.clone()).unwrap();
    let outstanding = current_resolution(&fixture, &old, 273);
    fixture
        .engine
        .queue_conflict_resolution(outstanding.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();
    let changed_request = WorkspaceConflictResolvedRequest {
        workspace_id: outstanding.workspace_id,
        client_id: outstanding.client_id,
        operation_id: outstanding.operation_id,
        conflict_id: outstanding.conflict_id,
        conflict_revision: support::conflict_revision("2"),
        choice: WorkspaceConflictChoice::Delete,
        path: outstanding.path,
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };

    assert_eq!(
        fixture
            .engine
            .conflict_resolved(resolved_from_request(&changed_request, 1))
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(
        fs::read(fixture.path("reused-operation.txt")).unwrap(),
        b"base"
    );
    assert!(
        fixture
            .engine
            .state()
            .conflict(old.conflict_id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(outstanding.operation_id)
            .unwrap()
            .is_some()
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
}

#[test]
fn local_resolution_operation_cannot_move_between_conflicts() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("first-conflict.txt", 0, b"base");
    let first = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000045",
        "1",
        "first-conflict.txt",
    );
    let second = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000046",
        "1",
        "second-conflict.txt",
    );
    fixture.engine.conflict_created(first.clone()).unwrap();
    fixture.engine.conflict_created(second.clone()).unwrap();
    let second_request = current_resolution(&fixture, &second, 274);
    fixture
        .engine
        .queue_conflict_resolution(second_request.clone())
        .unwrap();
    fixture
        .engine
        .state_mut()
        .remove_outbox(second_request.operation_id)
        .unwrap();
    let reused = WorkspaceConflictResolvedRequest {
        workspace_id: second_request.workspace_id,
        client_id: second_request.client_id,
        operation_id: second_request.operation_id,
        conflict_id: first.conflict_id,
        conflict_revision: first.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: first.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    let before = fixture.engine.state().row_counts().unwrap();

    assert_eq!(
        fixture
            .engine
            .conflict_resolved(resolved_from_request(&reused, 1))
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(
        fs::read(fixture.path("first-conflict.txt")).unwrap(),
        b"base"
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
}

#[test]
fn local_resolution_operation_cannot_reuse_pending_mutation_identity() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("mutation-reuse.txt", 0, b"base");
    let conflict = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000047",
        "1",
        "mutation-reuse.txt",
    );
    fixture.engine.conflict_created(conflict.clone()).unwrap();
    let mutation = WorkspaceMutation {
        workspace_id: fixture.engine.state().workspace_id(),
        client_id: fixture.engine.state().client_id(),
        operation_id: support::operation_id(275),
        path: support::workspace_path("unrelated.txt"),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceMutationKind::Delete,
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
    let reused = WorkspaceConflictResolvedRequest {
        workspace_id: mutation.workspace_id,
        client_id: mutation.client_id,
        operation_id: mutation.operation_id,
        conflict_id: conflict.conflict_id,
        conflict_revision: conflict.conflict_revision,
        choice: WorkspaceConflictChoice::Delete,
        path: conflict.path.clone(),
        content_hash: RequiredNullable::Null,
        metadata: support::zero_metadata(),
    };
    let before = fixture.engine.state().row_counts().unwrap();

    assert_eq!(
        fixture
            .engine
            .conflict_resolved(resolved_from_request(&reused, 1))
            .unwrap_err(),
        SyncError::OperationChanged
    );
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(
        fs::read(fixture.path("mutation-reuse.txt")).unwrap(),
        b"base"
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .outbox_entry(mutation.operation_id)
            .unwrap()
            .unwrap()
            .mutation()
            .unwrap(),
        mutation
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision,
        WorkspaceRevision::ZERO
    );
}

#[test]
fn retryable_resolution_error_retains_exact_durable_work() {
    let mut fixture = support::EngineFixture::new();
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000038", "1", "retry.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    let request = current_resolution(&fixture, &created, 258);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    assert!(
        fixture
            .engine
            .conflict_resolution_rejected(request.operation_id, WorkspaceV2ErrorCode::ServerBusy)
            .unwrap()
            .is_empty()
    );
    let outbox = fixture
        .engine
        .state()
        .outbox_entry(request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(outbox.stage, OutboxStage::Dispatched);
    assert_eq!(
        outbox.body_json,
        fns_sync_core::canonical_json(&request).unwrap()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .unwrap()
            .unwrap()
            .status,
        ConflictStatus::Resolving
    );
}

#[test]
fn conflict_not_found_marks_refresh_required_without_reusing_operation() {
    let mut fixture = support::EngineFixture::new();
    let created =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000039", "1", "missing.txt");
    fixture.engine.conflict_created(created.clone()).unwrap();
    let request = current_resolution(&fixture, &created, 259);
    fixture
        .engine
        .queue_conflict_resolution(request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    fixture
        .engine
        .conflict_resolution_rejected(request.operation_id, WorkspaceV2ErrorCode::ConflictNotFound)
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .unwrap()
            .unwrap()
            .status,
        ConflictStatus::RefreshRequired
    );
}

#[test]
fn late_stale_response_after_remote_resolution_is_idempotent() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("remote-won.txt", 0, b"base");
    let created = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000040",
        "1",
        "remote-won.txt",
    );
    fixture.engine.conflict_created(created.clone()).unwrap();
    let local_request = current_resolution(&fixture, &created, 260);
    fixture
        .engine
        .queue_conflict_resolution(local_request.clone())
        .unwrap();
    let _ = fixture.engine.pending_commands(1).unwrap();

    let mut remote_request = local_request.clone();
    remote_request.operation_id = support::operation_id(261);
    remote_request.client_id = support::remote_client_id();
    remote_request.choice = WorkspaceConflictChoice::Delete;
    remote_request.content_hash = RequiredNullable::Null;
    remote_request.metadata = support::zero_metadata();
    let remote = resolved_from_request(&remote_request, 1);
    fixture.engine.conflict_resolved(remote).unwrap();

    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
    assert!(
        fixture
            .engine
            .conflict_resolution_rejected(
                local_request.operation_id,
                WorkspaceV2ErrorCode::ConflictNotFound,
            )
            .unwrap()
            .is_empty()
    );
    assert!(fixture.engine.state().conflicts().unwrap().is_empty());
    assert!(fixture.engine.state().outbox().unwrap().is_empty());
}
