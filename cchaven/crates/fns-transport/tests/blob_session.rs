mod support;

use std::path::PathBuf;
use std::time::Duration;

use fns_protocol::{
    ClientId, ConflictId, DecodedEnvelope, DecodedFrame, MessageBody, OperationId, RequestId,
    RequiredNullable, StreamId, TransferId, WorkspaceAction, WorkspaceBlobBeginMessage,
    WorkspaceBlobDirection, WorkspaceBlobEndMessage, WorkspaceConflictChoice,
    WorkspaceConflictCreatedMessage, WorkspaceConflictKind, WorkspaceConflictResolvedMessage,
    WorkspaceConflictSide, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage,
    WorkspaceFileMetadata, WorkspaceFlow, WorkspaceId, WorkspaceMutation, WorkspaceMutationKind,
    WorkspaceMutationRejectReason, WorkspaceMutationRejectedMessage, WorkspacePath,
    WorkspacePathState, WorkspaceRevision, WorkspaceSnapshotBeginMessage,
    WorkspaceSnapshotEndMessage, WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode,
    decode_binary_frame, decode_text_frame, encode_binary_frame, encode_success,
};
use fns_sync_core::{SyncEngine, SyncEngineConfig};
use fns_transport::session::{Session, SessionLimits, SessionResult};
use fns_transport::{EngineHandle, EngineWorker, TransportErrorCode, WorkspaceEndpoint, socket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

const TEST_TIMEOUT: Duration = Duration::from_secs(4);

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

fn client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

fn remote_client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000003").unwrap()
}

fn operation_id(value: u128) -> OperationId {
    OperationId::parse(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}

fn stream_id(value: u128) -> StreamId {
    StreamId::parse(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}

fn transfer_id(value: u128) -> TransferId {
    TransferId::parse(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}

fn content_hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex())).unwrap()
}

fn limits() -> SessionLimits {
    SessionLimits {
        heartbeat_interval: Duration::from_secs(30),
        drain_interval: Duration::from_millis(5),
        request_timeout: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(2),
        transfer_idle_timeout: Duration::from_secs(1),
        transfer_max_lifetime: Duration::from_secs(3),
        drain_item_budget: 8,
        drain_byte_budget: fns_protocol::BLOB_HEADER_LEN + fns_protocol::BLOB_CHUNK_BYTES as usize,
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

#[derive(Clone)]
struct ExpectedDownload {
    path: String,
    bytes: Vec<u8>,
    hash: WorkspaceContentHash,
}

impl TestEngine {
    fn new(local_file: Option<(&str, &[u8])>) -> Self {
        let area = tempfile::tempdir().unwrap();
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        if let Some((path, bytes)) = local_file {
            std::fs::write(workspace.join(path), bytes).unwrap();
        }
        let mut engine = SyncEngine::open(SyncEngineConfig::new(
            workspace_id(),
            client_id(),
            &workspace,
            &state,
        ))
        .unwrap();
        if local_file.is_some() {
            engine
                .record_local_changes([fns_fs::FsChange::RescanRequired])
                .unwrap();
        }
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area: area,
            workspace,
            state,
            handle,
            worker,
        }
    }

    fn new_sparse(path: &str, size: u64) -> Self {
        let area = tempfile::tempdir().unwrap();
        let workspace = area.path().join("workspace");
        let state = area.path().join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let file = std::fs::File::create(workspace.join(path)).unwrap();
        file.set_len(size).unwrap();
        let mut engine = SyncEngine::open(SyncEngineConfig::new(
            workspace_id(),
            client_id(),
            &workspace,
            &state,
        ))
        .unwrap();
        engine
            .record_local_changes([fns_fs::FsChange::RescanRequired])
            .unwrap();
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area: area,
            workspace,
            state,
            handle,
            worker,
        }
    }

    async fn stop(self) {
        self.handle.shutdown().await.unwrap();
        drop(self.handle);
        self.worker.join().unwrap();
    }

    async fn restart(self) -> Self {
        let Self {
            _area,
            workspace,
            state,
            handle,
            worker,
        } = self;
        handle.shutdown().await.unwrap();
        drop(handle);
        worker.join().unwrap();
        let engine = SyncEngine::open(SyncEngineConfig::new(
            workspace_id(),
            client_id(),
            &workspace,
            &state,
        ))
        .unwrap();
        let (worker, handle) = EngineWorker::spawn(engine);
        Self {
            _area,
            workspace,
            state,
            handle,
            worker,
        }
    }
}

async fn run_download_case(case: &str, bytes: Vec<u8>, seed: u128) {
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join(format!("{case}.bin"));
    let remote_path = format!("{case}.bin");
    let server_destination = destination.clone();
    let expected_hash = content_hash(&bytes);
    let expected_hash_for_server = expected_hash.clone();
    let server_bytes = bytes.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let stream = stream_id(seed);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    from_revision: WorkspaceRevision::ZERO,
                    final_revision: WorkspaceRevision::new(1),
                    entry_count: 1,
                    event_count: 0,
                    conflict_count: 0,
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEntry,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    index: 0,
                    entry: WorkspacePathState {
                        path: WorkspacePath::parse(&remote_path).unwrap(),
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: RequiredNullable::Value(expected_hash_for_server.clone()),
                        metadata: WorkspaceFileMetadata {
                            size: server_bytes.len() as u64,
                            modified_at_ms: 1,
                            executable: false,
                        },
                        tombstone: false,
                    },
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    delivered_count: 1,
                    final_revision: WorkspaceRevision::new(1),
                }),
            )
            .await;
            let need = next_request(&mut socket).await;
            assert_eq!(need.action, WorkspaceAction::WorkspaceBlobNeed);
            let operation_id = match &need.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobNeedDownloadRequest(body),
                    ..
                } => body.operation_id.clone(),
                _ => panic!("expected BlobNeed download"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&need)),
                MessageBody::BlobNeedDownloadResponse(
                    fns_protocol::WorkspaceBlobNeedDownloadResponse {
                        workspace_id: workspace_id(),
                        direction: WorkspaceBlobDirection::Download,
                        operation_id,
                        content_hash: expected_hash_for_server.clone(),
                        size: server_bytes.len() as u64,
                    },
                ),
            )
            .await;
            let transfer = transfer_id(seed + 1);
            let chunk_count = fns_transport::blob::chunk_count(server_bytes.len() as u64);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
                    workspace_id: workspace_id(),
                    transfer_id: transfer,
                    direction: WorkspaceBlobDirection::Download,
                    content_hash: expected_hash_for_server.clone(),
                    size: server_bytes.len() as u64,
                    chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                    chunk_count,
                }),
            )
            .await;
            let mut offset = 0_u64;
            for (index, chunk) in server_bytes
                .chunks(fns_protocol::BLOB_CHUNK_BYTES as usize)
                .enumerate()
            {
                let final_chunk = offset + chunk.len() as u64 == server_bytes.len() as u64;
                socket
                    .send(Message::Binary(
                        encode_binary_frame(
                            WorkspaceBlobDirection::Download,
                            final_chunk,
                            transfer,
                            index as u64,
                            offset,
                            chunk,
                        )
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
                offset += chunk.len() as u64;
            }
            let end = WorkspaceBlobEndMessage {
                workspace_id: workspace_id(),
                transfer_id: transfer,
                direction: WorkspaceBlobDirection::Download,
                content_hash: expected_hash_for_server,
                size: server_bytes.len() as u64,
                chunk_count,
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end.clone()),
            )
            .await;
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            assert!(!server_destination.exists());
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;
            let ack = next_request(&mut socket).await;
            let ack_body = match &ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Ack"),
            };
            assert_eq!(ack_body.revision, WorkspaceRevision::new(1));
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&ack)),
                MessageBody::Ack(ack_body),
            )
            .await;
            let _ = socket.close(None).await;
        })
        .await;
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(
        matches!(result, SessionResult::Closed),
        "session result: {result:?}"
    );
    let actual = std::fs::read(&destination).unwrap();
    assert_eq!(actual, bytes);
    assert_eq!(actual.len(), bytes.len());
    assert_eq!(blake3::hash(&actual), blake3::hash(&bytes));
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[tokio::test]
async fn download_empty_binary_and_chunk_boundaries_are_streamed_and_acked() {
    let chunk = fns_protocol::BLOB_CHUNK_BYTES as usize;
    let cases = [
        ("empty", Vec::new()),
        ("binary", vec![0, 255, 1, 0, 128, 7]),
        ("exact-boundary", vec![0x31; chunk]),
        ("boundary-plus-one", vec![0x42; chunk + 1]),
        ("multi-chunk", vec![0x53; chunk * 2 + 17]),
    ];
    for (index, (name, bytes)) in cases.into_iter().enumerate() {
        run_download_case(name, bytes, 1_000 + index as u128 * 10).await;
    }
}

#[tokio::test]
async fn snapshot_downloads_larger_than_outbound_queue_are_drained_from_durable_state() {
    const FILE_COUNT: usize = 12;
    const ENTRY_COUNT: usize = FILE_COUNT + 1;
    const QUEUE_CAPACITY: usize = 8;

    let engine = TestEngine::new(None);
    let files = (0..FILE_COUNT)
        .map(|index| {
            let path = format!("nested/remote-{index:02}.bin");
            let bytes = vec![index as u8, 0, 255, (index as u8).wrapping_mul(17), 128];
            let hash = content_hash(&bytes);
            (path, bytes, hash)
        })
        .collect::<Vec<_>>();
    let server_files = files.clone();

    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let stream = stream_id(40_000);
            let final_revision = WorkspaceRevision::new(FILE_COUNT as u64);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    from_revision: WorkspaceRevision::ZERO,
                    final_revision,
                    entry_count: ENTRY_COUNT as u32,
                    event_count: 0,
                    conflict_count: 0,
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEntry,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    index: 0,
                    entry: WorkspacePathState {
                        path: WorkspacePath::parse("nested").unwrap(),
                        path_revision: final_revision,
                        kind: WorkspaceEntryKind::Directory,
                        content_hash: RequiredNullable::Null,
                        metadata: WorkspaceFileMetadata {
                            size: 0,
                            modified_at_ms: 0,
                            executable: false,
                        },
                        tombstone: false,
                    },
                }),
            )
            .await;
            for (index, (path, bytes, hash)) in server_files.iter().enumerate() {
                send_success(
                    &mut socket,
                    WorkspaceAction::WorkspaceSnapshotEntry,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
                        workspace_id: workspace_id(),
                        stream_id: stream,
                        index: index as u32 + 1,
                        entry: WorkspacePathState {
                            path: WorkspacePath::parse(path).unwrap(),
                            path_revision: final_revision,
                            kind: WorkspaceEntryKind::File,
                            content_hash: RequiredNullable::Value(hash.clone()),
                            metadata: WorkspaceFileMetadata {
                                size: bytes.len() as u64,
                                modified_at_ms: index as i64 + 1,
                                executable: false,
                            },
                            tombstone: false,
                        },
                    }),
                )
                .await;
            }
            // Match the real service: finish the snapshot before reading any BlobNeed.
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    delivered_count: ENTRY_COUNT as u32,
                    final_revision,
                }),
            )
            .await;

            let mut requested = [false; FILE_COUNT];
            let mut completed = [false; FILE_COUNT];
            let mut transfer_ends = Vec::new();
            while completed.iter().any(|done| !done) {
                let request = next_request(&mut socket).await;
                match &request.envelope {
                    DecodedEnvelope::Request {
                        body: MessageBody::BlobNeedDownloadRequest(body),
                        ..
                    } => {
                        assert_eq!(request.action, WorkspaceAction::WorkspaceBlobNeed);
                        assert_eq!(body.operation_id, RequiredNullable::Null);
                        assert_eq!(body.size, RequiredNullable::Null);
                        let index = server_files
                            .iter()
                            .position(|(_, _, hash)| *hash == body.content_hash)
                            .expect("BlobNeed requested an unknown hash");
                        assert!(!requested[index], "duplicate BlobNeed for index {index}");
                        requested[index] = true;
                        let (_, bytes, hash) = &server_files[index];
                        send_success(
                            &mut socket,
                            WorkspaceAction::WorkspaceBlobNeed,
                            WorkspaceFlow::ServerResponse,
                            Some(request_id(&request)),
                            MessageBody::BlobNeedDownloadResponse(
                                fns_protocol::WorkspaceBlobNeedDownloadResponse {
                                    workspace_id: workspace_id(),
                                    direction: WorkspaceBlobDirection::Download,
                                    operation_id: RequiredNullable::Null,
                                    content_hash: hash.clone(),
                                    size: bytes.len() as u64,
                                },
                            ),
                        )
                        .await;
                        let transfer = transfer_id(41_000 + index as u128);
                        let end = WorkspaceBlobEndMessage {
                            workspace_id: workspace_id(),
                            transfer_id: transfer,
                            direction: WorkspaceBlobDirection::Download,
                            content_hash: hash.clone(),
                            size: bytes.len() as u64,
                            chunk_count: 1,
                        };
                        send_success(
                            &mut socket,
                            WorkspaceAction::WorkspaceBlobBegin,
                            WorkspaceFlow::ServerPush,
                            None,
                            MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
                                workspace_id: workspace_id(),
                                transfer_id: transfer,
                                direction: WorkspaceBlobDirection::Download,
                                content_hash: hash.clone(),
                                size: bytes.len() as u64,
                                chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                                chunk_count: 1,
                            }),
                        )
                        .await;
                        socket
                            .send(Message::Binary(
                                encode_binary_frame(
                                    WorkspaceBlobDirection::Download,
                                    true,
                                    transfer,
                                    0,
                                    0,
                                    bytes,
                                )
                                .unwrap()
                                .into(),
                            ))
                            .await
                            .unwrap();
                        send_success(
                            &mut socket,
                            WorkspaceAction::WorkspaceBlobEnd,
                            WorkspaceFlow::ServerPush,
                            None,
                            MessageBody::BlobEnd(end.clone()),
                        )
                        .await;
                        transfer_ends.push((index, end));
                    }
                    DecodedEnvelope::Request {
                        body: MessageBody::BlobEnd(body),
                        ..
                    } => {
                        assert_eq!(request.action, WorkspaceAction::WorkspaceBlobEnd);
                        let (index, expected) = transfer_ends
                            .iter()
                            .find(|(_, end)| end.transfer_id == body.transfer_id)
                            .expect("BlobEnd referenced an unknown transfer");
                        assert_eq!(body, expected);
                        assert!(!completed[*index], "duplicate BlobEnd for index {index}");
                        completed[*index] = true;
                        send_success(
                            &mut socket,
                            WorkspaceAction::WorkspaceBlobEnd,
                            WorkspaceFlow::ServerResponse,
                            Some(request_id(&request)),
                            MessageBody::BlobEnd(expected.clone()),
                        )
                        .await;
                    }
                    DecodedEnvelope::Request {
                        body: MessageBody::Ack(_),
                        ..
                    } => panic!("stream Ack arrived before every Blob was committed"),
                    _ => panic!("unexpected request while draining snapshot blobs: {request:?}"),
                }
            }
            assert!(requested.into_iter().all(|value| value));

            let ack = next_request(&mut socket).await;
            let ack_body = match &ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected final stream Ack"),
            };
            assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
            assert_eq!(ack_body.revision, final_revision);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&ack)),
                MessageBody::Ack(ack_body),
            )
            .await;
            socket.close(None).await.unwrap();
        })
        .await;

    let mut bounded_limits = limits();
    bounded_limits.drain_interval = Duration::from_millis(200);
    bounded_limits.pending_outbound_capacity = QUEUE_CAPACITY;
    let (session, mut writer) =
        connected_session_with_limits(server.endpoint(), engine.handle.clone(), bounded_limits)
            .await;
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(matches!(result, SessionResult::Closed), "{result:?}");

    for (path, expected, expected_hash) in files {
        let actual = std::fs::read(engine.workspace.join(path)).unwrap();
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual, expected);
        assert_eq!(content_hash(&actual), expected_hash);
    }
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(
        cursor.last_ack_revision,
        WorkspaceRevision::new(FILE_COUNT as u64)
    );
    assert_eq!(
        cursor.last_applied_revision,
        WorkspaceRevision::new(FILE_COUNT as u64)
    );
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.handle.active_stream_mode().await.unwrap().is_none());
    let pending = engine.handle.pending_commands(32).await.unwrap();
    assert!(
        pending.is_empty(),
        "pending commands after Ack: {pending:?}"
    );
    engine.stop().await;
}

#[tokio::test]
async fn incremental_downloads_larger_than_outbound_queue_are_drained_without_reconnect() {
    const EVENT_COUNT: usize = 10;
    const RESOLUTION_COUNT: usize = 2;
    const FILE_COUNT: usize = EVENT_COUNT + RESOLUTION_COUNT;
    const QUEUE_CAPACITY: usize = 8;

    let engine = TestEngine::new(None);
    let stream = stream_id(42_000);
    let files = (0..FILE_COUNT)
        .map(|index| {
            let path = if index < EVENT_COUNT {
                format!("incremental-event-{index:02}.bin")
            } else {
                format!("incremental-resolved-{:02}.bin", index - EVENT_COUNT)
            };
            let bytes = vec![
                0x40_u8.wrapping_add(index as u8),
                0,
                255,
                (index as u8).wrapping_mul(19),
                128,
            ];
            let hash = content_hash(&bytes);
            ExpectedDownload { path, bytes, hash }
        })
        .collect::<Vec<_>>();
    let events = files[..EVENT_COUNT]
        .iter()
        .enumerate()
        .map(|(index, file)| {
            remote_file_event(
                stream,
                index as u32,
                index as u64 + 1,
                &file.path,
                &file.bytes,
                42_100 + index as u128,
            )
        })
        .collect::<Vec<_>>();
    let resolutions = files[EVENT_COUNT..]
        .iter()
        .enumerate()
        .map(|(index, file)| {
            remote_file_resolution(
                EVENT_COUNT as u64 + index as u64 + 1,
                &file.path,
                &file.bytes,
                42_200 + index as u128,
            )
        })
        .collect::<Vec<_>>();
    let server_files = files.clone();

    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let final_revision = WorkspaceRevision::new(FILE_COUNT as u64);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Incremental,
                    from_revision: WorkspaceRevision::ZERO,
                    final_revision,
                    entry_count: 0,
                    event_count: FILE_COUNT as u32,
                    conflict_count: 0,
                }),
            )
            .await;
            for event in events {
                send_success(
                    &mut socket,
                    WorkspaceAction::WorkspaceEvent,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::Event(event),
                )
                .await;
            }
            for resolution in resolutions {
                send_success(
                    &mut socket,
                    WorkspaceAction::WorkspaceConflictResolved,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::ConflictResolved(resolution),
                )
                .await;
            }
            // Match the real service: finish the stream before reading BlobNeed.
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Incremental,
                    delivered_count: FILE_COUNT as u32,
                    final_revision,
                }),
            )
            .await;

            service_downloads_before_ack(&mut socket, &server_files, 42_300, false).await;
            answer_final_ack(&mut socket, final_revision).await;
            socket.close(None).await.unwrap();
        })
        .await;

    let mut bounded_limits = limits();
    bounded_limits.drain_interval = Duration::from_millis(200);
    bounded_limits.pending_outbound_capacity = QUEUE_CAPACITY;
    let (session, mut writer) =
        connected_session_with_limits(server.endpoint(), engine.handle.clone(), bounded_limits)
            .await;
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(matches!(result, SessionResult::Closed), "{result:?}");

    assert_downloads_applied(&engine, &files).await;
    engine.stop().await;
}

#[tokio::test]
async fn online_event_burst_larger_than_outbound_queue_drains_without_reconnect() {
    const FILE_COUNT: usize = 12;
    const QUEUE_CAPACITY: usize = 8;

    let engine = TestEngine::new(None);
    let stream = stream_id(43_000);
    let files = (0..FILE_COUNT)
        .map(|index| {
            let path = format!("live-event-{index:02}.bin");
            let bytes = vec![
                0x80_u8.wrapping_add(index as u8),
                0,
                255,
                (index as u8).wrapping_mul(23),
                64,
            ];
            let hash = content_hash(&bytes);
            ExpectedDownload { path, bytes, hash }
        })
        .collect::<Vec<_>>();
    let events = files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            remote_file_event(
                stream,
                index as u32,
                index as u64 + 1,
                &file.path,
                &file.bytes,
                43_100 + index as u128,
            )
        })
        .collect::<Vec<_>>();
    let server_files = files.clone();

    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream).await;
            for event in events {
                send_success(
                    &mut socket,
                    WorkspaceAction::WorkspaceEvent,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::Event(event),
                )
                .await;
            }

            service_downloads_before_ack(&mut socket, &server_files, 43_300, true).await;
            answer_final_ack(&mut socket, WorkspaceRevision::new(FILE_COUNT as u64)).await;
            socket.close(None).await.unwrap();
        })
        .await;

    let mut bounded_limits = limits();
    bounded_limits.drain_interval = Duration::from_millis(200);
    bounded_limits.pending_outbound_capacity = QUEUE_CAPACITY;
    let (session, mut writer) =
        connected_session_with_limits(server.endpoint(), engine.handle.clone(), bounded_limits)
            .await;
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(matches!(result, SessionResult::Closed), "{result:?}");

    assert_downloads_applied(&engine, &files).await;
    engine.stop().await;
}

fn remote_file_event(
    stream_id: StreamId,
    index: u32,
    revision: u64,
    path: &str,
    bytes: &[u8],
    operation_seed: u128,
) -> WorkspaceEventMessage {
    let operation_id = operation_id(operation_seed);
    let path = WorkspacePath::parse(path).unwrap();
    let hash = content_hash(bytes);
    let metadata = WorkspaceFileMetadata {
        size: bytes.len() as u64,
        modified_at_ms: 1_900_000_000_000 + revision as i64,
        executable: false,
    };
    let message = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id,
        index,
        revision: WorkspaceRevision::new(revision),
        operation_id,
        origin_client_id: remote_client_id(),
        mutation: WorkspaceMutation {
            workspace_id: workspace_id(),
            client_id: remote_client_id(),
            operation_id,
            path: path.clone(),
            base_path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceMutationKind::UpsertFile,
            content_hash: RequiredNullable::Value(hash.clone()),
            metadata: metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        },
        path_state: WorkspacePathState {
            path,
            path_revision: WorkspaceRevision::new(revision),
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(hash),
            metadata,
            tombstone: false,
        },
        old_path_state: None,
        new_path_state: None,
    };
    message.validate().unwrap();
    message
}

fn remote_file_resolution(
    revision: u64,
    path: &str,
    bytes: &[u8],
    seed: u128,
) -> WorkspaceConflictResolvedMessage {
    let hash = content_hash(bytes);
    let message = WorkspaceConflictResolvedMessage {
        workspace_id: workspace_id(),
        conflict_id: ConflictId::parse(&uuid::Uuid::from_u128(seed).to_string()).unwrap(),
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        operation_id: operation_id(seed + 100),
        revision: WorkspaceRevision::new(revision),
        choice: WorkspaceConflictChoice::Current,
        path_state: WorkspacePathState {
            path: WorkspacePath::parse(path).unwrap(),
            path_revision: WorkspaceRevision::new(revision),
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(hash),
            metadata: WorkspaceFileMetadata {
                size: bytes.len() as u64,
                modified_at_ms: 1_900_000_000_000 + revision as i64,
                executable: false,
            },
            tombstone: false,
        },
        resolved_by_client_id: remote_client_id(),
    };
    message.validate().unwrap();
    message
}

async fn service_downloads_before_ack(
    socket: &mut ServerSocket,
    files: &[ExpectedDownload],
    transfer_seed: u128,
    allow_progress_acks: bool,
) {
    let mut requested = vec![false; files.len()];
    let mut completed = vec![false; files.len()];
    let mut transfer_ends = Vec::new();
    while completed.iter().any(|done| !done) {
        let request = next_request(socket).await;
        match &request.envelope {
            DecodedEnvelope::Request {
                body: MessageBody::BlobNeedDownloadRequest(body),
                ..
            } => {
                assert_eq!(request.action, WorkspaceAction::WorkspaceBlobNeed);
                assert_eq!(body.operation_id, RequiredNullable::Null);
                assert_eq!(body.size, RequiredNullable::Null);
                let index = files
                    .iter()
                    .position(|file| file.hash == body.content_hash)
                    .expect("BlobNeed requested an unknown hash");
                assert!(!requested[index], "duplicate BlobNeed for index {index}");
                requested[index] = true;
                let file = &files[index];
                send_success(
                    socket,
                    WorkspaceAction::WorkspaceBlobNeed,
                    WorkspaceFlow::ServerResponse,
                    Some(request_id(&request)),
                    MessageBody::BlobNeedDownloadResponse(
                        fns_protocol::WorkspaceBlobNeedDownloadResponse {
                            workspace_id: workspace_id(),
                            direction: WorkspaceBlobDirection::Download,
                            operation_id: RequiredNullable::Null,
                            content_hash: file.hash.clone(),
                            size: file.bytes.len() as u64,
                        },
                    ),
                )
                .await;
                let transfer = transfer_id(transfer_seed + index as u128);
                let end = WorkspaceBlobEndMessage {
                    workspace_id: workspace_id(),
                    transfer_id: transfer,
                    direction: WorkspaceBlobDirection::Download,
                    content_hash: file.hash.clone(),
                    size: file.bytes.len() as u64,
                    chunk_count: 1,
                };
                send_success(
                    socket,
                    WorkspaceAction::WorkspaceBlobBegin,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
                        workspace_id: workspace_id(),
                        transfer_id: transfer,
                        direction: WorkspaceBlobDirection::Download,
                        content_hash: file.hash.clone(),
                        size: file.bytes.len() as u64,
                        chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                        chunk_count: 1,
                    }),
                )
                .await;
                socket
                    .send(Message::Binary(
                        encode_binary_frame(
                            WorkspaceBlobDirection::Download,
                            true,
                            transfer,
                            0,
                            0,
                            &file.bytes,
                        )
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
                send_success(
                    socket,
                    WorkspaceAction::WorkspaceBlobEnd,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::BlobEnd(end.clone()),
                )
                .await;
                transfer_ends.push((index, end));
            }
            DecodedEnvelope::Request {
                body: MessageBody::BlobEnd(body),
                ..
            } => {
                assert_eq!(request.action, WorkspaceAction::WorkspaceBlobEnd);
                let (index, expected) = transfer_ends
                    .iter()
                    .find(|(_, end)| end.transfer_id == body.transfer_id)
                    .expect("BlobEnd referenced an unknown transfer");
                assert_eq!(body, expected);
                assert!(!completed[*index], "duplicate BlobEnd for index {index}");
                completed[*index] = true;
                send_success(
                    socket,
                    WorkspaceAction::WorkspaceBlobEnd,
                    WorkspaceFlow::ServerResponse,
                    Some(request_id(&request)),
                    MessageBody::BlobEnd(expected.clone()),
                )
                .await;
            }
            DecodedEnvelope::Request {
                body: MessageBody::Ack(body),
                ..
            } if allow_progress_acks => {
                assert!(
                    body.revision < WorkspaceRevision::new(files.len() as u64),
                    "final Ack arrived before every Blob was committed"
                );
                send_success(
                    socket,
                    WorkspaceAction::WorkspaceAck,
                    WorkspaceFlow::ServerResponse,
                    Some(request_id(&request)),
                    MessageBody::Ack(body.clone()),
                )
                .await;
            }
            DecodedEnvelope::Request {
                body: MessageBody::Ack(_),
                ..
            } => panic!("stream Ack arrived before every Blob was committed"),
            _ => panic!("unexpected request while draining blobs: {request:?}"),
        }
    }
    assert!(requested.into_iter().all(|value| value));
}

async fn answer_final_ack(socket: &mut ServerSocket, final_revision: WorkspaceRevision) {
    let ack = next_request(socket).await;
    let ack_body = match &ack.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::Ack(body),
            ..
        } => body.clone(),
        _ => panic!("expected final Ack"),
    };
    assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
    assert_eq!(ack_body.revision, final_revision);
    send_success(
        socket,
        WorkspaceAction::WorkspaceAck,
        WorkspaceFlow::ServerResponse,
        Some(request_id(&ack)),
        MessageBody::Ack(ack_body),
    )
    .await;
}

async fn assert_downloads_applied(engine: &TestEngine, files: &[ExpectedDownload]) {
    for file in files {
        let actual = std::fs::read(engine.workspace.join(&file.path)).unwrap();
        assert_eq!(actual.len(), file.bytes.len());
        assert_eq!(actual, file.bytes);
        assert_eq!(content_hash(&actual), file.hash);
    }
    let final_revision = WorkspaceRevision::new(files.len() as u64);
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_ack_revision, final_revision);
    assert_eq!(cursor.last_applied_revision, final_revision);
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.handle.active_stream_mode().await.unwrap().is_none());
    let pending = engine.handle.pending_commands(32).await.unwrap();
    assert!(
        pending.is_empty(),
        "pending commands after Ack: {pending:?}"
    );
}

fn request_id(frame: &DecodedFrame) -> RequestId {
    match &frame.envelope {
        DecodedEnvelope::Request { request_id, .. } => *request_id,
        _ => panic!("expected client request"),
    }
}

async fn next_request(socket: &mut ServerSocket) -> DecodedFrame {
    next_request_for(socket, "client request", TEST_TIMEOUT).await
}

async fn next_request_for(
    socket: &mut ServerSocket,
    label: &str,
    timeout: Duration,
) -> DecodedFrame {
    loop {
        let message = tokio::time::timeout(timeout, socket.next())
            .await
            .unwrap_or_else(|_| panic!("{label} frame timeout"))
            .expect("client disconnected")
            .expect("client frame error");
        if let Message::Text(text) = message {
            return decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
        }
    }
}

async fn send_frame(socket: &mut ServerSocket, frame: Vec<u8>) {
    socket
        .send(Message::Text(String::from_utf8(frame).unwrap().into()))
        .await
        .unwrap();
}

async fn send_success(
    socket: &mut ServerSocket,
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    request_id: Option<RequestId>,
    body: MessageBody,
) {
    send_frame(
        socket,
        encode_success(action, flow, request_id, body).unwrap(),
    )
    .await;
}

fn hello_response() -> fns_protocol::WorkspaceHelloResponse {
    fns_protocol::WorkspaceHelloResponse {
        protocol_version: "2".into(),
        server_version: "blob-test".into(),
        max_control_frame_bytes: fns_protocol::MAX_CONTROL_FRAME_BYTES as u32,
        max_binary_chunk_bytes: fns_protocol::BLOB_CHUNK_BYTES,
        max_blob_bytes: fns_protocol::MAX_BLOB_BYTES,
        max_transfers_per_connection: 4,
        heartbeat_seconds: 25,
    }
}

async fn answer_hello_and_subscribe(socket: &mut ServerSocket) {
    let hello = next_request(socket).await;
    assert_eq!(hello.action, WorkspaceAction::WorkspaceHello);
    send_success(
        socket,
        WorkspaceAction::WorkspaceHello,
        WorkspaceFlow::ServerResponse,
        Some(request_id(&hello)),
        MessageBody::HelloResponse(hello_response()),
    )
    .await;
    let subscribe = next_request(socket).await;
    assert_eq!(subscribe.action, WorkspaceAction::WorkspaceSubscribe);
}

async fn send_empty_snapshot(socket: &mut ServerSocket, stream: StreamId) {
    send_success(
        socket,
        WorkspaceAction::WorkspaceSnapshotBegin,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::ZERO,
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        }),
    )
    .await;
    send_success(
        socket,
        WorkspaceAction::WorkspaceSnapshotEnd,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Snapshot,
            delivered_count: 0,
            final_revision: WorkspaceRevision::ZERO,
        }),
    )
    .await;
}

async fn connected_session(
    endpoint: &str,
    engine: EngineHandle,
) -> (Session, fns_transport::socket::SocketWriter) {
    connected_session_with_limits(endpoint, engine, limits()).await
}

async fn connected_session_with_limits(
    endpoint: &str,
    engine: EngineHandle,
    session_limits: SessionLimits,
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
        session_limits,
    )
}

async fn prepare_download(
    socket: &mut ServerSocket,
    path: &str,
    expected_hash: WorkspaceContentHash,
    size: u64,
    seed: u128,
) -> (TransferId, WorkspaceBlobEndMessage) {
    let stream = stream_id(seed);
    send_success(
        socket,
        WorkspaceAction::WorkspaceSnapshotBegin,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(1),
            entry_count: 1,
            event_count: 0,
            conflict_count: 0,
        }),
    )
    .await;
    send_success(
        socket,
        WorkspaceAction::WorkspaceSnapshotEntry,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            index: 0,
            entry: WorkspacePathState {
                path: WorkspacePath::parse(path).unwrap(),
                path_revision: WorkspaceRevision::new(1),
                kind: WorkspaceEntryKind::File,
                content_hash: RequiredNullable::Value(expected_hash.clone()),
                metadata: WorkspaceFileMetadata {
                    size,
                    modified_at_ms: 1,
                    executable: false,
                },
                tombstone: false,
            },
        }),
    )
    .await;
    send_success(
        socket,
        WorkspaceAction::WorkspaceSnapshotEnd,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
            workspace_id: workspace_id(),
            stream_id: stream,
            mode: WorkspaceSnapshotMode::Snapshot,
            delivered_count: 1,
            final_revision: WorkspaceRevision::new(1),
        }),
    )
    .await;
    let need = next_request(socket).await;
    assert_eq!(need.action, WorkspaceAction::WorkspaceBlobNeed);
    let operation_id = match &need.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::BlobNeedDownloadRequest(body),
            ..
        } => body.operation_id.clone(),
        _ => panic!("expected BlobNeed download"),
    };
    send_success(
        socket,
        WorkspaceAction::WorkspaceBlobNeed,
        WorkspaceFlow::ServerResponse,
        Some(request_id(&need)),
        MessageBody::BlobNeedDownloadResponse(fns_protocol::WorkspaceBlobNeedDownloadResponse {
            workspace_id: workspace_id(),
            direction: WorkspaceBlobDirection::Download,
            operation_id,
            content_hash: expected_hash.clone(),
            size,
        }),
    )
    .await;
    let transfer = transfer_id(seed + 1);
    let chunk_count = fns_transport::blob::chunk_count(size);
    send_success(
        socket,
        WorkspaceAction::WorkspaceBlobBegin,
        WorkspaceFlow::ServerPush,
        None,
        MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
            workspace_id: workspace_id(),
            transfer_id: transfer,
            direction: WorkspaceBlobDirection::Download,
            content_hash: expected_hash.clone(),
            size,
            chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
            chunk_count,
        }),
    )
    .await;
    (
        transfer,
        WorkspaceBlobEndMessage {
            workspace_id: workspace_id(),
            transfer_id: transfer,
            direction: WorkspaceBlobDirection::Download,
            content_hash: expected_hash,
            size,
            chunk_count,
        },
    )
}

#[derive(Clone, Copy, Debug)]
enum DownloadFault {
    DuplicateChunk,
    Gap,
    Reordered,
    WrongOffset,
    WrongFinal,
    WrongTransfer,
    WrongDirection,
    WrongChunkDigest,
    WrongEndCount,
    ChangedDuplicateBegin,
    ChangedEnd,
}

impl DownloadFault {
    fn name(self) -> &'static str {
        match self {
            Self::DuplicateChunk => "duplicate-chunk",
            Self::Gap => "gap",
            Self::Reordered => "reordered",
            Self::WrongOffset => "wrong-offset",
            Self::WrongFinal => "wrong-final",
            Self::WrongTransfer => "wrong-transfer",
            Self::WrongDirection => "wrong-direction",
            Self::WrongChunkDigest => "wrong-chunk-digest",
            Self::WrongEndCount => "wrong-end-count",
            Self::ChangedDuplicateBegin => "changed-duplicate-begin",
            Self::ChangedEnd => "changed-end",
        }
    }
}

async fn run_download_fault(fault: DownloadFault, seed: u128) {
    let chunk = fns_protocol::BLOB_CHUNK_BYTES as usize;
    let bytes = vec![0x71; chunk + 1];
    let expected_hash = content_hash(&bytes);
    let server_hash = expected_hash.clone();
    let server_bytes = bytes.clone();
    let path = format!("{}.bin", fault.name());
    let server_path = path.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, mut end) = prepare_download(
                &mut socket,
                &server_path,
                server_hash.clone(),
                server_bytes.len() as u64,
                seed,
            )
            .await;
            let first = &server_bytes[..chunk];
            let last = &server_bytes[chunk..];
            match fault {
                DownloadFault::ChangedDuplicateBegin => {
                    send_success(
                        &mut socket,
                        WorkspaceAction::WorkspaceBlobBegin,
                        WorkspaceFlow::ServerPush,
                        None,
                        MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
                            workspace_id: workspace_id(),
                            transfer_id: transfer,
                            direction: WorkspaceBlobDirection::Download,
                            content_hash: content_hash(b"changed begin identity"),
                            size: server_bytes.len() as u64,
                            chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                            chunk_count: 2,
                        }),
                    )
                    .await;
                }
                DownloadFault::DuplicateChunk => {
                    let frame = encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        false,
                        transfer,
                        0,
                        0,
                        first,
                    )
                    .unwrap();
                    socket
                        .send(Message::Binary(frame.clone().into()))
                        .await
                        .unwrap();
                    socket.send(Message::Binary(frame.into())).await.unwrap();
                }
                DownloadFault::Gap => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Download,
                                false,
                                transfer,
                                1,
                                0,
                                first,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::Reordered => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Download,
                                true,
                                transfer,
                                1,
                                chunk as u64,
                                last,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::WrongOffset => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Download,
                                false,
                                transfer,
                                0,
                                1,
                                first,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::WrongFinal => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Download,
                                true,
                                transfer,
                                0,
                                0,
                                first,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::WrongTransfer => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Download,
                                false,
                                transfer_id(seed + 99),
                                0,
                                0,
                                first,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::WrongDirection => {
                    socket
                        .send(Message::Binary(
                            encode_binary_frame(
                                WorkspaceBlobDirection::Upload,
                                false,
                                transfer,
                                0,
                                0,
                                first,
                            )
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                DownloadFault::WrongChunkDigest => {
                    let mut frame = encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        false,
                        transfer,
                        0,
                        0,
                        first,
                    )
                    .unwrap();
                    frame[48] ^= 0xff;
                    socket.send(Message::Binary(frame.into())).await.unwrap();
                }
                DownloadFault::WrongEndCount | DownloadFault::ChangedEnd => {
                    for (index, (offset, payload, final_chunk)) in
                        [(0_u64, first, false), (chunk as u64, last, true)]
                            .into_iter()
                            .enumerate()
                    {
                        socket
                            .send(Message::Binary(
                                encode_binary_frame(
                                    WorkspaceBlobDirection::Download,
                                    final_chunk,
                                    transfer,
                                    index as u64,
                                    offset,
                                    payload,
                                )
                                .unwrap()
                                .into(),
                            ))
                            .await
                            .unwrap();
                    }
                    if matches!(fault, DownloadFault::WrongEndCount) {
                        end.chunk_count += 1;
                    } else {
                        end.content_hash = content_hash(b"changed end identity");
                    }
                    send_success(
                        &mut socket,
                        WorkspaceAction::WorkspaceBlobEnd,
                        WorkspaceFlow::ServerPush,
                        None,
                        MessageBody::BlobEnd(end),
                    )
                    .await;
                }
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join(&path);
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert_protocol_error(result);
    assert!(!destination.exists(), "{} materialized", fault.name());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::ZERO);
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::ZERO);
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(
        engine
            .handle
            .pending_commands(16)
            .await
            .unwrap()
            .iter()
            .any(|command| matches!(command, fns_sync_core::SyncCommand::DownloadBlob { .. }))
    );
    engine.stop().await;
}

#[tokio::test]
async fn download_out_of_order_and_mismatch_matrix_fails_without_settling() {
    let faults = [
        DownloadFault::DuplicateChunk,
        DownloadFault::Gap,
        DownloadFault::Reordered,
        DownloadFault::WrongOffset,
        DownloadFault::WrongFinal,
        DownloadFault::WrongTransfer,
        DownloadFault::WrongDirection,
        DownloadFault::WrongChunkDigest,
        DownloadFault::WrongEndCount,
        DownloadFault::ChangedDuplicateBegin,
        DownloadFault::ChangedEnd,
    ];
    for (index, fault) in faults.into_iter().enumerate() {
        run_download_fault(fault, 2_000 + index as u128 * 10).await;
    }
}

#[tokio::test]
async fn exact_duplicate_download_begin_end_and_end_response_are_idempotent() {
    let bytes = b"duplicate receipts remain exact".to_vec();
    let expected_hash = content_hash(&bytes);
    let server_hash = expected_hash.clone();
    let server_bytes = bytes.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, end) = prepare_download(
                &mut socket,
                "duplicate-receipts.bin",
                server_hash,
                server_bytes.len() as u64,
                3_000,
            )
            .await;
            let begin = WorkspaceBlobBeginMessage {
                workspace_id: workspace_id(),
                transfer_id: transfer,
                direction: WorkspaceBlobDirection::Download,
                content_hash: end.content_hash.clone(),
                size: end.size,
                chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                chunk_count: end.chunk_count,
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobBegin(begin),
            )
            .await;
            socket
                .send(Message::Binary(
                    encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        true,
                        transfer,
                        0,
                        0,
                        &server_bytes,
                    )
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            for _ in 0..2 {
                send_success(
                    &mut socket,
                    WorkspaceAction::WorkspaceBlobEnd,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::BlobEnd(end.clone()),
                )
                .await;
            }
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            let end_response = encode_success(
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .unwrap();
            send_frame(&mut socket, end_response.clone()).await;
            send_frame(&mut socket, end_response).await;
            let ack = next_request(&mut socket).await;
            assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
            let ack_body = match &ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Ack"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&ack)),
                MessageBody::Ack(ack_body),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join("duplicate-receipts.bin");
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert!(
        matches!(result, SessionResult::Closed),
        "session result: {result:?}"
    );
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

async fn send_download_chunks(socket: &mut ServerSocket, transfer: TransferId, bytes: &[u8]) {
    let mut offset = 0_u64;
    for (index, chunk) in bytes
        .chunks(fns_protocol::BLOB_CHUNK_BYTES as usize)
        .enumerate()
    {
        let final_chunk = offset + chunk.len() as u64 == bytes.len() as u64;
        socket
            .send(Message::Binary(
                encode_binary_frame(
                    WorkspaceBlobDirection::Download,
                    final_chunk,
                    transfer,
                    index as u64,
                    offset,
                    chunk,
                )
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        offset += chunk.len() as u64;
    }
}

#[tokio::test]
async fn download_end_response_loss_abandons_stage_and_redownloads_on_reconnect() {
    let bytes = vec![0x81; fns_protocol::BLOB_CHUNK_BYTES as usize + 13];
    let expected_hash = content_hash(&bytes);
    let first_hash = expected_hash.clone();
    let first_bytes = bytes.clone();
    let first =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, end) = prepare_download(
                &mut socket,
                "end-loss-download.bin",
                first_hash,
                first_bytes.len() as u64,
                4_000,
            )
            .await;
            send_download_chunks(&mut socket, transfer, &first_bytes).await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end),
            )
            .await;
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join("end-loss-download.bin");
    let (session, mut writer) = connected_session(first.endpoint(), engine.handle.clone()).await;
    let first_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("first session timeout");
    first.finish().await;

    assert!(matches!(first_result, SessionResult::Closed));
    assert!(!destination.exists());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        engine
            .handle
            .pending_commands(16)
            .await
            .unwrap()
            .iter()
            .any(|command| matches!(command, fns_sync_core::SyncCommand::DownloadBlob { .. }))
    );

    let second_hash = expected_hash.clone();
    let second_bytes = bytes.clone();
    let second =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, end) = prepare_download(
                &mut socket,
                "end-loss-download.bin",
                second_hash,
                second_bytes.len() as u64,
                4_000,
            )
            .await;
            send_download_chunks(&mut socket, transfer, &second_bytes).await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end.clone()),
            )
            .await;
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;
            let ack = next_request(&mut socket).await;
            assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
            let ack_body = match &ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Ack"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&ack)),
                MessageBody::Ack(ack_body),
            )
            .await;
            let _ = socket.close(None).await;
        })
        .await;
    let (session, mut writer) = connected_session(second.endpoint(), engine.handle.clone()).await;
    let second_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("second session timeout");
    second.finish().await;

    assert!(matches!(second_result, SessionResult::Closed));
    let actual = std::fs::read(destination).unwrap();
    assert_eq!(actual, bytes);
    assert_eq!(blake3::hash(&actual), blake3::hash(&bytes));
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[tokio::test]
async fn upload_end_response_loss_reconnects_by_replaying_original_mutation() {
    let bytes = b"server committed upload before response loss".to_vec();
    let expected_hash = content_hash(&bytes);
    let first_hash = expected_hash.clone();
    let first_bytes = bytes.clone();
    let (mutation_tx, mutation_rx) = tokio::sync::oneshot::channel();
    let first =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(5_000)).await;
            let mutation_request = next_request(&mut socket).await;
            let mutation = match &mutation_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Mutation"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationRejected,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&mutation_request)),
                MessageBody::MutationRejected(WorkspaceMutationRejectedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: mutation.operation_id,
                    reason: WorkspaceMutationRejectReason::BlobRequired,
                    current_path_state: RequiredNullable::Null,
                    conflict_id: RequiredNullable::Null,
                    required_hash: RequiredNullable::Value(first_hash.clone()),
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id: mutation.operation_id,
                    content_hash: first_hash,
                    size: first_bytes.len() as u64,
                }),
            )
            .await;
            let begin_request = next_request(&mut socket).await;
            let begin = match &begin_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobBegin(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobBegin upload"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&begin_request)),
                MessageBody::BlobBegin(begin.clone()),
            )
            .await;
            let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
                .await
                .expect("upload chunk timeout")
                .expect("client disconnected")
                .expect("upload frame error");
            let Message::Binary(frame) = message else {
                panic!("expected upload chunk");
            };
            let (header, payload) = decode_binary_frame(&frame).unwrap();
            header.validate_sequence(0, 0, true).unwrap();
            assert_eq!(header.transfer_id, begin.transfer_id);
            assert_eq!(payload, first_bytes);
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            let _ = mutation_tx.send(mutation);
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(Some(("end-loss-upload.bin", &bytes)));
    let (session, mut writer) = connected_session(first.endpoint(), engine.handle.clone()).await;
    let first_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("first session timeout");
    first.finish().await;
    let mutation = mutation_rx.await.unwrap();

    assert!(matches!(first_result, SessionResult::Closed));
    assert!(matches!(
        engine.handle.pending_commands(16).await.unwrap().as_slice(),
        [fns_sync_core::SyncCommand::UploadBlob { .. }]
    ));

    let expected_mutation = mutation.clone();
    let second =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(5_001)).await;
            let replay_request = next_request(&mut socket).await;
            assert_eq!(replay_request.action, WorkspaceAction::WorkspaceMutation);
            let replay = match &replay_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected replayed Mutation"),
            };
            assert_eq!(replay, expected_mutation);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&replay_request)),
                MessageBody::MutationAccepted(fns_protocol::WorkspaceMutationAcceptedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: replay.operation_id,
                    revision: WorkspaceRevision::new(1),
                    path_state: WorkspacePathState {
                        path: replay.path,
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: replay.content_hash,
                        metadata: replay.metadata,
                        tombstone: false,
                    },
                    old_path_state: None,
                    new_path_state: None,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let (session, mut writer) = connected_session(second.endpoint(), engine.handle.clone()).await;
    let second_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("second session timeout");
    second.finish().await;

    assert!(matches!(second_result, SessionResult::Closed));
    assert_eq!(
        std::fs::read(engine.workspace.join("end-loss-upload.bin")).unwrap(),
        bytes
    );
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[derive(Clone, Copy, Debug)]
enum UploadLossBoundary {
    BeforeNeed,
    AfterNeed,
    AfterBegin,
    MidChunk,
}

impl UploadLossBoundary {
    fn name(self) -> &'static str {
        match self {
            Self::BeforeNeed => "before-need",
            Self::AfterNeed => "after-need",
            Self::AfterBegin => "after-begin",
            Self::MidChunk => "mid-chunk",
        }
    }
}

async fn run_upload_loss_boundary(boundary: UploadLossBoundary, seed: u128) {
    let bytes = if matches!(boundary, UploadLossBoundary::MidChunk) {
        vec![0xa1; fns_protocol::BLOB_CHUNK_BYTES as usize + 9]
    } else {
        format!("upload loss {}", boundary.name()).into_bytes()
    };
    let expected_hash = content_hash(&bytes);
    let first_hash = expected_hash.clone();
    let first_bytes = bytes.clone();
    let path = format!("upload-loss-{}.bin", boundary.name());
    let server_path = path.clone();
    let (mutation_tx, mutation_rx) = tokio::sync::oneshot::channel();
    let first =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(seed)).await;
            let mutation_request = next_request(&mut socket).await;
            let mutation = match &mutation_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Mutation"),
            };
            assert_eq!(mutation.path.as_str(), server_path);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationRejected,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&mutation_request)),
                MessageBody::MutationRejected(WorkspaceMutationRejectedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: mutation.operation_id,
                    reason: WorkspaceMutationRejectReason::BlobRequired,
                    current_path_state: RequiredNullable::Null,
                    conflict_id: RequiredNullable::Null,
                    required_hash: RequiredNullable::Value(first_hash.clone()),
                }),
            )
            .await;
            if matches!(boundary, UploadLossBoundary::BeforeNeed) {
                let _ = mutation_tx.send(mutation);
                let _ = socket.close(None).await;
                return;
            }
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id: mutation.operation_id,
                    content_hash: first_hash,
                    size: first_bytes.len() as u64,
                }),
            )
            .await;
            if matches!(boundary, UploadLossBoundary::AfterNeed) {
                let _ = mutation_tx.send(mutation);
                let _ = socket.close(None).await;
                return;
            }
            let begin_request = next_request(&mut socket).await;
            let begin = match &begin_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobBegin(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobBegin upload"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&begin_request)),
                MessageBody::BlobBegin(begin.clone()),
            )
            .await;
            if matches!(boundary, UploadLossBoundary::MidChunk) {
                let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
                    .await
                    .expect("mid-chunk upload timeout")
                    .expect("mid-chunk client disconnected")
                    .expect("mid-chunk upload error");
                let Message::Binary(frame) = message else {
                    panic!("expected first upload chunk");
                };
                let (header, payload) = decode_binary_frame(&frame).unwrap();
                header.validate_sequence(0, 0, false).unwrap();
                assert_eq!(header.transfer_id, begin.transfer_id);
                assert_eq!(
                    payload,
                    &first_bytes[..fns_protocol::BLOB_CHUNK_BYTES as usize]
                );
            }
            let _ = mutation_tx.send(mutation);
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(Some((&path, &bytes)));
    let (session, mut writer) = connected_session(first.endpoint(), engine.handle.clone()).await;
    let first_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("first loss-boundary session timeout");
    first.finish().await;
    let mutation = mutation_rx.await.unwrap();

    match first_result {
        SessionResult::Closed => {}
        SessionResult::Error(error) => {
            assert_eq!(
                error.code(),
                TransportErrorCode::Network,
                "first upload-loss session at {boundary:?} returned {error:?}"
            );
            assert!(
                error.retryable(),
                "first upload-loss session at {boundary:?} returned a non-retryable network error"
            );
        }
    }
    assert!(matches!(
        engine.handle.pending_commands(16).await.unwrap().as_slice(),
        [fns_sync_core::SyncCommand::UploadBlob { .. }]
    ));
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    let engine = if matches!(boundary, UploadLossBoundary::BeforeNeed) {
        engine.restart().await
    } else {
        engine
    };

    let expected_mutation = mutation.clone();
    let second =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(seed + 1)).await;
            let replay_request = next_request(&mut socket).await;
            assert_eq!(replay_request.action, WorkspaceAction::WorkspaceMutation);
            let replay = match &replay_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected replayed Mutation"),
            };
            assert_eq!(replay, expected_mutation);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&replay_request)),
                MessageBody::MutationAccepted(fns_protocol::WorkspaceMutationAcceptedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: replay.operation_id,
                    revision: WorkspaceRevision::new(1),
                    path_state: WorkspacePathState {
                        path: replay.path,
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: replay.content_hash,
                        metadata: replay.metadata,
                        tombstone: false,
                    },
                    old_path_state: None,
                    new_path_state: None,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let (session, mut writer) = connected_session(second.endpoint(), engine.handle.clone()).await;
    let second_result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("second loss-boundary session timeout");
    second.finish().await;

    assert!(matches!(second_result, SessionResult::Closed));
    let actual = std::fs::read(engine.workspace.join(path)).unwrap();
    assert_eq!(actual, bytes);
    assert_eq!(blake3::hash(&actual), blake3::hash(&bytes));
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[tokio::test]
async fn awaiting_blob_reconnect_matrix_survives_all_upload_loss_boundaries_and_reopen() {
    let boundaries = [
        UploadLossBoundary::BeforeNeed,
        UploadLossBoundary::AfterNeed,
        UploadLossBoundary::AfterBegin,
        UploadLossBoundary::MidChunk,
    ];
    for (index, boundary) in boundaries.into_iter().enumerate() {
        run_upload_loss_boundary(boundary, 8_000 + index as u128 * 10).await;
    }
}

#[tokio::test]
async fn download_cancellation_drops_partial_stage_and_retains_durable_work() {
    let bytes = vec![0x91; fns_protocol::BLOB_CHUNK_BYTES as usize + 1];
    let expected_hash = content_hash(&bytes);
    let server_hash = expected_hash.clone();
    let first_chunk = bytes[..fns_protocol::BLOB_CHUNK_BYTES as usize].to_vec();
    let (chunk_tx, chunk_rx) = tokio::sync::oneshot::channel();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, _) = prepare_download(
                &mut socket,
                "cancelled-download.bin",
                server_hash,
                bytes.len() as u64,
                6_000,
            )
            .await;
            socket
                .send(Message::Binary(
                    encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        false,
                        transfer,
                        0,
                        0,
                        &first_chunk,
                    )
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            let _ = chunk_tx.send(());
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_secs(2), socket.next()).await
            {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join("cancelled-download.bin");
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;
    let shutdown = CancellationToken::new();
    let cancel = shutdown.clone();
    let run = tokio::spawn(async move { session.run(&mut writer, shutdown).await });
    chunk_rx.await.unwrap();
    cancel.cancel();
    let result = tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("cancelled session timeout")
        .unwrap();
    server.finish().await;

    assert!(matches!(result, SessionResult::Closed));
    assert!(!destination.exists());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::ZERO);
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::ZERO);
    assert!(
        engine
            .handle
            .pending_commands(16)
            .await
            .unwrap()
            .iter()
            .any(|command| matches!(command, fns_sync_core::SyncCommand::DownloadBlob { .. }))
    );
    engine.stop().await;
}

#[tokio::test]
async fn large_sparse_upload_keeps_live_payloads_bounded_to_one_chunk() {
    const LARGE_SIZE: u64 = 64 * 1024 * 1024;
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(7_000)).await;
            let mutation_request =
                next_request_for(&mut socket, "large mutation", Duration::from_secs(8)).await;
            let mutation = match &mutation_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Mutation"),
            };
            let expected_hash = mutation.content_hash.clone().into_option().unwrap();
            assert_eq!(mutation.metadata.size, LARGE_SIZE);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationRejected,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&mutation_request)),
                MessageBody::MutationRejected(WorkspaceMutationRejectedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: mutation.operation_id,
                    reason: WorkspaceMutationRejectReason::BlobRequired,
                    current_path_state: RequiredNullable::Null,
                    conflict_id: RequiredNullable::Null,
                    required_hash: RequiredNullable::Value(expected_hash.clone()),
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id: mutation.operation_id,
                    content_hash: expected_hash.clone(),
                    size: LARGE_SIZE,
                }),
            )
            .await;
            let begin_request =
                next_request_for(&mut socket, "large begin", Duration::from_secs(8)).await;
            let begin = match &begin_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobBegin(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobBegin upload"),
            };
            assert_eq!(begin.size, LARGE_SIZE);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&begin_request)),
                MessageBody::BlobBegin(begin.clone()),
            )
            .await;

            let mut hasher = blake3::Hasher::new();
            let mut next_index = 0_u64;
            let mut next_offset = 0_u64;
            let mut max_payload = 0_usize;
            let end_request = loop {
                let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
                    .await
                    .expect("large upload frame timeout")
                    .expect("large upload disconnected")
                    .expect("large upload frame error");
                match message {
                    Message::Binary(frame) => {
                        assert!(
                            frame.len()
                                <= fns_protocol::BLOB_HEADER_LEN
                                    + fns_protocol::BLOB_CHUNK_BYTES as usize
                        );
                        let (header, payload) = decode_binary_frame(&frame).unwrap();
                        let after = next_offset + payload.len() as u64;
                        header
                            .validate_sequence(next_index, next_offset, after == LARGE_SIZE)
                            .unwrap();
                        assert_eq!(header.transfer_id, begin.transfer_id);
                        hasher.update(payload);
                        max_payload = max_payload.max(payload.len());
                        next_index += 1;
                        next_offset = after;
                    }
                    Message::Text(text) => {
                        break decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest)
                            .unwrap();
                    }
                    Message::Ping(payload) => {
                        socket.send(Message::Pong(payload)).await.unwrap();
                    }
                    other => panic!("unexpected large upload message: {other:?}"),
                }
            };
            assert_eq!(next_offset, LARGE_SIZE);
            assert_eq!(next_index, begin.chunk_count);
            assert_eq!(max_payload, fns_protocol::BLOB_CHUNK_BYTES as usize);
            let actual_hash =
                WorkspaceContentHash::parse(&format!("blake3:{}", hasher.finalize().to_hex()))
                    .unwrap();
            assert_eq!(actual_hash, expected_hash);
            let end = match &end_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobEnd(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobEnd upload"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;
            let replay_request =
                next_request_for(&mut socket, "large replay", Duration::from_secs(8)).await;
            let replay = match &replay_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected replayed Mutation"),
            };
            assert_eq!(replay, mutation);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&replay_request)),
                MessageBody::MutationAccepted(fns_protocol::WorkspaceMutationAcceptedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: replay.operation_id,
                    revision: WorkspaceRevision::new(1),
                    path_state: WorkspacePathState {
                        path: replay.path,
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: replay.content_hash,
                        metadata: replay.metadata,
                        tombstone: false,
                    },
                    old_path_state: None,
                    new_path_state: None,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new_sparse("large-sparse.bin", LARGE_SIZE);
    let mut large_limits = limits();
    large_limits.idle_timeout = Duration::from_secs(30);
    large_limits.transfer_idle_timeout = Duration::from_secs(10);
    large_limits.transfer_max_lifetime = Duration::from_secs(30);
    let (session, mut writer) =
        connected_session_with_limits(server.endpoint(), engine.handle.clone(), large_limits).await;

    let result = tokio::time::timeout(
        Duration::from_secs(30),
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("large sparse session timeout");

    assert!(
        matches!(result, SessionResult::Closed),
        "large sparse session ended early: {result:?}"
    );
    server.finish().await;
    assert_eq!(
        std::fs::metadata(engine.workspace.join("large-sparse.bin"))
            .unwrap()
            .len(),
        LARGE_SIZE
    );
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

fn assert_protocol_error(result: SessionResult) {
    let SessionResult::Error(error) = result else {
        panic!("expected protocol error, got {result:?}");
    };
    assert_eq!(error.code(), TransportErrorCode::Protocol);
    assert!(!error.retryable());
}

async fn run_upload_case_with_limits(
    case: &str,
    bytes: Vec<u8>,
    seed: u128,
    session_limits: SessionLimits,
) {
    let expected_hash = content_hash(&bytes);
    let server_hash = expected_hash.clone();
    let server_bytes = bytes.clone();
    let remote_path = format!("{case}.bin");
    let server_path = remote_path.clone();
    let server_case = case.to_owned();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(seed)).await;
            let mutation_request = next_request(&mut socket).await;
            assert_eq!(mutation_request.action, WorkspaceAction::WorkspaceMutation);
            let mutation = match &mutation_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Mutation"),
            };
            assert_eq!(mutation.path.as_str(), server_path);
            assert_eq!(
                mutation.content_hash.as_ref().into_option(),
                Some(&server_hash)
            );
            assert_eq!(mutation.metadata.size, server_bytes.len() as u64);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationRejected,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&mutation_request)),
                MessageBody::MutationRejected(WorkspaceMutationRejectedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: mutation.operation_id,
                    reason: WorkspaceMutationRejectReason::BlobRequired,
                    current_path_state: RequiredNullable::Null,
                    conflict_id: RequiredNullable::Null,
                    required_hash: RequiredNullable::Value(server_hash.clone()),
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id: mutation.operation_id,
                    content_hash: server_hash.clone(),
                    size: server_bytes.len() as u64,
                }),
            )
            .await;
            let begin_request = next_request(&mut socket).await;
            assert_eq!(begin_request.action, WorkspaceAction::WorkspaceBlobBegin);
            let begin = match &begin_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobBegin(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobBegin upload"),
            };
            assert_eq!(begin.direction, WorkspaceBlobDirection::Upload);
            assert_eq!(begin.content_hash, server_hash);
            assert_eq!(begin.size, server_bytes.len() as u64);
            assert_eq!(
                begin.chunk_count,
                fns_transport::blob::chunk_count(server_bytes.len() as u64)
            );
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&begin_request)),
                MessageBody::BlobBegin(begin.clone()),
            )
            .await;

            let mut actual = Vec::new();
            let mut next_index = 0_u64;
            let mut next_offset = 0_u64;
            let mut max_payload = 0_usize;
            let end_request = loop {
                let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
                    .await
                    .unwrap_or_else(|_| panic!("{server_case}: client upload frame timeout"))
                    .expect("client disconnected during upload")
                    .expect("client upload frame error");
                match message {
                    Message::Binary(frame) => {
                        let (header, payload) = decode_binary_frame(&frame).unwrap();
                        assert_eq!(header.direction, WorkspaceBlobDirection::Upload);
                        assert_eq!(header.transfer_id, begin.transfer_id);
                        let after = next_offset + payload.len() as u64;
                        let is_last = after == begin.size;
                        header
                            .validate_sequence(next_index, next_offset, is_last)
                            .unwrap();
                        assert!(after <= begin.size);
                        max_payload = max_payload.max(payload.len());
                        actual.extend_from_slice(payload);
                        next_index += 1;
                        next_offset = after;
                    }
                    Message::Text(text) => {
                        break decode_text_frame(text.as_bytes(), WorkspaceFlow::ClientRequest)
                            .unwrap();
                    }
                    Message::Ping(payload) => {
                        socket.send(Message::Pong(payload)).await.unwrap();
                    }
                    other => panic!("unexpected upload message: {other:?}"),
                }
            };
            assert_eq!(actual, server_bytes);
            assert_eq!(blake3::hash(&actual), blake3::hash(&server_bytes));
            assert!(max_payload <= fns_protocol::BLOB_CHUNK_BYTES as usize);
            assert_eq!(next_offset, begin.size);
            assert_eq!(next_index, begin.chunk_count);
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            let end = match &end_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobEnd(body),
                    ..
                } => body.clone(),
                _ => panic!("expected BlobEnd upload"),
            };
            assert_eq!(end.workspace_id, begin.workspace_id);
            assert_eq!(end.transfer_id, begin.transfer_id);
            assert_eq!(end.direction, WorkspaceBlobDirection::Upload);
            assert_eq!(end.content_hash, begin.content_hash);
            assert_eq!(end.size, begin.size);
            assert_eq!(end.chunk_count, begin.chunk_count);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;

            let replay_request = next_request(&mut socket).await;
            assert_eq!(replay_request.action, WorkspaceAction::WorkspaceMutation);
            let replay = match &replay_request.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.clone(),
                _ => panic!("expected replayed Mutation"),
            };
            assert_eq!(
                replay, mutation,
                "upload must replay the immutable mutation"
            );
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationAccepted,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&replay_request)),
                MessageBody::MutationAccepted(fns_protocol::WorkspaceMutationAcceptedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id: replay.operation_id,
                    revision: WorkspaceRevision::new(1),
                    path_state: WorkspacePathState {
                        path: replay.path,
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: replay.content_hash,
                        metadata: replay.metadata,
                        tombstone: false,
                    },
                    old_path_state: None,
                    new_path_state: None,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(Some((&remote_path, &bytes)));
    let source = engine.workspace.join(&remote_path);
    let (session, mut writer) =
        connected_session_with_limits(server.endpoint(), engine.handle.clone(), session_limits)
            .await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert!(
        matches!(result, SessionResult::Closed),
        "session result: {result:?}"
    );
    let actual = std::fs::read(source).unwrap();
    assert_eq!(actual, bytes);
    assert_eq!(actual.len(), bytes.len());
    assert_eq!(blake3::hash(&actual), blake3::hash(&bytes));
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

async fn run_upload_case(case: &str, bytes: Vec<u8>, seed: u128) {
    run_upload_case_with_limits(case, bytes, seed, limits()).await;
}

#[tokio::test]
async fn production_default_budget_streams_a_blob_larger_than_the_control_budget() {
    let legacy_control_budget =
        fns_protocol::MAX_CONTROL_FRAME_BYTES * fns_transport::OUTBOUND_QUEUE_CAPACITY;
    let bytes = vec![0x5a; legacy_control_budget + 1];

    run_upload_case_with_limits(
        "upload-production-default-budget",
        bytes,
        90,
        SessionLimits::default(),
    )
    .await;
}

#[tokio::test]
async fn upload_empty_binary_and_chunk_boundaries_are_streamed_then_replayed() {
    let chunk = fns_protocol::BLOB_CHUNK_BYTES as usize;
    let cases = [
        ("upload-empty", Vec::new()),
        ("upload-binary", vec![0, 255, 1, 0, 128, 7]),
        ("upload-exact-boundary", vec![0x61; chunk]),
        ("upload-boundary-plus-one", vec![0x62; chunk + 1]),
        ("upload-multi-chunk", vec![0x63; chunk * 2 + 17]),
    ];
    for (index, (name, bytes)) in cases.into_iter().enumerate() {
        run_upload_case(name, bytes, 100 + index as u128 * 10).await;
    }
}

#[tokio::test]
async fn unsolicited_upload_need_does_not_manufacture_a_durable_intent() {
    let bytes = b"not in the durable outbox";
    let expected_hash = content_hash(bytes);
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(30)).await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id: fns_protocol::OperationId::parse(
                        "10000000-0000-4000-8000-000000000030",
                    )
                    .unwrap(),
                    content_hash: expected_hash,
                    size: bytes.len() as u64,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = socket.close(None).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert!(
        matches!(result, SessionResult::Closed),
        "session result: {result:?}"
    );
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    engine.stop().await;
}

#[tokio::test]
async fn staged_full_hash_mismatch_is_a_protocol_error_and_is_abandoned() {
    let expected_hash = content_hash(b"good");
    let server_hash = expected_hash.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let (transfer, end) =
                prepare_download(&mut socket, "digest.bin", server_hash, 4, 40).await;
            socket
                .send(Message::Binary(
                    encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        true,
                        transfer,
                        0,
                        0,
                        b"evil",
                    )
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let destination = engine.workspace.join("digest.bin");
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert_protocol_error(result);
    assert!(!destination.exists());
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[tokio::test]
async fn oversized_binary_message_is_classified_as_protocol() {
    let expected_hash = content_hash(b"x");
    let server_hash = expected_hash.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            let _ = prepare_download(&mut socket, "oversize.bin", server_hash, 1, 50).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                socket.send(Message::Binary(
                    vec![
                        0_u8;
                        fns_protocol::BLOB_HEADER_LEN + fns_protocol::BLOB_CHUNK_BYTES as usize + 1
                    ]
                    .into(),
                )),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;
    let engine = TestEngine::new(None);
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");

    server.finish().await;
    assert_protocol_error(result);
    assert!(
        std::fs::read_dir(engine.state.join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
    engine.stop().await;
}

#[tokio::test]
async fn delayed_begin_ack_proves_no_early_upload_chunk() {
    let bytes = b"upload must wait for begin acknowledgement".to_vec();
    let expected_hash = content_hash(&bytes);
    let server_bytes = bytes.clone();
    let server = support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| {
        let expected_hash = expected_hash.clone();
        let bytes = server_bytes;
        async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_empty_snapshot(&mut socket, stream_id(10)).await;
            let mutation = next_request(&mut socket).await;
            assert_eq!(mutation.action, WorkspaceAction::WorkspaceMutation);
            let operation_id = match &mutation.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Mutation(body),
                    ..
                } => body.operation_id,
                _ => panic!("expected mutation"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceMutationRejected,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&mutation)),
                MessageBody::MutationRejected(WorkspaceMutationRejectedMessage {
                    workspace_id: workspace_id(),
                    client_id: client_id(),
                    operation_id,
                    reason: WorkspaceMutationRejectReason::BlobRequired,
                    current_path_state: RequiredNullable::Null,
                    conflict_id: RequiredNullable::Null,
                    required_hash: RequiredNullable::Value(expected_hash.clone()),
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobNeedUploadPush(fns_protocol::WorkspaceBlobNeedUploadPush {
                    workspace_id: workspace_id(),
                    direction: WorkspaceBlobDirection::Upload,
                    operation_id,
                    content_hash: expected_hash,
                    size: bytes.len() as u64,
                }),
            )
            .await;
            let begin = next_request(&mut socket).await;
            assert_eq!(begin.action, WorkspaceAction::WorkspaceBlobBegin);
            assert!(
                tokio::time::timeout(Duration::from_millis(120), socket.next())
                    .await
                    .is_err(),
                "upload emitted a frame before the exact BlobBegin response"
            );
        }
    })
    .await;
    let engine = TestEngine::new(Some(("upload.bin", &bytes)));
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    assert!(matches!(
        result,
        SessionResult::Closed | SessionResult::Error(_)
    ));
    server.finish().await;
    engine.stop().await;
}

#[tokio::test]
async fn download_waits_for_exact_end_response_before_materialize_and_ack() {
    let bytes = b"download end response ordering".to_vec();
    let expected_hash = content_hash(&bytes);
    let (path_tx, path_rx) = tokio::sync::oneshot::channel::<PathBuf>();
    let server = support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| {
        let expected_hash = expected_hash.clone();
        async move {
            let destination = path_rx.await.unwrap();
            answer_hello_and_subscribe(&mut socket).await;
            let stream = stream_id(20);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    from_revision: WorkspaceRevision::ZERO,
                    final_revision: WorkspaceRevision::new(1),
                    entry_count: 1,
                    event_count: 0,
                    conflict_count: 0,
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEntry,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEntry(WorkspaceSnapshotEntryMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    index: 0,
                    entry: WorkspacePathState {
                        path: WorkspacePath::parse("download.bin").unwrap(),
                        path_revision: WorkspaceRevision::new(1),
                        kind: WorkspaceEntryKind::File,
                        content_hash: RequiredNullable::Value(expected_hash.clone()),
                        metadata: WorkspaceFileMetadata {
                            size: bytes.len() as u64,
                            modified_at_ms: 1,
                            executable: false,
                        },
                        tombstone: false,
                    },
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Snapshot,
                    delivered_count: 1,
                    final_revision: WorkspaceRevision::new(1),
                }),
            )
            .await;
            let need = next_request(&mut socket).await;
            assert_eq!(need.action, WorkspaceAction::WorkspaceBlobNeed);
            let operation_id = match &need.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobNeedDownloadRequest(body),
                    ..
                } => body.operation_id.clone(),
                _ => panic!("expected BlobNeed download"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&need)),
                MessageBody::BlobNeedDownloadResponse(
                    fns_protocol::WorkspaceBlobNeedDownloadResponse {
                        workspace_id: workspace_id(),
                        direction: WorkspaceBlobDirection::Download,
                        operation_id,
                        content_hash: expected_hash.clone(),
                        size: bytes.len() as u64,
                    },
                ),
            )
            .await;
            let transfer = transfer_id(21);
            let begin = WorkspaceBlobBeginMessage {
                workspace_id: workspace_id(),
                transfer_id: transfer,
                direction: WorkspaceBlobDirection::Download,
                content_hash: expected_hash.clone(),
                size: bytes.len() as u64,
                chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                chunk_count: 1,
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobBegin(begin),
            )
            .await;
            socket
                .send(Message::Binary(
                    encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        true,
                        transfer,
                        0,
                        0,
                        &bytes,
                    )
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            let end = WorkspaceBlobEndMessage {
                workspace_id: workspace_id(),
                transfer_id: transfer,
                direction: WorkspaceBlobDirection::Download,
                content_hash: expected_hash,
                size: bytes.len() as u64,
                chunk_count: 1,
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end.clone()),
            )
            .await;

            let end_request = next_request(&mut socket).await;
            assert_eq!(
                end_request.action,
                WorkspaceAction::WorkspaceBlobEnd,
                "server End push must not be treated as download completion"
            );
            assert!(
                !destination.exists(),
                "download materialized before End response"
            );
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;
            let ack = next_request(&mut socket).await;
            assert_eq!(ack.action, WorkspaceAction::WorkspaceAck);
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
            let ack_body = match &ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected Ack"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&ack)),
                MessageBody::Ack(ack_body),
            )
            .await;
            let _ = socket.close(None).await;
        }
    })
    .await;
    let engine = TestEngine::new(None);
    path_tx.send(engine.workspace.join("download.bin")).unwrap();
    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(
        matches!(result, SessionResult::Closed),
        "session result: {result:?}"
    );
    engine.stop().await;
}

#[tokio::test]
async fn live_messages_wait_for_incremental_blob_stream_ack_then_replay_in_order() {
    let engine = TestEngine::new(None);
    let workspace = engine.workspace.clone();
    let stream_path = WorkspacePath::parse("stream.bin").unwrap();
    let live_path = WorkspacePath::parse("live-dir").unwrap();
    let conflict_path = WorkspacePath::parse("conflict.txt").unwrap();
    let stream = stream_id(30_000);
    let transfer = transfer_id(30_001);
    let conflict_id = ConflictId::parse("10000000-0000-4000-8000-000000030002").unwrap();
    let bytes = b"incremental-stream-payload".to_vec();
    let expected_hash = content_hash(&bytes);
    let file_metadata = WorkspaceFileMetadata {
        size: bytes.len() as u64,
        modified_at_ms: 1_800_000_000_001,
        executable: false,
    };
    let directory_metadata = WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 1_800_000_000_002,
        executable: false,
    };

    let stream_operation = operation_id(30_010);
    let stream_event = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id: stream,
        index: 0,
        revision: WorkspaceRevision::new(1),
        operation_id: stream_operation,
        origin_client_id: remote_client_id(),
        mutation: WorkspaceMutation {
            workspace_id: workspace_id(),
            client_id: remote_client_id(),
            operation_id: stream_operation,
            path: stream_path.clone(),
            base_path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceMutationKind::UpsertFile,
            content_hash: RequiredNullable::Value(expected_hash.clone()),
            metadata: file_metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        },
        path_state: WorkspacePathState {
            path: stream_path.clone(),
            path_revision: WorkspaceRevision::new(1),
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(expected_hash.clone()),
            metadata: file_metadata.clone(),
            tombstone: false,
        },
        old_path_state: None,
        new_path_state: None,
    };

    let live_operation = operation_id(30_011);
    let live_event = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id: stream,
        index: 1,
        revision: WorkspaceRevision::new(2),
        operation_id: live_operation,
        origin_client_id: remote_client_id(),
        mutation: WorkspaceMutation {
            workspace_id: workspace_id(),
            client_id: remote_client_id(),
            operation_id: live_operation,
            path: live_path.clone(),
            base_path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceMutationKind::Mkdir,
            content_hash: RequiredNullable::Null,
            metadata: directory_metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        },
        path_state: WorkspacePathState {
            path: live_path.clone(),
            path_revision: WorkspaceRevision::new(2),
            kind: WorkspaceEntryKind::Directory,
            content_hash: RequiredNullable::Null,
            metadata: directory_metadata,
            tombstone: false,
        },
        old_path_state: None,
        new_path_state: None,
    };

    let conflict_side = WorkspaceConflictSide {
        path: RequiredNullable::Value(conflict_path.clone()),
        path_revision: WorkspaceRevision::new(2),
        content_hash: RequiredNullable::Value(expected_hash.clone()),
        metadata: file_metadata.clone(),
        tombstone: false,
    };
    let conflict_created = WorkspaceConflictCreatedMessage {
        workspace_id: workspace_id(),
        conflict_id,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        path: conflict_path.clone(),
        kind: WorkspaceConflictKind::Content,
        ancestor: conflict_side.clone(),
        current: conflict_side.clone(),
        incoming: conflict_side,
        created_by_operation_id: operation_id(30_012),
    };
    let conflict_resolved = WorkspaceConflictResolvedMessage {
        workspace_id: workspace_id(),
        conflict_id,
        conflict_revision: conflict_created.conflict_revision,
        operation_id: operation_id(30_013),
        revision: WorkspaceRevision::new(3),
        choice: WorkspaceConflictChoice::Incoming,
        path_state: WorkspacePathState {
            path: conflict_path.clone(),
            path_revision: WorkspaceRevision::new(3),
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(expected_hash.clone()),
            metadata: file_metadata,
            tombstone: false,
        },
        resolved_by_client_id: remote_client_id(),
    };

    stream_event.validate().unwrap();
    live_event.validate().unwrap();
    conflict_created.validate().unwrap();
    conflict_resolved.validate().unwrap();

    let server_workspace = workspace.clone();
    let server_bytes = bytes.clone();
    let server_hash = expected_hash.clone();
    let server =
        support::fake_server::ScriptedWorkspaceServer::start(move |mut socket| async move {
            answer_hello_and_subscribe(&mut socket).await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Incremental,
                    from_revision: WorkspaceRevision::ZERO,
                    final_revision: WorkspaceRevision::new(1),
                    entry_count: 0,
                    event_count: 1,
                    conflict_count: 0,
                }),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceEvent,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::Event(stream_event),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceSnapshotEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::SnapshotEnd(WorkspaceSnapshotEndMessage {
                    workspace_id: workspace_id(),
                    stream_id: stream,
                    mode: WorkspaceSnapshotMode::Incremental,
                    delivered_count: 1,
                    final_revision: WorkspaceRevision::new(1),
                }),
            )
            .await;

            let need = next_request(&mut socket).await;
            assert_eq!(need.action, WorkspaceAction::WorkspaceBlobNeed);
            let operation_id = match &need.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::BlobNeedDownloadRequest(body),
                    ..
                } => {
                    assert_eq!(body.operation_id, RequiredNullable::Null);
                    assert_eq!(body.size, RequiredNullable::Null);
                    body.operation_id.clone()
                }
                _ => panic!("expected BlobNeed download"),
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobNeed,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&need)),
                MessageBody::BlobNeedDownloadResponse(
                    fns_protocol::WorkspaceBlobNeedDownloadResponse {
                        workspace_id: workspace_id(),
                        direction: WorkspaceBlobDirection::Download,
                        operation_id,
                        content_hash: server_hash.clone(),
                        size: server_bytes.len() as u64,
                    },
                ),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobBegin,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobBegin(WorkspaceBlobBeginMessage {
                    workspace_id: workspace_id(),
                    transfer_id: transfer,
                    direction: WorkspaceBlobDirection::Download,
                    content_hash: server_hash.clone(),
                    size: server_bytes.len() as u64,
                    chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
                    chunk_count: 1,
                }),
            )
            .await;

            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceEvent,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::Event(live_event),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceConflictCreated,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::ConflictCreated(conflict_created),
            )
            .await;
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceConflictResolved,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::ConflictResolved(conflict_resolved),
            )
            .await;

            socket
                .send(Message::Binary(
                    encode_binary_frame(
                        WorkspaceBlobDirection::Download,
                        true,
                        transfer,
                        0,
                        0,
                        &server_bytes,
                    )
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            let end = WorkspaceBlobEndMessage {
                workspace_id: workspace_id(),
                transfer_id: transfer,
                direction: WorkspaceBlobDirection::Download,
                content_hash: server_hash,
                size: server_bytes.len() as u64,
                chunk_count: 1,
            };
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerPush,
                None,
                MessageBody::BlobEnd(end.clone()),
            )
            .await;
            let end_request = next_request(&mut socket).await;
            assert_eq!(end_request.action, WorkspaceAction::WorkspaceBlobEnd);
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceBlobEnd,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&end_request)),
                MessageBody::BlobEnd(end),
            )
            .await;

            let stream_ack = next_request(&mut socket).await;
            let stream_ack_body = match &stream_ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected stream Ack"),
            };
            assert_eq!(stream_ack_body.revision, WorkspaceRevision::new(1));
            assert!(server_workspace.join("stream.bin").exists());
            assert!(!server_workspace.join("live-dir").exists());
            assert!(!server_workspace.join("conflict.txt").exists());
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(100),
                    next_request_for(
                        &mut socket,
                        "second request while stream Ack is in flight",
                        TEST_TIMEOUT,
                    ),
                )
                .await
                .is_err(),
                "a second request escaped while the stream Ack was in flight"
            );
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&stream_ack)),
                MessageBody::Ack(stream_ack_body),
            )
            .await;

            let live_ack = next_request(&mut socket).await;
            let live_ack_body = match &live_ack.envelope {
                DecodedEnvelope::Request {
                    body: MessageBody::Ack(body),
                    ..
                } => body.clone(),
                _ => panic!("expected live Ack"),
            };
            assert_eq!(live_ack_body.revision, WorkspaceRevision::new(3));
            send_success(
                &mut socket,
                WorkspaceAction::WorkspaceAck,
                WorkspaceFlow::ServerResponse,
                Some(request_id(&live_ack)),
                MessageBody::Ack(live_ack_body),
            )
            .await;
            socket.close(None).await.unwrap();
        })
        .await;

    let (session, mut writer) = connected_session(server.endpoint(), engine.handle.clone()).await;
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        session.run(&mut writer, CancellationToken::new()),
    )
    .await
    .expect("session timeout");
    server.finish().await;
    assert!(matches!(result, SessionResult::Closed), "{result:?}");
    let stream_bytes = std::fs::read(workspace.join(stream_path.as_str())).unwrap();
    assert_eq!(stream_bytes, bytes);
    assert_eq!(blake3::hash(&stream_bytes), blake3::hash(&bytes));
    assert!(workspace.join(live_path.as_str()).is_dir());
    let conflict_bytes = std::fs::read(workspace.join(conflict_path.as_str())).unwrap();
    assert_eq!(conflict_bytes.len(), bytes.len());
    assert_eq!(blake3::hash(&conflict_bytes), blake3::hash(&bytes));
    assert!(engine.handle.list_conflicts().await.unwrap().is_empty());
    let cursor = engine.handle.cursor().await.unwrap();
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(3));
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(3));
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.handle.pending_commands(16).await.unwrap().is_empty());
    engine.stop().await;
}
