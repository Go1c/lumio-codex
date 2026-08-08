//! Session: WebSocket lifecycle — Hello, Subscribe, snapshot/incremental stream,
//! heartbeat, and inbound frame routing to the engine worker.
//!
//! Some fields and variants are used by later sub-tasks (dispatch, transfer).

#![allow(dead_code)]

use crate::blob;
use crate::engine::EngineHandle;
use crate::error::{TransportError, TransportErrorCode};
use crate::socket::{self, InboundMessage, SocketReader, SocketWriter};
use crate::transfer::{ActiveTransfer, TransferTable, UploadIntent, UploadTransfer};

use fns_protocol::{
    MessageBody, RequestId, WorkspaceAction, WorkspaceSnapshotBeginMessage, decode_binary_frame,
    decode_server_text_frame, encode_request,
};
use fns_sync_core::SyncCommand;

use std::collections::HashSet;
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
    /// Commands collected from engine responses, waiting to be drained and sent.
    pending_outbound: Vec<SyncCommand>,
    /// Active transfer table for blob upload/download coordination.
    transfers: TransferTable,
    /// Pending download staging: transfer_id → collected bytes.
    download_staging: std::collections::HashMap<fns_protocol::TransferId, Vec<u8>>,
    /// Download requests awaiting the server-assigned Begin transfer ID.
    pending_downloads: Vec<(
        fns_protocol::WorkspaceId,
        Option<fns_protocol::OperationId>,
        fns_protocol::WorkspaceContentHash,
        u64,
    )>,
    /// Requests already sent on this connection; durable engine replay is
    /// used after reconnect instead of sending duplicates while waiting.
    inflight_mutations: HashSet<fns_protocol::OperationId>,
    inflight_acks: HashSet<fns_protocol::WorkspaceRevision>,
    /// Live events received while the engine is still completing a full
    /// snapshot. They are replayed after the snapshot Ack is confirmed.
    deferred_events: Vec<fns_protocol::WorkspaceEventMessage>,
}

/// Heartbeat interval and idle timeout.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SESSION_TRANSFERS: usize = 2;

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
                pending_outbound: Vec::new(),
                transfers: TransferTable::new(2),
                download_staging: std::collections::HashMap::new(),
                pending_downloads: Vec::new(),
                inflight_mutations: HashSet::new(),
                inflight_acks: HashSet::new(),
                deferred_events: Vec::new(),
            },
            writer,
        )
    }

    /// Run the session: send Hello, Subscribe, then process the read loop
    /// with periodic outbound drain and heartbeat.
    pub async fn run(
        mut self,
        writer: &mut SocketWriter,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> SessionResult {
        // Phase 1: Send Hello and await the response.
        if let Err(e) = self.send_hello(writer).await {
            return SessionResult::Error(e);
        }

        // Phase 2: Send Subscribe.
        if let Err(e) = self.send_subscribe(writer).await {
            return SessionResult::Error(e);
        }

        // Phase 3: Main loop — read inbound, drain outbound, heartbeat.
        let mut drain_ticker = tokio::time::interval(Duration::from_millis(200));
        drain_ticker.tick().await; // skip first immediate tick
        let mut heartbeat_ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat_ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    let _ = writer.close().await;
                    return SessionResult::Closed;
                }
                // Outbound drain: collect pending commands from engine and send them.
                _ = drain_ticker.tick() => {
                    if let Err(e) = self.drain_outbound(writer).await {
                        return SessionResult::Error(e);
                    }
                }
                // Heartbeat: send periodic ping.
                _ = heartbeat_ticker.tick() => {
                    if writer.send_pong(vec![0]).await.is_err() {
                        return SessionResult::Error(TransportError::new(
                            TransportErrorCode::Network,
                            true,
                        ));
                    }
                }
                // Inbound: read frames from server.
                msg = self.reader.next() => {
                    match msg {
                        None => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Close)) => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Ping(data))) => {
                            if writer.send_pong(data).await.is_err() {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Network,
                                    true,
                                ));
                            }
                        }
                        Some(Ok(InboundMessage::Text(data))) => {
                            match self.handle_text_frame(data).await {
                                Ok(()) => {}
                                Err(e) => return SessionResult::Error(e),
                            }
                        }
                        Some(Ok(InboundMessage::Binary(data))) => {
                            // Blob chunk from server (download direction).
                            let (header, payload) = match decode_binary_frame(&data) {
                                Ok(value) => value,
                                Err(_) => {
                                    return SessionResult::Error(TransportError::new(
                                        TransportErrorCode::Protocol,
                                        false,
                                    ));
                                }
                            };
                            if header.direction != fns_protocol::WorkspaceBlobDirection::Download
                            {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Protocol,
                                    false,
                                ));
                            }
                            let Some(staging) = self.download_staging.get_mut(&header.transfer_id)
                            else {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Protocol,
                                    false,
                                ));
                            };
                            let next_size = staging.len().saturating_add(payload.len());
                            if next_size > fns_protocol::MAX_BLOB_BYTES as usize {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Protocol,
                                    false,
                                ));
                            }
                            staging.extend_from_slice(payload);
                        }
                        Some(Ok(InboundMessage::Pong(_))) => {
                            // Heartbeat pong — connection is alive.
                        }
                        Some(Err(e)) => return SessionResult::Error(e),
                    }
                }
            }
        }
    }

    /// Drain pending outbound commands: pull from engine, encode, and send.
    async fn drain_outbound(&mut self, writer: &mut SocketWriter) -> Result<(), TransportError> {
        // First, drain any commands collected from inbound processing.
        while let Some(command) = self.pending_outbound.pop() {
            self.send_command(writer, command).await?;
        }

        // Then, poll the engine for new pending commands.
        let commands = self.engine.pending_commands(64).await?;
        for command in commands {
            self.send_command(writer, command).await?;
        }

        Ok(())
    }

    /// Encode a single SyncCommand to a wire frame and send it.
    async fn send_command(
        &mut self,
        writer: &mut SocketWriter,
        command: SyncCommand,
    ) -> Result<(), TransportError> {
        match command {
            SyncCommand::Mutation(body) => {
                if !self.inflight_mutations.insert(body.operation_id) {
                    return Ok(());
                }
                let request_id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceMutation,
                    request_id,
                    MessageBody::Mutation(body),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                writer.send_text(frame).await?;
            }
            SyncCommand::SendAck(body) => {
                if !self.inflight_acks.insert(body.revision) {
                    return Ok(());
                }
                let request_id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceAck,
                    request_id,
                    MessageBody::Ack(body),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                writer.send_text(frame).await?;
            }
            SyncCommand::DownloadBlob {
                workspace_id,
                operation_id,
                content_hash,
                size,
            } => {
                if self
                    .pending_downloads
                    .iter()
                    .any(|(_, _, pending_hash, pending_size)| {
                        pending_hash == &content_hash && *pending_size == size
                    })
                    || self.transfers.has_active_download(&content_hash, size)
                {
                    return Ok(());
                }
                // A request is already consuming a server transfer slot as
                // soon as BlobNeed is sent, even though BlobBegin has not
                // arrived yet. Keep queued downloads within our configured
                // connection limit so an upload cannot be rejected merely
                // because several downloads are waiting for their begins.
                if self.transfers.active_count() + self.pending_downloads.len()
                    >= MAX_SESSION_TRANSFERS
                {
                    return Ok(());
                }
                self.pending_downloads.push((
                    workspace_id,
                    operation_id,
                    content_hash.clone(),
                    size,
                ));
                // Send BlobNeed(download) request to server.
                let request_id = fresh_request_id();
                let need_body = fns_protocol::WorkspaceBlobNeedDownloadRequest {
                    workspace_id,
                    direction: fns_protocol::WorkspaceBlobDirection::Download,
                    operation_id: operation_id
                        .map(fns_protocol::RequiredNullable::Value)
                        .unwrap_or(fns_protocol::RequiredNullable::Null),
                    content_hash: content_hash.clone(),
                    // The v2 request contract requires size to be null; the
                    // server resolves the canonical size from its CAS and
                    // returns it in BlobNeedDownloadResponse.
                    size: fns_protocol::RequiredNullable::Null,
                };
                let frame = encode_request(
                    WorkspaceAction::WorkspaceBlobNeed,
                    request_id,
                    MessageBody::BlobNeedDownloadRequest(need_body),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                writer.send_text(frame).await?;
            }
            SyncCommand::UploadBlob {
                workspace_id,
                operation_id,
                content_hash,
                size,
            } => {
                // The engine command and the server BlobNeed can arrive in
                // either order. Register the engine half before attempting
                // to pair it with a server need.
                let retry = SyncCommand::UploadBlob {
                    workspace_id,
                    operation_id,
                    content_hash: content_hash.clone(),
                    size,
                };
                self.transfers.add_upload_intent(
                    UploadIntent {
                        workspace_id,
                        operation_id,
                        content_hash: content_hash.clone(),
                        size,
                    },
                    retry,
                );
                if self
                    .transfers
                    .has_matching_upload(&operation_id, &content_hash, size)
                    && !self.transfers.has_active_upload(&operation_id)
                    && self.transfers.active_count() + self.pending_downloads.len()
                        < MAX_SESSION_TRANSFERS
                {
                    self.upload_blob(writer, workspace_id, operation_id, content_hash, size)
                        .await?;
                }
            }
        }
        Ok(())
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
    /// Engine-returned commands are collected in pending_outbound for draining.
    async fn handle_text_frame(&mut self, data: Vec<u8>) -> Result<(), TransportError> {
        let decoded = decode_server_text_frame(&data).map_err(|_| {
            tracing::warn!("workspace sync frame decode failed");
            TransportError::new(TransportErrorCode::Protocol, false)
        })?;
        if let fns_protocol::DecodedEnvelope::Success { body, .. } = &decoded.envelope {
            tracing::debug!(
                action = %decoded.action,
                body = ?body.kind(),
                "workspace sync frame received"
            );
        } else if let fns_protocol::DecodedEnvelope::Failure { error, .. } = &decoded.envelope {
            tracing::warn!(
                action = %decoded.action,
                error = %error.code.as_str(),
                "workspace sync request rejected"
            );
        } else {
            tracing::debug!(action = %decoded.action, "workspace sync frame received");
        }
        if matches!(
            decoded.action,
            WorkspaceAction::WorkspaceBlobBegin | WorkspaceAction::WorkspaceBlobEnd
        ) && matches!(
            &decoded.envelope,
            fns_protocol::DecodedEnvelope::Failure { .. }
        ) {
            tracing::warn!(action = %decoded.action, "workspace blob transfer rejected");
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }

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
                    let cmds = self.engine_pending_from_snapshot_entry(entry).await?;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceSnapshotEnd => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotEnd(end) = body
                {
                    let cmds = self.engine_snapshot_end(end).await?;
                    self.phase = SessionPhase::Online;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceEvent => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::Event(event) = body
                {
                    let cursor = self.engine.cursor().await?;
                    let active_stream = self.engine.active_stream_mode().await?;
                    tracing::info!(
                        operation_id = %event.operation_id,
                        origin_client_id = %event.origin_client_id,
                        local_client_id = %cursor.client_id,
                        revision = %event.revision,
                        pending_ack = ?cursor.pending_ack_revision,
                        active_stream = ?active_stream,
                        "workspace event received"
                    );
                    if active_stream == Some(fns_sync_core::StreamMode::Snapshot) {
                        tracing::debug!(
                            operation_id = %event.operation_id,
                            "workspace event deferred until snapshot ack"
                        );
                        self.deferred_events.push(event);
                    } else {
                        let cmds = self.engine_workspace_event(event).await?;
                        self.pending_outbound.extend(cmds);
                    }
                }
            }
            WorkspaceAction::WorkspaceMutationAccepted => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::MutationAccepted(msg) = body
                {
                    tracing::info!(
                        operation_id = %msg.operation_id,
                        revision = %msg.revision,
                        "workspace mutation accepted"
                    );
                    self.inflight_mutations.remove(&msg.operation_id);
                    let cmds = self.engine_mutation_accepted(msg).await?;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceMutationRejected => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::MutationRejected(msg) = body
                {
                    tracing::info!(
                        operation_id = %msg.operation_id,
                        reason = ?msg.reason,
                        "workspace mutation rejected"
                    );
                    self.inflight_mutations.remove(&msg.operation_id);
                    let cmds = self.engine_mutation_rejected(msg).await?;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceConflictCreated => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::ConflictCreated(msg) = body
                {
                    tracing::info!(conflict_id = %msg.conflict_id, "workspace conflict created");
                    let cmds = self.engine_conflict_created(msg).await?;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceConflictResolved => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::ConflictResolved(msg) = body
                {
                    tracing::info!(
                        conflict_id = %msg.conflict_id,
                        revision = %msg.revision,
                        "workspace conflict resolved"
                    );
                    let cmds = self.engine_conflict_resolved(msg).await?;
                    self.pending_outbound.extend(cmds);
                }
            }
            WorkspaceAction::WorkspaceAck => {
                if matches!(
                    &decoded.envelope,
                    fns_protocol::DecodedEnvelope::Failure { .. }
                ) {
                    self.inflight_acks.clear();
                } else if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::Ack(msg) = body
                {
                    self.inflight_acks.remove(&msg.revision);
                    self.engine_ack_confirmed(msg).await?;
                    let deferred = std::mem::take(&mut self.deferred_events);
                    for event in deferred {
                        let cmds = self.engine_workspace_event(event).await?;
                        self.pending_outbound.extend(cmds);
                    }
                }
            }
            // Blob upload: server pushes BlobNeed(upload) when it needs our content.
            WorkspaceAction::WorkspaceBlobNeed => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope {
                    match body {
                        MessageBody::BlobNeedUploadPush(need) => {
                            need.validate().map_err(|_| {
                                TransportError::new(TransportErrorCode::Protocol, false)
                            })?;
                            if need.workspace_id != self.workspace_id {
                                return Err(TransportError::new(
                                    TransportErrorCode::Protocol,
                                    false,
                                ));
                            }
                            tracing::info!(
                                operation_id = %need.operation_id,
                                content_hash = %need.content_hash,
                                size = need.size,
                                "workspace blob upload requested"
                            );
                            // Server needs us to upload this blob. Keep the
                            // command pending so it is paired with an engine
                            // intent even when the push arrives first.
                            let need_clone = need.clone();
                            self.pending_outbound.push(SyncCommand::UploadBlob {
                                workspace_id: need.workspace_id,
                                operation_id: need.operation_id,
                                content_hash: need.content_hash,
                                size: need.size,
                            });
                            // Store the need for pairing.
                            self.transfers.add_server_need(need_clone)?;
                        }
                        MessageBody::BlobNeedDownloadResponse(resp) => {
                            resp.validate().map_err(|_| {
                                TransportError::new(TransportErrorCode::Protocol, false)
                            })?;
                            if resp.workspace_id != self.workspace_id {
                                return Err(TransportError::new(
                                    TransportErrorCode::Protocol,
                                    false,
                                ));
                            }
                            // The server assigns the transfer ID in the following
                            // BlobBegin push; retain only the request identity here.
                            if let Some((workspace_id, operation_id, _, pending_size)) = self
                                .pending_downloads
                                .iter_mut()
                                .find(|(_, _, pending_hash, _)| pending_hash == &resp.content_hash)
                            {
                                *workspace_id = resp.workspace_id;
                                *operation_id = resp.operation_id.into_option();
                                *pending_size = resp.size;
                            } else if !self
                                .transfers
                                .has_active_download(&resp.content_hash, resp.size)
                            {
                                self.pending_downloads.push((
                                    resp.workspace_id,
                                    resp.operation_id.into_option(),
                                    resp.content_hash,
                                    resp.size,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            WorkspaceAction::WorkspaceBlobBegin => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::BlobBegin(begin) = body
                {
                    begin
                        .validate()
                        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                    if begin.workspace_id != self.workspace_id {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    }
                    // Download begin: prepare staging for incoming chunks.
                    if begin.direction == fns_protocol::WorkspaceBlobDirection::Download {
                        let Some(index) = self.pending_downloads.iter().position(
                            |(_, _, pending_hash, pending_size)| {
                                pending_hash == &begin.content_hash && *pending_size == begin.size
                            },
                        ) else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        let (workspace_id, operation_id, _, _) =
                            self.pending_downloads.remove(index);
                        self.transfers.reserve_transfer(begin.transfer_id)?;
                        self.transfers
                            .add_download(crate::transfer::DownloadTransfer {
                                transfer_id: begin.transfer_id,
                                workspace_id,
                                operation_id,
                                content_hash: begin.content_hash.clone(),
                                size: begin.size,
                            });
                        if !self.transfers.matches_download(
                            &begin.transfer_id,
                            &begin.content_hash,
                            begin.size,
                        ) {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                        self.download_staging
                            .insert(begin.transfer_id, Vec::with_capacity(begin.size as usize));
                    }
                }
            }
            WorkspaceAction::WorkspaceBlobEnd => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::BlobEnd(end) = body
                {
                    end.validate()
                        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                    if end.workspace_id != self.workspace_id {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    }

                    if end.direction == fns_protocol::WorkspaceBlobDirection::Upload {
                        let Some(ActiveTransfer::Upload(upload)) =
                            self.transfers.remove(&end.transfer_id)
                        else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        if upload.workspace_id != end.workspace_id
                            || upload.content_hash != end.content_hash
                            || upload.size != end.size
                        {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                        tracing::info!(
                            operation_id = %upload.operation_id,
                            transfer_id = %upload.transfer_id,
                            content_hash = %upload.content_hash,
                            size = upload.size,
                            "workspace blob upload accepted"
                        );
                        // BlobEnd success is the commit point. Only after it
                        // arrives may the mutation referencing this blob be
                        // sent again.
                        self.engine.blob_uploaded(upload.operation_id).await?;
                        let _ = self.transfers.take_pending_retry(&upload.operation_id);
                        return Ok(());
                    }

                    // Download complete: verify and import into engine.
                    let Some(data) = self.download_staging.remove(&end.transfer_id) else {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    };
                    if data.len() as u64 != end.size {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    }
                    let cmds = self
                        .engine
                        .blob_available(end.content_hash.clone(), end.size, data)
                        .await?;
                    self.pending_outbound.extend(cmds);
                    self.transfers.remove(&end.transfer_id);
                }
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

    /// Upload a blob to the server: Begin → binary chunks → End.
    async fn upload_blob(
        &mut self,
        writer: &mut SocketWriter,
        workspace_id: fns_protocol::WorkspaceId,
        operation_id: fns_protocol::OperationId,
        content_hash: fns_protocol::WorkspaceContentHash,
        size: u64,
    ) -> Result<(), TransportError> {
        // Read blob content from engine's content cache.
        let file = self.engine.open_blob(&content_hash).await?;
        let content = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut buf = Vec::with_capacity(size as usize);
            let mut file = file;
            file.read_to_end(&mut buf)
                .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))?;
            Ok::<Vec<u8>, TransportError>(buf)
        })
        .await
        .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))??;

        // Verify size.
        if content.len() as u64 != size {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }

        // Reserve a transfer slot.
        let transfer_id = self.transfers.reserve_slot()?;
        self.transfers.add_upload(UploadTransfer {
            transfer_id,
            workspace_id,
            operation_id,
            content_hash: content_hash.clone(),
            size,
        });

        // Send BlobBegin.
        let begin_request_id = fresh_request_id();
        let begin_frame = blob::encode_blob_begin_upload(
            workspace_id,
            transfer_id,
            &content_hash,
            size,
            begin_request_id,
        )?;
        writer.send_text(begin_frame).await?;

        // Send binary chunks.
        if content.is_empty() {
            // Zero-byte blob: no binary frames, Begin/End only.
        } else {
            let frames = blob::chunk_blob_for_upload(transfer_id, &content)?;
            for (_, frame_bytes) in frames {
                writer.send_binary(frame_bytes).await?;
            }
        }

        // Send BlobEnd.
        let end_request_id = fresh_request_id();
        let end_frame = blob::encode_blob_end_upload(
            workspace_id,
            transfer_id,
            &content_hash,
            size,
            end_request_id,
        )?;
        writer.send_text(end_frame).await?;

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
