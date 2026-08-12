//! Session: WebSocket lifecycle — Hello, Subscribe, snapshot/incremental stream,
//! heartbeat, and inbound frame routing to the engine worker.
//!
//! Some fields and variants are used by later sub-tasks (dispatch, transfer).

#![allow(dead_code)]

use crate::blob;
use crate::config::{
    IDLE_TIMEOUT, INBOUND_QUEUE_CAPACITY, OUTBOUND_QUEUE_CAPACITY, REQUEST_TIMEOUT,
    TRANSFER_IDLE_TIMEOUT, TRANSFER_MAX_LIFETIME,
};
use crate::dispatch::{ExpectedResponse, RequestTracker};
use crate::engine::EngineHandle;
use crate::error::{TransportError, TransportErrorCode};
use crate::socket::{self, InboundMessage, SocketReader, SocketWriter};
use crate::transfer::{
    ActiveTransfer, DownloadPhase, DownloadTransfer, TransferTable, UploadIntent, UploadPhase,
    UploadTransfer,
};

use fns_protocol::{
    MessageBody, RequestId, WorkspaceAction, WorkspaceBlobEndMessage,
    WorkspaceSnapshotBeginMessage, decode_binary_frame, decode_server_text_frame, encode_request,
};
use fns_sync_core::SyncCommand;

use std::collections::VecDeque;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::Instant;

/// Session phases.
#[derive(Clone, Debug)]
enum SessionPhase {
    AwaitingHello,
    AwaitingSubscribe,
    Streaming(StreamState),
    Online,
    Closing,
}

/// Coarse connection phase published to the owning agent process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionConnectionPhase {
    Handshaking,
    Subscribing,
    Online,
}

/// Ephemeral connection metrics. Durable queue/Ack metrics remain owned by the
/// engine and are intentionally reported separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionRuntimeStatus {
    pub phase: SessionConnectionPhase,
    pub active_transfers: usize,
}

impl SessionRuntimeStatus {
    const fn handshaking() -> Self {
        Self {
            phase: SessionConnectionPhase::Handshaking,
            active_transfers: 0,
        }
    }
}

/// State tracked during an active snapshot or incremental stream.
#[derive(Clone, Debug)]
struct StreamState {
    begin: WorkspaceSnapshotBeginMessage,
    next_event_index: u32,
    event_count: u32,
    conflict_count: u32,
    end_received: bool,
}

struct PendingDownload {
    workspace_id: fns_protocol::WorkspaceId,
    operation_id: Option<fns_protocol::OperationId>,
    content_hash: fns_protocol::WorkspaceContentHash,
    size: u64,
    started_at: Instant,
    last_progress_at: Instant,
}

enum DeferredLiveMessage {
    Event(Box<fns_protocol::WorkspaceEventMessage>),
    ConflictCreated(fns_protocol::WorkspaceConflictCreatedMessage),
    ConflictResolved(fns_protocol::WorkspaceConflictResolvedMessage),
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
    pending_outbound: VecDeque<SyncCommand>,
    /// Active transfer table for blob upload/download coordination.
    transfers: TransferTable,
    /// Download requests awaiting the server-assigned Begin transfer ID.
    pending_downloads: Vec<PendingDownload>,
    requests: RequestTracker,
    /// Live messages received after a stream End while that stream is still
    /// awaiting durable completion and its final Ack.
    deferred_events: VecDeque<DeferredLiveMessage>,
    deferred_events_ready: bool,
    deferred_until_ack_revision: Option<fns_protocol::WorkspaceRevision>,
    refresh_requested: bool,
    subscribe_deadline: Option<Instant>,
    last_inbound_at: Instant,
    limits: SessionLimits,
    status_tx: Option<tokio::sync::watch::Sender<SessionRuntimeStatus>>,
    /// Best-effort transport diagnostics (shared RuntimeDiagnostics runId).
    diagnostics: crate::obs::TransportDiagnostics,
    last_published_phase: Option<SessionConnectionPhase>,
}

/// Production session deadlines and per-poll work limits.
#[derive(Clone, Copy, Debug)]
pub struct SessionLimits {
    pub heartbeat_interval: Duration,
    pub drain_interval: Duration,
    pub request_timeout: Duration,
    pub idle_timeout: Duration,
    pub transfer_idle_timeout: Duration,
    pub transfer_max_lifetime: Duration,
    pub drain_item_budget: usize,
    pub drain_byte_budget: usize,
    pub pending_outbound_capacity: usize,
    pub deferred_event_capacity: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            heartbeat_interval: HEARTBEAT_INTERVAL,
            drain_interval: Duration::from_millis(200),
            request_timeout: REQUEST_TIMEOUT,
            idle_timeout: IDLE_TIMEOUT,
            transfer_idle_timeout: TRANSFER_IDLE_TIMEOUT,
            transfer_max_lifetime: TRANSFER_MAX_LIFETIME,
            drain_item_budget: OUTBOUND_QUEUE_CAPACITY,
            drain_byte_budget: (fns_protocol::MAX_CONTROL_FRAME_BYTES * OUTBOUND_QUEUE_CAPACITY)
                .max(fns_protocol::BLOB_HEADER_LEN + fns_protocol::BLOB_CHUNK_BYTES as usize),
            pending_outbound_capacity: OUTBOUND_QUEUE_CAPACITY,
            deferred_event_capacity: INBOUND_QUEUE_CAPACITY,
        }
    }
}

/// Heartbeat interval advertised by the v2 protocol.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const MAX_SESSION_TRANSFERS: usize = 2;

struct DrainBudget {
    items_remaining: usize,
    bytes_remaining: usize,
}

impl DrainBudget {
    fn new(limits: SessionLimits) -> Self {
        Self {
            items_remaining: limits.drain_item_budget,
            bytes_remaining: limits.drain_byte_budget,
        }
    }

    fn claim_item(&mut self) -> bool {
        if self.items_remaining == 0 {
            return false;
        }
        self.items_remaining -= 1;
        true
    }

    fn reserve_bytes(&mut self, bytes: usize) -> bool {
        if bytes > self.bytes_remaining {
            return false;
        }
        self.bytes_remaining -= bytes;
        true
    }

    fn can_reserve_bytes(&self, bytes: usize) -> bool {
        bytes <= self.bytes_remaining
    }
}

enum CommandSendResult {
    Consumed,
    Deferred(SyncCommand),
}

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
        Self::new_with_limits(
            stream,
            engine,
            workspace_id,
            client_id,
            pkg_version,
            SessionLimits::default(),
        )
    }

    /// Construct a production session and return a read-only runtime status
    /// receiver for the owning agent.
    pub fn new_observed(
        stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        engine: EngineHandle,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        pkg_version: String,
    ) -> (
        Self,
        SocketWriter,
        tokio::sync::watch::Receiver<SessionRuntimeStatus>,
    ) {
        let (status_tx, status_rx) =
            tokio::sync::watch::channel(SessionRuntimeStatus::handshaking());
        let (session, writer) = Self::new_with_limits_and_status(
            stream,
            engine,
            workspace_id,
            client_id,
            pkg_version,
            SessionLimits::default(),
            Some(status_tx),
        );
        (session, writer, status_rx)
    }

    /// Construct a session with explicit limits. Production callers use
    /// `Session::new`; tests use this constructor to prove deadlines without
    /// wall-clock sleeps.
    pub fn new_with_limits(
        stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        engine: EngineHandle,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        pkg_version: String,
        limits: SessionLimits,
    ) -> (Self, SocketWriter) {
        Self::new_with_limits_and_status(
            stream,
            engine,
            workspace_id,
            client_id,
            pkg_version,
            limits,
            None,
        )
    }

    fn new_with_limits_and_status(
        stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        engine: EngineHandle,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        pkg_version: String,
        limits: SessionLimits,
        status_tx: Option<tokio::sync::watch::Sender<SessionRuntimeStatus>>,
    ) -> (Self, SocketWriter) {
        let (writer, reader) = socket::split(stream);
        let now = Instant::now();
        (
            Self {
                reader,
                phase: SessionPhase::AwaitingHello,
                engine,
                workspace_id,
                client_id,
                pkg_version,
                pending_outbound: VecDeque::new(),
                transfers: TransferTable::new(2),
                pending_downloads: Vec::new(),
                requests: RequestTracker::new(),
                deferred_events: VecDeque::new(),
                deferred_events_ready: false,
                deferred_until_ack_revision: None,
                refresh_requested: false,
                subscribe_deadline: None,
                last_inbound_at: now,
                limits,
                status_tx,
                diagnostics: crate::obs::TransportDiagnostics::none(),
                last_published_phase: None,
            },
            writer,
        )
    }

    /// Attach shared runtime diagnostics so phase/reconnect/request events
    /// share the agent process `runId`. Safe to call before `run`.
    pub fn with_diagnostics(mut self, diagnostics: crate::obs::TransportDiagnostics) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// Run the session: send Hello, Subscribe, then process the read loop
    /// with periodic outbound drain and heartbeat.
    pub async fn run(
        mut self,
        writer: &mut SocketWriter,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> SessionResult {
        if let Err(error) = self.engine.prepare_connection_attempt().await {
            return SessionResult::Error(error);
        }
        let result = self.run_inner(writer, shutdown).await;
        match self.engine.abort_blob_imports().await {
            Ok(()) => result,
            Err(error) => SessionResult::Error(error),
        }
    }

    async fn run_inner(
        &mut self,
        writer: &mut SocketWriter,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> SessionResult {
        // Phase 1: Send Hello and await the response.
        let hello = tokio::select! {
            _ = shutdown.cancelled() => {
                let _ = writer.close().await;
                return SessionResult::Closed;
            }
            result = tokio::time::timeout(self.limits.request_timeout, self.send_hello(writer)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(TransportError::new(
                        TransportErrorCode::RequestTimeout,
                        true,
                    )),
                }
            }
        };
        if let Err(error) = hello {
            return SessionResult::Error(error);
        }

        // Phase 2: Send Subscribe.
        let subscribe = tokio::select! {
            _ = shutdown.cancelled() => {
                let _ = writer.close().await;
                return SessionResult::Closed;
            }
            result = self.send_subscribe(writer) => result,
        };
        if let Err(error) = subscribe {
            return SessionResult::Error(error);
        }

        // Phase 3: Main loop — read inbound, drain outbound, heartbeat.
        let mut drain_ticker = tokio::time::interval(self.limits.drain_interval);
        drain_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        drain_ticker.tick().await; // skip first immediate tick
        let mut heartbeat_ticker = tokio::time::interval(self.limits.heartbeat_interval);
        heartbeat_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat_ticker.tick().await;

        loop {
            let deadline = self.next_deadline();
            tokio::select! {
                _ = shutdown.cancelled() => {
                    let _ = writer.close().await;
                    return SessionResult::Closed;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    if let Some(error) = self.expired_error(Instant::now()) {
                        return SessionResult::Error(error);
                    }
                }
                // Outbound drain: collect pending commands from engine and send them.
                _ = drain_ticker.tick() => {
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => {
                            let _ = writer.close().await;
                            return SessionResult::Closed;
                        }
                        result = self.drain_outbound(writer) => result,
                    };
                    if let Err(e) = result {
                        return SessionResult::Error(e);
                    }
                }
                // Heartbeat: send periodic ping.
                _ = heartbeat_ticker.tick() => {
                    let result = tokio::select! {
                        _ = shutdown.cancelled() => {
                            let _ = writer.close().await;
                            return SessionResult::Closed;
                        }
                        result = writer.send_ping(vec![0]) => result,
                    };
                    if result.is_err() {
                        return SessionResult::Error(TransportError::new(
                            TransportErrorCode::Network,
                            true,
                        ));
                    }
                }
                // Inbound: read frames from server.
                msg = self.reader.next() => {
                    self.last_inbound_at = Instant::now();
                    match msg {
                        None => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Close)) => return SessionResult::Closed,
                        Some(Ok(InboundMessage::Ping(data))) => {
                            let result = tokio::select! {
                                _ = shutdown.cancelled() => {
                                    let _ = writer.close().await;
                                    return SessionResult::Closed;
                                }
                                result = writer.send_pong(data) => result,
                            };
                            if result.is_err() {
                                return SessionResult::Error(TransportError::new(
                                    TransportErrorCode::Network,
                                    true,
                                ));
                            }
                        }
                        Some(Ok(InboundMessage::Text(data))) => {
                            let result = tokio::select! {
                                _ = shutdown.cancelled() => {
                                    let _ = writer.close().await;
                                    return SessionResult::Closed;
                                }
                                result = self.handle_text_frame(data) => result,
                            };
                            match result {
                                Ok(()) => {}
                                Err(e) => return SessionResult::Error(e),
                            }
                        }
                        Some(Ok(InboundMessage::Binary(data))) => {
                            let result = tokio::select! {
                                _ = shutdown.cancelled() => {
                                    let _ = writer.close().await;
                                    return SessionResult::Closed;
                                }
                                result = self.handle_binary_frame(data) => result,
                            };
                            if let Err(error) = result {
                                return SessionResult::Error(error);
                            }
                        }
                        Some(Ok(InboundMessage::Pong(_))) => {
                            // Heartbeat pong — connection is alive.
                        }
                        Some(Err(e)) => return SessionResult::Error(e),
                    }
                }
            }
            self.publish_runtime_status();
        }
    }

    fn publish_runtime_status(&mut self) {
        let phase = match &self.phase {
            SessionPhase::AwaitingHello => SessionConnectionPhase::Handshaking,
            SessionPhase::AwaitingSubscribe | SessionPhase::Streaming(_) => {
                SessionConnectionPhase::Subscribing
            }
            SessionPhase::Online => SessionConnectionPhase::Online,
            SessionPhase::Closing => return,
        };
        let active_transfers = self.transfers.active_count();
        if self.last_published_phase != Some(phase) {
            let phase_name = match phase {
                SessionConnectionPhase::Handshaking => "handshaking",
                SessionConnectionPhase::Subscribing => "subscribing",
                SessionConnectionPhase::Online => "online",
            };
            self.diagnostics
                .on_phase(phase_name, "session connection phase advanced");
            self.last_published_phase = Some(phase);
        }
        self.diagnostics.on_transfer("session", active_transfers);
        let Some(status_tx) = &self.status_tx else {
            return;
        };
        let next = SessionRuntimeStatus {
            phase,
            active_transfers,
        };
        let _ = status_tx.send_if_modified(|current| {
            if *current == next {
                return false;
            }
            *current = next;
            true
        });
    }

    async fn handle_binary_frame(
        &mut self,
        mut data: tokio_tungstenite::tungstenite::Bytes,
    ) -> Result<(), TransportError> {
        let (header, payload) = decode_binary_frame(&data)
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        if header.direction != fns_protocol::WorkspaceBlobDirection::Download {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let payload_len = payload.len();
        let transfer_id = header.transfer_id;
        let (expected_index, expected_offset, size, chunk_count, phase) =
            match self.transfers.get(&transfer_id) {
                Some(ActiveTransfer::Download(download)) => (
                    download.next_chunk_index,
                    download.next_offset,
                    download.size,
                    download.begin.chunk_count,
                    download.phase,
                ),
                _ => return Err(TransportError::new(TransportErrorCode::Protocol, false)),
            };
        if phase != DownloadPhase::Receiving {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let next_offset = expected_offset
            .checked_add(payload_len as u64)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        if next_offset > size {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let is_last = next_offset == size;
        header
            .validate_sequence(expected_index, expected_offset, is_last)
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        let next_index = expected_index
            .checked_add(1)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        if next_index > chunk_count || (is_last && next_index != chunk_count) {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }

        let payload = data.split_off(fns_protocol::BLOB_HEADER_LEN);
        self.engine.write_blob_chunk(transfer_id, payload).await?;
        let Some(ActiveTransfer::Download(download)) = self.transfers.get_mut(&transfer_id) else {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        };
        download.next_chunk_index = next_index;
        download.next_offset = next_offset;
        self.transfers.mark_progress(&transfer_id, Instant::now())
    }

    fn next_deadline(&self) -> Instant {
        let mut deadline = self.last_inbound_at + self.limits.idle_timeout;
        if let Some(request_deadline) = self.requests.next_deadline(self.limits.request_timeout) {
            deadline = deadline.min(request_deadline);
        }
        if let Some(subscribe_deadline) = self.subscribe_deadline {
            deadline = deadline.min(subscribe_deadline);
        }
        if let Some(transfer_deadline) = self.transfers.next_deadline(
            self.limits.transfer_idle_timeout,
            self.limits.transfer_max_lifetime,
        ) {
            deadline = deadline.min(transfer_deadline);
        }
        if let Some(pending_deadline) = self
            .pending_downloads
            .iter()
            .map(|pending| {
                (pending.started_at + self.limits.transfer_max_lifetime)
                    .min(pending.last_progress_at + self.limits.transfer_idle_timeout)
            })
            .min()
        {
            deadline = deadline.min(pending_deadline);
        }
        deadline
    }

    fn expired_error(&self, now: Instant) -> Option<TransportError> {
        if self
            .requests
            .expired(now, self.limits.request_timeout)
            .is_some()
            || self
                .subscribe_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            return Some(TransportError::new(
                TransportErrorCode::RequestTimeout,
                true,
            ));
        }
        if self
            .transfers
            .expired(
                now,
                self.limits.transfer_idle_timeout,
                self.limits.transfer_max_lifetime,
            )
            .is_some()
            || self.pending_downloads.iter().any(|pending| {
                now.saturating_duration_since(pending.started_at)
                    >= self.limits.transfer_max_lifetime
                    || now.saturating_duration_since(pending.last_progress_at)
                        >= self.limits.transfer_idle_timeout
            })
        {
            return Some(TransportError::new(
                TransportErrorCode::TransferTimeout,
                true,
            ));
        }
        if now.saturating_duration_since(self.last_inbound_at) >= self.limits.idle_timeout {
            return Some(TransportError::new(TransportErrorCode::IdleTimeout, true));
        }
        None
    }

    /// Drain pending outbound commands: pull from engine, encode, and send.
    async fn drain_outbound(&mut self, writer: &mut SocketWriter) -> Result<(), TransportError> {
        if self.requests.needs_reconnect_for_id_limit() {
            return Err(TransportError::new(TransportErrorCode::Network, true));
        }
        if self.refresh_requested {
            self.send_subscribe(writer).await?;
            self.refresh_requested = false;
            return Ok(());
        }
        self.refresh_stream_completion().await?;
        let mut budget = DrainBudget::new(self.limits);

        if budget.claim_item() {
            self.advance_download_end(writer, &mut budget).await?;
        }
        if budget.claim_item() {
            self.advance_upload(writer, &mut budget).await?;
        }

        // Ack has priority over an arbitrarily large outbox. It is synthesized
        // from the durable cursor, so no queued command is dropped or reordered.
        let cursor = self.engine.cursor().await?;
        if matches!(self.phase, SessionPhase::Online)
            && let Some(revision) = cursor.pending_ack_revision
            && !self.requests.has_ack_in_flight()
            && budget.claim_item()
        {
            let ack = SyncCommand::SendAck(fns_protocol::WorkspaceAckRequest {
                workspace_id: cursor.workspace_id,
                client_id: cursor.client_id,
                revision,
            });
            if let CommandSendResult::Deferred(command) =
                self.send_command(writer, ack, &mut budget).await?
            {
                self.enqueue_front(command)?;
                return Ok(());
            }
        }

        // Deferred live messages become eligible only after the preceding
        // stream Ack is confirmed. Process a bounded number per drain tick.
        while self.deferred_events_ready && budget.claim_item() {
            let Some(message) = self.deferred_events.pop_front() else {
                self.deferred_events_ready = false;
                break;
            };
            let commands = match message {
                DeferredLiveMessage::Event(event) => self.engine_workspace_event(*event).await?,
                DeferredLiveMessage::ConflictCreated(message) => {
                    self.engine_conflict_created(message).await?
                }
                DeferredLiveMessage::ConflictResolved(message) => {
                    self.engine_conflict_resolved(message).await?
                }
            };
            self.enqueue_replayable_download_commands(commands)?;
        }

        self.drain_pending_queue(writer, &mut budget).await?;

        if budget.items_remaining == 0
            || self.pending_outbound.len() >= self.limits.pending_outbound_capacity
        {
            return Ok(());
        }

        let fetch_limit = budget
            .items_remaining
            .min(self.limits.pending_outbound_capacity - self.pending_outbound.len())
            .min(self.requests.available_slots().max(1));
        if fetch_limit == 0 {
            return Ok(());
        }
        let commands = self.engine.pending_commands(fetch_limit).await?;
        self.refresh_stream_completion().await?;
        self.enqueue_commands(commands)?;
        self.drain_pending_queue(writer, &mut budget).await?;

        Ok(())
    }

    async fn advance_download_end(
        &mut self,
        writer: &mut SocketWriter,
        budget: &mut DrainBudget,
    ) -> Result<(), TransportError> {
        let Some(transfer_id) = self.transfers.download_ready_for_end() else {
            return Ok(());
        };
        let end = match self.transfers.get(&transfer_id) {
            Some(ActiveTransfer::Download(download)) => WorkspaceBlobEndMessage {
                workspace_id: download.workspace_id,
                transfer_id,
                direction: fns_protocol::WorkspaceBlobDirection::Download,
                content_hash: download.content_hash.clone(),
                size: download.size,
                chunk_count: download.begin.chunk_count,
            },
            _ => return Err(TransportError::new(TransportErrorCode::Protocol, false)),
        };
        let request_id = fresh_request_id();
        let frame = encode_request(
            WorkspaceAction::WorkspaceBlobEnd,
            request_id,
            MessageBody::BlobEnd(end.clone()),
        )
        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        if !self
            .send_tracked_text(
                writer,
                request_id,
                ExpectedResponse::BlobEndDownload(end),
                frame,
                budget,
            )
            .await?
        {
            return Ok(());
        }
        let Some(ActiveTransfer::Download(download)) = self.transfers.get_mut(&transfer_id) else {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        };
        download.phase = DownloadPhase::AwaitingEndResponse;
        self.transfers.mark_progress(&transfer_id, Instant::now())
    }

    async fn advance_upload(
        &mut self,
        writer: &mut SocketWriter,
        budget: &mut DrainBudget,
    ) -> Result<(), TransportError> {
        if let Some(mut upload) = self.transfers.take_upload_ready_for_end() {
            let request_id = fresh_request_id();
            let end = WorkspaceBlobEndMessage {
                workspace_id: upload.workspace_id,
                transfer_id: upload.transfer_id,
                direction: fns_protocol::WorkspaceBlobDirection::Upload,
                content_hash: upload.content_hash.clone(),
                size: upload.size,
                chunk_count: blob::chunk_count(upload.size),
            };
            let frame = blob::encode_blob_end_upload(
                upload.workspace_id,
                upload.transfer_id,
                &upload.content_hash,
                upload.size,
                request_id,
            )?;
            if !self
                .send_tracked_text(
                    writer,
                    request_id,
                    ExpectedResponse::BlobEndUpload(end),
                    frame,
                    budget,
                )
                .await?
            {
                self.transfers.put_upload(upload);
                return Ok(());
            }
            upload.phase = UploadPhase::AwaitingEndResponse;
            let transfer_id = upload.transfer_id;
            self.transfers.put_upload(upload);
            self.transfers.mark_progress(&transfer_id, Instant::now())?;
            return Ok(());
        }

        let Some(mut upload) = self.transfers.take_streaming_upload() else {
            return Ok(());
        };
        if upload.next_offset == upload.size {
            let mut extra = [0_u8; 1];
            if upload
                .file
                .read(&mut extra)
                .await
                .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))?
                != 0
            {
                return Err(TransportError::new(TransportErrorCode::Filesystem, false));
            }
            let actual = fns_protocol::WorkspaceContentHash::parse(&format!(
                "blake3:{}",
                upload.hasher.clone().finalize().to_hex()
            ))
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
            if actual != upload.content_hash {
                return Err(TransportError::new(TransportErrorCode::Filesystem, false));
            }
            upload.phase = UploadPhase::ReadyForEnd;
            self.transfers.put_upload(upload);
            return Ok(());
        }

        let remaining = upload.size - upload.next_offset;
        let payload_len = remaining.min(u64::from(fns_protocol::BLOB_CHUNK_BYTES)) as usize;
        let frame_len = fns_protocol::BLOB_HEADER_LEN + payload_len;
        if !budget.can_reserve_bytes(frame_len) {
            self.transfers.put_upload(upload);
            return Ok(());
        }
        let mut payload = Vec::with_capacity(frame_len);
        payload.resize(payload_len, 0);
        upload
            .file
            .read_exact(&mut payload)
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))?;
        upload.hasher.update(&payload);
        let final_chunk = upload.next_offset + payload_len as u64 == upload.size;
        if final_chunk {
            let mut extra = [0_u8; 1];
            if upload
                .file
                .read(&mut extra)
                .await
                .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))?
                != 0
            {
                return Err(TransportError::new(TransportErrorCode::Filesystem, false));
            }
            let actual = fns_protocol::WorkspaceContentHash::parse(&format!(
                "blake3:{}",
                upload.hasher.clone().finalize().to_hex()
            ))
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
            if actual != upload.content_hash {
                return Err(TransportError::new(TransportErrorCode::Filesystem, false));
            }
        }
        let frame = fns_protocol::encode_binary_frame_owned(
            fns_protocol::WorkspaceBlobDirection::Upload,
            final_chunk,
            upload.transfer_id,
            upload.next_chunk_index,
            upload.next_offset,
            payload,
        )
        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        if !budget.reserve_bytes(frame.len()) {
            self.transfers.put_upload(upload);
            return Ok(());
        }
        writer.send_binary(frame).await?;
        upload.next_offset += payload_len as u64;
        upload.next_chunk_index += 1;
        if final_chunk {
            upload.phase = UploadPhase::ReadyForEnd;
        }
        let transfer_id = upload.transfer_id;
        self.transfers.put_upload(upload);
        self.transfers.mark_progress(&transfer_id, Instant::now())
    }

    async fn drain_pending_queue(
        &mut self,
        writer: &mut SocketWriter,
        budget: &mut DrainBudget,
    ) -> Result<(), TransportError> {
        while budget.claim_item() {
            let Some(command) = self.pending_outbound.pop_front() else {
                break;
            };
            match self.send_command(writer, command, budget).await? {
                CommandSendResult::Consumed => {}
                CommandSendResult::Deferred(command) => {
                    self.pending_outbound.push_front(command);
                    break;
                }
            }
        }
        Ok(())
    }

    fn enqueue_commands(&mut self, commands: Vec<SyncCommand>) -> Result<(), TransportError> {
        if self.pending_outbound.len().saturating_add(commands.len())
            > self.limits.pending_outbound_capacity
        {
            return Err(TransportError::new(TransportErrorCode::ResourceLimit, true));
        }
        self.pending_outbound.extend(commands);
        Ok(())
    }

    // The engine retains these inbound downloads until apply and Ack complete.
    fn enqueue_replayable_download_commands(
        &mut self,
        commands: Vec<SyncCommand>,
    ) -> Result<(), TransportError> {
        if !commands
            .iter()
            .all(|command| matches!(command, SyncCommand::DownloadBlob { .. }))
        {
            return self.enqueue_commands(commands);
        }

        let available = self
            .limits
            .pending_outbound_capacity
            .saturating_sub(self.pending_outbound.len());
        let deferred = commands.len().saturating_sub(available);
        self.pending_outbound
            .extend(commands.into_iter().take(available));
        if deferred > 0 {
            tracing::debug!(
                deferred,
                capacity = self.limits.pending_outbound_capacity,
                "replayable blob downloads deferred until the outbound queue drains"
            );
        }
        Ok(())
    }

    fn enqueue_front(&mut self, command: SyncCommand) -> Result<(), TransportError> {
        if self.pending_outbound.len() >= self.limits.pending_outbound_capacity {
            return Err(TransportError::new(TransportErrorCode::ResourceLimit, true));
        }
        self.pending_outbound.push_front(command);
        Ok(())
    }

    fn complete_response(
        &mut self,
        request_id: Option<RequestId>,
    ) -> Result<ExpectedResponse, TransportError> {
        let request_id =
            request_id.ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        self.requests.complete(&request_id)
    }

    async fn send_tracked_text(
        &mut self,
        writer: &mut SocketWriter,
        request_id: RequestId,
        expected: ExpectedResponse,
        frame: Vec<u8>,
        budget: &mut DrainBudget,
    ) -> Result<bool, TransportError> {
        if self.requests.available_slots() == 0 || !budget.reserve_bytes(frame.len()) {
            return Ok(false);
        }
        self.requests.track(request_id, expected, Instant::now())?;
        if let Err(error) = writer.send_text(frame).await {
            self.requests.cancel_unsent(&request_id);
            return Err(error);
        }
        Ok(true)
    }

    /// Encode a single SyncCommand to a wire frame and send it.
    async fn send_command(
        &mut self,
        writer: &mut SocketWriter,
        command: SyncCommand,
        budget: &mut DrainBudget,
    ) -> Result<CommandSendResult, TransportError> {
        match command {
            SyncCommand::Mutation(body) => {
                if self.requests.has_mutation(body.operation_id) {
                    return Ok(CommandSendResult::Consumed);
                }
                let request_id = fresh_request_id();
                let expected = ExpectedResponse::Mutation {
                    workspace_id: body.workspace_id,
                    client_id: body.client_id,
                    operation_id: body.operation_id,
                };
                let frame = encode_request(
                    WorkspaceAction::WorkspaceMutation,
                    request_id,
                    MessageBody::Mutation(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                if !self
                    .send_tracked_text(writer, request_id, expected, frame, budget)
                    .await?
                {
                    return Ok(CommandSendResult::Deferred(SyncCommand::Mutation(body)));
                }
            }
            SyncCommand::ResolveConflict(body) => {
                if self.requests.has_conflict_resolution(body.operation_id) {
                    return Ok(CommandSendResult::Consumed);
                }
                body.validate()
                    .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                let request_id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceConflictResolved,
                    request_id,
                    MessageBody::ConflictResolvedRequest(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                if !self
                    .send_tracked_text(
                        writer,
                        request_id,
                        ExpectedResponse::ConflictResolved(body.clone()),
                        frame,
                        budget,
                    )
                    .await?
                {
                    return Ok(CommandSendResult::Deferred(SyncCommand::ResolveConflict(
                        body,
                    )));
                }
            }
            SyncCommand::SendAck(body) => {
                let cursor = self.engine.cursor().await?;
                // Classify the ack: terminal (stream-completion) or segment
                // (partial applied prefix of an unfinished incremental stream).
                let is_terminal_ack = cursor.pending_ack_revision == Some(body.revision);
                let is_segment_ack = cursor.pending_segment_ack_revision == Some(body.revision)
                    && cursor.pending_ack_revision.is_none();
                if !is_terminal_ack && !is_segment_ack {
                    return Ok(CommandSendResult::Consumed);
                }
                // Terminal acks are sent once the session is fully online.
                // Segment acks are also safe during Streaming: the connection
                // is established and every expected event in the range has
                // been fully processed (no blob download in flight).
                let is_online = matches!(self.phase, SessionPhase::Online);
                if is_terminal_ack && !is_online {
                    return Ok(CommandSendResult::Consumed);
                }
                if !is_online && !matches!(self.phase, SessionPhase::Streaming(_)) {
                    return Ok(CommandSendResult::Consumed);
                }
                if self.requests.has_ack_in_flight() {
                    return Ok(CommandSendResult::Consumed);
                }
                let request_id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceAck,
                    request_id,
                    MessageBody::Ack(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                if !self
                    .send_tracked_text(
                        writer,
                        request_id,
                        ExpectedResponse::Ack(body.clone()),
                        frame,
                        budget,
                    )
                    .await?
                {
                    return Ok(CommandSendResult::Deferred(SyncCommand::SendAck(body)));
                }
            }
            SyncCommand::ResolveConflict(body) => {
                let request_id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceConflictResolved,
                    request_id,
                    MessageBody::ConflictResolvedRequest(body),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                writer.send_text(frame).await?;
            }
            SyncCommand::DownloadBlob { .. } | SyncCommand::UploadBlob { .. } => {
                // Blob transfers require transfer-table coordination (Task 5/6 deeper integration).
                // For now these are skipped — they'll be handled when transfer wire is wired.
            }
        }
        Ok(CommandSendResult::Consumed)
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
        self.requests
            .track(request_id, ExpectedResponse::Hello, Instant::now())?;
        if let Err(error) = writer.send_text(frame).await {
            self.requests.cancel_unsent(&request_id);
            return Err(error);
        }
        self.diagnostics.on_request_sent("workspace.hello");

        // Await Hello response.
        loop {
            match self.reader.next().await {
                Some(Ok(InboundMessage::Text(data))) => {
                    self.last_inbound_at = Instant::now();
                    let decoded = decode_server_text_frame(&data)
                        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                    let correlated = self.requests.validate(&decoded)?;
                    match decoded.envelope {
                        fns_protocol::DecodedEnvelope::Success {
                            body: MessageBody::HelloResponse(_),
                            ..
                        } => {
                            self.requests.complete(&correlated)?;
                            self.phase = SessionPhase::AwaitingSubscribe;
                            self.publish_runtime_status();
                            return Ok(());
                        }
                        fns_protocol::DecodedEnvelope::Failure { error, .. } => {
                            self.requests.complete(&correlated)?;
                            return Err(server_failure_error(&error));
                        }
                        _ => {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                    }
                }
                Some(Ok(InboundMessage::Close)) | None => {
                    return Err(TransportError::new(TransportErrorCode::Network, true));
                }
                Some(Ok(InboundMessage::Ping(data))) => {
                    self.last_inbound_at = Instant::now();
                    writer.send_pong(data).await?;
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
        self.requests.reserve_untracked_id(request_id)?;
        writer.send_text(frame).await?;
        self.diagnostics.on_request_sent("workspace.subscribe");

        self.phase = SessionPhase::AwaitingSubscribe;
        self.subscribe_deadline = Some(Instant::now() + self.limits.request_timeout);
        self.publish_runtime_status();
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
        let correlated = if decoded.flow == fns_protocol::WorkspaceFlow::ServerResponse {
            Some(self.requests.validate(&decoded)?)
        } else {
            None
        };
        if correlated.is_some_and(|request_id| self.requests.is_completed(&request_id)) {
            tracing::debug!("exact duplicate workspace response ignored");
            return Ok(());
        }
        if let fns_protocol::DecodedEnvelope::Failure { error, .. } = &decoded.envelope {
            let request_id = correlated
                .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
            let expected = self.requests.complete(&request_id)?;
            if let ExpectedResponse::ConflictResolved(request) = expected {
                let conflict_id = request.conflict_id;
                let commands = self
                    .engine
                    .conflict_resolution_rejected(request.operation_id, error.code)
                    .await?;
                self.enqueue_commands(commands)?;
                match error.code {
                    fns_protocol::WorkspaceV2ErrorCode::BlobRequired => return Ok(()),
                    fns_protocol::WorkspaceV2ErrorCode::ConflictRevisionStale
                    | fns_protocol::WorkspaceV2ErrorCode::ConflictNotFound => {
                        self.refresh_requested =
                            self.engine.list_conflicts().await?.iter().any(|conflict| {
                                conflict.conflict_id == conflict_id
                                    && conflict.status
                                        == fns_sync_core::ConflictStatus::RefreshRequired
                            });
                        return Ok(());
                    }
                    _ => {}
                }
            }
            return Err(server_failure_error(error));
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
                        end_received: false,
                    });
                    self.publish_runtime_status();
                    // Call snapshot_begin on the engine.
                    self.engine_snapshot_begin(begin).await?;
                    self.subscribe_deadline = None;
                }
            }
            WorkspaceAction::WorkspaceSnapshotEntry => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotEntry(entry) = body
                {
                    let cmds = self.engine_pending_from_snapshot_entry(entry).await?;
                    self.enqueue_replayable_download_commands(cmds)?;
                }
            }
            WorkspaceAction::WorkspaceSnapshotEnd => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::SnapshotEnd(end) = body
                {
                    let final_revision = end.final_revision;
                    let duplicate_while_ack_pending = matches!(self.phase, SessionPhase::Online)
                        && self.deferred_until_ack_revision == Some(final_revision);
                    if !matches!(self.phase, SessionPhase::Streaming(_))
                        && !duplicate_while_ack_pending
                    {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    }
                    let cmds = self.engine_snapshot_end(end).await?;
                    if let SessionPhase::Streaming(stream) = &mut self.phase {
                        stream.end_received = true;
                        self.deferred_until_ack_revision = self
                            .engine
                            .active_stream_mode()
                            .await?
                            .map(|_| final_revision);
                        self.refresh_stream_completion().await?;
                    }
                    self.enqueue_commands(cmds)?;
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
                    if self.deferred_until_ack_revision.is_some()
                        || active_stream == Some(fns_sync_core::StreamMode::Snapshot)
                    {
                        tracing::debug!(
                            operation_id = %event.operation_id,
                            "workspace event deferred until stream ack"
                        );
                        self.defer_live_message(DeferredLiveMessage::Event(Box::new(event)))?;
                    } else {
                        let cmds = self.engine_workspace_event(event).await?;
                        self.enqueue_replayable_download_commands(cmds)?;
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
                    let cmds = self.engine_mutation_accepted(msg).await?;
                    self.complete_response(correlated)?;
                    self.enqueue_commands(cmds)?;
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
                    let cmds = self.engine_mutation_rejected(msg).await?;
                    self.complete_response(correlated)?;
                    self.enqueue_commands(cmds)?;
                }
            }
            WorkspaceAction::WorkspaceConflictCreated => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::ConflictCreated(msg) = body
                {
                    tracing::info!(conflict_id = %msg.conflict_id, "workspace conflict created");
                    if self.deferred_until_ack_revision.is_some() {
                        self.defer_live_message(DeferredLiveMessage::ConflictCreated(msg))?;
                    } else {
                        let cmds = self.engine_conflict_created(msg).await?;
                        self.enqueue_commands(cmds)?;
                    }
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
                    if correlated.is_some() {
                        self.engine.conflict_resolution_accepted(msg).await?;
                        self.complete_response(correlated)?;
                    } else if self.deferred_until_ack_revision.is_some()
                        || self.engine.active_stream_mode().await?
                            == Some(fns_sync_core::StreamMode::Snapshot)
                    {
                        self.defer_live_message(DeferredLiveMessage::ConflictResolved(msg))?;
                    } else {
                        let commands = self.engine_conflict_resolved(msg).await?;
                        self.enqueue_replayable_download_commands(commands)?;
                    }
                }
            }
            WorkspaceAction::WorkspaceAck => {
                if let fns_protocol::DecodedEnvelope::Success { body, .. } = decoded.envelope
                    && let MessageBody::Ack(msg) = body
                {
                    let ack_revision = msg.revision;
                    self.engine_ack_confirmed(msg).await?;
                    self.complete_response(correlated)?;
                    if self.deferred_until_ack_revision == Some(ack_revision)
                        && self.engine.active_stream_mode().await?.is_none()
                    {
                        self.deferred_until_ack_revision = None;
                        self.deferred_events_ready = !self.deferred_events.is_empty();
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
                            // Store only the server half. An upload cannot start
                            // until fns-sync-core independently emits the
                            // durable AwaitingBlob intent for the same identity.
                            self.transfers.add_server_need(need)?;
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
                            let pending = self
                                .pending_downloads
                                .iter_mut()
                                .find(|pending| pending.content_hash == resp.content_hash)
                                .ok_or_else(|| {
                                    TransportError::new(TransportErrorCode::Protocol, false)
                                })?;
                            pending.workspace_id = resp.workspace_id;
                            pending.size = resp.size;
                            pending.last_progress_at = Instant::now();
                            self.complete_response(correlated)?;
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
                    if begin.direction == fns_protocol::WorkspaceBlobDirection::Download {
                        if self.transfers.classify_download_begin(&begin)? {
                            return Ok(());
                        }
                        let Some(index) = self.pending_downloads.iter().position(|pending| {
                            pending.content_hash == begin.content_hash && pending.size == begin.size
                        }) else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        let pending = self.pending_downloads.remove(index);
                        self.transfers.reserve_transfer(begin.transfer_id)?;
                        self.engine
                            .begin_blob_import(
                                begin.transfer_id,
                                begin.content_hash.clone(),
                                begin.size,
                            )
                            .await?;
                        self.transfers
                            .add_download(DownloadTransfer::new_with_progress(
                                begin.transfer_id,
                                pending.workspace_id,
                                pending.operation_id,
                                begin.content_hash.clone(),
                                begin.size,
                                pending.started_at,
                                Instant::now(),
                            ));
                        if !self.transfers.matches_download(
                            &begin.transfer_id,
                            &begin.content_hash,
                            begin.size,
                        ) {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                    } else {
                        let expected = self.complete_response(correlated)?;
                        let ExpectedResponse::BlobBeginUpload(expected_begin) = expected else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        self.transfers
                            .mark_upload_begin_accepted(&expected_begin.transfer_id)?;
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
                            self.transfers.get(&end.transfer_id)
                        else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        if upload.workspace_id != end.workspace_id
                            || upload.content_hash != end.content_hash
                            || upload.size != end.size
                            || upload.phase != UploadPhase::AwaitingEndResponse
                        {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                        let operation_id = upload.operation_id;
                        tracing::info!(
                            operation_id = %operation_id,
                            transfer_id = %upload.transfer_id,
                            content_hash = %upload.content_hash,
                            size = upload.size,
                            "workspace blob upload accepted"
                        );
                        // BlobEnd success is the commit point. Only after it
                        // arrives may the mutation referencing this blob be
                        // sent again.
                        let expected = self.complete_response(correlated)?;
                        if !matches!(expected, ExpectedResponse::BlobEndUpload(_)) {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                        self.engine.blob_uploaded(operation_id).await?;
                        self.transfers.remove(&end.transfer_id);
                        let _ = self.transfers.take_pending_retry(&operation_id);
                        return Ok(());
                    }

                    if correlated.is_some() {
                        let expected = self.complete_response(correlated)?;
                        let ExpectedResponse::BlobEndDownload(expected_end) = expected else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        let Some(ActiveTransfer::Download(download)) =
                            self.transfers.get(&expected_end.transfer_id)
                        else {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        };
                        if download.phase != DownloadPhase::AwaitingEndResponse {
                            return Err(TransportError::new(TransportErrorCode::Protocol, false));
                        }
                        let commands = self
                            .engine
                            .commit_blob_import(expected_end.transfer_id)
                            .await?;
                        self.transfers.remove(&expected_end.transfer_id);
                        self.refresh_stream_completion().await?;
                        self.enqueue_replayable_download_commands(commands)?;
                        return Ok(());
                    }

                    if self.transfers.classify_download_end(&end)? {
                        return Ok(());
                    }
                    let Some(ActiveTransfer::Download(download)) =
                        self.transfers.get(&end.transfer_id)
                    else {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    };
                    if download.phase != DownloadPhase::Receiving
                        || download.workspace_id != end.workspace_id
                        || download.content_hash != end.content_hash
                        || download.size != end.size
                        || download.begin.chunk_count != end.chunk_count
                        || download.next_offset != end.size
                        || download.next_chunk_index != end.chunk_count
                    {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    }
                    self.engine.seal_blob_import(end.transfer_id).await?;
                    let Some(ActiveTransfer::Download(download)) =
                        self.transfers.get_mut(&end.transfer_id)
                    else {
                        return Err(TransportError::new(TransportErrorCode::Protocol, false));
                    };
                    download.phase = DownloadPhase::ReadyForEnd;
                    self.transfers.record_download_end(end);
                }
            }
            // Client-only actions should never come from the server.
            WorkspaceAction::WorkspaceHello | WorkspaceAction::WorkspaceSubscribe => {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
            WorkspaceAction::WorkspaceMutation => {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
        }
        Ok(())
    }

    /// Open and validate the immutable cached blob, then send only Begin. The
    /// drain loop cannot stream a chunk until the exact Begin response changes
    /// the active transfer phase.
    async fn upload_blob(
        &mut self,
        writer: &mut SocketWriter,
        workspace_id: fns_protocol::WorkspaceId,
        operation_id: fns_protocol::OperationId,
        content_hash: fns_protocol::WorkspaceContentHash,
        size: u64,
        budget: &mut DrainBudget,
    ) -> Result<bool, TransportError> {
        if self.requests.available_slots() == 0 {
            return Ok(false);
        }
        if size > fns_protocol::MAX_BLOB_BYTES {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let file = tokio::fs::File::from_std(self.engine.open_blob(&content_hash).await?);
        if file
            .metadata()
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Filesystem, false))?
            .len()
            != size
        {
            return Err(TransportError::new(TransportErrorCode::Filesystem, false));
        }

        // Reserve a transfer slot.
        let transfer_id = self.transfers.reserve_slot()?;

        let begin_request_id = fresh_request_id();
        let begin_frame = blob::encode_blob_begin_upload(
            workspace_id,
            transfer_id,
            &content_hash,
            size,
            begin_request_id,
        )?;
        let begin = fns_protocol::WorkspaceBlobBeginMessage {
            workspace_id,
            transfer_id,
            direction: fns_protocol::WorkspaceBlobDirection::Upload,
            content_hash: content_hash.clone(),
            size,
            chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
            chunk_count: blob::chunk_count(size),
        };

        if !budget.can_reserve_bytes(begin_frame.len()) {
            return Ok(false);
        }

        self.transfers.add_upload(UploadTransfer::new(
            transfer_id,
            workspace_id,
            operation_id,
            content_hash.clone(),
            size,
            file,
            Instant::now(),
        ));

        // Send BlobBegin.
        if !self
            .send_tracked_text(
                writer,
                begin_request_id,
                ExpectedResponse::BlobBeginUpload(begin),
                begin_frame,
                budget,
            )
            .await?
        {
            self.transfers.remove(&transfer_id);
            return Ok(false);
        }
        self.transfers.mark_progress(&transfer_id, Instant::now())?;

        Ok(true)
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

    fn defer_live_message(&mut self, message: DeferredLiveMessage) -> Result<(), TransportError> {
        if self.deferred_events.len() >= self.limits.deferred_event_capacity {
            return Err(TransportError::new(TransportErrorCode::ResourceLimit, true));
        }
        self.deferred_events.push_back(message);
        Ok(())
    }

    async fn refresh_stream_completion(&mut self) -> Result<(), TransportError> {
        let final_revision = match &self.phase {
            SessionPhase::Streaming(stream) if stream.end_received => stream.begin.final_revision,
            _ => return Ok(()),
        };
        if self.engine.completed_stream_ack_revision().await? == Some(final_revision) {
            self.phase = SessionPhase::Online;
            self.publish_runtime_status();
            return Ok(());
        }
        let cursor = self.engine.cursor().await?;
        if cursor.last_ack_revision >= final_revision
            && self.engine.active_stream_mode().await?.is_none()
        {
            self.phase = SessionPhase::Online;
            if self.deferred_until_ack_revision == Some(final_revision) {
                self.deferred_until_ack_revision = None;
            }
            self.deferred_events_ready = !self.deferred_events.is_empty();
            self.publish_runtime_status();
        }
        Ok(())
    }
}

fn server_failure_error(error: &fns_protocol::WorkspaceV2Error) -> TransportError {
    match error.code {
        fns_protocol::WorkspaceV2ErrorCode::Unauthenticated => {
            TransportError::new(TransportErrorCode::AuthenticationRejected, false)
        }
        fns_protocol::WorkspaceV2ErrorCode::Forbidden => {
            TransportError::new(TransportErrorCode::Forbidden, false)
        }
        code => TransportError::new(TransportErrorCode::Protocol, code.retryable()),
    }
}

#[cfg(test)]
mod tests {
    use super::SessionLimits;

    #[test]
    fn default_drain_budget_can_send_a_full_blob_chunk() {
        let limits = SessionLimits::default();
        let full_chunk_frame =
            fns_protocol::BLOB_HEADER_LEN + fns_protocol::BLOB_CHUNK_BYTES as usize;

        assert!(limits.drain_byte_budget >= full_chunk_frame);
    }
}
