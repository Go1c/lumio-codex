mod support;

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use fns_protocol::{
    ClientId, DecodedEnvelope, DecodedFrame, MessageBody, OperationId, RequestId, RequiredNullable,
    StreamId, WorkspaceAckRequest, WorkspaceAction, WorkspaceEntryKind, WorkspaceEventMessage,
    WorkspaceFileMetadata, WorkspaceFlow, WorkspaceId, WorkspaceMutation,
    WorkspaceMutationAcceptedMessage, WorkspaceMutationKind, WorkspacePath, WorkspacePathState,
    WorkspaceRevision, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage,
    WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode, decode_text_frame, encode_success,
};
use fns_sync_core::{SyncCommand, SyncEngine, SyncEngineConfig};
use fns_transport::session::{Session, SessionLimits, SessionResult};
use fns_transport::{EngineHandle, EngineWorker, TransportErrorCode, WorkspaceEndpoint, socket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type ServerSocket = WebSocketStream<tokio::net::TcpStream>;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug)]
enum ReconnectBoundary {
    SnapshotBegin,
    SnapshotEntry,
    Event,
    EndBeforeAck,
}

struct TestEngine {
    _area: tempfile::TempDir,
    workspace: PathBuf,
    handle: EngineHandle,
    worker: EngineWorker,
}

impl TestEngine {
    fn new() -> Self {
        let area = tempfile::tempdir().expect("test area");
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).expect("workspace directory");
        std::fs::create_dir_all(&state).expect("state directory");
        let config = SyncEngineConfig::new(workspace_id(), client_id(), &workspace, &state);
        let engine = SyncEngine::open(config).expect("sync engine");
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area: area,
            workspace,
            handle,
            worker,
        }
    }

    async fn stop(self) {
        self.handle.shutdown().await.expect("engine shutdown");
        drop(self.handle);
        self.worker.join().expect("engine worker join");
    }

    async fn restart(self) -> Self {
        self.handle.shutdown().await.expect("engine shutdown");
        drop(self.handle);
        self.worker.join().expect("engine worker join");
        let state = self._area.path().join("state");
        let config = SyncEngineConfig::new(workspace_id(), client_id(), &self.workspace, state);
        let engine = SyncEngine::open(config).expect("restart sync engine");
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area: self._area,
            workspace: self.workspace,
            handle,
            worker,
        }
    }

    fn with_migrated_legacy_outbox(
        stream_id: StreamId,
    ) -> (
        Self,
        WorkspaceMutation,
        WorkspaceMutationAcceptedMessage,
        WorkspaceEventMessage,
    ) {
        let area = tempfile::tempdir().expect("test area");
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).expect("workspace directory");
        std::fs::create_dir_all(&state).expect("state directory");
        std::fs::write(workspace.join("legacy-local.txt"), b"accepted").expect("legacy local file");
        let config = SyncEngineConfig::new(workspace_id(), client_id(), &workspace, &state);
        let mut engine = SyncEngine::open(config).expect("sync engine");
        engine
            .record_local_change(fns_fs::FsChange::Create(
                WorkspacePath::parse("legacy-local.txt").unwrap(),
            ))
            .expect("record local mutation");
        let mutation = engine.pending_commands(1).unwrap()[0]
            .mutation()
            .expect("mutation command");
        let path_state = WorkspacePathState {
            path: mutation.path.clone(),
            path_revision: WorkspaceRevision::new(1),
            kind: WorkspaceEntryKind::File,
            content_hash: mutation.content_hash.clone(),
            metadata: mutation.metadata.clone(),
            tombstone: false,
        };
        let accepted = WorkspaceMutationAcceptedMessage {
            workspace_id: workspace_id(),
            client_id: client_id(),
            operation_id: mutation.operation_id,
            revision: WorkspaceRevision::new(1),
            path_state: path_state.clone(),
            old_path_state: None,
            new_path_state: None,
        };
        engine
            .mutation_accepted(accepted.clone())
            .expect("seed accepted receipt");
        let event = WorkspaceEventMessage {
            workspace_id: workspace_id(),
            stream_id,
            index: 0,
            revision: WorkspaceRevision::new(1),
            operation_id: mutation.operation_id,
            origin_client_id: client_id(),
            mutation: mutation.clone(),
            path_state,
            old_path_state: None,
            new_path_state: None,
        };
        event.validate().expect("authoritative own event");
        engine.close().expect("close seed engine");
        drop(engine);

        let mutation_digest = fns_sync_core::body_digest(
            &fns_sync_core::canonical_json(&mutation).expect("canonical mutation"),
        );
        let connection = rusqlite::Connection::open(state.join("state.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE applied_operations SET body_digest = ?1, receipt_kind = 'legacy', mutation_json = NULL WHERE origin_client_id = ?2 AND operation_id = ?3",
                rusqlite::params![
                    mutation_digest.as_slice(),
                    client_id().to_string(),
                    mutation.operation_id.to_string(),
                ],
            )
            .unwrap();
        drop(connection);

        let config = SyncEngineConfig::new(workspace_id(), client_id(), &workspace, &state);
        let mut engine = SyncEngine::open(config).expect("reopen migrated state");
        engine
            .state_mut()
            .enqueue_mutation(&mutation)
            .expect("correlated legacy outbox");
        let (worker, handle) = EngineWorker::spawn(engine);
        (
            Self {
                _area: area,
                workspace,
                handle,
                worker,
            },
            mutation,
            accepted,
            event,
        )
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("10000000-0000-4000-8000-000000000001").expect("workspace id")
}

fn client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000002").expect("client id")
}

fn remote_client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000003").expect("remote client id")
}

fn stream_id(value: u128) -> StreamId {
    StreamId::parse(&uuid::Uuid::from_u128(value).to_string()).expect("stream id")
}

fn operation_id() -> OperationId {
    OperationId::parse("10000000-0000-4000-8000-000000000004").expect("operation id")
}

fn metadata() -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 1_800_000_000_000,
        executable: false,
    }
}

fn directory_state(revision: u64) -> WorkspacePathState {
    WorkspacePathState {
        path: WorkspacePath::parse("remote-dir").expect("workspace path"),
        path_revision: WorkspaceRevision::new(revision),
        kind: WorkspaceEntryKind::Directory,
        content_hash: RequiredNullable::Null,
        metadata: metadata(),
        tombstone: false,
    }
}

fn snapshot_begin(stream_id: StreamId) -> WorkspaceSnapshotBeginMessage {
    WorkspaceSnapshotBeginMessage {
        workspace_id: workspace_id(),
        stream_id,
        mode: WorkspaceSnapshotMode::Snapshot,
        from_revision: WorkspaceRevision::ZERO,
        final_revision: WorkspaceRevision::new(1),
        entry_count: 1,
        event_count: 0,
        conflict_count: 0,
    }
}

fn incremental_begin(stream_id: StreamId) -> WorkspaceSnapshotBeginMessage {
    WorkspaceSnapshotBeginMessage {
        workspace_id: workspace_id(),
        stream_id,
        mode: WorkspaceSnapshotMode::Incremental,
        from_revision: WorkspaceRevision::ZERO,
        final_revision: WorkspaceRevision::new(1),
        entry_count: 0,
        event_count: 1,
        conflict_count: 0,
    }
}

fn snapshot_entry(stream_id: StreamId) -> WorkspaceSnapshotEntryMessage {
    WorkspaceSnapshotEntryMessage {
        workspace_id: workspace_id(),
        stream_id,
        index: 0,
        entry: directory_state(1),
    }
}

fn mkdir_event(stream_id: StreamId) -> WorkspaceEventMessage {
    let path = WorkspacePath::parse("remote-dir").expect("workspace path");
    let operation_id = operation_id();
    WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id,
        index: 0,
        revision: WorkspaceRevision::new(1),
        operation_id,
        origin_client_id: remote_client_id(),
        mutation: WorkspaceMutation {
            workspace_id: workspace_id(),
            client_id: remote_client_id(),
            operation_id,
            path,
            base_path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceMutationKind::Mkdir,
            content_hash: RequiredNullable::Null,
            metadata: metadata(),
            new_path: None,
            target_base_path_revision: None,
        },
        path_state: directory_state(1),
        old_path_state: None,
        new_path_state: None,
    }
}

fn stream_end(stream_id: StreamId, mode: WorkspaceSnapshotMode) -> WorkspaceSnapshotEndMessage {
    WorkspaceSnapshotEndMessage {
        workspace_id: workspace_id(),
        stream_id,
        mode,
        delivered_count: 1,
        final_revision: WorkspaceRevision::new(1),
    }
}

fn hello_response() -> fns_protocol::WorkspaceHelloResponse {
    fns_protocol::WorkspaceHelloResponse {
        protocol_version: "2".into(),
        server_version: "reconnect-test".into(),
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

async fn next_client_request(socket: &mut ServerSocket) -> DecodedFrame {
    loop {
        let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
            .await
            .expect("timed out waiting for client frame")
            .expect("client closed before request")
            .expect("client WebSocket read");
        if let Message::Text(text) = message {
            return decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest)
                .expect("valid client request");
        }
    }
}

fn client_request_id(frame: &DecodedFrame) -> RequestId {
    match &frame.envelope {
        DecodedEnvelope::Request { request_id, .. } => *request_id,
        _ => panic!("expected client request"),
    }
}

async fn send_frame(socket: &mut ServerSocket, frame: Vec<u8>) {
    socket
        .send(Message::Text(
            String::from_utf8(frame).expect("UTF-8 frame").into(),
        ))
        .await
        .expect("send server frame");
}

async fn send_push(socket: &mut ServerSocket, action: WorkspaceAction, body: MessageBody) {
    let frame =
        encode_success(action, WorkspaceFlow::ServerPush, None, body).expect("encode server push");
    send_frame(socket, frame).await;
}

async fn answer_hello_and_read_subscribe(
    socket: &mut ServerSocket,
) -> fns_protocol::WorkspaceSubscribeRequest {
    let hello = next_client_request(socket).await;
    assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
    let response = encode_success(
        WorkspaceAction::WorkspaceHello,
        WorkspaceFlow::ServerResponse,
        Some(client_request_id(&hello)),
        MessageBody::HelloResponse(hello_response()),
    )
    .expect("encode Hello response");
    send_frame(socket, response).await;

    let subscribe = next_client_request(socket).await;
    assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
    let message = match subscribe.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::SubscribeRequest(message),
            ..
        } => message,
        _ => panic!("expected Subscribe request"),
    };
    assert_eq!(message.workspace_id, workspace_id());
    assert_eq!(message.client_id, client_id());
    message
}

async fn send_begin(socket: &mut ServerSocket, boundary: ReconnectBoundary, id: StreamId) {
    let begin = match boundary {
        ReconnectBoundary::SnapshotEntry => snapshot_begin(id),
        ReconnectBoundary::SnapshotBegin
        | ReconnectBoundary::Event
        | ReconnectBoundary::EndBeforeAck => incremental_begin(id),
    };
    send_push(
        socket,
        WorkspaceAction::WorkspaceSnapshotBegin,
        MessageBody::SnapshotBegin(begin),
    )
    .await;
}

async fn send_stream_item(socket: &mut ServerSocket, boundary: ReconnectBoundary, id: StreamId) {
    match boundary {
        ReconnectBoundary::SnapshotEntry => {
            send_push(
                socket,
                WorkspaceAction::WorkspaceSnapshotEntry,
                MessageBody::SnapshotEntry(snapshot_entry(id)),
            )
            .await;
        }
        ReconnectBoundary::SnapshotBegin
        | ReconnectBoundary::Event
        | ReconnectBoundary::EndBeforeAck => {
            send_push(
                socket,
                WorkspaceAction::WorkspaceEvent,
                MessageBody::Event(mkdir_event(id)),
            )
            .await;
        }
    }
}

async fn send_end(socket: &mut ServerSocket, boundary: ReconnectBoundary, id: StreamId) {
    let mode = match boundary {
        ReconnectBoundary::SnapshotEntry => WorkspaceSnapshotMode::Snapshot,
        ReconnectBoundary::SnapshotBegin
        | ReconnectBoundary::Event
        | ReconnectBoundary::EndBeforeAck => WorkspaceSnapshotMode::Incremental,
    };
    send_push(
        socket,
        WorkspaceAction::WorkspaceSnapshotEnd,
        MessageBody::SnapshotEnd(stream_end(id, mode)),
    )
    .await;
}

fn ack_from_request(frame: &DecodedFrame) -> WorkspaceAckRequest {
    assert_eq!(frame.action, WorkspaceAction::WorkspaceAck);
    match &frame.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::Ack(message),
            ..
        } => message.clone(),
        _ => panic!("expected Ack request"),
    }
}

async fn send_ack_response(socket: &mut ServerSocket, request_id: RequestId) {
    send_ack_response_for(socket, request_id, 1).await;
}

async fn send_ack_response_for(socket: &mut ServerSocket, request_id: RequestId, revision: u64) {
    let frame = encode_success(
        WorkspaceAction::WorkspaceAck,
        WorkspaceFlow::ServerResponse,
        Some(request_id),
        MessageBody::Ack(WorkspaceAckRequest {
            workspace_id: workspace_id(),
            client_id: client_id(),
            revision: WorkspaceRevision::new(revision),
        }),
    )
    .expect("encode Ack response");
    send_frame(socket, frame).await;
}

async fn close_server_socket(socket: &mut ServerSocket) {
    socket.close(None).await.expect("close server socket");
}

async fn run_connection<F, Fut>(engine: EngineHandle, script: F) -> SessionResult
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let server = support::fake_server::ScriptedWorkspaceServer::start(script).await;
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).expect("test endpoint");
    let token = support::secret_token("reconnect.test.jwt");
    let stream = socket::connect(&endpoint, &token, "0.1.0")
        .await
        .expect("client connect");
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
        Ok(result) => result.expect("session task"),
        Err(_) => {
            shutdown.cancel();
            tokio::time::timeout(Duration::from_secs(1), &mut run)
                .await
                .expect("session ignored cancellation")
                .expect("session task after cancellation");
            panic!("session did not finish before timeout");
        }
    };
    server.finish().await;
    result
}

fn assert_closed_or_retryable_network(result: SessionResult) {
    match result {
        SessionResult::Closed => {}
        SessionResult::Error(error) => {
            assert_eq!(error.code(), TransportErrorCode::Network);
            assert!(error.retryable());
        }
    }
}

#[tokio::test]
async fn migrated_legacy_acceptance_preserves_newer_edit_through_ack_loss_and_restart() {
    let id = stream_id(900);
    let (engine, expected_mutation, accepted, event) = TestEngine::with_migrated_legacy_outbox(id);
    let newer_bytes = b"newer-local";
    std::fs::write(engine.workspace.join("legacy-local.txt"), newer_bytes)
        .expect("newer local bytes");
    let newer_hash = fns_protocol::WorkspaceContentHash::parse(&format!(
        "blake3:{}",
        blake3::hash(newer_bytes).to_hex()
    ))
    .expect("newer local hash");
    let expected_stale_operation = expected_mutation.operation_id;
    let first_newer_hash = newer_hash.clone();
    let first_expected_mutation = expected_mutation.clone();
    let first_accepted = accepted.clone();
    let first_event = event.clone();
    let result = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotBegin,
            MessageBody::SnapshotBegin(incremental_begin(id)),
        )
        .await;

        let request = next_client_request(&mut socket).await;
        assert_eq!(request.action, WorkspaceAction::WorkspaceMutation);
        match &request.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::Mutation(actual),
                ..
            } => assert_eq!(actual, &first_expected_mutation),
            _ => panic!("expected correlated legacy mutation request"),
        }
        let response = encode_success(
            WorkspaceAction::WorkspaceMutationAccepted,
            WorkspaceFlow::ServerResponse,
            Some(client_request_id(&request)),
            MessageBody::MutationAccepted(first_accepted),
        )
        .expect("encode legacy Accepted response");
        send_frame(&mut socket, response).await;

        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceEvent,
            MessageBody::Event(first_event),
        )
        .await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotEnd,
            MessageBody::SnapshotEnd(stream_end(id, WorkspaceSnapshotMode::Incremental)),
        )
        .await;
        let mut saw_replacement = false;
        let mut saw_ack = false;
        while !saw_replacement || !saw_ack {
            let request = next_client_request(&mut socket).await;
            match &request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(mutation),
                    ..
                } => {
                    assert_ne!(mutation.operation_id, expected_stale_operation);
                    assert_eq!(
                        mutation.content_hash,
                        RequiredNullable::Value(first_newer_hash.clone())
                    );
                    assert_eq!(mutation.base_path_revision, WorkspaceRevision::new(1));
                    saw_replacement = true;
                }
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(ack),
                    ..
                } => {
                    assert_eq!(ack.revision, WorkspaceRevision::new(1));
                    saw_ack = true;
                }
                _ => panic!("unexpected request while awaiting replacement and Ack"),
            }
        }
        // Drop the first Ack response to force durable retry across an engine restart.
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(result);

    let cursor = engine
        .handle
        .cursor()
        .await
        .expect("legacy recovery cursor");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, Some(WorkspaceRevision::new(1)));
    assert_eq!(
        std::fs::read(engine.workspace.join("legacy-local.txt")).unwrap(),
        newer_bytes
    );
    let pending = engine
        .handle
        .pending_commands(16)
        .await
        .expect("pending replacement after Ack loss");
    assert!(pending.iter().any(|command| matches!(command,
        SyncCommand::Mutation(mutation)
            if mutation.operation_id != expected_mutation.operation_id
                && mutation.content_hash == RequiredNullable::Value(newer_hash.clone())
                && mutation.base_path_revision == WorkspaceRevision::new(1))));
    assert!(pending.iter().any(|command| matches!(command,
        SyncCommand::SendAck(ack) if ack.revision == WorkspaceRevision::new(1))));

    let engine = engine.restart().await;
    let cursor = engine.handle.cursor().await.expect("restarted cursor");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(cursor.pending_ack_revision, Some(WorkspaceRevision::new(1)));

    let replay_id = stream_id(901);
    let mut replay_event = event.clone();
    replay_event.stream_id = replay_id;
    let expected_replacement_hash = newer_hash.clone();
    let result = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotBegin,
            MessageBody::SnapshotBegin(incremental_begin(replay_id)),
        )
        .await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceEvent,
            MessageBody::Event(replay_event),
        )
        .await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotEnd,
            MessageBody::SnapshotEnd(stream_end(replay_id, WorkspaceSnapshotMode::Incremental)),
        )
        .await;

        let mut saw_replacement = false;
        let mut acknowledged = false;
        while !saw_replacement || !acknowledged {
            let request = next_client_request(&mut socket).await;
            match &request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(mutation),
                    ..
                } => {
                    assert_eq!(
                        mutation.content_hash,
                        RequiredNullable::Value(expected_replacement_hash.clone())
                    );
                    saw_replacement = true;
                }
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(ack),
                    ..
                } => {
                    assert_eq!(ack.revision, WorkspaceRevision::new(1));
                    send_ack_response(&mut socket, client_request_id(&request)).await;
                    acknowledged = true;
                }
                _ => panic!("unexpected request during legacy replay convergence"),
            }
        }
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(result);

    let cursor = engine.handle.cursor().await.expect("converged cursor");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert_eq!(engine.handle.active_stream_mode().await.unwrap(), None);
    assert_eq!(
        std::fs::read(engine.workspace.join("legacy-local.txt")).unwrap(),
        newer_bytes
    );
    let pending = engine
        .handle
        .pending_commands(16)
        .await
        .expect("replacement after exact Ack");
    assert!(pending.iter().any(|command| matches!(command,
        SyncCommand::Mutation(mutation)
            if mutation.content_hash == RequiredNullable::Value(newer_hash.clone())
                && mutation.base_path_revision == WorkspaceRevision::new(1))));
    assert!(
        pending
            .iter()
            .all(|command| !matches!(command, SyncCommand::SendAck(_)))
    );

    let state_database = engine._area.path().join("state/state.sqlite");
    let connection = rusqlite::Connection::open(state_database).unwrap();
    let (receipt_kind, outbox_count, intent_count, provisional_count): (String, i64, i64, i64) =
        connection
            .query_row(
                "SELECT (SELECT receipt_kind FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2), (SELECT COUNT(*) FROM outbox), (SELECT COUNT(*) FROM local_intents), (SELECT COUNT(*) FROM provisional_mutation_acceptances)",
                rusqlite::params![client_id().to_string(), expected_mutation.operation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    assert_eq!(receipt_kind, "mutation_result");
    assert_eq!(outbox_count, 1);
    assert_eq!(intent_count, 0);
    assert_eq!(provisional_count, 0);
    drop(connection);

    let engine = engine.restart().await;
    let pending = engine
        .handle
        .pending_commands(16)
        .await
        .expect("replacement after final restart");
    assert!(pending.iter().any(|command| matches!(command,
        SyncCommand::Mutation(mutation)
            if mutation.content_hash == RequiredNullable::Value(newer_hash.clone()))));
    engine.stop().await;
}

#[tokio::test]
async fn disconnect_at_each_stream_boundary_converges() {
    for (case, boundary) in [
        ReconnectBoundary::SnapshotBegin,
        ReconnectBoundary::SnapshotEntry,
        ReconnectBoundary::Event,
        ReconnectBoundary::EndBeforeAck,
    ]
    .into_iter()
    .enumerate()
    {
        let engine = TestEngine::new();
        let old_stream_id = stream_id(100 + case as u128);
        let first = run_connection(engine.handle.clone(), move |mut socket| async move {
            let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
            assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
            send_begin(&mut socket, boundary, old_stream_id).await;
            match boundary {
                ReconnectBoundary::SnapshotBegin => {}
                ReconnectBoundary::SnapshotEntry | ReconnectBoundary::Event => {
                    send_stream_item(&mut socket, boundary, old_stream_id).await;
                }
                ReconnectBoundary::EndBeforeAck => {
                    send_stream_item(&mut socket, boundary, old_stream_id).await;
                    send_end(&mut socket, boundary, old_stream_id).await;
                }
            }
            close_server_socket(&mut socket).await;
        })
        .await;
        assert_closed_or_retryable_network(first);

        let before_reconnect = engine
            .handle
            .cursor()
            .await
            .expect("cursor after disconnect");
        assert_eq!(before_reconnect.last_ack_revision, WorkspaceRevision::ZERO);
        assert_eq!(
            before_reconnect.pending_ack_revision,
            matches!(boundary, ReconnectBoundary::EndBeforeAck)
                .then_some(WorkspaceRevision::new(1))
        );
        assert!(engine.handle.active_stream_mode().await.unwrap().is_some());

        let replacement_stream_id = stream_id(200 + case as u128);
        let (early_ack_tx, early_ack_rx) = tokio::sync::oneshot::channel();
        let second = run_connection(engine.handle.clone(), move |mut socket| async move {
            let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
            assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
            send_begin(&mut socket, boundary, replacement_stream_id).await;

            let early_ack = if matches!(boundary, ReconnectBoundary::EndBeforeAck) {
                tokio::time::timeout(Duration::from_millis(80), next_client_request(&mut socket))
                    .await
                    .ok()
            } else {
                None
            };
            let _ = early_ack_tx.send(early_ack.is_some());

            send_stream_item(&mut socket, boundary, replacement_stream_id).await;
            send_end(&mut socket, boundary, replacement_stream_id).await;
            let ack_request = match early_ack {
                Some(request) => request,
                None => next_client_request(&mut socket).await,
            };
            let ack = ack_from_request(&ack_request);
            assert_eq!(ack.revision, WorkspaceRevision::new(1));
            send_ack_response(&mut socket, client_request_id(&ack_request)).await;
            close_server_socket(&mut socket).await;
        })
        .await;
        assert_closed_or_retryable_network(second);
        assert!(
            !early_ack_rx.await.expect("early Ack observation"),
            "pending Ack escaped before the replacement stream ended at {boundary:?}"
        );

        let cursor = engine.handle.cursor().await.expect("converged cursor");
        assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
        assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
        assert_eq!(cursor.pending_ack_revision, None);
        assert_eq!(engine.handle.active_stream_mode().await.unwrap(), None);
        assert!(engine.workspace.join("remote-dir").is_dir());
        assert!(
            engine
                .handle
                .pending_commands(16)
                .await
                .expect("stable pending commands")
                .into_iter()
                .all(|command| !matches!(command, SyncCommand::SendAck(_)))
        );
        engine.stop().await;
    }
}

#[tokio::test]
async fn ack_response_loss_converges() {
    let engine = TestEngine::new();
    let first_stream_id = stream_id(301);
    let (first_ack_tx, first_ack_rx) = tokio::sync::oneshot::channel();
    let first = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_begin(&mut socket, ReconnectBoundary::Event, first_stream_id).await;
        send_stream_item(&mut socket, ReconnectBoundary::Event, first_stream_id).await;
        send_end(&mut socket, ReconnectBoundary::Event, first_stream_id).await;
        let request = next_client_request(&mut socket).await;
        let ack = ack_from_request(&request);
        assert_eq!(ack.revision, WorkspaceRevision::new(1));
        first_ack_tx
            .send((client_request_id(&request), ack))
            .expect("first Ack observation");
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(first);
    let (first_request_id, durable_ack) = first_ack_rx.await.expect("first Ack request");

    let after_loss = engine
        .handle
        .cursor()
        .await
        .expect("cursor after response loss");
    assert_eq!(after_loss.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(
        after_loss.pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );

    let second_stream_id = stream_id(302);
    let (second_ack_tx, second_ack_rx) = tokio::sync::oneshot::channel();
    let second_expected_ack = durable_ack.clone();
    let second = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_begin(&mut socket, ReconnectBoundary::Event, second_stream_id).await;
        send_stream_item(&mut socket, ReconnectBoundary::Event, second_stream_id).await;
        send_end(&mut socket, ReconnectBoundary::Event, second_stream_id).await;
        let retry = next_client_request(&mut socket).await;
        let retry_ack = ack_from_request(&retry);
        assert_eq!(retry_ack, second_expected_ack);
        let retry_request_id = client_request_id(&retry);
        assert_ne!(retry_request_id, first_request_id);
        second_ack_tx
            .send(retry_request_id)
            .expect("second Ack observation");

        send_ack_response(&mut socket, first_request_id).await;
    })
    .await;
    match second {
        SessionResult::Error(error) => {
            assert_eq!(error.code(), TransportErrorCode::Protocol);
            assert!(!error.retryable());
        }
        SessionResult::Closed => panic!("stale Ack response was silently accepted"),
    }
    let second_request_id = second_ack_rx.await.expect("second Ack request");
    let after_stale_response = engine
        .handle
        .cursor()
        .await
        .expect("cursor after stale response");
    assert_eq!(
        after_stale_response.last_ack_revision,
        WorkspaceRevision::ZERO
    );
    assert_eq!(
        after_stale_response.pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );

    let third_stream_id = stream_id(303);
    let (third_ack_tx, third_ack_rx) = tokio::sync::oneshot::channel();
    let third = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_begin(&mut socket, ReconnectBoundary::Event, third_stream_id).await;
        send_stream_item(&mut socket, ReconnectBoundary::Event, third_stream_id).await;
        send_end(&mut socket, ReconnectBoundary::Event, third_stream_id).await;
        let retry = next_client_request(&mut socket).await;
        let retry_ack = ack_from_request(&retry);
        assert_eq!(retry_ack, durable_ack);
        let retry_request_id = client_request_id(&retry);
        assert_ne!(retry_request_id, first_request_id);
        assert_ne!(retry_request_id, second_request_id);
        third_ack_tx
            .send(retry_request_id)
            .expect("third Ack observation");
        send_ack_response(&mut socket, retry_request_id).await;
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(third);
    let _third_request_id = third_ack_rx.await.expect("third Ack request");

    let cursor = engine.handle.cursor().await.expect("converged cursor");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert_eq!(engine.handle.active_stream_mode().await.unwrap(), None);
    assert!(engine.workspace.join("remote-dir").is_dir());
    assert!(
        engine
            .handle
            .pending_commands(16)
            .await
            .expect("stable pending commands")
            .into_iter()
            .all(|command| !matches!(command, SyncCommand::SendAck(_)))
    );
    engine.stop().await;
}

#[tokio::test]
async fn duplicate_stream_end_while_ack_is_in_flight_is_idempotent() {
    let engine = TestEngine::new();
    let id = stream_id(350);
    let result = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_begin(&mut socket, ReconnectBoundary::Event, id).await;
        send_stream_item(&mut socket, ReconnectBoundary::Event, id).await;
        send_end(&mut socket, ReconnectBoundary::Event, id).await;

        let ack = next_client_request(&mut socket).await;
        assert_eq!(ack_from_request(&ack).revision, WorkspaceRevision::new(1));

        // The stream may be retransmitted before the response to its durable Ack.
        send_end(&mut socket, ReconnectBoundary::Event, id).await;
        send_ack_response(&mut socket, client_request_id(&ack)).await;
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(result);

    let cursor = engine
        .handle
        .cursor()
        .await
        .expect("cursor after duplicate End");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert_eq!(engine.handle.active_stream_mode().await.unwrap(), None);
    engine.stop().await;
}

#[tokio::test]
async fn replacement_blob_wait_drops_stale_ack_and_only_acks_durable_final() {
    let engine = TestEngine::new();
    let old_stream_id = stream_id(401);
    let (old_ack_tx, old_ack_rx) = tokio::sync::oneshot::channel();
    let first = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_begin(&mut socket, ReconnectBoundary::Event, old_stream_id).await;
        send_stream_item(&mut socket, ReconnectBoundary::Event, old_stream_id).await;
        send_end(&mut socket, ReconnectBoundary::Event, old_stream_id).await;
        let ack = next_client_request(&mut socket).await;
        assert_eq!(ack_from_request(&ack).revision, WorkspaceRevision::new(1));
        old_ack_tx
            .send(client_request_id(&ack))
            .expect("old Ack observation");
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(first);
    let old_ack_request_id = old_ack_rx.await.expect("old Ack request");
    let after_loss = engine.handle.cursor().await.expect("cursor after Ack loss");
    assert_eq!(after_loss.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(
        after_loss.pending_ack_revision,
        Some(WorkspaceRevision::new(1))
    );

    let content = b"replacement-content".to_vec();
    let content_hash = fns_protocol::WorkspaceContentHash::parse(&format!(
        "blake3:{}",
        blake3::hash(&content).to_hex()
    ))
    .expect("content hash");
    let replacement_stream_id = stream_id(402);
    let transfer_id = fns_protocol::TransferId::parse("10000000-0000-4000-8000-000000000403")
        .expect("transfer id");
    let expected_content = content.clone();
    let expected_hash = content_hash.clone();
    let second = run_connection(engine.handle.clone(), move |mut socket| async move {
        let subscribe = answer_hello_and_read_subscribe(&mut socket).await;
        assert_eq!(subscribe.last_ack_revision, WorkspaceRevision::ZERO);
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotBegin,
            MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                workspace_id: workspace_id(),
                stream_id: replacement_stream_id,
                mode: WorkspaceSnapshotMode::Snapshot,
                from_revision: WorkspaceRevision::ZERO,
                final_revision: WorkspaceRevision::new(2),
                entry_count: 1,
                event_count: 0,
                conflict_count: 0,
            }),
        )
        .await;

        tokio::time::sleep(Duration::from_millis(80)).await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotEntry,
            MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
                workspace_id: workspace_id(),
                stream_id: replacement_stream_id,
                index: 0,
                entry: WorkspacePathState {
                    path: WorkspacePath::parse("replacement.bin").expect("replacement path"),
                    path_revision: WorkspaceRevision::new(2),
                    kind: WorkspaceEntryKind::File,
                    content_hash: RequiredNullable::Value(expected_hash.clone()),
                    metadata: WorkspaceFileMetadata {
                        size: expected_content.len() as u64,
                        modified_at_ms: 1_800_000_000_002,
                        executable: false,
                    },
                    tombstone: false,
                },
            }),
        )
        .await;

        let need =
            tokio::time::timeout(Duration::from_millis(300), next_client_request(&mut socket))
                .await
                .expect("stale Ack blocked replacement BlobNeed");
        assert_eq!(need.action, WorkspaceAction::WorkspaceBlobNeed);
        let need_request_id = client_request_id(&need);
        let operation_id = match need.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::BlobNeedDownloadRequest(message),
                ..
            } => {
                assert_eq!(message.content_hash, expected_hash);
                assert_eq!(message.size, RequiredNullable::Null);
                message.operation_id.into_option()
            }
            _ => panic!("expected download BlobNeed"),
        };
        send_frame(
            &mut socket,
            encode_success(
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerResponse,
                Some(need_request_id),
                MessageBody::BlobNeedDownloadResponse(
                    fns_protocol::WorkspaceBlobNeedDownloadResponse {
                        workspace_id: workspace_id(),
                        direction: fns_protocol::WorkspaceBlobDirection::Download,
                        operation_id: operation_id
                            .map(RequiredNullable::Value)
                            .unwrap_or(RequiredNullable::Null),
                        content_hash: expected_hash.clone(),
                        size: expected_content.len() as u64,
                    },
                ),
            )
            .expect("encode BlobNeed response"),
        )
        .await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceBlobBegin,
            MessageBody::BlobBegin(fns_protocol::WorkspaceBlobBeginMessage {
                workspace_id: workspace_id(),
                transfer_id,
                direction: fns_protocol::WorkspaceBlobDirection::Download,
                content_hash: expected_hash.clone(),
                size: expected_content.len() as u64,
                chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                chunk_count: 1,
            }),
        )
        .await;
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceSnapshotEnd,
            MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                workspace_id: workspace_id(),
                stream_id: replacement_stream_id,
                mode: WorkspaceSnapshotMode::Snapshot,
                delivered_count: 1,
                final_revision: WorkspaceRevision::new(2),
            }),
        )
        .await;

        if let Ok(request) =
            tokio::time::timeout(Duration::from_millis(120), next_client_request(&mut socket)).await
        {
            panic!(
                "request {:?} escaped before replacement blob was durable",
                request.action
            );
        }

        let binary = fns_protocol::encode_binary_frame(
            fns_protocol::WorkspaceBlobDirection::Download,
            true,
            transfer_id,
            0,
            0,
            &expected_content,
        )
        .expect("encode blob chunk");
        socket
            .send(Message::Binary(binary.into()))
            .await
            .expect("send blob chunk");
        let download_end = fns_protocol::WorkspaceBlobEndMessage {
            workspace_id: workspace_id(),
            transfer_id,
            direction: fns_protocol::WorkspaceBlobDirection::Download,
            content_hash: expected_hash,
            size: expected_content.len() as u64,
            chunk_count: 1,
        };
        send_push(
            &mut socket,
            WorkspaceAction::WorkspaceBlobEnd,
            MessageBody::BlobEnd(download_end.clone()),
        )
        .await;

        let end_request = next_client_request(&mut socket).await;
        assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
        send_frame(
            &mut socket,
            encode_success(
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(client_request_id(&end_request)),
                MessageBody::BlobEnd(download_end),
            )
            .expect("encode download End response"),
        )
        .await;
        let ack = next_client_request(&mut socket).await;
        let ack_body = ack_from_request(&ack);
        assert_eq!(ack_body.revision, WorkspaceRevision::new(2));
        assert_ne!(client_request_id(&ack), old_ack_request_id);
        send_ack_response_for(&mut socket, client_request_id(&ack), 2).await;
        close_server_socket(&mut socket).await;
    })
    .await;
    assert_closed_or_retryable_network(second);

    let bytes = std::fs::read(engine.workspace.join("replacement.bin"))
        .expect("replacement file was not materialized");
    assert_eq!(bytes, content);
    assert_eq!(bytes.len(), content.len());
    assert_eq!(blake3::hash(&bytes), blake3::hash(&content));
    let cursor = engine.handle.cursor().await.expect("converged cursor");
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(2));
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(2));
    assert_eq!(cursor.pending_ack_revision, None);
    assert_eq!(engine.handle.active_stream_mode().await.unwrap(), None);
    engine.stop().await;
}
