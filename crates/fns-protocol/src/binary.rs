use uuid::Uuid;

use crate::{
    BLOB_CHUNK_BYTES, BLOB_HEADER_LEN, TransferId, WorkspaceBlobDirection,
    WorkspaceValidationError, error::validation_error,
};

const BLOB_MAGIC: &[u8; 4] = b"FNS2";
const BLOB_VERSION: u8 = 0x02;
const UPLOAD_DIRECTION: u8 = 0x01;
const DOWNLOAD_DIRECTION: u8 = 0x02;
const FINAL_CHUNK_FLAG: u8 = 0x01;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceBlobHeader {
    pub direction: WorkspaceBlobDirection,
    pub final_chunk: bool,
    pub transfer_id: TransferId,
    pub chunk_index: u64,
    pub offset: u64,
    pub payload_len: u32,
    pub chunk_digest: [u8; 16],
}

impl WorkspaceBlobHeader {
    pub fn validate_sequence(
        &self,
        expected_index: u64,
        expected_offset: u64,
        is_last: bool,
    ) -> Result<(), WorkspaceValidationError> {
        if self.payload_len == 0 {
            return Err(validation_error("payloadLength", "empty_payload_forbidden"));
        }
        if self.chunk_index != expected_index {
            return Err(validation_error("chunkIndex", "out_of_order"));
        }
        if self.offset != expected_offset {
            return Err(validation_error("offset", "out_of_order"));
        }
        if self.final_chunk != is_last {
            return Err(validation_error("final", "flag_mismatch"));
        }
        if !is_last && self.payload_len != BLOB_CHUNK_BYTES {
            return Err(validation_error("payloadLength", "non_final_must_be_full"));
        }
        if self.payload_len > BLOB_CHUNK_BYTES {
            return Err(validation_error("payloadLength", "limit_exceeded"));
        }
        Ok(())
    }
}

pub fn compute_blob_digest(payload: &[u8]) -> ([u8; 32], [u8; 16]) {
    let full = *blake3::hash(payload).as_bytes();
    let mut first16 = [0; 16];
    first16.copy_from_slice(&full[..16]);
    (full, first16)
}

pub fn encode_binary_frame(
    direction: WorkspaceBlobDirection,
    final_chunk: bool,
    transfer_id: TransferId,
    chunk_index: u64,
    offset: u64,
    payload: &[u8],
) -> Result<Vec<u8>, WorkspaceValidationError> {
    if transfer_id.as_uuid().is_nil() {
        return Err(validation_error("transferId", "invalid_uuid"));
    }
    validate_payload_len(payload.len())?;

    let payload_len = payload.len() as u32;
    let (_, chunk_digest) = compute_blob_digest(payload);
    let mut frame = vec![0; BLOB_HEADER_LEN];
    frame[0..4].copy_from_slice(BLOB_MAGIC);
    frame[4] = BLOB_VERSION;
    frame[5] = match direction {
        WorkspaceBlobDirection::Upload => UPLOAD_DIRECTION,
        WorkspaceBlobDirection::Download => DOWNLOAD_DIRECTION,
    };
    if final_chunk {
        frame[6] = FINAL_CHUNK_FLAG;
    }
    frame[7] = BLOB_HEADER_LEN as u8;
    frame[8..24].copy_from_slice(transfer_id.as_uuid().as_bytes());
    frame[24..32].copy_from_slice(&chunk_index.to_be_bytes());
    frame[32..40].copy_from_slice(&offset.to_be_bytes());
    frame[40..44].copy_from_slice(&payload_len.to_be_bytes());
    frame[48..64].copy_from_slice(&chunk_digest);
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_binary_frame(
    frame: &[u8],
) -> Result<(WorkspaceBlobHeader, &[u8]), WorkspaceValidationError> {
    if frame.len() < BLOB_HEADER_LEN {
        return Err(validation_error("header", "invalid_length"));
    }
    if &frame[0..4] != BLOB_MAGIC {
        return Err(validation_error("magic", "invalid"));
    }
    if frame[4] != BLOB_VERSION {
        return Err(validation_error("version", "invalid"));
    }
    let direction = match frame[5] {
        UPLOAD_DIRECTION => WorkspaceBlobDirection::Upload,
        DOWNLOAD_DIRECTION => WorkspaceBlobDirection::Download,
        _ => return Err(validation_error("direction", "invalid_enum")),
    };
    if frame[6] & !FINAL_CHUNK_FLAG != 0 {
        return Err(validation_error("flags", "reserved_bits"));
    }
    let final_chunk = frame[6] & FINAL_CHUNK_FLAG != 0;
    if usize::from(frame[7]) != BLOB_HEADER_LEN {
        return Err(validation_error("headerLength", "invalid"));
    }

    let mut transfer_bytes = [0; 16];
    transfer_bytes.copy_from_slice(&frame[8..24]);
    let transfer_uuid = Uuid::from_bytes(transfer_bytes);
    if transfer_uuid.is_nil() {
        return Err(validation_error("transferId", "invalid_uuid"));
    }
    let transfer_id = TransferId::parse(&transfer_uuid.to_string())?;

    let chunk_index = u64::from_be_bytes(frame[24..32].try_into().expect("fixed-width slice"));
    let offset = u64::from_be_bytes(frame[32..40].try_into().expect("fixed-width slice"));
    let payload_len = u32::from_be_bytes(frame[40..44].try_into().expect("fixed-width slice"));
    if payload_len == 0 {
        return Err(validation_error("payloadLength", "empty_payload_forbidden"));
    }
    if payload_len > BLOB_CHUNK_BYTES {
        return Err(validation_error("payloadLength", "limit_exceeded"));
    }
    if frame.len() != BLOB_HEADER_LEN + payload_len as usize {
        return Err(validation_error("payloadLength", "frame_mismatch"));
    }
    if frame[44..48] != [0, 0, 0, 0] {
        return Err(validation_error("reserved", "non_zero"));
    }

    let payload = &frame[BLOB_HEADER_LEN..];
    let (_, expected_digest) = compute_blob_digest(payload);
    let mut chunk_digest = [0; 16];
    chunk_digest.copy_from_slice(&frame[48..64]);
    if chunk_digest != expected_digest {
        return Err(validation_error("chunkDigest", "mismatch"));
    }

    Ok((
        WorkspaceBlobHeader {
            direction,
            final_chunk,
            transfer_id,
            chunk_index,
            offset,
            payload_len,
            chunk_digest,
        },
        payload,
    ))
}

fn validate_payload_len(payload_len: usize) -> Result<(), WorkspaceValidationError> {
    if payload_len == 0 {
        return Err(validation_error("payloadLength", "empty_payload_forbidden"));
    }
    if payload_len > BLOB_CHUNK_BYTES as usize {
        return Err(validation_error("payloadLength", "limit_exceeded"));
    }
    Ok(())
}
