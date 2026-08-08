//! Integration tests for the engine worker bridge.

use fns_sync_core::{SyncEngine, SyncEngineConfig};
use fns_transport::{EngineHandle, EngineWorker};

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
