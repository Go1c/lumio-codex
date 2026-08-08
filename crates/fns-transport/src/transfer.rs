//! Active transfer table: pairs durable upload intent with server BlobNeed,
//! tracks active upload/download transfers, and enforces transfer limits.

#![allow(dead_code)] // Full wire integration in later sub-tasks.

use crate::MAX_ACTIVE_TRANSFERS;
use crate::error::{TransportError, TransportErrorCode};

use fns_protocol::{OperationId, TransferId, WorkspaceContentHash, WorkspaceId};
use fns_sync_core::SyncCommand;

use std::collections::HashMap;

/// Durable upload intent from the engine, waiting for server BlobNeed push to match.
#[derive(Clone, Debug)]
pub struct UploadIntent {
    pub workspace_id: WorkspaceId,
    pub operation_id: OperationId,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
}

/// A pending upload waiting for both engine intent and server need.
#[derive(Debug)]
pub struct PendingUpload {
    pub intent: Option<UploadIntent>,
    pub need: Option<fns_protocol::WorkspaceBlobNeedUploadPush>,
    pub retry: SyncCommand,
}

/// Active transfer state.
#[derive(Debug)]
pub enum ActiveTransfer {
    Upload(UploadTransfer),
    Download(DownloadTransfer),
}

#[derive(Debug)]
pub struct UploadTransfer {
    pub transfer_id: TransferId,
    pub workspace_id: WorkspaceId,
    pub operation_id: OperationId,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
}

#[derive(Debug)]
pub struct DownloadTransfer {
    pub transfer_id: TransferId,
    pub workspace_id: WorkspaceId,
    pub operation_id: Option<OperationId>,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
}

/// Manages pending uploads and active transfers for one connection.
pub struct TransferTable {
    pending_uploads: HashMap<OperationId, PendingUpload>,
    active: HashMap<TransferId, ActiveTransfer>,
    max_active: usize,
    per_connection_ids: Vec<TransferId>,
}

impl TransferTable {
    pub fn new(max_active: usize) -> Self {
        Self {
            pending_uploads: HashMap::new(),
            active: HashMap::new(),
            max_active: max_active.min(MAX_ACTIVE_TRANSFERS),
            per_connection_ids: Vec::new(),
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Register a durable upload intent from the engine.
    pub fn add_upload_intent(&mut self, intent: UploadIntent, retry: SyncCommand) {
        let op_id = intent.operation_id;
        self.pending_uploads
            .entry(op_id)
            .or_insert_with(|| PendingUpload {
                intent: None,
                need: None,
                retry,
            })
            .intent = Some(intent);
    }

    /// Check if there is a matching server need for the given operation/hash/size.
    pub fn has_matching_upload(
        &self,
        operation_id: &OperationId,
        content_hash: &WorkspaceContentHash,
        size: u64,
    ) -> bool {
        self.pending_uploads
            .get(operation_id)
            .and_then(|p| p.need.as_ref())
            .is_some_and(|need| need.content_hash == *content_hash && need.size == size)
    }

    /// Register a server BlobNeed upload push.
    /// Returns true if both halves now match and a transfer can begin.
    pub fn add_server_need(
        &mut self,
        need: fns_protocol::WorkspaceBlobNeedUploadPush,
    ) -> Result<bool, TransportError> {
        let op_id = need.operation_id;
        let entry = self
            .pending_uploads
            .entry(op_id)
            .or_insert_with(|| PendingUpload {
                intent: None,
                need: None,
                retry: SyncCommand::UploadBlob {
                    workspace_id: need.workspace_id,
                    operation_id: need.operation_id,
                    content_hash: need.content_hash.clone(),
                    size: need.size,
                },
            });
        entry.need = Some(need);

        // Check if both halves match.
        if let (Some(intent), Some(server_need)) = (&entry.intent, &entry.need)
            && intent.workspace_id == server_need.workspace_id
            && intent.operation_id == server_need.operation_id
            && intent.content_hash == server_need.content_hash
            && intent.size == server_need.size
        {
            return Ok(true);
        }
        Ok(false)
    }

    /// Try to start a new active transfer. Returns Err if at capacity.
    pub fn reserve_slot(&mut self) -> Result<TransferId, TransportError> {
        if self.active.len() >= self.max_active {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }
        if self.per_connection_ids.len() >= crate::MAX_TRANSFER_IDS_PER_CONNECTION {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }

        let uuid = uuid::Uuid::new_v4();
        let transfer_id = TransferId::parse(&uuid.to_string()).expect("valid uuid string");
        self.per_connection_ids.push(transfer_id);
        Ok(transfer_id)
    }

    /// Insert an active upload transfer.
    pub fn add_upload(&mut self, transfer: UploadTransfer) {
        self.active
            .insert(transfer.transfer_id, ActiveTransfer::Upload(transfer));
    }

    /// Insert an active download transfer.
    pub fn add_download(&mut self, transfer: DownloadTransfer) {
        self.active
            .insert(transfer.transfer_id, ActiveTransfer::Download(transfer));
    }

    /// Remove an active transfer by ID.
    pub fn remove(&mut self, transfer_id: &TransferId) -> Option<ActiveTransfer> {
        self.active.remove(transfer_id)
    }

    /// Take a pending upload's retry command (after upload completes or on reconnect).
    pub fn take_pending_retry(&mut self, op_id: &OperationId) -> Option<SyncCommand> {
        self.pending_uploads.remove(op_id).map(|p| p.retry)
    }

    /// Clear all state for a fresh connection.
    pub fn reset_connection(&mut self) {
        self.active.clear();
        self.per_connection_ids.clear();
    }
}
