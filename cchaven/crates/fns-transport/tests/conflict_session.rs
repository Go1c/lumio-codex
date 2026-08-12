mod support;

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use fns_protocol::{
    ClientId, ConflictId, DecodedEnvelope, DecodedFrame, MessageBody, OperationId, RequestId,
    RequiredNullable, StreamId, WorkspaceAction, WorkspaceBlobDirection,
    WorkspaceBlobNeedUploadPush, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest,
    WorkspaceConflictSide, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata,
    WorkspaceFlow, WorkspaceId, WorkspaceMutation, WorkspaceMutationKind, WorkspacePath,
    WorkspacePathState, WorkspaceRevision, WorkspaceSnapshotBeginMessage,
    WorkspaceSnapshotEndMessage, WorkspaceSnapshotMode, WorkspaceV2Error, WorkspaceV2ErrorCode,
    decode_text_frame, encode_failure, encode_success,
};
use fns_sync_core::{ConflictStatus, OutboxStage, SyncEngine, SyncEngineConfig};
use fns_transport::session::{Session, SessionLimits, SessionResult};
use fns_transport::{EngineHandle, EngineWorker, TransportErrorCode, WorkspaceEndpoint, socket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type ServerSocket = WebSocketStream<tokio::net::TcpStream>;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

fn client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

fn remote_client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000003").unwrap()
}

fn operation_id(value: u32) -> OperationId {
    OperationId::parse(&format!("10000000-0000-4000-8000-{value:012}")).unwrap()
}

fn stream_id(value: u32) -> StreamId {
    StreamId::parse(&format!("10000000-0000-4000-8000-{value:012}")).unwrap()
}

fn content_hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex())).unwrap()
}

fn file_metadata(bytes: &[u8]) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: bytes.len() as u64,
        modified_at_ms: 1_800_000_000_000,
        executable: false,
    }
}

fn zero_metadata() -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 0,
        executable: false,
    }
}

fn hello_response() -> fns_protocol::WorkspaceHelloResponse {
    fns_protocol::WorkspaceHelloResponse {
        protocol_version: "2".into(),
        server_version: "conflict-test".into(),
        max_control_frame_bytes: fns_protocol::MAX_CONTROL_FRAME_BYTES as u32,
        max_binary_chunk_bytes: fns_protocol::BLOB_CHUNK_BYTES,
        max_blob_bytes: fns_protocol::MAX_BLOB_BYTES,
        max_transfers_per_connection: 4,
        heartbeat_seconds: 25,
    }
}

fn test_limits() -> SessionLimits {
    SessionLimits {
        heartbeat_interval: Duration::from_secs(1),
        drain_interval: Duration::from_millis(10),
        request_timeout: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(2),
        transfer_idle_timeout: Duration::from_secs(1),
        transfer_max_lifetime: Duration::from_secs(2),
        drain_item_budget: 16,
        drain_byte_budget: fns_protocol::MAX_CONTROL_FRAME_BYTES * 16,
        pending_outbound_capacity: 32,
        deferred_event_capacity: 32,
    }
}

struct TestEngine {
    _area: tempfile::TempDir,
    workspace: PathBuf,
    state: PathBuf,
    handle: EngineHandle,
    worker: EngineWorker,
}

struct ResolutionFixture {
    engine: TestEngine,
    created: WorkspaceConflictCreatedMessage,
    request: WorkspaceConflictResolvedRequest,
    resolved: WorkspaceConflictResolvedMessage,
}

impl TestEngine {
    async fn stop_and_reopen(self) -> SyncEngine {
        self.handle.shutdown().await.unwrap();
        drop(self.handle);
        self.worker.join().unwrap();
        SyncEngine::open(SyncEngineConfig::new(
            workspace_id(),
            client_id(),
            &self.workspace,
            &self.state,
        ))
        .unwrap()
    }
}

fn resolution_fixture(choice: WorkspaceConflictChoice) -> ResolutionFixture {
    let area = tempfile::tempdir().unwrap();
    let workspace = area.path().join("workspace");
    let state = area.path().join("state");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let path = WorkspacePath::parse("resolved.txt").unwrap();
    let base = b"base";
    std::fs::write(workspace.join(path.as_str()), base).unwrap();
    let base_modified_at_ms = std::fs::metadata(workspace.join(path.as_str()))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let base_state = WorkspacePathState {
        path: path.clone(),
        path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceEntryKind::File,
        content_hash: RequiredNullable::Value(content_hash(base)),
        metadata: WorkspaceFileMetadata {
            size: base.len() as u64,
            modified_at_ms: base_modified_at_ms,
            executable: false,
        },
        tombstone: false,
    };
    let side = WorkspaceConflictSide {
        path: RequiredNullable::Value(path.clone()),
        path_revision: WorkspaceRevision::ZERO,
        content_hash: base_state.content_hash.clone(),
        metadata: base_state.metadata.clone(),
        tombstone: false,
    };
    let created = WorkspaceConflictCreatedMessage {
        workspace_id: workspace_id(),
        conflict_id: ConflictId::parse("10000000-0000-4000-8000-000000000030").unwrap(),
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        path: path.clone(),
        kind: WorkspaceConflictKind::Content,
        ancestor: side.clone(),
        current: side.clone(),
        incoming: side,
        created_by_operation_id: operation_id(240),
    };
    let (resolved_content_hash, metadata) = match choice {
        WorkspaceConflictChoice::Delete => (RequiredNullable::Null, zero_metadata()),
        WorkspaceConflictChoice::Merged => {
            let merged = b"merged";
            (
                RequiredNullable::Value(content_hash(merged)),
                file_metadata(merged),
            )
        }
        WorkspaceConflictChoice::Current | WorkspaceConflictChoice::Incoming => {
            (base_state.content_hash.clone(), base_state.metadata.clone())
        }
    };
    let request = WorkspaceConflictResolvedRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: operation_id(250),
        conflict_id: created.conflict_id,
        conflict_revision: created.conflict_revision,
        choice,
        path: path.clone(),
        content_hash: resolved_content_hash.clone(),
        metadata: metadata.clone(),
    };
    let resolved = WorkspaceConflictResolvedMessage {
        workspace_id: workspace_id(),
        conflict_id: created.conflict_id,
        conflict_revision: created.conflict_revision,
        operation_id: request.operation_id,
        revision: WorkspaceRevision::new(1),
        choice,
        path_state: WorkspacePathState {
            path: path.clone(),
            path_revision: WorkspaceRevision::new(1),
            kind: if choice == WorkspaceConflictChoice::Delete {
                WorkspaceEntryKind::Tombstone
            } else {
                WorkspaceEntryKind::File
            },
            content_hash: resolved_content_hash,
            metadata,
            tombstone: choice == WorkspaceConflictChoice::Delete,
        },
        resolved_by_client_id: client_id(),
    };

    let mut core = SyncEngine::open(SyncEngineConfig::new(
        workspace_id(),
        client_id(),
        &workspace,
        &state,
    ))
    .unwrap();
    core.state_mut().put_path_state(&base_state).unwrap();
    core.state_mut()
        .record_conflict(&created, ConflictStatus::Manual)
        .unwrap();
    let blocked = WorkspaceMutation {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: created.created_by_operation_id,
        path: path.clone(),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: base_state.content_hash,
        metadata: base_state.metadata,
        new_path: None,
        target_base_path_revision: None,
    };
    core.state_mut().enqueue_mutation(&blocked).unwrap();
    core.state_mut()
        .set_outbox_stage(blocked.operation_id, OutboxStage::BlockedConflict)
        .unwrap();
    if choice == WorkspaceConflictChoice::Merged {
        core.stage_bytes(&content_hash(b"merged"), b"merged")
            .unwrap();
    }
    core.queue_conflict_resolution(request.clone()).unwrap();
    let (worker, handle) = EngineWorker::spawn(core);
    ResolutionFixture {
        engine: TestEngine {
            _area: area,
            workspace,
            state,
            handle,
            worker,
        },
        created,
        request,
        resolved,
    }
}

async fn next_client_request(socket: &mut ServerSocket) -> DecodedFrame {
    loop {
        let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
            .await
            .expect("timed out waiting for client frame")
            .expect("client closed before request")
            .expect("client WebSocket read");
        if let Message::Text(text) = message {
            return decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
        }
    }
}

fn request_id(frame: &DecodedFrame) -> RequestId {
    match &frame.envelope {
        DecodedEnvelope::Request { request_id, .. } => *request_id,
        _ => panic!("expected client request"),
    }
}

fn resolution_request(frame: &DecodedFrame) -> WorkspaceConflictResolvedRequest {
    assert_eq!(frame.action, WorkspaceAction::WorkspaceConflictResolved);
    match &frame.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::ConflictResolvedRequest(request),
            ..
        } => request.clone(),
        _ => panic!("expected conflict resolution request"),
    }
}

async fn send_frame(socket: &mut ServerSocket, frame: Vec<u8>) {
    socket
        .send(Message::Text(String::from_utf8(frame).unwrap().into()))
        .await
        .unwrap();
}

async fn send_push(socket: &mut ServerSocket, action: WorkspaceAction, body: MessageBody) {
    send_frame(
        socket,
        encode_success(action, WorkspaceFlow::ServerPush, None, body).unwrap(),
    )
    .await;
}

async fn answer_hello_and_open_empty_stream(socket: &mut ServerSocket, stream: StreamId) {
    let hello = next_client_request(socket).await;
    assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
    send_frame(
        socket,
        encode_success(
            WorkspaceAction::WorkspaceHello,
            WorkspaceFlow::ServerResponse,
            Some(request_id(&hello)),
            MessageBody::HelloResponse(hello_response()),
        )
        .unwrap(),
    )
    .await;
    let subscribe = next_client_request(socket).await;
    assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
    send_push(
        socket,
        WorkspaceAction::WorkspaceSnapshotBegin,
        MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::ZERO,
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        }),
    )
    .await;
    send_push(
        socket,
        WorkspaceAction::WorkspaceSnapshotEnd,
        MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Incremental,
            delivered_count: 0,
            final_revision: WorkspaceRevision::ZERO,
        }),
    )
    .await;
}

async fn send_resolution_response(
    socket: &mut ServerSocket,
    id: RequestId,
    message: WorkspaceConflictResolvedMessage,
) {
    send_frame(
        socket,
        encode_success(
            WorkspaceAction::WorkspaceConflictResolved,
            WorkspaceFlow::ServerResponse,
            Some(id),
            MessageBody::ConflictResolved(message),
        )
        .unwrap(),
    )
    .await;
}

async fn receive_and_answer_ack(socket: &mut ServerSocket, revision: u64) {
    let ack = next_client_request(socket).await;
    assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
    let body = match &ack.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::Ack(body),
            ..
        } => body.clone(),
        _ => panic!("expected Ack request"),
    };
    assert_eq!(body.revision, WorkspaceRevision::new(revision));
    send_frame(
        socket,
        encode_success(
            WorkspaceAction::WorkspaceAck,
            WorkspaceFlow::ServerResponse,
            Some(request_id(&ack)),
            MessageBody::Ack(body),
        )
        .unwrap(),
    )
    .await;
}

async fn run_connection<F, Fut>(engine: EngineHandle, script: F) -> SessionResult
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let server = support::fake_server::ScriptedWorkspaceServer::start(script).await;
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).unwrap();
    let token = support::secret_token("conflict.test.jwt");
    let stream = socket::connect(&endpoint, &token, "0.1.0").await.unwrap();
    let (session, mut writer) = Session::new_with_limits(
        stream,
        engine,
        workspace_id(),
        client_id(),
        "0.1.0".into(),
        test_limits(),
    );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let mut run = tokio::spawn(async move { session.run(&mut writer, run_shutdown).await });
    let result = match tokio::time::timeout(TEST_TIMEOUT, &mut run).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            shutdown.cancel();
            tokio::time::timeout(Duration::from_secs(1), &mut run)
                .await
                .expect("session ignored cancellation")
                .unwrap();
            panic!("session did not finish before timeout");
        }
    };
    server.finish().await;
    result
}

#[tokio::test]
async fn live_resolved_after_ack_applies_only_after_uncorrelated_push() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Delete);
    let workspace = fixture.engine.workspace.clone();
    let response = fixture.resolved.clone();
    let push = response.clone();
    let expected = fixture.request.clone();
    let (response_sent_tx, response_sent_rx) = tokio::sync::oneshot::channel();
    let (allow_push_tx, allow_push_rx) = tokio::sync::oneshot::channel();
    let handle = fixture.engine.handle.clone();
    let run = tokio::spawn(run_connection(handle, move |mut socket| async move {
        answer_hello_and_open_empty_stream(&mut socket, stream_id(501)).await;
        let request = next_client_request(&mut socket).await;
        assert_eq!(resolution_request(&request), expected);
        send_resolution_response(&mut socket, request_id(&request), response).await;
        response_sent_tx.send(()).unwrap();
        allow_push_rx.await.unwrap();
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceConflictResolved,
            MessageBody::ConflictResolved(push),
        )
        .await;
        receive_and_answer_ack(&mut socket, 1).await;
        socket.close(None).await.unwrap();
    }));

    response_sent_rx.await.unwrap();
    assert_eq!(
        std::fs::read(workspace.join("resolved.txt")).unwrap(),
        b"base"
    );
    allow_push_tx.send(()).unwrap();
    assert!(matches!(run.await.unwrap(), SessionResult::Closed));

    let reopened = fixture.engine.stop_and_reopen().await;
    assert!(!workspace.join("resolved.txt").exists());
    assert!(reopened.state().conflicts().unwrap().is_empty());
    assert!(reopened.state().outbox().unwrap().is_empty());
    assert!(reopened.state().local_intents().unwrap().is_empty());
    assert!(reopened.state().stream_state().unwrap().is_none());
    assert_eq!(reopened.cursor().unwrap().last_ack_revision.get(), 1);
}

#[tokio::test]
async fn resolve_response_loss_replays_exact_body_with_fresh_request_id() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Delete);
    let expected = fixture.request.clone();
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let first = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(502)).await;
            let request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&request), expected);
            first_tx.send(request_id(&request)).unwrap();
            socket.close(None).await.unwrap();
        },
    )
    .await;
    assert!(matches!(first, SessionResult::Closed));
    let first_request_id = first_rx.await.unwrap();

    let expected = fixture.request.clone();
    let response = fixture.resolved.clone();
    let push = response.clone();
    let second = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(503)).await;
            let request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&request), expected);
            assert_ne!(request_id(&request), first_request_id);
            send_resolution_response(&mut socket, request_id(&request), response).await;
            send_push(
                &mut socket,
                WorkspaceAction::WorkspaceConflictResolved,
                MessageBody::ConflictResolved(push),
            )
            .await;
            receive_and_answer_ack(&mut socket, 1).await;
            socket.close(None).await.unwrap();
        },
    )
    .await;
    assert!(matches!(second, SessionResult::Closed));
    let reopened = fixture.engine.stop_and_reopen().await;
    assert!(reopened.state().conflicts().unwrap().is_empty());
    assert!(reopened.state().outbox().unwrap().is_empty());
    assert_eq!(reopened.cursor().unwrap().last_ack_revision.get(), 1);
}

#[tokio::test]
async fn blob_required_keeps_awaiting_resolution_and_reads_following_blob_need() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Merged);
    let expected = fixture.request.clone();
    let request_hash = expected.content_hash.clone().into_option().unwrap();
    let request_size = expected.metadata.size;
    let result = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(504)).await;
            let request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&request), expected);
            send_frame(
                &mut socket,
                encode_failure(
                    WorkspaceAction::WorkspaceConflictResolved,
                    Some(request_id(&request)),
                    WorkspaceV2Error::new(WorkspaceV2ErrorCode::BlobRequired, Vec::new()),
                )
                .unwrap(),
            )
            .await;
            send_push(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                MessageBody::BlobNeedUploadPush(WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    operation_id: expected.operation_id,
                    direction: WorkspaceBlobDirection::Upload,
                    content_hash: request_hash,
                    size: request_size,
                }),
            )
            .await;
            let begin = next_client_request(&mut socket).await;
            assert_eq!(begin.action, WorkspaceAction::WorkspaceBlobBegin);
            socket.close(None).await.unwrap();
        },
    )
    .await;
    assert!(matches!(result, SessionResult::Closed));
    let reopened = fixture.engine.stop_and_reopen().await;
    assert_eq!(
        reopened
            .state()
            .outbox_entry(fixture.request.operation_id)
            .unwrap()
            .unwrap()
            .stage,
        OutboxStage::AwaitingBlob
    );
}

#[tokio::test]
async fn stale_resolution_marks_refresh_required_and_triggers_subscribe_refresh() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Delete);
    let expected = fixture.request.clone();
    let result = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(505)).await;
            let request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&request), expected);
            send_frame(
                &mut socket,
                encode_failure(
                    WorkspaceAction::WorkspaceConflictResolved,
                    Some(request_id(&request)),
                    WorkspaceV2Error::new(WorkspaceV2ErrorCode::ConflictRevisionStale, Vec::new()),
                )
                .unwrap(),
            )
            .await;
            let refresh = next_client_request(&mut socket).await;
            assert_eq!(refresh.action, WorkspaceAction::WorkspaceSubscribe);
            socket.close(None).await.unwrap();
        },
    )
    .await;
    assert!(matches!(result, SessionResult::Closed));
    let reopened = fixture.engine.stop_and_reopen().await;
    assert!(
        reopened
            .state()
            .outbox_entry(fixture.request.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .state()
            .conflict(fixture.created.conflict_id)
            .unwrap()
            .unwrap()
            .status,
        ConflictStatus::RefreshRequired
    );
}

#[tokio::test]
async fn mismatched_resolution_response_is_observable_and_retains_durable_work() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Delete);
    let expected = fixture.request.clone();
    let mut changed = fixture.resolved.clone();
    changed.operation_id = operation_id(999);
    let result = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(506)).await;
            let request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&request), expected);
            send_resolution_response(&mut socket, request_id(&request), changed).await;
        },
    )
    .await;
    match result {
        SessionResult::Error(error) => {
            assert_eq!(error.code(), TransportErrorCode::Protocol);
            assert!(!error.retryable());
        }
        SessionResult::Closed => panic!("mismatched response closed silently"),
    }
    let reopened = fixture.engine.stop_and_reopen().await;
    assert!(
        reopened
            .state()
            .outbox_entry(fixture.request.operation_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn remote_resolver_push_cancels_local_resolution_and_applies_authoritative_state() {
    let fixture = resolution_fixture(WorkspaceConflictChoice::Delete);
    let workspace = fixture.engine.workspace.clone();
    let expected = fixture.request.clone();
    let mut remote = fixture.resolved.clone();
    remote.operation_id = operation_id(260);
    remote.resolved_by_client_id = remote_client_id();
    let result = run_connection(
        fixture.engine.handle.clone(),
        move |mut socket| async move {
            answer_hello_and_open_empty_stream(&mut socket, stream_id(507)).await;

            // Force the legal race: our durable request is already on the wire
            // when another client wins the same conflict.
            let local_request = next_client_request(&mut socket).await;
            assert_eq!(resolution_request(&local_request), expected);
            send_push(
                &mut socket,
                WorkspaceAction::WorkspaceConflictResolved,
                MessageBody::ConflictResolved(remote),
            )
            .await;
            send_frame(
                &mut socket,
                encode_failure(
                    WorkspaceAction::WorkspaceConflictResolved,
                    Some(request_id(&local_request)),
                    WorkspaceV2Error::new(WorkspaceV2ErrorCode::ConflictNotFound, Vec::new()),
                )
                .unwrap(),
            )
            .await;
            receive_and_answer_ack(&mut socket, 1).await;
            socket.close(None).await.unwrap();
        },
    )
    .await;
    assert!(matches!(result, SessionResult::Closed));

    let reopened = fixture.engine.stop_and_reopen().await;
    assert!(!workspace.join("resolved.txt").exists());
    assert!(reopened.state().conflicts().unwrap().is_empty());
    assert!(reopened.state().outbox().unwrap().is_empty());
    assert!(reopened.state().local_intents().unwrap().is_empty());
    assert!(reopened.state().stream_state().unwrap().is_none());
    assert_eq!(reopened.cursor().unwrap().last_ack_revision.get(), 1);
}
