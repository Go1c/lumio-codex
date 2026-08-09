//! Active transfer table: pairs durable upload intent with server BlobNeed,
//! tracks active upload/download transfers, and enforces transfer limits.

#![allow(dead_code)] // Full wire integration in later sub-tasks.

use crate::MAX_ACTIVE_TRANSFERS;
use crate::error::{TransportError, TransportErrorCode};

use fns_protocol::{OperationId, TransferId, WorkspaceContentHash, WorkspaceId};
use fns_sync_core::SyncCommand;

use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::time::Instant;

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
    Upload(Box<UploadTransfer>),
    Download(Box<DownloadTransfer>),
}

#[derive(Debug)]
pub struct UploadTransfer {
    pub transfer_id: TransferId,
    pub workspace_id: WorkspaceId,
    pub operation_id: OperationId,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
    pub file: tokio::fs::File,
    pub phase: UploadPhase,
    pub next_chunk_index: u64,
    pub next_offset: u64,
    pub hasher: blake3::Hasher,
    started_at: Instant,
    last_progress_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadPhase {
    AwaitingBeginResponse,
    Streaming,
    ReadyForEnd,
    AwaitingEndResponse,
}

impl UploadTransfer {
    pub fn new(
        transfer_id: TransferId,
        workspace_id: WorkspaceId,
        operation_id: OperationId,
        content_hash: WorkspaceContentHash,
        size: u64,
        file: tokio::fs::File,
        now: Instant,
    ) -> Self {
        Self {
            transfer_id,
            workspace_id,
            operation_id,
            content_hash,
            size,
            file,
            phase: UploadPhase::AwaitingBeginResponse,
            next_chunk_index: 0,
            next_offset: 0,
            hasher: blake3::Hasher::new(),
            started_at: now,
            last_progress_at: now,
        }
    }
}

#[derive(Debug)]
pub struct DownloadTransfer {
    pub transfer_id: TransferId,
    pub workspace_id: WorkspaceId,
    pub operation_id: Option<OperationId>,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
    pub begin: fns_protocol::WorkspaceBlobBeginMessage,
    pub phase: DownloadPhase,
    pub next_chunk_index: u64,
    pub next_offset: u64,
    started_at: Instant,
    last_progress_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadPhase {
    Receiving,
    ReadyForEnd,
    AwaitingEndResponse,
}

impl DownloadTransfer {
    pub fn new(
        transfer_id: TransferId,
        workspace_id: WorkspaceId,
        operation_id: Option<OperationId>,
        content_hash: WorkspaceContentHash,
        size: u64,
        now: Instant,
    ) -> Self {
        Self::new_with_progress(
            transfer_id,
            workspace_id,
            operation_id,
            content_hash,
            size,
            now,
            now,
        )
    }

    pub fn new_with_progress(
        transfer_id: TransferId,
        workspace_id: WorkspaceId,
        operation_id: Option<OperationId>,
        content_hash: WorkspaceContentHash,
        size: u64,
        started_at: Instant,
        last_progress_at: Instant,
    ) -> Self {
        let begin = fns_protocol::WorkspaceBlobBeginMessage {
            workspace_id,
            transfer_id,
            direction: fns_protocol::WorkspaceBlobDirection::Download,
            content_hash: content_hash.clone(),
            size,
            chunk_size: fns_protocol::BLOB_CHUNK_BYTES,
            chunk_count: crate::blob::chunk_count(size),
        };
        Self {
            transfer_id,
            workspace_id,
            operation_id,
            content_hash,
            size,
            begin,
            phase: DownloadPhase::Receiving,
            next_chunk_index: 0,
            next_offset: 0,
            started_at,
            last_progress_at,
        }
    }
}

/// Manages pending uploads and active transfers for one connection.
pub struct TransferTable {
    pending_uploads: HashMap<OperationId, PendingUpload>,
    active: HashMap<TransferId, ActiveTransfer>,
    max_active: usize,
    per_connection_ids: Vec<TransferId>,
    download_begin_receipts: HashMap<TransferId, fns_protocol::WorkspaceBlobBeginMessage>,
    download_end_receipts: HashMap<TransferId, fns_protocol::WorkspaceBlobEndMessage>,
    receipt_order: VecDeque<(TransferId, bool)>,
}

const MAX_COMPLETED_TRANSFER_RECEIPTS: usize = 256;

impl TransferTable {
    pub fn new(max_active: usize) -> Self {
        Self {
            pending_uploads: HashMap::new(),
            active: HashMap::new(),
            max_active: max_active.min(MAX_ACTIVE_TRANSFERS),
            per_connection_ids: Vec::new(),
            download_begin_receipts: HashMap::new(),
            download_end_receipts: HashMap::new(),
            receipt_order: VecDeque::new(),
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

    /// Whether an upload for this operation is already on the wire.
    pub fn has_active_upload(&self, operation_id: &OperationId) -> bool {
        self.active.values().any(|transfer| {
            matches!(
                transfer,
                ActiveTransfer::Upload(upload) if upload.operation_id == *operation_id
            )
        })
    }

    /// Verify that a server download frame belongs to the reserved transfer.
    pub fn matches_download(
        &self,
        transfer_id: &TransferId,
        content_hash: &WorkspaceContentHash,
        size: u64,
    ) -> bool {
        matches!(
            self.active.get(transfer_id),
            Some(ActiveTransfer::Download(download))
                if download.content_hash == *content_hash && download.size == size
        )
    }

    /// Whether a download with this content is already active.
    pub fn has_active_download(&self, content_hash: &WorkspaceContentHash, size: u64) -> bool {
        self.active.values().any(|transfer| {
            matches!(
                transfer,
                ActiveTransfer::Download(download)
                    if download.content_hash == *content_hash && download.size == size
            )
        })
    }

    /// Register a server BlobNeed upload push.
    /// Returns true if both halves now match and a transfer can begin.
    pub fn add_server_need(
        &mut self,
        need: fns_protocol::WorkspaceBlobNeedUploadPush,
    ) -> Result<bool, TransportError> {
        let op_id = need.operation_id;
        if let Some(existing) = self.pending_uploads.get(&op_id) {
            if let Some(previous) = &existing.need
                && (previous.workspace_id != need.workspace_id
                    || previous.content_hash != need.content_hash
                    || previous.size != need.size)
            {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
            if let Some(intent) = &existing.intent
                && (intent.workspace_id != need.workspace_id
                    || intent.content_hash != need.content_hash
                    || intent.size != need.size)
            {
                return Err(TransportError::new(TransportErrorCode::Protocol, false));
            }
        }
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
        let uuid = uuid::Uuid::new_v4();
        let transfer_id = TransferId::parse(&uuid.to_string()).expect("valid uuid string");
        self.reserve_transfer(transfer_id)?;
        Ok(transfer_id)
    }

    /// Reserve a server-assigned transfer ID after the Begin push arrives.
    pub fn reserve_transfer(&mut self, transfer_id: TransferId) -> Result<(), TransportError> {
        if self.active.len() >= self.max_active {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }
        if self.per_connection_ids.len() >= crate::MAX_TRANSFER_IDS_PER_CONNECTION
            || self.per_connection_ids.contains(&transfer_id)
        {
            return Err(TransportError::new(
                TransportErrorCode::ResourceLimit,
                false,
            ));
        }
        self.per_connection_ids.push(transfer_id);
        Ok(())
    }

    /// Insert an active upload transfer.
    pub fn add_upload(&mut self, transfer: UploadTransfer) {
        self.active.insert(
            transfer.transfer_id,
            ActiveTransfer::Upload(Box::new(transfer)),
        );
    }

    /// Insert an active download transfer.
    pub fn add_download(&mut self, transfer: DownloadTransfer) {
        self.record_download_begin(transfer.begin.clone());
        self.active.insert(
            transfer.transfer_id,
            ActiveTransfer::Download(Box::new(transfer)),
        );
    }

    /// Remove an active transfer by ID.
    pub fn remove(&mut self, transfer_id: &TransferId) -> Option<ActiveTransfer> {
        self.active.remove(transfer_id)
    }

    pub fn get(&self, transfer_id: &TransferId) -> Option<&ActiveTransfer> {
        self.active.get(transfer_id)
    }

    pub fn get_mut(&mut self, transfer_id: &TransferId) -> Option<&mut ActiveTransfer> {
        self.active.get_mut(transfer_id)
    }

    pub fn take_streaming_upload(&mut self) -> Option<UploadTransfer> {
        let transfer_id = self.active.iter().find_map(|(transfer_id, transfer)| {
            matches!(
                transfer,
                ActiveTransfer::Upload(upload) if upload.phase == UploadPhase::Streaming
            )
            .then_some(*transfer_id)
        })?;
        match self.active.remove(&transfer_id) {
            Some(ActiveTransfer::Upload(upload)) => Some(*upload),
            _ => None,
        }
    }

    pub fn take_upload_ready_for_end(&mut self) -> Option<UploadTransfer> {
        let transfer_id = self.active.iter().find_map(|(transfer_id, transfer)| {
            matches!(
                transfer,
                ActiveTransfer::Upload(upload) if upload.phase == UploadPhase::ReadyForEnd
            )
            .then_some(*transfer_id)
        })?;
        match self.active.remove(&transfer_id) {
            Some(ActiveTransfer::Upload(upload)) => Some(*upload),
            _ => None,
        }
    }

    pub fn download_ready_for_end(&self) -> Option<TransferId> {
        self.active.iter().find_map(|(transfer_id, transfer)| {
            matches!(
                transfer,
                ActiveTransfer::Download(download) if download.phase == DownloadPhase::ReadyForEnd
            )
            .then_some(*transfer_id)
        })
    }

    pub fn put_upload(&mut self, upload: UploadTransfer) {
        self.active
            .insert(upload.transfer_id, ActiveTransfer::Upload(Box::new(upload)));
    }

    pub fn mark_upload_begin_accepted(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<(), TransportError> {
        let Some(ActiveTransfer::Upload(upload)) = self.active.get_mut(transfer_id) else {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        };
        if upload.phase != UploadPhase::AwaitingBeginResponse {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        upload.phase = UploadPhase::Streaming;
        upload.last_progress_at = Instant::now();
        Ok(())
    }

    pub fn classify_download_begin(
        &self,
        begin: &fns_protocol::WorkspaceBlobBeginMessage,
    ) -> Result<bool, TransportError> {
        if let Some(transfer) = self.active.get(&begin.transfer_id) {
            return match transfer {
                ActiveTransfer::Download(download) if download.begin == *begin => Ok(true),
                _ => Err(TransportError::new(TransportErrorCode::Protocol, false)),
            };
        }
        if let Some(receipt) = self.download_begin_receipts.get(&begin.transfer_id) {
            return if receipt == begin {
                Ok(true)
            } else {
                Err(TransportError::new(TransportErrorCode::Protocol, false))
            };
        }
        if self.per_connection_ids.contains(&begin.transfer_id) {
            return Err(TransportError::new(TransportErrorCode::Protocol, false));
        }
        Ok(false)
    }

    pub fn classify_download_end(
        &self,
        end: &fns_protocol::WorkspaceBlobEndMessage,
    ) -> Result<bool, TransportError> {
        if let Some(receipt) = self.download_end_receipts.get(&end.transfer_id) {
            return if receipt == end {
                Ok(true)
            } else {
                Err(TransportError::new(TransportErrorCode::Protocol, false))
            };
        }
        Ok(false)
    }

    pub fn record_download_end(&mut self, end: fns_protocol::WorkspaceBlobEndMessage) {
        let transfer_id = end.transfer_id;
        self.download_end_receipts.insert(transfer_id, end);
        self.receipt_order.push_back((transfer_id, false));
        self.trim_receipts();
    }

    fn record_download_begin(&mut self, begin: fns_protocol::WorkspaceBlobBeginMessage) {
        let transfer_id = begin.transfer_id;
        self.download_begin_receipts.insert(transfer_id, begin);
        self.receipt_order.push_back((transfer_id, true));
        self.trim_receipts();
    }

    fn trim_receipts(&mut self) {
        while self.receipt_order.len() > MAX_COMPLETED_TRANSFER_RECEIPTS {
            let Some((transfer_id, begin)) = self.receipt_order.pop_front() else {
                break;
            };
            if begin {
                self.download_begin_receipts.remove(&transfer_id);
            } else {
                self.download_end_receipts.remove(&transfer_id);
            }
        }
    }

    pub fn mark_progress(
        &mut self,
        transfer_id: &TransferId,
        now: Instant,
    ) -> Result<(), TransportError> {
        let transfer = self
            .active
            .get_mut(transfer_id)
            .ok_or_else(|| TransportError::new(TransportErrorCode::Protocol, false))?;
        match transfer {
            ActiveTransfer::Upload(upload) => upload.last_progress_at = now,
            ActiveTransfer::Download(download) => download.last_progress_at = now,
        }
        Ok(())
    }

    pub fn expired(
        &self,
        now: Instant,
        idle_timeout: Duration,
        max_lifetime: Duration,
    ) -> Option<TransferId> {
        self.active
            .iter()
            .filter(|(_, transfer)| transfer_expired(transfer, now, idle_timeout, max_lifetime))
            .min_by_key(|(_, transfer)| transfer_started_at(transfer))
            .map(|(transfer_id, _)| *transfer_id)
    }

    pub fn next_deadline(&self, idle_timeout: Duration, max_lifetime: Duration) -> Option<Instant> {
        self.active
            .values()
            .map(|transfer| {
                let started = transfer_started_at(transfer);
                let progress = transfer_last_progress_at(transfer);
                (started + max_lifetime).min(progress + idle_timeout)
            })
            .min()
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

fn transfer_started_at(transfer: &ActiveTransfer) -> Instant {
    match transfer {
        ActiveTransfer::Upload(upload) => upload.started_at,
        ActiveTransfer::Download(download) => download.started_at,
    }
}

fn transfer_last_progress_at(transfer: &ActiveTransfer) -> Instant {
    match transfer {
        ActiveTransfer::Upload(upload) => upload.last_progress_at,
        ActiveTransfer::Download(download) => download.last_progress_at,
    }
}

fn transfer_expired(
    transfer: &ActiveTransfer,
    now: Instant,
    idle_timeout: Duration,
    max_lifetime: Duration,
) -> bool {
    now.saturating_duration_since(transfer_started_at(transfer)) >= max_lifetime
        || now.saturating_duration_since(transfer_last_progress_at(transfer)) >= idle_timeout
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_id() -> WorkspaceId {
        WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
    }

    fn operation_id() -> OperationId {
        OperationId::parse("10000000-0000-4000-8000-000000000002").unwrap()
    }

    fn content_hash() -> WorkspaceContentHash {
        WorkspaceContentHash::parse(
            "blake3:abababababababababababababababababababababababababababababababab",
        )
        .unwrap()
    }

    fn upload_command() -> SyncCommand {
        SyncCommand::UploadBlob {
            workspace_id: workspace_id(),
            operation_id: operation_id(),
            content_hash: content_hash(),
            size: 8,
        }
    }

    fn upload_need() -> fns_protocol::WorkspaceBlobNeedUploadPush {
        fns_protocol::WorkspaceBlobNeedUploadPush {
            workspace_id: workspace_id(),
            direction: fns_protocol::WorkspaceBlobDirection::Upload,
            operation_id: operation_id(),
            content_hash: content_hash(),
            size: 8,
        }
    }

    #[test]
    fn engine_intent_and_server_need_pair_in_either_order() {
        let intent = UploadIntent {
            workspace_id: workspace_id(),
            operation_id: operation_id(),
            content_hash: content_hash(),
            size: 8,
        };

        let mut engine_first = TransferTable::new(1);
        engine_first.add_upload_intent(intent.clone(), upload_command());
        assert!(!engine_first.has_matching_upload(&operation_id(), &content_hash(), 8));
        assert!(engine_first.add_server_need(upload_need()).unwrap());

        let mut need_first = TransferTable::new(1);
        assert!(!need_first.add_server_need(upload_need()).unwrap());
        need_first.add_upload_intent(intent, upload_command());
        assert!(need_first.has_matching_upload(&operation_id(), &content_hash(), 8));
    }

    #[test]
    fn conflicting_need_is_rejected_instead_of_looping() {
        let mut table = TransferTable::new(1);
        table.add_server_need(upload_need()).unwrap();
        let mut conflicting = upload_need();
        conflicting.size = 9;
        assert!(table.add_server_need(conflicting).is_err());
    }
}
