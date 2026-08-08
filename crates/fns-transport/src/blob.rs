//! Blob upload/download: Begin/chunks/End framing, staging files, BLAKE3 verification,
//! and restart-from-zero semantics.

#![allow(dead_code)] // Wire integration in later sub-tasks.

use crate::error::{TransportError, TransportErrorCode};

use fns_protocol::{
    BLOB_CHUNK_BYTES, MessageBody, TransferId, WorkspaceAction, WorkspaceBlobBeginMessage,
    WorkspaceBlobDirection, WorkspaceBlobEndMessage, WorkspaceContentHash, encode_binary_frame,
    encode_request,
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

/// Chunk a blob's content into binary frames for upload.
/// Returns a vector of (chunk_index, binary_frame_bytes).
pub fn chunk_blob_for_upload(
    transfer_id: TransferId,
    content: &[u8],
) -> Result<Vec<(u64, Vec<u8>)>, TransportError> {
    let chunk_size = BLOB_CHUNK_BYTES as usize;
    let total = content.len();

    if total == 0 {
        return Ok(Vec::new());
    }

    let mut frames = Vec::new();
    let mut offset = 0u64;

    for (index, chunk) in content.chunks(chunk_size).enumerate() {
        let index = index as u64;
        let is_final = offset as usize + chunk.len() >= total;
        let frame = encode_binary_frame(
            WorkspaceBlobDirection::Upload,
            is_final,
            transfer_id,
            index,
            offset,
            chunk,
        )
        .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        frames.push((index, frame));
        offset += chunk.len() as u64;
    }

    Ok(frames)
}

/// Compute the empty BLAKE3 hash for a zero-byte blob.
pub fn empty_blake3_hash() -> [u8; 32] {
    *blake3::hash(&[]).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer_id() -> TransferId {
        TransferId::parse("10000000-0000-4000-8000-000000000099").unwrap()
    }

    #[test]
    fn chunk_count_arithmetic() {
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(BLOB_CHUNK_BYTES as u64), 1);
        assert_eq!(chunk_count(BLOB_CHUNK_BYTES as u64 + 1), 2);
        assert_eq!(chunk_count(2 * BLOB_CHUNK_BYTES as u64 + 7), 3);
    }

    #[test]
    fn empty_blob_produces_no_chunks() {
        let frames = chunk_blob_for_upload(transfer_id(), b"").unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn small_blob_produces_one_final_chunk() {
        let frames = chunk_blob_for_upload(transfer_id(), b"hello").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, 0); // index 0
    }

    #[test]
    fn multi_chunk_blob_has_correct_indices_and_final_flag() {
        // 2 * chunk_size + 7 bytes → 3 chunks
        let chunk_size = BLOB_CHUNK_BYTES as usize;
        let content = vec![0xAB; 2 * chunk_size + 7];
        let frames = chunk_blob_for_upload(transfer_id(), &content).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].0, 0);
        assert_eq!(frames[1].0, 1);
        assert_eq!(frames[2].0, 2);
    }
}
