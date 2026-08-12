//! Dispatch: bounded request correlation and exact SyncCommand encoding.
//!
//! Maintains a bounded in-flight table keyed by RequestId, stores the originating
//! SyncCommand (not its wire frame) so retry re-encodes with a fresh request ID
//! and the unchanged body. No serde_json in production dispatch.

#![allow(dead_code)] // Transfer-related variants used by later sub-tasks.

use crate::MAX_IN_FLIGHT_REQUESTS;
use crate::error::{TransportError, TransportErrorCode};

use fns_protocol::{MessageBody, RequestId, RequiredNullable, WorkspaceAction, encode_request};
use fns_sync_core::SyncCommand;

use std::collections::HashMap;
use std::time::Instant;

/// What response we expect for a given in-flight request.
#[derive(Clone, Debug)]
enum ExpectedResponse {
    Mutation {
        command: SyncCommand,
    },
    Ack {
        body: fns_protocol::WorkspaceAckRequest,
    },
    BlobNeedDownload {
        command: SyncCommand,
    },
    ConflictResolution {
        body: fns_protocol::WorkspaceConflictResolvedRequest,
    },
}

/// A tracked in-flight request.
struct InFlight {
    expected: ExpectedResponse,
    sent_at: Instant,
}

/// Bounded in-flight request table.
pub struct DispatchTable {
    entries: HashMap<RequestId, InFlight>,
    per_connection_ids: Vec<RequestId>,
}

impl DispatchTable {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            per_connection_ids: Vec::new(),
        }
    }

    /// Current number of in-flight requests.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Available request slots.
    pub fn available_slots(&self) -> usize {
        MAX_IN_FLIGHT_REQUESTS.saturating_sub(self.entries.len())
    }

    /// Whether we need to reconnect due to per-connection ID limit.
    pub fn needs_reconnect_for_id_limit(&self) -> bool {
        self.per_connection_ids.len() >= crate::MAX_REQUEST_IDS_PER_CONNECTION
    }

    /// Record a per-connection request ID (non-evicting).
    pub fn record_connection_id(&mut self, id: RequestId) {
        self.per_connection_ids.push(id);
    }

    /// Encode a SyncCommand to a wire frame and register it in the in-flight table.
    /// Returns the encoded frame bytes and the request ID used.
    pub fn encode_and_track(
        &mut self,
        command: SyncCommand,
    ) -> Result<(RequestId, Vec<u8>), TransportError> {
        if self.entries.len() >= MAX_IN_FLIGHT_REQUESTS {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }

        let (request_id, frame, expected) = match command {
            SyncCommand::Mutation(body) => {
                let id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceMutation,
                    id,
                    MessageBody::Mutation(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                (
                    id,
                    frame,
                    ExpectedResponse::Mutation {
                        command: SyncCommand::Mutation(body),
                    },
                )
            }
            SyncCommand::SendAck(body) => {
                let id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceAck,
                    id,
                    MessageBody::Ack(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                (id, frame, ExpectedResponse::Ack { body })
            }
            SyncCommand::DownloadBlob {
                workspace_id,
                operation_id,
                content_hash,
                size,
            } => {
                let id = fresh_request_id();
                let need_body = fns_protocol::WorkspaceBlobNeedDownloadRequest {
                    workspace_id,
                    direction: fns_protocol::WorkspaceBlobDirection::Download,
                    operation_id: operation_id
                        .map(RequiredNullable::Value)
                        .unwrap_or(RequiredNullable::Null),
                    content_hash: content_hash.clone(),
                    size: RequiredNullable::Value(size),
                };
                let frame = encode_request(
                    WorkspaceAction::WorkspaceBlobNeed,
                    id,
                    MessageBody::BlobNeedDownloadRequest(need_body),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                (
                    id,
                    frame,
                    ExpectedResponse::BlobNeedDownload {
                        command: SyncCommand::DownloadBlob {
                            workspace_id,
                            operation_id,
                            content_hash,
                            size,
                        },
                    },
                )
            }
            SyncCommand::ResolveConflict(body) => {
                let id = fresh_request_id();
                let frame = encode_request(
                    WorkspaceAction::WorkspaceConflictResolved,
                    id,
                    MessageBody::ConflictResolvedRequest(body.clone()),
                )
                .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
                (id, frame, ExpectedResponse::ConflictResolution { body })
            }
            SyncCommand::UploadBlob { .. } => {
                // UploadBlob sends nothing by itself — it's paired with server BlobNeed.
                // Transfer module (Task 5) handles this.
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
        };

        self.record_connection_id(request_id);
        self.entries.insert(
            request_id,
            InFlight {
                expected,
                sent_at: Instant::now(),
            },
        );

        Ok((request_id, frame))
    }

    /// Take an in-flight entry by request ID (for correlation on response).
    fn take(&mut self, request_id: &RequestId) -> Option<ExpectedResponse> {
        self.entries.remove(request_id).map(|entry| entry.expected)
    }

    /// Drain all in-flight entries and return their originating commands for retry.
    pub fn drain_for_retry(&mut self) -> Vec<SyncCommand> {
        self.entries
            .drain()
            .map(|(_, entry)| match entry.expected {
                ExpectedResponse::Mutation { command } => command,
                ExpectedResponse::Ack { body } => SyncCommand::SendAck(body),
                ExpectedResponse::BlobNeedDownload { command } => command,
                ExpectedResponse::ConflictResolution { body } => SyncCommand::ResolveConflict(body),
            })
            .collect()
    }

    /// Clear per-connection tracking for a fresh connection.
    pub fn reset_connection(&mut self) {
        self.per_connection_ids.clear();
        self.entries.clear();
    }
}

impl Default for DispatchTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a fresh RequestId from a random UUID v4.
fn fresh_request_id() -> RequestId {
    let uuid = uuid::Uuid::new_v4();
    RequestId::parse(&uuid.to_string()).expect("valid uuid string")
}
