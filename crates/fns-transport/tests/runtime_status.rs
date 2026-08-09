use fns_protocol::{
    DecodedEnvelope, MessageBody, RequestId, WorkspaceAction, WorkspaceFlow, WorkspaceRevision,
    decode_text_frame, encode_success,
};
use fns_sync_core::{SyncEngine, SyncEngineConfig};
use fns_transport::session::{Session, SessionConnectionPhase, SessionResult};
use fns_transport::{EngineHandle, EngineWorker, WorkspaceEndpoint, socket};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;

mod support;

fn workspace_id() -> fns_protocol::WorkspaceId {
    fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

fn client_id() -> fns_protocol::ClientId {
    fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

struct TestEngine {
    _area: tempfile::TempDir,
    handle: EngineHandle,
    worker: EngineWorker,
}

impl TestEngine {
    fn new(local_file: bool) -> Self {
        let area = tempfile::tempdir().unwrap();
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        if local_file {
            std::fs::write(workspace.join("pending.txt"), b"pending").unwrap();
        }
        let config = SyncEngineConfig::new(workspace_id(), client_id(), &workspace, &state);
        let mut engine = SyncEngine::open(config).unwrap();
        if local_file {
            engine
                .record_local_changes(vec![fns_fs::FsChange::RescanRequired])
                .unwrap();
        }
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area: area,
            handle,
            worker,
        }
    }

    async fn stop(self) {
        self.handle.shutdown().await.unwrap();
        drop(self.handle);
        self.worker.join().unwrap();
    }
}

fn request_id(frame: &fns_protocol::DecodedFrame) -> RequestId {
    match &frame.envelope {
        DecodedEnvelope::Request { request_id, .. } => *request_id,
        _ => panic!("expected client request"),
    }
}

async fn next_request(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> fns_protocol::DecodedFrame {
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
            return decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
        }
    }
}

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    frame: Vec<u8>,
) {
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            String::from_utf8(frame).unwrap().into(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn engine_runtime_status_is_read_only_and_reports_durable_work() {
    let engine = TestEngine::new(true);

    let first = engine.handle.runtime_status().await.unwrap();
    let second = engine.handle.runtime_status().await.unwrap();

    assert_eq!(first, second);
    assert_eq!(first.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(first.pending_commands, 1);
    assert_eq!(engine.handle.pending_commands(8).await.unwrap().len(), 1);
    engine.stop().await;
}

#[tokio::test]
async fn observed_session_is_not_online_before_snapshot_end_is_processed() {
    let (begin_tx, begin_rx) = tokio::sync::oneshot::channel();
    let (end_tx, end_rx) = tokio::sync::oneshot::channel();
    let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000003").unwrap();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            let hello = next_request(&mut socket).await;
            assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
            send(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceHello,
                    WorkspaceFlow::ServerResponse,
                    Some(request_id(&hello)),
                    MessageBody::HelloResponse(fns_protocol::WorkspaceHelloResponse {
                        protocol_version: "2".into(),
                        server_version: "test".into(),
                        max_control_frame_bytes: fns_protocol::MAX_CONTROL_FRAME_BYTES as u32,
                        max_binary_chunk_bytes: fns_protocol::BLOB_CHUNK_BYTES,
                        max_blob_bytes: fns_protocol::MAX_BLOB_BYTES,
                        max_transfers_per_connection: 4,
                        heartbeat_seconds: 25,
                    }),
                )
                .unwrap(),
            )
            .await;
            let subscribe = next_request(&mut socket).await;
            assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
            send(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceSnapshotBegin,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotBegin(fns_protocol::WorkspaceSnapshotBeginMessage {
                        workspace_id: workspace_id(),
                        stream_id,
                        mode: fns_protocol::WorkspaceSnapshotMode::Snapshot,
                        from_revision: WorkspaceRevision::ZERO,
                        final_revision: WorkspaceRevision::ZERO,
                        entry_count: 0,
                        event_count: 0,
                        conflict_count: 0,
                    }),
                )
                .unwrap(),
            )
            .await;
            begin_tx.send(()).unwrap();
            end_rx.await.unwrap();
            send(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceSnapshotEnd,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotEnd(fns_protocol::WorkspaceSnapshotEndMessage {
                        workspace_id: workspace_id(),
                        stream_id,
                        mode: fns_protocol::WorkspaceSnapshotMode::Snapshot,
                        delivered_count: 0,
                        final_revision: WorkspaceRevision::ZERO,
                    }),
                )
                .unwrap(),
            )
            .await;
            let _ = socket.next().await;
        })
        .await;
    let engine = TestEngine::new(false);
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).unwrap();
    let token = support::secret_token("test.jwt");
    let stream = socket::connect(&endpoint, &token, "0.1.0").await.unwrap();
    let (session, mut writer, mut status_rx) = Session::new_observed(
        stream,
        engine.handle.clone(),
        workspace_id(),
        client_id(),
        "0.1.0".into(),
    );
    assert_eq!(
        status_rx.borrow().phase,
        SessionConnectionPhase::Handshaking
    );
    let shutdown = tokio_util::sync::CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let session_task = tokio::spawn(async move { session.run(&mut writer, task_shutdown).await });

    begin_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if status_rx.borrow_and_update().phase == SessionConnectionPhase::Subscribing {
                break;
            }
            status_rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        status_rx.borrow().phase,
        SessionConnectionPhase::Subscribing
    );

    end_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            status_rx.changed().await.unwrap();
            if status_rx.borrow_and_update().phase == SessionConnectionPhase::Online {
                break;
            }
        }
    })
    .await
    .unwrap();

    shutdown.cancel();
    assert!(matches!(session_task.await.unwrap(), SessionResult::Closed));
    server.finish().await;
    engine.stop().await;
}
