mod support;

use std::io::Read;

use fns_protocol::{
    BLOB_CHUNK_BYTES, RequiredNullable, WorkspaceConflictChoice, WorkspaceFileMetadata,
};
use fns_sync_core::{
    ConflictBlockedReason, ConflictResolutionInput, ConflictResolutionReceiptStatus,
    ConflictStatus, OutboxBody, SyncError,
};

fn create_conflict(
    fixture: &mut support::EngineFixture,
    id: &str,
    revision: &str,
    path: &str,
) -> fns_protocol::WorkspaceConflictCreatedMessage {
    let created = fixture.remote_conflict_created(id, revision, path);
    fixture
        .engine
        .conflict_created(created.clone())
        .expect("record conflict");
    created
}

fn queued_resolution(
    fixture: &support::EngineFixture,
    operation_id: fns_protocol::OperationId,
) -> fns_protocol::WorkspaceConflictResolvedRequest {
    let row = fixture
        .engine
        .state()
        .outbox_entry(operation_id)
        .expect("read outbox")
        .expect("queued resolution");
    match row.decoded_body().expect("decode resolution") {
        OutboxBody::ConflictResolution(request) => request,
        OutboxBody::Mutation(_) => panic!("expected conflict resolution"),
    }
}

#[test]
fn list_conflicts_decodes_validated_stable_views_in_id_order() {
    let mut fixture = support::EngineFixture::new();
    let second = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000042",
        "2",
        "nested/second.txt",
    );
    let first = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000041",
        "7",
        "first.txt",
    );

    let views = fixture.engine.list_conflicts().expect("list conflicts");
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].conflict_id, first.conflict_id);
    assert_eq!(views[1].conflict_id, second.conflict_id);

    let view = &views[0];
    assert_eq!(view.conflict_revision, first.conflict_revision);
    assert_eq!(view.path, first.path);
    assert_eq!(view.kind, first.kind);
    assert_eq!(view.status, ConflictStatus::Manual);
    assert_eq!(
        view.ancestor.path,
        first.ancestor.path.clone().into_option()
    );
    assert_eq!(view.ancestor.path_revision, first.ancestor.path_revision);
    assert_eq!(
        view.ancestor.content_hash,
        first.ancestor.content_hash.clone().into_option()
    );
    assert_eq!(view.ancestor.size, first.ancestor.metadata.size);
    assert_eq!(
        view.ancestor.modified_at_ms,
        first.ancestor.metadata.modified_at_ms
    );
    assert_eq!(view.ancestor.executable, first.ancestor.metadata.executable);
    assert_eq!(view.ancestor.tombstone, first.ancestor.tombstone);
    assert_eq!(view.created_by_operation_id, first.created_by_operation_id);
    assert!(view.pending_resolution.is_none());
    assert!(view.can_resolve);
    assert_eq!(view.blocked_reason, None);

    let json = serde_json::to_value(view).expect("serialize conflict view");
    assert_eq!(json["conflictId"], first.conflict_id.to_string());
    assert_eq!(json["conflictRevision"], "7");
    assert_eq!(json["ancestor"]["pathRevision"], "1");
    assert_eq!(
        json["ancestor"]["contentHash"],
        first
            .ancestor
            .content_hash
            .clone()
            .into_option()
            .unwrap()
            .to_string()
    );
    assert_eq!(json["ancestor"]["size"], first.ancestor.metadata.size);
    assert_eq!(json["pendingResolution"], serde_json::Value::Null);
    assert_eq!(json["blockedReason"], serde_json::Value::Null);

    let input = ConflictResolutionInput {
        conflict_id: first.conflict_id,
        conflict_revision: first.conflict_revision,
        choice: WorkspaceConflictChoice::Current,
    };
    assert_eq!(
        serde_json::to_value(input).unwrap(),
        serde_json::json!({
            "conflictId": first.conflict_id.to_string(),
            "conflictRevision": "7",
            "choice": "current"
        })
    );
}

#[test]
fn semantic_current_incoming_and_delete_resolutions_derive_exact_requests() {
    let mut fixture = support::EngineFixture::new();
    let current = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000043",
        "1",
        "current.txt",
    );
    let incoming = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000044",
        "2",
        "incoming.txt",
    );
    let deleted = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000045",
        "3",
        "deleted.txt",
    );

    let current_receipt = fixture
        .engine
        .resolve_conflict(
            current.conflict_id,
            current.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
        .expect("queue current");
    assert_eq!(
        current_receipt.status,
        ConflictResolutionReceiptStatus::Queued
    );
    assert_eq!(
        serde_json::to_value(current_receipt).unwrap(),
        serde_json::json!({
            "status": "queued",
            "operationId": current_receipt.operation_id.to_string()
        })
    );
    let current_request = queued_resolution(&fixture, current_receipt.operation_id);
    assert_eq!(
        current_request.workspace_id,
        fixture.engine.state().workspace_id()
    );
    assert_eq!(
        current_request.client_id,
        fixture.engine.state().client_id()
    );
    assert_eq!(current_request.conflict_id, current.conflict_id);
    assert_eq!(current_request.conflict_revision, current.conflict_revision);
    assert_eq!(current_request.choice, WorkspaceConflictChoice::Current);
    assert_eq!(
        current_request.path,
        current.current.path.clone().into_option().unwrap()
    );
    assert_eq!(current_request.content_hash, current.current.content_hash);
    assert_eq!(current_request.metadata, current.current.metadata);

    let incoming_receipt = fixture
        .engine
        .resolve_conflict(
            incoming.conflict_id,
            incoming.conflict_revision,
            WorkspaceConflictChoice::Incoming,
        )
        .expect("queue incoming");
    let incoming_request = queued_resolution(&fixture, incoming_receipt.operation_id);
    assert_eq!(incoming_request.choice, WorkspaceConflictChoice::Incoming);
    assert_eq!(
        incoming_request.path,
        incoming.incoming.path.clone().into_option().unwrap()
    );
    assert_eq!(
        incoming_request.content_hash,
        incoming.incoming.content_hash
    );
    assert_eq!(incoming_request.metadata, incoming.incoming.metadata);

    let delete_receipt = fixture
        .engine
        .resolve_conflict(
            deleted.conflict_id,
            deleted.conflict_revision,
            WorkspaceConflictChoice::Delete,
        )
        .expect("queue delete");
    let delete_request = queued_resolution(&fixture, delete_receipt.operation_id);
    assert_eq!(delete_request.choice, WorkspaceConflictChoice::Delete);
    assert_eq!(delete_request.path, deleted.path);
    assert_eq!(delete_request.content_hash, RequiredNullable::Null);
    assert_eq!(
        delete_request.metadata,
        WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false,
        }
    );

    let views = fixture
        .engine
        .list_conflicts()
        .expect("list queued conflicts");
    let pending = views
        .iter()
        .find(|view| view.conflict_id == current.conflict_id)
        .and_then(|view| view.pending_resolution.as_ref())
        .expect("pending current resolution");
    assert_eq!(pending.operation_id, current_receipt.operation_id);
    assert_eq!(pending.choice, WorkspaceConflictChoice::Current);
    assert_eq!(
        pending.content_hash,
        current.current.content_hash.into_option()
    );
    assert_eq!(pending.size, Some(current.current.metadata.size));
}

#[test]
fn merged_resolution_streams_large_workspace_file_into_cas_and_survives_reopen() {
    let mut fixture = support::EngineFixture::new();
    let created = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000046",
        "9",
        "nested/merged.bin",
    );
    let bytes = (0..(BLOB_CHUNK_BYTES as usize + 37))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    fixture.write(created.path.as_str(), &bytes);

    let receipt = fixture
        .engine
        .resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Merged,
        )
        .expect("queue merged");
    let request = queued_resolution(&fixture, receipt.operation_id);
    assert_eq!(request.choice, WorkspaceConflictChoice::Merged);
    assert_eq!(request.path, created.path);
    assert_eq!(
        request.content_hash,
        RequiredNullable::Value(support::hash(&bytes))
    );
    assert_eq!(request.metadata.size, bytes.len() as u64);

    let mut cached = Vec::new();
    fixture
        .engine
        .open_blob(&support::hash(&bytes))
        .expect("merged CAS blob")
        .read_to_end(&mut cached)
        .expect("read merged CAS blob");
    assert_eq!(cached, bytes);

    let mut fixture = fixture.reopen();
    let replay = fixture
        .engine
        .resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Merged,
        )
        .expect("idempotent merged replay");
    assert_eq!(replay, receipt);
    assert_eq!(fixture.engine.state().outbox().unwrap().len(), 1);

    fixture.write(created.path.as_str(), b"changed merged candidate");
    assert_eq!(
        fixture
            .engine
            .resolve_conflict(
                created.conflict_id,
                created.conflict_revision,
                WorkspaceConflictChoice::Merged,
            )
            .unwrap_err(),
        SyncError::ConflictResolutionChanged
    );
    assert_eq!(fixture.engine.state().outbox().unwrap().len(), 1);
}

#[test]
fn resolution_rejects_changed_choice_stale_revision_and_non_regular_merge() {
    let mut fixture = support::EngineFixture::new();
    let created = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000047",
        "4",
        "choice.txt",
    );
    let receipt = fixture
        .engine
        .resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
        .expect("queue current");
    let duplicate = fixture
        .engine
        .resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
        .expect("idempotent current");
    assert_eq!(duplicate, receipt);
    assert_eq!(fixture.engine.state().outbox().unwrap().len(), 1);
    assert_eq!(
        fixture
            .engine
            .resolve_conflict(
                created.conflict_id,
                created.conflict_revision,
                WorkspaceConflictChoice::Incoming,
            )
            .unwrap_err(),
        SyncError::ConflictResolutionChanged
    );

    let stale = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000048",
        "6",
        "stale-control.txt",
    );
    assert_eq!(
        fixture
            .engine
            .resolve_conflict(
                stale.conflict_id,
                support::conflict_revision("5"),
                WorkspaceConflictChoice::Delete,
            )
            .unwrap_err(),
        SyncError::ConflictRevisionStale
    );

    let directory = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000049",
        "1",
        "merged-directory",
    );
    std::fs::create_dir_all(fixture.path(directory.path.as_str())).unwrap();
    assert_eq!(
        fixture
            .engine
            .resolve_conflict(
                directory.conflict_id,
                directory.conflict_revision,
                WorkspaceConflictChoice::Merged,
            )
            .unwrap_err(),
        SyncError::MergeRejected {
            reason: "merged_file_required"
        }
    );

    let mut deleted_side = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000050",
        "1",
        "deleted-side.txt",
    );
    deleted_side.kind = fns_protocol::WorkspaceConflictKind::DeleteModify;
    deleted_side.current.content_hash = RequiredNullable::Null;
    deleted_side.current.metadata = support::zero_metadata();
    deleted_side.current.tombstone = true;
    deleted_side.validate().unwrap();
    fixture
        .engine
        .conflict_created(deleted_side.clone())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .resolve_conflict(
                deleted_side.conflict_id,
                deleted_side.conflict_revision,
                WorkspaceConflictChoice::Current,
            )
            .unwrap_err(),
        SyncError::ConflictResolutionBlocked {
            reason: ConflictBlockedReason::SelectedSideDeleted
        }
    );
}

#[test]
fn non_manual_statuses_are_visible_and_return_stable_blocking_errors() {
    let mut fixture = support::EngineFixture::new();
    let cases = [
        (
            "10000000-0000-4000-8000-000000000051",
            "waiting.txt",
            ConflictStatus::WaitingBlobs,
            ConflictBlockedReason::WaitingBlobs,
        ),
        (
            "10000000-0000-4000-8000-000000000052",
            "automatic.txt",
            ConflictStatus::AutoReady,
            ConflictBlockedReason::AutomaticResolutionPending,
        ),
        (
            "10000000-0000-4000-8000-000000000053",
            "refresh.txt",
            ConflictStatus::RefreshRequired,
            ConflictBlockedReason::RefreshRequired,
        ),
    ];

    for (id, path, status, reason) in cases {
        let created = create_conflict(&mut fixture, id, "1", path);
        fixture
            .engine
            .state_mut()
            .set_conflict_status(created.conflict_id, status)
            .unwrap();
        assert_eq!(
            fixture
                .engine
                .resolve_conflict(
                    created.conflict_id,
                    created.conflict_revision,
                    WorkspaceConflictChoice::Current,
                )
                .unwrap_err(),
            SyncError::ConflictResolutionBlocked { reason }
        );
    }

    let views = fixture.engine.list_conflicts().unwrap();
    for view in views {
        assert!(!view.can_resolve);
        assert!(view.blocked_reason.is_some());
        assert!(view.pending_resolution.is_none());
    }
}

#[test]
fn corrupt_created_json_is_observable_from_list_and_resolve() {
    let mut fixture = support::EngineFixture::new();
    let created = create_conflict(
        &mut fixture,
        "10000000-0000-4000-8000-000000000054",
        "1",
        "corrupt.txt",
    );
    let connection = rusqlite::Connection::open(fixture.state.path().join("state.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE conflicts SET created_json = ?1 WHERE conflict_id = ?2",
            rusqlite::params![b"{".as_slice(), created.conflict_id.to_string()],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        fixture.engine.list_conflicts().unwrap_err(),
        SyncError::CorruptState {
            table: "conflicts",
            ..
        }
    ));
    assert!(matches!(
        fixture
            .engine
            .resolve_conflict(
                created.conflict_id,
                created.conflict_revision,
                WorkspaceConflictChoice::Delete,
            )
            .unwrap_err(),
        SyncError::CorruptState {
            table: "conflicts",
            ..
        }
    ));
}
