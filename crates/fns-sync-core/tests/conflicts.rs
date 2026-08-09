mod support;

use std::fs;

use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest, WorkspaceEntryKind,
    WorkspaceMutation, WorkspaceMutationKind, WorkspaceRevision, WorkspaceV2ErrorCode,
};
use fns_sync_core::{ConflictStatus, OutboxStage, SyncCommand, SyncError};

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
        base_path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: created.current.content_hash.clone(),
        metadata: created.current.metadata.clone(),
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
fn current_resolution_accepts_authoritative_tombstone_after_source_drift() {
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

    fixture
        .engine
        .conflict_resolution_accepted(resolved)
        .unwrap();
    assert!(
        fixture
            .engine
            .state()
            .outbox_entry(request.operation_id)
            .unwrap()
            .is_none()
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
    let resolved = resolved_from_request(&request, 2);

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
