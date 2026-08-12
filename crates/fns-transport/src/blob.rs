//! Blob upload/download: Begin/chunks/End framing, staging files, BLAKE3 verification,
//! and restart-from-zero semantics.

#![allow(dead_code)] // Wire integration in later sub-tasks.

use crate::error::{TransportError, TransportErrorCode};

use fns_protocol::{
    BLOB_CHUNK_BYTES, MessageBody, TransferId, WorkspaceAction, WorkspaceBlobBeginMessage,
    WorkspaceBlobDirection, WorkspaceBlobEndMessage, WorkspaceContentHash, encode_request,
};

/// Compute the number of chunks for a given blob size.
pub fn chunk_count(size: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    (size - 1) / BLOB_CHUNK_BYTES as u64 + 1
}

/// Encode a WorkspaceBlobBegin message for an upload.
pub fn encode_blob_begin_upload(
    workspace_id: fns_protocol::WorkspaceId,
    transfer_id: TransferId,
    content_hash: &WorkspaceContentHash,
    size: u64,
    request_id: fns_protocol::RequestId,
) -> Result<Vec<u8>, TransportError> {
    let begin = WorkspaceBlobBeginMessage {
        workspace_id,
        transfer_id,
        direction: WorkspaceBlobDirection::Upload,
        content_hash: content_hash.clone(),
        size,
        chunk_size: BLOB_CHUNK_BYTES,
        chunk_count: chunk_count(size),
    };
    encode_request(
        WorkspaceAction::WorkspaceBlobBegin,
        request_id,
        MessageBody::BlobBegin(begin),
    )
    .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))
}

/// Encode a WorkspaceBlobEnd message for an upload.
pub fn encode_blob_end_upload(
    workspace_id: fns_protocol::WorkspaceId,
    transfer_id: TransferId,
    content_hash: &WorkspaceContentHash,
    size: u64,
    request_id: fns_protocol::RequestId,
) -> Result<Vec<u8>, TransportError> {
    let end = WorkspaceBlobEndMessage {
        workspace_id,
        transfer_id,
        direction: WorkspaceBlobDirection::Upload,
        content_hash: content_hash.clone(),
        size,
        chunk_count: chunk_count(size),
    };
    encode_request(
        WorkspaceAction::WorkspaceBlobEnd,
        request_id,
        MessageBody::BlobEnd(end),
    )
    .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))
}

/// Compute the empty BLAKE3 hash for a zero-byte blob.
pub fn empty_blake3_hash() -> [u8; 32] {
    *blake3::hash(&[]).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_count_arithmetic() {
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(BLOB_CHUNK_BYTES as u64), 1);
        assert_eq!(chunk_count(BLOB_CHUNK_BYTES as u64 + 1), 2);
        assert_eq!(chunk_count(2 * BLOB_CHUNK_BYTES as u64 + 7), 3);
    }
}
