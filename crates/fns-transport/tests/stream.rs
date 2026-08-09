//! Integration tests for the engine worker bridge.

use fns_protocol::{
    RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictSide, WorkspaceContentHash, WorkspaceFileMetadata,
    WorkspacePath, WorkspaceRevision,
};
use fns_sync_core::{ConflictResolutionReceiptStatus, SyncEngine, SyncEngineConfig};
use fns_transport::{EngineHandle, EngineWorker, TransportErrorCode};

fn ws_id() -> fns_protocol::WorkspaceId {
    fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

fn client_id() -> fns_protocol::ClientId {
    fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

struct TestEngine {
    _area: tempfile::TempDir,
    engine: SyncEngine,
}

fn make_test_engine_with_area() -> TestEngine {
    let area = tempfile::tempdir().unwrap();
    let workspace_root = area.path().join("workspace");
    let state_root = area.path().join("state");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();

    let config = SyncEngineConfig::new(ws_id(), client_id(), &workspace_root, &state_root);
    let engine = SyncEngine::open(config).expect("failed to open engine");
    TestEngine {
        _area: area,
        engine,
    }
}

fn conflict_created() -> WorkspaceConflictCreatedMessage {
    let path = WorkspacePath::parse("conflict.txt").unwrap();
    let side = WorkspaceConflictSide {
        path: RequiredNullable::Value(path.clone()),
        path_revision: WorkspaceRevision::new(1),
        content_hash: RequiredNullable::Value(
            WorkspaceContentHash::parse(
                "blake3:0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap(),
        ),
        metadata: WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false,
        },
        tombstone: false,
    };
    WorkspaceConflictCreatedMessage {
        workspace_id: ws_id(),
        conflict_id: fns_protocol::ConflictId::parse("10000000-0000-4000-8000-000000000030")
            .unwrap(),
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        path,
        kind: WorkspaceConflictKind::Content,
        ancestor: side.clone(),
        current: side.clone(),
        incoming: side,
        created_by_operation_id: fns_protocol::OperationId::parse(
            "10000000-0000-4000-8000-000000000031",
        )
        .unwrap(),
    }
}

#[tokio::test]
async fn engine_worker_cursor_and_shutdown() {
    let test = make_test_engine_with_area();
    let (worker, handle) = EngineWorker::spawn(test.engine);

    let cursor = handle.cursor().await.unwrap();
    assert_eq!(cursor.workspace_id, ws_id());

    let commands = handle.pending_commands(64).await.unwrap();
    assert!(commands.is_empty());

    handle.shutdown().await.unwrap();
    worker.join().unwrap();
}

#[tokio::test]
async fn engine_worker_record_local_changes() {
    let test = make_test_engine_with_area();
    let (worker, handle) = EngineWorker::spawn(test.engine);

    handle
        .record_local_changes(vec![fns_fs::FsChange::RescanRequired])
        .await
        .unwrap();

    // Should not hang.
    let _commands = handle.pending_commands(64).await.unwrap();

    handle.shutdown().await.unwrap();
    worker.join().unwrap();
}

#[tokio::test]
async fn engine_handle_is_cloneable() {
    let test = make_test_engine_with_area();
    let (worker, handle) = EngineWorker::spawn(test.engine);
    let handle2: EngineHandle = handle.clone();

    // Both handles should work.
    let cursor1 = handle.cursor().await.unwrap();
    let cursor2 = handle2.cursor().await.unwrap();
    assert_eq!(cursor1.workspace_id, cursor2.workspace_id);

    handle.shutdown().await.unwrap();
    worker.join().unwrap();
}

#[tokio::test]
async fn conflict_calls_are_serialized_idempotently_and_closed_worker_fails_explicitly() {
    let mut test = make_test_engine_with_area();
    let created = conflict_created();
    test.engine.conflict_created(created.clone()).unwrap();
    let (worker, handle) = EngineWorker::spawn(test.engine);
    let second = handle.clone();

    let (first_result, second_result) = tokio::join!(
        handle.resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Current,
        ),
        second.resolve_conflict(
            created.conflict_id,
            created.conflict_revision,
            WorkspaceConflictChoice::Current,
        )
    );
    let first_receipt = first_result.unwrap();
    let second_receipt = second_result.unwrap();
    assert_eq!(first_receipt, second_receipt);
    assert_eq!(
        first_receipt.status,
        ConflictResolutionReceiptStatus::Queued
    );

    let views = handle.list_conflicts().await.unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(
        views[0].pending_resolution.as_ref().unwrap().operation_id,
        first_receipt.operation_id
    );
    assert_eq!(handle.pending_commands(16).await.unwrap().len(), 1);

    handle.shutdown().await.unwrap();
    let error = second.list_conflicts().await.unwrap_err();
    assert_eq!(error.code(), TransportErrorCode::Core);
    assert!(!error.retryable());
    worker.join().unwrap();
}
