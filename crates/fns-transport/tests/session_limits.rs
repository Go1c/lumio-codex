use fns_protocol::{
    ClientId, DecodedEnvelope, DecodedFrame, MessageBody, OperationId, RequestId,
    WorkspaceAckRequest, WorkspaceAction, WorkspaceFlow, WorkspaceId, WorkspaceRevision,
    decode_server_text_frame, decode_text_frame, encode_request, encode_success,
};
use fns_sync_core::{SyncCommand, SyncEngine, SyncEngineConfig};
use fns_transport::dispatch::{ExpectedResponse, RequestTracker};
use fns_transport::session::{Session, SessionLimits, SessionResult};
use fns_transport::{
    EngineHandle, EngineWorker, MAX_IN_FLIGHT_REQUESTS, REQUEST_TIMEOUT, TransportErrorCode,
    WorkspaceEndpoint, socket,
};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::time::Instant;

mod support;

fn request_id(value: u128) -> RequestId {
    RequestId::parse(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}

fn operation_id(value: u128) -> OperationId {
    OperationId::parse(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

fn client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

fn ack(revision: u64) -> WorkspaceAckRequest {
    WorkspaceAckRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        revision: WorkspaceRevision::new(revision),
    }
}

fn ack_response(id: RequestId, body: WorkspaceAckRequest) -> DecodedFrame {
    let frame = encode_success(
        WorkspaceAction::WorkspaceAck,
        WorkspaceFlow::ServerResponse,
        Some(id),
        MessageBody::Ack(body),
    )
    .unwrap();
    decode_server_text_frame(&frame).unwrap()
}

fn hello_response() -> fns_protocol::WorkspaceHelloResponse {
    fns_protocol::WorkspaceHelloResponse {
        protocol_version: "2".into(),
        server_version: "test".into(),
        max_control_frame_bytes: fns_protocol::MAX_CONTROL_FRAME_BYTES as u32,
        max_binary_chunk_bytes: fns_protocol::BLOB_CHUNK_BYTES,
        max_blob_bytes: fns_protocol::MAX_BLOB_BYTES,
        max_transfers_per_connection: 4,
        heartbeat_seconds: 25,
    }
}

fn test_limits() -> SessionLimits {
    SessionLimits {
        heartbeat_interval: Duration::from_millis(10),
        drain_interval: Duration::from_millis(5),
        request_timeout: Duration::from_millis(60),
        idle_timeout: Duration::from_millis(120),
        transfer_idle_timeout: Duration::from_millis(40),
        transfer_max_lifetime: Duration::from_millis(200),
        drain_item_budget: 4,
        drain_byte_budget: fns_protocol::MAX_CONTROL_FRAME_BYTES * 4,
        pending_outbound_capacity: 8,
        deferred_event_capacity: 8,
    }
}

struct TestEngine {
    _area: tempfile::TempDir,
    handle: EngineHandle,
    worker: EngineWorker,
}

impl TestEngine {
    fn new(local_file: Option<(&str, &[u8])>) -> Self {
        let area = tempfile::tempdir().unwrap();
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        if let Some((path, content)) = local_file {
            std::fs::write(workspace.join(path), content).unwrap();
        }
        let config = SyncEngineConfig::new(workspace_id(), client_id(), &workspace, &state);
        let mut engine = SyncEngine::open(config).unwrap();
        if local_file.is_some() {
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

async fn next_client_request(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> DecodedFrame {
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
            return decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
        }
    }
}

fn client_request_id(frame: &DecodedFrame) -> RequestId {
    match &frame.envelope {
        DecodedEnvelope::Request { request_id, .. } => *request_id,
        _ => panic!("expected client request"),
    }
}

async fn send_server_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    frame: Vec<u8>,
) {
    let text = String::from_utf8(frame).unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
        .await
        .unwrap();
}

async fn answer_hello(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let hello = next_client_request(socket).await;
    assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
    let response = encode_success(
        WorkspaceAction::WorkspaceHello,
        WorkspaceFlow::ServerResponse,
        Some(client_request_id(&hello)),
        MessageBody::HelloResponse(hello_response()),
    )
    .unwrap();
    send_server_frame(socket, response).await;
}

async fn send_empty_snapshot(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    stream_id: fns_protocol::StreamId,
) {
    send_server_frame(
        socket,
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
    send_server_frame(
        socket,
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
}

async fn connected_session(
    endpoint: &str,
    engine: EngineHandle,
    limits: SessionLimits,
) -> (Session, fns_transport::socket::SocketWriter) {
    let endpoint = WorkspaceEndpoint::parse(endpoint).unwrap();
    let token = support::secret_token("test.jwt");
    let stream = socket::connect(&endpoint, &token, "0.1.0").await.unwrap();
    Session::new_with_limits(
        stream,
        engine,
        workspace_id(),
        client_id(),
        "0.1.0".into(),
        limits,
    )
}

#[test]
fn wrong_request_id_does_not_consume_the_real_request() {
    let now = Instant::now();
    let expected_id = request_id(1);
    let mut tracker = RequestTracker::new();
    tracker
        .track(expected_id, ExpectedResponse::Ack(ack(7)), now)
        .unwrap();

    let error = tracker
        .validate(&ack_response(request_id(2), ack(7)))
        .unwrap_err();

    assert_eq!(error.code(), TransportErrorCode::Protocol);
    assert_eq!(tracker.len(), 1);
    assert!(tracker.contains(&expected_id));
}

#[test]
fn exact_duplicate_response_is_a_bounded_completed_receipt() {
    let now = Instant::now();
    let id = request_id(2_001);
    let mut tracker = RequestTracker::new();
    tracker
        .track(id, ExpectedResponse::Ack(ack(7)), now)
        .unwrap();
    let response = ack_response(id, ack(7));
    assert_eq!(tracker.validate(&response).unwrap(), id);
    tracker.complete(&id).unwrap();

    assert_eq!(tracker.validate(&response).unwrap(), id);
    assert!(tracker.is_completed(&id));
    assert!(tracker.is_empty());

    let changed = ack_response(id, ack(8));
    assert_eq!(
        tracker.validate(&changed).unwrap_err().code(),
        TransportErrorCode::Protocol
    );
    assert!(tracker.is_completed(&id));
}

#[test]
fn right_id_wrong_action_or_body_does_not_consume_the_request() {
    let now = Instant::now();
    let id = request_id(3);
    let mut tracker = RequestTracker::new();
    tracker
        .track(id, ExpectedResponse::Ack(ack(7)), now)
        .unwrap();

    let wrong_action = encode_success(
        WorkspaceAction::WorkspaceHello,
        WorkspaceFlow::ServerResponse,
        Some(id),
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
    .unwrap();
    let wrong_action = decode_server_text_frame(&wrong_action).unwrap();
    assert_eq!(
        tracker.validate(&wrong_action).unwrap_err().code(),
        TransportErrorCode::Protocol
    );
    assert!(tracker.contains(&id));

    assert_eq!(
        tracker
            .validate(&ack_response(id, ack(8)))
            .unwrap_err()
            .code(),
        TransportErrorCode::Protocol
    );
    assert!(tracker.contains(&id));
}

#[test]
fn exact_duplicate_response_matches_after_completion() {
    let now = Instant::now();
    let id = request_id(4);
    let response = ack_response(id, ack(9));
    let mut tracker = RequestTracker::new();
    tracker
        .track(id, ExpectedResponse::Ack(ack(9)), now)
        .unwrap();

    assert_eq!(tracker.validate(&response).unwrap(), id);
    tracker.complete(&id).unwrap();
    assert!(tracker.is_empty());
    assert_eq!(tracker.validate(&response).unwrap(), id);
    assert!(tracker.is_completed(&id));
}

#[test]
fn tracker_capacity_and_request_deadline_are_bounded() {
    let now = Instant::now();
    let mut tracker = RequestTracker::new();
    for offset in 0..MAX_IN_FLIGHT_REQUESTS {
        tracker
            .track(
                request_id(100 + offset as u128),
                ExpectedResponse::Ack(ack(offset as u64 + 1)),
                now,
            )
            .unwrap();
    }

    let error = tracker
        .track(request_id(999), ExpectedResponse::Ack(ack(999)), now)
        .unwrap_err();
    assert_eq!(error.code(), TransportErrorCode::ResourceLimit);
    assert_eq!(tracker.len(), MAX_IN_FLIGHT_REQUESTS);
    assert!(
        tracker
            .expired(now + REQUEST_TIMEOUT, REQUEST_TIMEOUT)
            .is_some()
    );
}

#[test]
fn tracker_reports_an_ack_in_flight_independent_of_revision() {
    let now = Instant::now();
    let mut tracker = RequestTracker::new();
    assert!(!tracker.has_ack_in_flight());

    tracker
        .track(request_id(2_500), ExpectedResponse::Ack(ack(7)), now)
        .unwrap();

    assert!(tracker.has_ack(WorkspaceRevision::new(7)));
    assert!(!tracker.has_ack(WorkspaceRevision::new(8)));
    assert!(tracker.has_ack_in_flight());
}

#[tokio::test]
async fn real_session_rejects_wrong_hello_request_id() {
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            let hello = next_client_request(&mut socket).await;
            let response = encode_success(
                WorkspaceAction::WorkspaceHello,
                WorkspaceFlow::ServerResponse,
                Some(request_id(70_001)),
                MessageBody::HelloResponse(hello_response()),
            )
            .unwrap();
            assert_ne!(client_request_id(&hello), request_id(70_001));
            send_server_frame(&mut socket, response).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), test_limits()).await;

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("wrong Hello ID did not terminate Session");

    let SessionResult::Error(error) = result else {
        panic!("wrong request ID did not fail the session");
    };
    assert_eq!(error.code(), TransportErrorCode::Protocol);
    assert!(!error.retryable());
    server.finish().await;
    engine.stop().await;
}

#[tokio::test]
async fn hello_request_timeout_is_retryable() {
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        let hello = next_client_request(&mut socket).await;
        assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
        let _ = received_tx.send(());
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(None);
    let mut limits = test_limits();
    limits.request_timeout = Duration::from_millis(30);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;
    let run = tokio::spawn(async move {
        session
            .run(&mut writer, tokio_util::sync::CancellationToken::new())
            .await
    });
    received_rx.await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("session ignored request deadline")
        .unwrap();

    let SessionResult::Error(error) = result else {
        panic!("silent Hello did not time out");
    };
    assert_eq!(error.code(), TransportErrorCode::RequestTimeout);
    assert!(error.retryable());
    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn hello_wait_is_shutdown_cancellable() {
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        let hello = next_client_request(&mut socket).await;
        assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
        let _ = received_tx.send(());
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(None);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), test_limits()).await;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let cancel = shutdown.clone();
    let run = tokio::spawn(async move { session.run(&mut writer, shutdown).await });
    received_rx.await.unwrap();
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("Hello cancellation did not stop Session")
        .unwrap();
    assert!(matches!(result, SessionResult::Closed));
    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn subscribe_begin_timeout_is_retryable() {
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let _ = subscribed_tx.send(());
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(None);
    let mut limits = test_limits();
    limits.request_timeout = Duration::from_millis(30);
    limits.idle_timeout = Duration::from_secs(1);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;
    let run = tokio::spawn(async move {
        session
            .run(&mut writer, tokio_util::sync::CancellationToken::new())
            .await
    });
    subscribed_rx.await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("Subscribe did not observe its Begin deadline")
        .unwrap();
    let SessionResult::Error(error) = result else {
        panic!("missing SnapshotBegin did not time out");
    };
    assert_eq!(error.code(), TransportErrorCode::RequestTimeout);
    assert!(error.retryable());
    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn inbound_idle_timeout_is_not_reset_by_outbound_heartbeat() {
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000003").unwrap();
        let begin = fns_protocol::WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id,
            mode: fns_protocol::WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::ZERO,
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        };
        let frame = encode_success(
            WorkspaceAction::WorkspaceSnapshotBegin,
            WorkspaceFlow::ServerPush,
            None,
            MessageBody::SnapshotBegin(begin),
        )
        .unwrap();
        send_server_frame(&mut socket, frame).await;
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(None);
    let mut limits = test_limits();
    limits.request_timeout = Duration::from_secs(1);
    limits.idle_timeout = Duration::from_millis(45);
    limits.heartbeat_interval = Duration::from_millis(5);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("outbound heartbeat concealed inbound idle timeout");
    let SessionResult::Error(error) = result else {
        panic!("idle connection did not time out");
    };
    assert_eq!(error.code(), TransportErrorCode::IdleTimeout);
    assert!(error.retryable());
    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn wrong_mutation_response_id_does_not_settle_durable_outbox() {
    let (operation_tx, operation_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000004").unwrap();
        let begin = fns_protocol::WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id,
            mode: fns_protocol::WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::ZERO,
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        };
        send_server_frame(
            &mut socket,
            encode_success(
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(begin.clone()),
            )
            .unwrap(),
        )
        .await;
        send_server_frame(
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

        let mutation_frame = next_client_request(&mut socket).await;
        assert_eq!(mutation_frame.action, WorkspaceAction::WorkspaceMutation);
        let mutation = match mutation_frame.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(mutation),
                ..
            } => mutation,
            _ => panic!("expected mutation body"),
        };
        let _ = operation_tx.send(mutation.operation_id);
        let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
            workspace_id: mutation.workspace_id,
            client_id: mutation.client_id,
            operation_id: mutation.operation_id,
            revision: WorkspaceRevision::new(1),
            path_state: fns_protocol::WorkspacePathState {
                path: mutation.path,
                path_revision: WorkspaceRevision::new(1),
                kind: fns_protocol::WorkspaceEntryKind::File,
                content_hash: mutation.content_hash,
                metadata: mutation.metadata,
                tombstone: false,
            },
            old_path_state: None,
            new_path_state: None,
        };
        send_server_frame(
            &mut socket,
            encode_success(
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(request_id(70_002)),
                MessageBody::MutationAccepted(accepted),
            )
            .unwrap(),
        )
        .await;
        let _ = release_rx.await;
    })
    .await;
    let engine = TestEngine::new(Some(("local.txt", b"local")));
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), test_limits()).await;

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("wrong Mutation ID did not terminate Session");
    let operation_id = operation_rx.await.unwrap();
    let SessionResult::Error(error) = result else {
        panic!("wrong mutation response ID did not fail Session");
    };
    assert_eq!(error.code(), TransportErrorCode::Protocol);
    let pending = engine.handle.pending_commands(16).await.unwrap();
    assert!(pending.iter().any(
        |command| matches!(command, SyncCommand::Mutation(body) if body.operation_id == operation_id)
    ));

    let _ = release_tx.send(());
    server.finish().await;
    engine.stop().await;
}

#[tokio::test]
async fn wrong_mutation_response_identity_does_not_settle_durable_outbox() {
    let (operation_tx, operation_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000011").unwrap();
        send_empty_snapshot(&mut socket, stream_id).await;

        let mutation_frame = next_client_request(&mut socket).await;
        assert_eq!(mutation_frame.action, WorkspaceAction::WorkspaceMutation);
        let mutation_request_id = client_request_id(&mutation_frame);
        let mutation = match mutation_frame.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(mutation),
                ..
            } => mutation,
            _ => panic!("expected mutation body"),
        };
        let _ = operation_tx.send(mutation.operation_id);
        let accepted = fns_protocol::WorkspaceMutationAcceptedMessage {
            workspace_id: mutation.workspace_id,
            client_id: mutation.client_id,
            operation_id: operation_id(71_001),
            revision: WorkspaceRevision::new(1),
            path_state: fns_protocol::WorkspacePathState {
                path: mutation.path,
                path_revision: WorkspaceRevision::new(1),
                kind: fns_protocol::WorkspaceEntryKind::File,
                content_hash: mutation.content_hash,
                metadata: mutation.metadata,
                tombstone: false,
            },
            old_path_state: None,
            new_path_state: None,
        };
        send_server_frame(
            &mut socket,
            encode_success(
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(mutation_request_id),
                MessageBody::MutationAccepted(accepted),
            )
            .unwrap(),
        )
        .await;
        let _ = release_rx.await;
    })
    .await;
    let engine = TestEngine::new(Some(("local.txt", b"local")));
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), test_limits()).await;

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("wrong Mutation identity did not terminate Session");
    let operation_id = operation_rx.await.unwrap();
    let SessionResult::Error(error) = result else {
        panic!("wrong mutation response identity did not fail Session");
    };
    assert_eq!(error.code(), TransportErrorCode::Protocol);
    assert!(!error.retryable());
    let pending = engine.handle.pending_commands(16).await.unwrap();
    assert!(pending.iter().any(
        |command| matches!(command, SyncCommand::Mutation(body) if body.operation_id == operation_id)
    ));

    let _ = release_tx.send(());
    server.finish().await;
    engine.stop().await;
}

#[tokio::test]
async fn unanswered_mutation_times_out_without_settling_durable_outbox() {
    let (operation_tx, operation_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000012").unwrap();
        send_empty_snapshot(&mut socket, stream_id).await;

        let mutation_frame = next_client_request(&mut socket).await;
        assert_eq!(mutation_frame.action, WorkspaceAction::WorkspaceMutation);
        let operation_id = match mutation_frame.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(mutation),
                ..
            } => mutation.operation_id,
            _ => panic!("expected mutation body"),
        };
        let _ = operation_tx.send(operation_id);
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(Some(("local.txt", b"local")));
    let mut limits = test_limits();
    limits.request_timeout = Duration::from_millis(35);
    limits.idle_timeout = Duration::from_secs(1);
    limits.transfer_idle_timeout = Duration::from_secs(1);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("unanswered Mutation ignored its request deadline");
    let operation_id = operation_rx.await.unwrap();
    let SessionResult::Error(error) = result else {
        panic!("unanswered Mutation did not fail Session");
    };
    assert_eq!(error.code(), TransportErrorCode::RequestTimeout);
    assert!(error.retryable());
    let pending = engine.handle.pending_commands(16).await.unwrap();
    assert!(pending.iter().any(
        |command| matches!(command, SyncCommand::Mutation(body) if body.operation_id == operation_id)
    ));

    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn download_begin_wait_uses_transfer_timeout_not_connection_idle() {
    let content = b"blob";
    let content_hash = fns_protocol::WorkspaceContentHash::parse(&format!(
        "blake3:{}",
        blake3::hash(content).to_hex()
    ))
    .unwrap();
    let expected_hash = content_hash.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello(&mut socket).await;
            let subscribe = next_client_request(&mut socket).await;
            assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
            let stream_id =
                fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000005").unwrap();
            let begin = fns_protocol::WorkspaceSnapshotBeginMessage {
                workspace_id: workspace_id(),
                stream_id,
                mode: fns_protocol::WorkspaceSnapshotMode::Snapshot,
                from_revision: WorkspaceRevision::ZERO,
                final_revision: WorkspaceRevision::new(1),
                entry_count: 1,
                event_count: 0,
                conflict_count: 0,
            };
            send_server_frame(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceSnapshotBegin,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotBegin(begin),
                )
                .unwrap(),
            )
            .await;
            send_server_frame(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceSnapshotEntry,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotEntry(fns_protocol::WorkspaceSnapshotEntryMessage {
                        workspace_id: workspace_id(),
                        stream_id,
                        index: 0,
                        entry: fns_protocol::WorkspacePathState {
                            path: fns_protocol::WorkspacePath::parse("remote.bin").unwrap(),
                            path_revision: WorkspaceRevision::new(1),
                            kind: fns_protocol::WorkspaceEntryKind::File,
                            content_hash: fns_protocol::RequiredNullable::Value(
                                expected_hash.clone(),
                            ),
                            metadata: fns_protocol::WorkspaceFileMetadata {
                                size: content.len() as u64,
                                modified_at_ms: 1,
                                executable: false,
                            },
                            tombstone: false,
                        },
                    }),
                )
                .unwrap(),
            )
            .await;

            let need = loop {
                let request = next_client_request(&mut socket).await;
                if request.action == WorkspaceAction::WorkspaceBlobNeed {
                    break request;
                }
            };
            let need_id = client_request_id(&need);
            let operation_id = match need.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobNeedDownloadRequest(body),
                    ..
                } => body.operation_id.into_option(),
                _ => panic!("expected download BlobNeed"),
            };
            send_server_frame(
                &mut socket,
                encode_success(
                    WorkspaceAction::WorkspaceBlobNeed,
                    WorkspaceFlow::ServerResponse,
                    Some(need_id),
                    MessageBody::BlobNeedDownloadResponse(
                        fns_protocol::WorkspaceBlobNeedDownloadResponse {
                            workspace_id: workspace_id(),
                            direction: fns_protocol::WorkspaceBlobDirection::Download,
                            operation_id: operation_id
                                .map(fns_protocol::RequiredNullable::Value)
                                .unwrap_or(fns_protocol::RequiredNullable::Null),
                            content_hash: expected_hash.clone(),
                            size: content.len() as u64,
                        },
                    ),
                )
                .unwrap(),
            )
            .await;
            std::future::pending::<()>().await;
        })
        .await;
    let engine = TestEngine::new(None);
    let mut limits = test_limits();
    limits.request_timeout = Duration::from_secs(1);
    limits.idle_timeout = Duration::from_secs(1);
    limits.transfer_idle_timeout = Duration::from_millis(35);
    limits.transfer_max_lifetime = Duration::from_secs(1);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        session.run(&mut writer, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("missing download Begin did not time out");
    let SessionResult::Error(error) = result else {
        panic!("missing download Begin did not return an error");
    };
    assert_eq!(error.code(), TransportErrorCode::TransferTimeout);
    assert!(error.retryable());
    drop(server);
    engine.stop().await;
}

#[test]
fn transfer_max_lifetime_expires_despite_recent_progress() {
    let now = Instant::now();
    let transfer_id =
        fns_protocol::TransferId::parse("10000000-0000-4000-8000-000000000007").unwrap();
    let hash = fns_protocol::WorkspaceContentHash::parse(
        "blake3:abababababababababababababababababababababababababababababababab",
    )
    .unwrap();
    let mut transfers = fns_transport::transfer::TransferTable::new(1);
    transfers.reserve_transfer(transfer_id).unwrap();
    transfers.add_download(fns_transport::transfer::DownloadTransfer::new(
        transfer_id,
        workspace_id(),
        None,
        hash,
        4,
        now,
    ));
    assert_eq!(
        transfers.expired(
            now + Duration::from_millis(51),
            Duration::from_millis(50),
            Duration::from_secs(1),
        ),
        Some(transfer_id)
    );
    transfers
        .mark_progress(&transfer_id, now + Duration::from_millis(90))
        .unwrap();

    assert_eq!(
        transfers.expired(
            now + Duration::from_millis(101),
            Duration::from_millis(50),
            Duration::from_millis(100),
        ),
        Some(transfer_id)
    );
}

#[test]
fn upload_begin_and_end_require_exact_transfer_identity() {
    let now = Instant::now();
    let transfer_id =
        fns_protocol::TransferId::parse("10000000-0000-4000-8000-000000000008").unwrap();
    let other_transfer_id =
        fns_protocol::TransferId::parse("10000000-0000-4000-8000-000000000009").unwrap();
    let hash = fns_protocol::WorkspaceContentHash::parse(
        "blake3:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
    )
    .unwrap();
    let begin = fns_protocol::WorkspaceBlobBeginMessage {
        workspace_id: workspace_id(),
        transfer_id,
        direction: fns_protocol::WorkspaceBlobDirection::Upload,
        content_hash: hash.clone(),
        size: 4,
        chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
        chunk_count: 1,
    };
    let id = request_id(80_001);
    let mut tracker = RequestTracker::new();
    tracker
        .track(id, ExpectedResponse::BlobBeginUpload(begin.clone()), now)
        .unwrap();
    let mut wrong = begin.clone();
    wrong.transfer_id = other_transfer_id;
    let wrong = decode_server_text_frame(
        &encode_success(
            WorkspaceAction::WorkspaceBlobBegin,
            WorkspaceFlow::ServerResponse,
            Some(id),
            MessageBody::BlobBegin(wrong),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        tracker.validate(&wrong).unwrap_err().code(),
        TransportErrorCode::Protocol
    );
    assert!(tracker.contains(&id));

    let correct = decode_server_text_frame(
        &encode_success(
            WorkspaceAction::WorkspaceBlobBegin,
            WorkspaceFlow::ServerResponse,
            Some(id),
            MessageBody::BlobBegin(begin),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(tracker.validate(&correct).unwrap(), id);
    tracker.complete(&id).unwrap();

    let end_id = request_id(80_002);
    let end = fns_protocol::WorkspaceBlobEndMessage {
        workspace_id: workspace_id(),
        transfer_id,
        direction: fns_protocol::WorkspaceBlobDirection::Upload,
        content_hash: hash,
        size: 4,
        chunk_count: 1,
    };
    tracker
        .track(end_id, ExpectedResponse::BlobEndUpload(end.clone()), now)
        .unwrap();
    let end_response = decode_server_text_frame(
        &encode_success(
            WorkspaceAction::WorkspaceBlobEnd,
            WorkspaceFlow::ServerResponse,
            Some(end_id),
            MessageBody::BlobEnd(end),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(tracker.validate(&end_response).unwrap(), end_id);
}

#[tokio::test]
async fn subscribe_wait_is_shutdown_cancellable() {
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let _ = subscribed_tx.send(());
        std::future::pending::<()>().await;
    })
    .await;
    let engine = TestEngine::new(None);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), test_limits()).await;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let cancel = shutdown.clone();
    let run = tokio::spawn(async move { session.run(&mut writer, shutdown).await });
    subscribed_rx.await.unwrap();
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("Subscribe cancellation did not stop Session")
        .unwrap();
    assert!(matches!(result, SessionResult::Closed));
    drop(server);
    engine.stop().await;
}

#[tokio::test]
async fn pending_ack_is_sent_before_local_mutation_backlog() {
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000010").unwrap();
        send_server_frame(
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
                    final_revision: WorkspaceRevision::new(1),
                    entry_count: 0,
                    event_count: 0,
                    conflict_count: 0,
                }),
            )
            .unwrap(),
        )
        .await;
        send_server_frame(
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
                    final_revision: WorkspaceRevision::new(1),
                }),
            )
            .unwrap(),
        )
        .await;
        let request = next_client_request(&mut socket).await;
        let _ = observed_tx.send(request.action);
    })
    .await;
    let engine = TestEngine::new(Some(("local.txt", b"local")));
    let mut limits = test_limits();
    limits.drain_interval = Duration::from_millis(50);
    limits.request_timeout = Duration::from_secs(1);
    limits.idle_timeout = Duration::from_secs(1);
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;
    let run = tokio::spawn(async move {
        session
            .run(&mut writer, tokio_util::sync::CancellationToken::new())
            .await
    });

    let action = tokio::time::timeout(Duration::from_secs(2), observed_rx)
        .await
        .expect("Session sent no command after completed stream")
        .unwrap();
    let ack_was_first = action == WorkspaceAction::WorkspaceAck;
    server.finish().await;
    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("Session did not stop after scripted server closed")
        .unwrap();
    assert!(match result {
        SessionResult::Closed => true,
        SessionResult::Error(error) => error.code() == TransportErrorCode::Network,
    });
    engine.stop().await;
    assert!(ack_was_first, "local mutation was sent before pending Ack");
}

#[tokio::test]
async fn outbound_mutations_are_fifo_and_respect_encoded_byte_budget() {
    let engine = TestEngine::new(None);
    let workspace = engine._area.path().join("workspace");
    let first_path = fns_protocol::WorkspacePath::parse("first.txt").unwrap();
    let second_path = fns_protocol::WorkspacePath::parse("second-with-longer-name.txt").unwrap();
    std::fs::write(workspace.join(first_path.as_str()), b"first").unwrap();
    std::fs::write(workspace.join(second_path.as_str()), b"second").unwrap();
    engine
        .handle
        .record_local_changes(vec![
            fns_fs::FsChange::Create(first_path),
            fns_fs::FsChange::Create(second_path),
        ])
        .await
        .unwrap();

    let mutations = engine
        .handle
        .pending_commands(16)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|command| match command {
            SyncCommand::Mutation(mutation) => Some(mutation),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 2);
    let expected_paths = mutations
        .iter()
        .map(|mutation| mutation.path.clone())
        .collect::<Vec<_>>();
    let drain_byte_budget = mutations
        .iter()
        .map(|mutation| {
            encode_request(
                WorkspaceAction::WorkspaceMutation,
                request_id(90_001),
                MessageBody::Mutation(mutation.clone()),
            )
            .unwrap()
            .len()
        })
        .max()
        .unwrap();

    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let server = support::fake_server::ScriptedWorkspaceServer::start(|mut socket| async move {
        answer_hello(&mut socket).await;
        let subscribe = next_client_request(&mut socket).await;
        assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
        let stream_id =
            fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000013").unwrap();
        send_empty_snapshot(&mut socket, stream_id).await;

        let first = next_client_request(&mut socket).await;
        assert_eq!(first.action, WorkspaceAction::WorkspaceMutation);
        let first_path = match first.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(mutation),
                ..
            } => mutation.path,
            _ => panic!("expected first mutation body"),
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(80), next_client_request(&mut socket),)
                .await
                .is_err(),
            "second mutation escaped the current encoded-byte budget"
        );
        let second = tokio::time::timeout(Duration::from_secs(1), next_client_request(&mut socket))
            .await
            .expect("deferred second mutation was not sent on the next drain");
        assert_eq!(second.action, WorkspaceAction::WorkspaceMutation);
        let second_path = match second.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(mutation),
                ..
            } => mutation.path,
            _ => panic!("expected second mutation body"),
        };
        let _ = observed_tx.send(vec![first_path, second_path]);
    })
    .await;

    let mut limits = test_limits();
    limits.drain_interval = Duration::from_millis(200);
    limits.request_timeout = Duration::from_secs(2);
    limits.idle_timeout = Duration::from_secs(2);
    limits.transfer_idle_timeout = Duration::from_secs(2);
    limits.heartbeat_interval = Duration::from_secs(2);
    limits.drain_item_budget = 8;
    limits.drain_byte_budget = drain_byte_budget;
    let (session, mut writer) =
        connected_session(server.endpoint(), engine.handle.clone(), limits).await;
    let run = tokio::spawn(async move {
        session
            .run(&mut writer, tokio_util::sync::CancellationToken::new())
            .await
    });

    let observed_paths = tokio::time::timeout(Duration::from_secs(2), observed_rx)
        .await
        .expect("Session did not drain both mutations")
        .unwrap();
    assert_eq!(observed_paths, expected_paths);
    server.finish().await;
    let result = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("Session did not stop after scripted server closed")
        .unwrap();
    assert!(match result {
        SessionResult::Closed => true,
        SessionResult::Error(error) => error.code() == TransportErrorCode::Network,
    });
    engine.stop().await;
}
