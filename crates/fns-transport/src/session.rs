//! Session: WebSocket lifecycle — Hello, Subscribe, snapshot/incremental stream,
//! heartbeat, and inbound frame routing to the engine worker.
//!
//! Some fields and variants are used by later sub-tasks (dispatch, transfer).

#![allow(dead_code)]

use crate::engine::EngineHandle;
use crate::error::{TransportError, TransportErrorCode};
use crate::socket::{self, InboundMessage, SocketReader, SocketWriter};

use fns_protocol::{
    MessageBody, RequestId, WorkspaceAction, WorkspaceSnapshotBeginMessage,
    decode_server_text_frame, encode_request,
};

use std::time::Duration;

/// Session phases.
#[derive(Clone, Debug)]
enum SessionPhase {
    AwaitingHello,
    AwaitingSubscribe,
    Streaming(StreamState),
    Online,
    Closing,
}

/// State tracked during an active snapshot or incremental stream.
#[derive(Clone, Debug)]
struct StreamState {
    begin: WorkspaceSnapshotBeginMessage,
    next_event_index: u32,
    event_count: u32,
    conflict_count: u32,
}

/// A connected workspace session that drives the read loop and routes inbound
/// frames to the engine worker.
pub struct Session {
    reader: SocketReader,
    phase: SessionPhase,
    engine: EngineHandle,
    workspace_id: fns_protocol::WorkspaceId,
    client_id: fns_protocol::ClientId,
    pkg_version: String,
}

/// Heartbeat interval and idle timeout.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Generate a fresh RequestId from a random UUID v4.
fn fresh_request_id() -> RequestId {
    let uuid = uuid::Uuid::new_v4();
    RequestId::parse(&uuid.to_string()).expect("valid uuid string")
}

/// Result of running the session read loop.
#[derive(Debug)]
pub enum SessionResult {
    /// The session ended normally (close frame received).
    Closed,
    /// A fatal error occurred; the caller should decide whether to reconnect.
    Error(TransportError),
}

impl Session {
    /// Create a new session from a connected socket and engine handle.
    pub fn new(
        stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        engine: EngineHandle,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        pkg_version: String,
    ) -> (Self, SocketWriter) {
        let (writer, reader) = socket::split(stream);
        (
            Self {
                reader,
                phase: SessionPhase::AwaitingHello,
                engine,
                workspace_id,
                client_id,
                pkg_version,
            },
            writer,
        )
    }

    /// Run the session: send Hello, Subscribe, then process the read loop.
    pub async fn run(
        mut self,
        writer: &mut SocketWriter,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> SessionResult {
        // Phase 1: Send Hello and await the response.
        if let Err(e) = self.send_hello(writer).await {
            return SessionResult::Error(e);
        }

        // Phase 2: Send Subscribe and await the first Begin (or failure).
        if let Err(e) = self.send_subscribe(writer).await {
            return SessionResult::Error(e);
        }

        // Phase 3: Read loop — process inbound frames until close or error.
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    let _ = writer.close().await;
                    return SessionResult::Closed;
                }
                // In a full implementation, a heartbeat timer would send pings.
                // For now, we rely on server-initiated pings.
                msg = self.reader.next() => {
                    match msg {
                        None => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Close)) => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Ping(data))) => {
                            // Respond with pong.
                            if writer.send_pong(data).await.is_err() {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Network,
                                    true,
                                ));
                            }
                        }
                        Some(Ok(InboundMessage::Text(data))) => {
                            match self.handle_text_frame(data, writer).await {
                                Ok(()) => {}
                                Err(e) => return SessionResult::Error(e),
                            }
                        }
                        Some(Ok(InboundMessage::Binary(_))) => {
                            // Binary frames are blob chunks — handled by transfer module (Task 5/6).
                            // For now, they are unexpected outside a transfer context.
                        }
                        Some(Ok(InboundMessage::Pong(_))) => {
                            // Heartbeat pong — no action needed.
                        }
                        Some(Err(e)) => return SessionResult::Error(e),
                    }
                }
            }
        }
    }

    /// Send Hello request and await the correlated response.
    async fn send_hello(&mut self, writer: &mut SocketWriter) -> Result<(), TransportError> {
        let request_id = fresh_request_id();
        let hello_body = MessageBody::HelloRequest(fns_protocol::WorkspaceHelloRequest {
            protocol_version: "2".into(),
            client_id: self.client_id,
            client_version: self.pkg_version.clone(),
            capabilities: vec![
                "binary_chunks".into(),
                "conflicts".into(),
                "snapshot_v1".into(),
            ],
        });

        let frame = encode_request(WorkspaceAction::WorkspaceHello, request_id, hello_body)
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        writer.send_text(frame).await?;

        // Await Hello response.
        loop {
            match self.reader.next().await {
                Some(Ok(InboundMessage::Text(data))) => {
                    let decoded = decode_server_text_frame(&data)
                        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;

                    if decoded.action == WorkspaceAction::WorkspaceHello {
                        if let fns_protocol::DecodedEnvelope::Success { body, .. } =
                            decoded.envelope
                            && let MessageBody::HelloResponse(hello_resp) = body
                        {
                            hello_resp.validate().map_err(|_| {
                                TransportError::new(TransportErrorCode::Protocol, false)
                            })?;
                            self.phase = SessionPhase::AwaitingSubscribe;
                            return Ok(());
                        }
                        // Hello failure or wrong body type.
                        return Err(TransportError::new(
                            TransportErrorCode::AuthenticationRejected,
                            false,
                        ));
                    }
                    // Unexpected frame before Hello response.
                    return Err(TransportError::new(TransportErrorCode::Protocol, false));
                }
                Some(Ok(InboundMessage::Close)) | None => {
                    return Err(TransportError::new(TransportErrorCode::Network, true));
                }
                Some(Ok(InboundMessage::Ping(data))) => {
                    let _ = writer.send_pong(data).await;
                }
                _ => {}
            }
        }
    }

    /// Send Subscribe request and await the first SnapshotBegin push.
    async fn send_subscribe(&mut self, writer: &mut SocketWriter) -> Result<(), TransportError> {
        let cursor = self.engine.cursor().await?;
        let request_id = fresh_request_id();

        let subscribe_body =
            MessageBody::SubscribeRequest(fns_protocol::WorkspaceSubscribeRequest {
                workspace_id: self.workspace_id,
                client_id: self.client_id,
                last_ack_revision: cursor.last_ack_revision,
            });

        let frame = encode_request(
            WorkspaceAction::WorkspaceSubscribe,
            request_id,
            subscribe_body,
        )
        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        writer.send_text(frame).await?;

        self.phase = SessionPhase::AwaitingSubscribe;
        Ok(())
    }

    /// Handle an inbound text frame by routing it to the engine worker.
    async fn handle_text_frame(
        &mut self,
        data: Vec<u8>,
        _writer: &mut SocketWriter,
    ) -> Result<(), TransportError> {
        let decoded = decode_server_text_frame(&data)
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;

        match decoded.action {
            WorkspaceAction::WorkspaceSnapshotBegin => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotBegin(begin) = body
                {
                    begin
                        .validate()
                        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                    self.phase = SessionPhase::Streaming(StreamState {
                        begin: begin.clone(),
                        next_event_index: 0,
                        event_count: begin.event_count,
                        conflict_count: begin.conflict_count,
                    });
                    // Call snapshot_begin on the engine.
                    self.engine_snapshot_begin(begin).await?;
                }
            }
            WorkspaceAction::WorkspaceSnapshotEntry => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotEntry(entry) = body
                {
                    let _ = self.engine_pending_from_snapshot_entry(entry).await?;
                }
            }
            WorkspaceAction::WorkspaceSnapshotEnd => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotEnd(end) = body
                {
                    let commands = self.engine_snapshot_end(end).await?;
                    self.phase = SessionPhase::Online;
                    // In a full implementation, commands would be encoded and sent.
                    let _ = commands;
                }
            }
            WorkspaceAction::WorkspaceEvent => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::Event(event) = body
                {
                    let _ = self.engine_workspace_event(event).await?;
                }
            }
            WorkspaceAction::WorkspaceMutationAccepted => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::MutationAccepted(msg) = body
                {
                    let _ = self.engine_mutation_accepted(msg).await?;
                }
            }
            WorkspaceAction::WorkspaceMutationRejected => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::MutationRejected(msg) = body
                {
                    let _ = self.engine_mutation_rejected(msg).await?;
                }
            }
            WorkspaceAction::WorkspaceConflictCreated => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::ConflictCreated(msg) = body
                {
                    let _ = self.engine_conflict_created(msg).await?;
                }
            }
            WorkspaceAction::WorkspaceConflictResolved => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::ConflictResolved(msg) = body
                {
                    let _ = self.engine_conflict_resolved(msg).await?;
                }
            }
            WorkspaceAction::WorkspaceAck => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::Ack(msg) = body
                {
                    self.engine_ack_confirmed(msg).await?;
                }
            }
            // Blob-related actions are handled by the transfer module (Task 5/6).
            WorkspaceAction::WorkspaceBlobNeed
            | WorkspaceAction::WorkspaceBlobBegin
            | WorkspaceAction::WorkspaceBlobEnd => {
                // TODO: Route to transfer module.
            }
            // Client-only actions should never come from the server.
            WorkspaceAction::WorkspaceHello | WorkspaceAction::WorkspaceSubscribe => {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
            WorkspaceAction::WorkspaceMutation => {
                // Mutation is a client request; a failure response uses the same action.
                if let fns_protocol::DecodedEnvelope::Failure { .. } = decoded.envelope {
                    // Protocol-level failure for a mutation request.
                    // In dispatch, this would be correlated by request ID.
                }
            }
        }
        Ok(())
    }

    // --- Engine method wrappers ---

    async fn engine_snapshot_begin(
        &self,
        msg: WorkspaceSnapshotBeginMessage,
    ) -> Result<(), TransportError> {
        self.engine.snapshot_begin(msg).await
    }

    async fn engine_pending_from_snapshot_entry(
        &self,
        msg: fns_protocol::WorkspaceSnapshotEntryMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.snapshot_entry(msg).await
    }

    async fn engine_snapshot_end(
        &self,
        msg: fns_protocol::WorkspaceSnapshotEndMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.snapshot_end(msg).await
    }

    async fn engine_workspace_event(
        &self,
        msg: fns_protocol::WorkspaceEventMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.workspace_event(msg).await
    }

    async fn engine_mutation_accepted(
        &self,
        msg: fns_protocol::WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.mutation_accepted(msg).await
    }

    async fn engine_mutation_rejected(
        &self,
        msg: fns_protocol::WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.mutation_rejected(msg).await
    }

    async fn engine_conflict_created(
        &self,
        msg: fns_protocol::WorkspaceConflictCreatedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.conflict_created(msg).await
    }

    async fn engine_conflict_resolved(
        &self,
        msg: fns_protocol::WorkspaceConflictResolvedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        self.engine.conflict_resolved(msg).await
    }

    async fn engine_ack_confirmed(
        &self,
        msg: fns_protocol::WorkspaceAckRequest,
    ) -> Result<(), TransportError> {
        self.engine.ack_confirmed(msg).await
    }
}
