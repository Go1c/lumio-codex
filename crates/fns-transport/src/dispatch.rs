//! Bounded per-connection request correlation.
//!
//! The tracker owns only ephemeral wire correlation. Durable retry state remains
//! in `fns-sync-core`, so dropping a session never settles or rewrites an
//! operation merely because its response was lost.

use crate::error::{TransportError, TransportErrorCode};
use crate::{MAX_IN_FLIGHT_REQUESTS, MAX_REQUEST_IDS_PER_CONNECTION};

use fns_protocol::{
    ClientId, DecodedEnvelope, DecodedFrame, MessageBody, OperationId, RequestId,
    WorkspaceAckRequest, WorkspaceAction, WorkspaceBlobEndMessage,
    WorkspaceConflictResolvedRequest, WorkspaceContentHash, WorkspaceFlow, WorkspaceId,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tokio::time::Instant;

/// The exact successful response expected for an in-flight request.
///
/// Variants for conflict resolution and download completion are intentionally
/// present before their send paths are implemented, keeping later protocol work
/// inside this same correlation boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExpectedResponse {
    Hello,
    Mutation {
        workspace_id: WorkspaceId,
        client_id: ClientId,
        operation_id: OperationId,
    },
    Ack(WorkspaceAckRequest),
    BlobNeedDownload {
        workspace_id: WorkspaceId,
        operation_id: Option<OperationId>,
        content_hash: WorkspaceContentHash,
        size: u64,
    },
    BlobBeginUpload(fns_protocol::WorkspaceBlobBeginMessage),
    BlobEndUpload(WorkspaceBlobEndMessage),
    BlobEndDownload(WorkspaceBlobEndMessage),
    ConflictResolved(WorkspaceConflictResolvedRequest),
}

impl ExpectedResponse {
    fn failure_action(&self) -> WorkspaceAction {
        match self {
            Self::Hello => WorkspaceAction::WorkspaceHello,
            Self::Mutation { .. } => WorkspaceAction::WorkspaceMutation,
            Self::Ack(_) => WorkspaceAction::WorkspaceAck,
            Self::BlobNeedDownload { .. } => WorkspaceAction::WorkspaceBlobNeed,
            Self::BlobBeginUpload(_) => WorkspaceAction::WorkspaceBlobBegin,
            Self::BlobEndUpload(_) | Self::BlobEndDownload(_) => WorkspaceAction::WorkspaceBlobEnd,
            Self::ConflictResolved(_) => WorkspaceAction::WorkspaceConflictResolved,
        }
    }

    fn matches_success(&self, action: WorkspaceAction, body: &MessageBody) -> bool {
        match (self, action, body) {
            (Self::Hello, WorkspaceAction::WorkspaceHello, MessageBody::HelloResponse(message)) => {
                message.validate().is_ok()
            }
            (
                Self::Mutation {
                    workspace_id,
                    client_id,
                    operation_id,
                },
                WorkspaceAction::WorkspaceMutationAccepted,
                MessageBody::MutationAccepted(message),
            ) => {
                message.validate().is_ok()
                    && message.workspace_id == *workspace_id
                    && message.client_id == *client_id
                    && message.operation_id == *operation_id
            }
            (
                Self::Mutation {
                    workspace_id,
                    client_id,
                    operation_id,
                },
                WorkspaceAction::WorkspaceMutationRejected,
                MessageBody::MutationRejected(message),
            ) => {
                message.validate().is_ok()
                    && message.workspace_id == *workspace_id
                    && message.client_id == *client_id
                    && message.operation_id == *operation_id
            }
            (Self::Ack(expected), WorkspaceAction::WorkspaceAck, MessageBody::Ack(message)) => {
                message.validate().is_ok() && message == expected
            }
            (
                Self::BlobNeedDownload {
                    workspace_id,
                    operation_id,
                    content_hash,
                    size,
                },
                WorkspaceAction::WorkspaceBlobNeed,
                MessageBody::BlobNeedDownloadResponse(message),
            ) => {
                message.validate().is_ok()
                    && message.workspace_id == *workspace_id
                    && message.operation_id.clone().into_option() == *operation_id
                    && message.content_hash == *content_hash
                    && message.size == *size
            }
            (
                Self::BlobBeginUpload(expected),
                WorkspaceAction::WorkspaceBlobBegin,
                MessageBody::BlobBegin(message),
            ) => message.validate().is_ok() && message == expected,
            (
                Self::BlobEndUpload(expected) | Self::BlobEndDownload(expected),
                WorkspaceAction::WorkspaceBlobEnd,
                MessageBody::BlobEnd(message),
            ) => message.validate().is_ok() && message == expected,
            (
                Self::ConflictResolved(expected),
                WorkspaceAction::WorkspaceConflictResolved,
                MessageBody::ConflictResolved(message),
            ) => {
                message.validate().is_ok()
                    && message.workspace_id == expected.workspace_id
                    && message.resolved_by_client_id == expected.client_id
                    && message.operation_id == expected.operation_id
                    && message.conflict_id == expected.conflict_id
                    && message.conflict_revision == expected.conflict_revision
                    && message.choice == expected.choice
                    && message.path_state.path == expected.path
                    && message.path_state.content_hash == expected.content_hash
                    && message.path_state.metadata == expected.metadata
                    && message.path_state.tombstone
                        == (expected.choice == fns_protocol::WorkspaceConflictChoice::Delete)
            }
            _ => false,
        }
    }

    pub fn mutation_operation_id(&self) -> Option<OperationId> {
        match self {
            Self::Mutation { operation_id, .. } => Some(*operation_id),
            _ => None,
        }
    }

    pub fn ack_revision(&self) -> Option<fns_protocol::WorkspaceRevision> {
        match self {
            Self::Ack(message) => Some(message.revision),
            _ => None,
        }
    }

    pub fn conflict_operation_id(&self) -> Option<OperationId> {
        match self {
            Self::ConflictResolved(request) => Some(request.operation_id),
            _ => None,
        }
    }

    pub fn download_identity(&self) -> Option<(&WorkspaceContentHash, u64, Option<OperationId>)> {
        match self {
            Self::BlobNeedDownload {
                operation_id,
                content_hash,
                size,
                ..
            } => Some((content_hash, *size, *operation_id)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct InFlight {
    expected: ExpectedResponse,
    sent_at: Instant,
}

#[derive(Clone, Debug)]
struct CompletedResponse {
    frame: DecodedFrame,
}

const MAX_COMPLETED_REQUEST_RECEIPTS: usize = 256;

/// Bounded request table for exactly one WebSocket connection.
pub struct RequestTracker {
    entries: HashMap<RequestId, InFlight>,
    validated: HashMap<RequestId, DecodedFrame>,
    completed: HashMap<RequestId, CompletedResponse>,
    completed_order: VecDeque<RequestId>,
    per_connection_ids: HashSet<RequestId>,
}

impl RequestTracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            validated: HashMap::new(),
            completed: HashMap::new(),
            completed_order: VecDeque::new(),
            per_connection_ids: HashSet::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, request_id: &RequestId) -> bool {
        self.entries.contains_key(request_id)
    }

    pub fn is_completed(&self, request_id: &RequestId) -> bool {
        self.completed.contains_key(request_id)
    }

    pub fn available_slots(&self) -> usize {
        MAX_IN_FLIGHT_REQUESTS.saturating_sub(self.entries.len())
    }

    pub fn needs_reconnect_for_id_limit(&self) -> bool {
        self.per_connection_ids.len() >= MAX_REQUEST_IDS_PER_CONNECTION
    }

    pub fn track(
        &mut self,
        request_id: RequestId,
        expected: ExpectedResponse,
        sent_at: Instant,
    ) -> Result<(), TransportError> {
        if self.entries.len() >= MAX_IN_FLIGHT_REQUESTS
            || self.per_connection_ids.len() >= MAX_REQUEST_IDS_PER_CONNECTION
        {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }
        if self.per_connection_ids.contains(&request_id) {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        self.per_connection_ids.insert(request_id);
        self.entries
            .insert(request_id, InFlight { expected, sent_at });
        Ok(())
    }

    /// Reserve an ID for a request that has no protocol response, such as
    /// Subscribe. Its phase-specific acknowledgement is tracked by Session.
    pub fn reserve_untracked_id(&mut self, request_id: RequestId) -> Result<(), TransportError> {
        if self.per_connection_ids.len() >= MAX_REQUEST_IDS_PER_CONNECTION {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }
        if !self.per_connection_ids.insert(request_id) {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        Ok(())
    }

    /// Validate correlation and semantic identity without consuming the entry.
    pub fn validate(&mut self, frame: &DecodedFrame) -> Result<RequestId, TransportError> {
        if frame.flow != WorkspaceFlow::ServerResponse {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let request_id = match &frame.envelope {
            DecodedEnvelope::Success {
                request_id: Some(request_id),
                ..
            }
            | DecodedEnvelope::Failure {
                request_id: Some(request_id),
                ..
            } => *request_id,
            DecodedEnvelope::Success {
                request_id: None, ..
            }
            | DecodedEnvelope::Failure {
                request_id: None, ..
            }
            | DecodedEnvelope::Request { .. } => {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
        };
        if let Some(completed) = self.completed.get(&request_id) {
            if completed.frame == *frame {
                return Ok(request_id);
            }
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        let in_flight = self
            .entries
            .get(&request_id)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        let matches = match &frame.envelope {
            DecodedEnvelope::Success { body, .. } => {
                in_flight.expected.matches_success(frame.action, body)
            }
            DecodedEnvelope::Failure { .. } => frame.action == in_flight.expected.failure_action(),
            DecodedEnvelope::Request { .. } => false,
        };
        if !matches {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        if let Some(previous) = self.validated.get(&request_id)
            && previous != frame
        {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        self.validated.insert(request_id, frame.clone());
        Ok(request_id)
    }

    pub fn complete(&mut self, request_id: &RequestId) -> Result<ExpectedResponse, TransportError> {
        let expected = self
            .entries
            .remove(request_id)
            .map(|entry| entry.expected)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        let frame = self
            .validated
            .remove(request_id)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        self.completed
            .insert(*request_id, CompletedResponse { frame });
        self.completed_order.push_back(*request_id);
        while self.completed_order.len() > MAX_COMPLETED_REQUEST_RECEIPTS {
            if let Some(expired) = self.completed_order.pop_front() {
                self.completed.remove(&expired);
            }
        }
        Ok(expected)
    }

    /// Remove a request whose socket send failed. Its ID remains reserved for
    /// the lifetime of this connection and can never be reused.
    pub fn cancel_unsent(&mut self, request_id: &RequestId) {
        self.entries.remove(request_id);
        self.validated.remove(request_id);
    }

    pub fn expired(&self, now: Instant, timeout: Duration) -> Option<RequestId> {
        self.entries
            .iter()
            .filter(|(_, entry)| now.saturating_duration_since(entry.sent_at) >= timeout)
            .min_by_key(|(_, entry)| entry.sent_at)
            .map(|(request_id, _)| *request_id)
    }

    pub fn next_deadline(&self, timeout: Duration) -> Option<Instant> {
        self.entries
            .values()
            .map(|entry| entry.sent_at + timeout)
            .min()
    }

    pub fn has_mutation(&self, operation_id: OperationId) -> bool {
        self.entries
            .values()
            .any(|entry| entry.expected.mutation_operation_id() == Some(operation_id))
    }

    pub fn has_ack(&self, revision: fns_protocol::WorkspaceRevision) -> bool {
        self.entries
            .values()
            .any(|entry| entry.expected.ack_revision() == Some(revision))
    }

    pub fn has_ack_in_flight(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.expected.ack_revision().is_some())
    }

    pub fn has_conflict_resolution(&self, operation_id: OperationId) -> bool {
        self.entries
            .values()
            .any(|entry| entry.expected.conflict_operation_id() == Some(operation_id))
    }

    pub fn has_download(&self, content_hash: &WorkspaceContentHash, size: u64) -> bool {
        self.entries.values().any(|entry| {
            entry
                .expected
                .download_identity()
                .is_some_and(|(hash, expected_size, _)| {
                    hash == content_hash && expected_size == size
                })
        })
    }
}

impl Default for RequestTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Compatibility name for the original, unintegrated dispatch skeleton.
pub type DispatchTable = RequestTracker;
