use std::{fmt::Debug, fs, path::PathBuf};

use fns_protocol::{
    BLOB_CHUNK_BYTES, BLOB_HEADER_LEN, TransferId, WorkspaceBlobDirection, WorkspaceBlobHeader,
    WorkspaceValidationError, compute_blob_digest, decode_binary_frame,
    deserialize_optional_non_null, encode_binary_frame,
};
use serde::Deserialize;

const TRANSFER_ID: &str = "10000000-0000-4000-8000-000000000009";

fn transfer_id() -> TransferId {
    TransferId::parse(TRANSFER_ID).expect("test transfer ID is canonical")
}

fn assert_validation_error<T: Debug>(
    result: Result<T, WorkspaceValidationError>,
    field: &str,
    reason: &str,
) {
    let error = result.expect_err("operation should reject the invalid frame");
    assert_eq!(error.field, field);
    assert_eq!(error.reason, reason);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BinaryHeaderVector {
    case: String,
    direction: WorkspaceBlobDirection,
    #[serde(rename = "final")]
    final_: bool,
    transfer_id: TransferId,
    chunk_index: u64,
    offset: u64,
    payload_hex: String,
    digest_hex: String,
    header_hex: String,
    valid: bool,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    reason: Option<String>,
}

#[test]
fn fixture_vectors() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/workspace-sync-v2/binary/header-vectors.json");
    let rows: Vec<BinaryHeaderVector> = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(rows.len(), 9);
    assert!(
        rows.iter()
            .all(|row| !row.valid || !row.payload_hex.is_empty())
    );

    for row in rows {
        let payload = hex::decode(&row.payload_hex)
            .unwrap_or_else(|error| panic!("{} payloadHex is not hex: {error}", row.case));
        let committed_digest = hex::decode(&row.digest_hex)
            .unwrap_or_else(|error| panic!("{} digestHex is not hex: {error}", row.case));
        let header_bytes = hex::decode(&row.header_hex)
            .unwrap_or_else(|error| panic!("{} headerHex is not hex: {error}", row.case));
        assert_eq!(committed_digest.len(), 32, "{}", row.case);
        assert_eq!(header_bytes.len(), BLOB_HEADER_LEN, "{}", row.case);

        let (full_digest, _) = compute_blob_digest(&payload);
        let full_digest_matches = full_digest.as_slice() == committed_digest;
        let mut frame = header_bytes.clone();
        frame.extend_from_slice(&payload);

        if row.valid {
            assert!(full_digest_matches, "{} full digest", row.case);
            let (decoded, decoded_payload) =
                decode_binary_frame(&frame).unwrap_or_else(|error| panic!("{}: {error}", row.case));
            assert_eq!(decoded.direction, row.direction, "{}", row.case);
            assert_eq!(decoded.final_chunk, row.final_, "{}", row.case);
            assert_eq!(decoded.transfer_id, row.transfer_id, "{}", row.case);
            assert_eq!(decoded.chunk_index, row.chunk_index, "{}", row.case);
            assert_eq!(decoded.offset, row.offset, "{}", row.case);
            assert_eq!(decoded_payload, payload, "{}", row.case);
            assert_eq!(decoded.chunk_digest.as_slice(), &committed_digest[..16]);
            assert_eq!(
                encode_binary_frame(
                    row.direction,
                    row.final_,
                    row.transfer_id,
                    row.chunk_index,
                    row.offset,
                    &payload,
                )
                .unwrap(),
                frame,
                "{}",
                row.case
            );
        } else {
            let expected_reason = row.reason.as_deref().expect("invalid vector has a reason");
            let actual_reason = if !full_digest_matches {
                "full_digest_mismatch".to_owned()
            } else {
                decode_binary_frame(&frame).expect_err(&row.case).reason
            };
            assert_eq!(actual_reason, expected_reason, "{}", row.case);
        }
    }
}

#[test]
fn unit_codec_encodes_exact_upload_final_layout() {
    let payload = b"hello";
    let frame = encode_binary_frame(
        WorkspaceBlobDirection::Upload,
        true,
        transfer_id(),
        0,
        0,
        payload,
    )
    .expect("partial final upload should encode");

    let expected_header = hex::decode(
        "464e533202010140100000000000400080000000000000090000000000000000\
         00000000000000000000000500000000ea8f163db38682925e4491c5e58d4bb3",
    )
    .expect("header vector is valid hex");
    assert_eq!(frame.len(), BLOB_HEADER_LEN + payload.len());
    assert_eq!(&frame[..BLOB_HEADER_LEN], expected_header);
    assert_eq!(&frame[0..4], b"FNS2");
    assert_eq!(frame[4], 0x02);
    assert_eq!(frame[5], 0x01);
    assert_eq!(frame[6], 0x01);
    assert_eq!(frame[7], 0x40);
    assert_eq!(&frame[8..24], transfer_id().as_uuid().as_bytes());
    assert_eq!(u64::from_be_bytes(frame[24..32].try_into().unwrap()), 0);
    assert_eq!(u64::from_be_bytes(frame[32..40].try_into().unwrap()), 0);
    assert_eq!(u32::from_be_bytes(frame[40..44].try_into().unwrap()), 5);
    assert_eq!(&frame[44..48], &[0, 0, 0, 0]);

    let (header, decoded_payload) =
        decode_binary_frame(&frame).expect("encoded upload should decode");
    assert_eq!(header.direction, WorkspaceBlobDirection::Upload);
    assert!(header.final_chunk);
    assert_eq!(header.transfer_id, transfer_id());
    assert_eq!(header.chunk_index, 0);
    assert_eq!(header.offset, 0);
    assert_eq!(header.payload_len, 5);
    assert_eq!(decoded_payload, payload);
    header
        .validate_sequence(0, 0, true)
        .expect("single final chunk is a valid sequence");
}

#[test]
fn unit_codec_encodes_exact_download_nonzero_big_endian_layout() {
    let payload = [0, 1, 2, 3, 4, 5, 6];
    let frame = encode_binary_frame(
        WorkspaceBlobDirection::Download,
        true,
        transfer_id(),
        2,
        2_097_152,
        &payload,
    )
    .expect("partial final download should encode");

    let expected_header = hex::decode(
        "464e533202020140100000000000400080000000000000090000000000000002\
         000000000020000000000007000000003f8770f387faad08faa9d8414e9f449a",
    )
    .expect("header vector is valid hex");
    assert_eq!(&frame[..BLOB_HEADER_LEN], expected_header);
    assert_eq!(frame[5], 0x02);
    assert_eq!(u64::from_be_bytes(frame[24..32].try_into().unwrap()), 2);
    assert_eq!(
        u64::from_be_bytes(frame[32..40].try_into().unwrap()),
        2_097_152
    );
    assert_eq!(u32::from_be_bytes(frame[40..44].try_into().unwrap()), 7);

    let (header, decoded_payload) =
        decode_binary_frame(&frame).expect("encoded download should decode");
    assert_eq!(header.direction, WorkspaceBlobDirection::Download);
    assert_eq!(header.chunk_index, 2);
    assert_eq!(header.offset, 2_097_152);
    assert_eq!(decoded_payload, payload);
    header
        .validate_sequence(2, 2_097_152, true)
        .expect("partial final chunk is valid");
}

#[test]
fn unit_codec_accepts_a_full_sized_non_final_payload() {
    let payload = vec![0x5a; BLOB_CHUNK_BYTES as usize];
    let offset = 3 * u64::from(BLOB_CHUNK_BYTES);
    let frame = encode_binary_frame(
        WorkspaceBlobDirection::Upload,
        false,
        transfer_id(),
        3,
        offset,
        &payload,
    )
    .expect("maximum payload should encode");

    assert_eq!(frame.len(), BLOB_HEADER_LEN + BLOB_CHUNK_BYTES as usize);
    assert_eq!(frame[6], 0);
    assert_eq!(
        u32::from_be_bytes(frame[40..44].try_into().unwrap()),
        BLOB_CHUNK_BYTES
    );
    let (header, decoded_payload) =
        decode_binary_frame(&frame).expect("maximum payload should decode");
    assert_eq!(decoded_payload, payload);
    header
        .validate_sequence(3, offset, false)
        .expect("full-sized non-final chunk is valid");
}

#[test]
fn unit_codec_rejects_invalid_header_fields_and_frame_lengths() {
    let payload = b"hello";
    let valid = encode_binary_frame(
        WorkspaceBlobDirection::Upload,
        true,
        transfer_id(),
        0,
        0,
        payload,
    )
    .expect("test frame should encode");

    assert_validation_error(
        decode_binary_frame(&valid[..BLOB_HEADER_LEN - 1]),
        "header",
        "invalid_length",
    );

    let mut invalid = valid.clone();
    invalid[0] = b'X';
    assert_validation_error(decode_binary_frame(&invalid), "magic", "invalid");

    let mut invalid = valid.clone();
    invalid[4] = 1;
    assert_validation_error(decode_binary_frame(&invalid), "version", "invalid");

    let mut invalid = valid.clone();
    invalid[5] = 3;
    assert_validation_error(decode_binary_frame(&invalid), "direction", "invalid_enum");

    let mut invalid = valid.clone();
    invalid[6] = 3;
    assert_validation_error(decode_binary_frame(&invalid), "flags", "reserved_bits");

    let mut invalid = valid.clone();
    invalid[7] = 63;
    assert_validation_error(decode_binary_frame(&invalid), "headerLength", "invalid");

    let mut invalid = valid.clone();
    invalid[8..24].fill(0);
    assert_validation_error(decode_binary_frame(&invalid), "transferId", "invalid_uuid");

    let mut invalid = valid.clone();
    invalid[44] = 1;
    assert_validation_error(decode_binary_frame(&invalid), "reserved", "non_zero");

    let mut invalid = valid.clone();
    invalid[48] ^= 0xff;
    assert_validation_error(decode_binary_frame(&invalid), "chunkDigest", "mismatch");

    assert_validation_error(
        decode_binary_frame(&valid[..valid.len() - 1]),
        "payloadLength",
        "frame_mismatch",
    );
    let mut invalid = valid.clone();
    invalid.push(0);
    assert_validation_error(
        decode_binary_frame(&invalid),
        "payloadLength",
        "frame_mismatch",
    );
}

#[test]
fn unit_codec_rejects_zero_and_oversized_payload_frames() {
    let nil_transfer =
        TransferId::parse("00000000-0000-0000-0000-000000000000").expect("nil UUID is canonical");
    assert_validation_error(
        encode_binary_frame(
            WorkspaceBlobDirection::Upload,
            true,
            nil_transfer,
            0,
            0,
            b"x",
        ),
        "transferId",
        "invalid_uuid",
    );

    assert_validation_error(
        encode_binary_frame(
            WorkspaceBlobDirection::Download,
            true,
            transfer_id(),
            0,
            0,
            &[],
        ),
        "payloadLength",
        "empty_payload_forbidden",
    );

    let oversized_payload = vec![0x5a; BLOB_CHUNK_BYTES as usize + 1];
    assert_validation_error(
        encode_binary_frame(
            WorkspaceBlobDirection::Upload,
            true,
            transfer_id(),
            0,
            0,
            &oversized_payload,
        ),
        "payloadLength",
        "limit_exceeded",
    );

    let mut oversized_frame = encode_binary_frame(
        WorkspaceBlobDirection::Upload,
        false,
        transfer_id(),
        0,
        0,
        &oversized_payload[..BLOB_CHUNK_BYTES as usize],
    )
    .expect("maximum payload test frame should encode");
    oversized_frame.push(0x5a);
    oversized_frame[40..44].copy_from_slice(&(BLOB_CHUNK_BYTES + 1).to_be_bytes());
    let (_, first16) = compute_blob_digest(&oversized_payload);
    oversized_frame[48..64].copy_from_slice(&first16);
    assert_validation_error(
        decode_binary_frame(&oversized_frame),
        "payloadLength",
        "limit_exceeded",
    );
}

#[test]
fn unit_codec_keeps_empty_content_digest_valid_without_a_valid_empty_frame() {
    let (full, first16) = compute_blob_digest(&[]);
    assert_eq!(
        hex::encode(full),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(hex::encode(first16), "af1349b9f5f9a1a6a0404dea36dcc949");

    let mut frame = vec![0; BLOB_HEADER_LEN];
    frame[0..4].copy_from_slice(b"FNS2");
    frame[4] = 2;
    frame[5] = 2;
    frame[6] = 1;
    frame[7] = BLOB_HEADER_LEN as u8;
    frame[8..24].copy_from_slice(transfer_id().as_uuid().as_bytes());
    frame[48..64].copy_from_slice(&first16);
    assert_validation_error(
        decode_binary_frame(&frame),
        "payloadLength",
        "empty_payload_forbidden",
    );

    let header = WorkspaceBlobHeader {
        direction: WorkspaceBlobDirection::Download,
        final_chunk: true,
        transfer_id: transfer_id(),
        chunk_index: 0,
        offset: 0,
        payload_len: 0,
        chunk_digest: first16,
    };
    assert_validation_error(
        header.validate_sequence(0, 0, true),
        "payloadLength",
        "empty_payload_forbidden",
    );
}

#[test]
fn unit_codec_rejects_duplicate_out_of_order_and_invalid_sequence_chunks() {
    let base = WorkspaceBlobHeader {
        direction: WorkspaceBlobDirection::Upload,
        final_chunk: false,
        transfer_id: transfer_id(),
        chunk_index: 2,
        offset: 2 * u64::from(BLOB_CHUNK_BYTES),
        payload_len: BLOB_CHUNK_BYTES,
        chunk_digest: [0; 16],
    };
    base.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), false)
        .expect("contiguous full chunk should validate");

    for chunk_index in [1, 3] {
        let header = WorkspaceBlobHeader {
            chunk_index,
            ..base.clone()
        };
        assert_validation_error(
            header.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), false),
            "chunkIndex",
            "out_of_order",
        );
    }

    for offset in [u64::from(BLOB_CHUNK_BYTES), 3 * u64::from(BLOB_CHUNK_BYTES)] {
        let header = WorkspaceBlobHeader {
            offset,
            ..base.clone()
        };
        assert_validation_error(
            header.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), false),
            "offset",
            "out_of_order",
        );
    }

    let final_mismatch = WorkspaceBlobHeader {
        final_chunk: true,
        ..base.clone()
    };
    assert_validation_error(
        final_mismatch.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), false),
        "final",
        "flag_mismatch",
    );

    let short_non_final = WorkspaceBlobHeader {
        payload_len: BLOB_CHUNK_BYTES - 1,
        ..base.clone()
    };
    assert_validation_error(
        short_non_final.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), false),
        "payloadLength",
        "non_final_must_be_full",
    );

    let oversized_final = WorkspaceBlobHeader {
        final_chunk: true,
        payload_len: BLOB_CHUNK_BYTES + 1,
        ..base
    };
    assert_validation_error(
        oversized_final.validate_sequence(2, 2 * u64::from(BLOB_CHUNK_BYTES), true),
        "payloadLength",
        "limit_exceeded",
    );
}
