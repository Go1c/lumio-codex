use std::collections::BTreeMap;

use fns_protocol::{
    ClientId, ConflictId, MAX_BLOB_BYTES, OperationId, RequestId, RequiredNullable, StreamId,
    TransferId, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspaceId,
    WorkspacePath, WorkspaceRevision, WorkspaceValidationError, deserialize_optional_non_null,
    strict_json,
};
use serde::{Deserialize, Serialize};

const CANONICAL_UUID: &str = "10000000-0000-4000-8000-000000000001";

fn assert_validation_error<T>(
    result: Result<T, WorkspaceValidationError>,
    field: &str,
    reason: &str,
) {
    match result {
        Ok(_) => panic!("expected {field}: {reason}"),
        Err(error) => {
            assert_eq!(error.field, field);
            assert_eq!(error.reason, reason);
        }
    }
}

#[test]
fn revisions_use_canonical_decimal_json_strings() {
    for value in ["0", "1", "18446744073709551615"] {
        let raw = format!("\"{value}\"");
        let decoded = WorkspaceRevision::decode_json(raw.as_bytes()).expect("valid revision");
        assert_eq!(
            decoded,
            WorkspaceRevision::parse(value).expect("valid revision")
        );
        assert_eq!(serde_json::to_string(&decoded).unwrap(), raw);
    }

    for (raw, reason) in [
        ("1", "must_be_string"),
        ("\"-1\"", "non_canonical_decimal"),
        ("\"01\"", "non_canonical_decimal"),
        ("\"18446744073709551616\"", "non_canonical_decimal"),
        ("\"\"", "empty"),
    ] {
        assert_validation_error(
            WorkspaceRevision::decode_json(raw.as_bytes()),
            "revision",
            reason,
        );
    }
}

macro_rules! assert_uuid_wrapper {
    ($type:ty, $field:literal) => {{
        let parsed = <$type>::parse(CANONICAL_UUID).expect("canonical UUID");
        let raw = format!("\"{CANONICAL_UUID}\"");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), raw);
        assert_eq!(serde_json::from_str::<$type>(&raw).unwrap(), parsed);

        for invalid in [
            "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF",
            "10000000000040008000000000000001",
            "{10000000-0000-4000-8000-000000000001}",
            "urn:uuid:10000000-0000-4000-8000-000000000001",
        ] {
            assert_validation_error(<$type>::parse(invalid), $field, "invalid_uuid");
            assert!(serde_json::from_str::<$type>(&format!("\"{invalid}\"")).is_err());
        }
    }};
}

#[test]
fn uuid_newtypes_require_canonical_lowercase_hyphenated_spelling() {
    assert_uuid_wrapper!(WorkspaceId, "workspaceId");
    assert_uuid_wrapper!(ClientId, "clientId");
    assert_uuid_wrapper!(OperationId, "operationId");
    assert_uuid_wrapper!(RequestId, "requestId");
    assert_uuid_wrapper!(StreamId, "streamId");
    assert_uuid_wrapper!(TransferId, "transferId");
    assert_uuid_wrapper!(ConflictId, "conflictId");
}

#[test]
fn content_hashes_require_blake3_and_64_lowercase_hex_characters() {
    let valid = format!("blake3:{}", "ab".repeat(32));
    let parsed = WorkspaceContentHash::parse(&valid).expect("valid BLAKE3 hash");
    let raw = format!("\"{valid}\"");
    assert_eq!(
        WorkspaceContentHash::decode_json(raw.as_bytes()).unwrap(),
        parsed
    );
    assert_eq!(serde_json::to_string(&parsed).unwrap(), raw);

    assert_validation_error(
        WorkspaceContentHash::decode_json(b"1"),
        "contentHash",
        "must_be_string",
    );

    for invalid in [
        format!("sha256:{}", "ab".repeat(32)),
        "blake3:abab".to_owned(),
        format!("blake3:{}", "AB".repeat(32)),
        format!("blake3:{}", "zz".repeat(32)),
    ] {
        assert_validation_error(
            WorkspaceContentHash::parse(&invalid),
            "contentHash",
            "invalid_blake3",
        );
    }
}

#[test]
fn paths_accept_canonical_nfc_workspace_relative_posix_values() {
    for valid in ["notes/café.md".to_owned(), "a".repeat(4096)] {
        let parsed = WorkspacePath::parse(&valid).expect("valid path");
        let raw = serde_json::to_vec(&valid).unwrap();
        assert_eq!(WorkspacePath::decode_json(&raw).unwrap(), parsed);
        assert_eq!(serde_json::to_vec(&parsed).unwrap(), raw);
    }

    assert_validation_error(WorkspacePath::decode_json(b"1"), "path", "must_be_string");
}

#[test]
fn paths_reject_all_committed_invalid_categories_with_exact_reasons() {
    let cases = [
        ("empty", String::new(), "invalid_length_or_utf8"),
        ("absolute", "/notes/a.md".to_owned(), "not_relative_posix"),
        ("traversal", "notes/../a.md".to_owned(), "invalid_segment"),
        ("backslash", r"notes\a.md".to_owned(), "not_relative_posix"),
        (
            "control",
            "notes/\u{0001}a.md".to_owned(),
            "unsafe_character",
        ),
        ("nfd", "notes/cafe\u{0301}.md".to_owned(), "not_nfc"),
        (
            "windows-suffix",
            "notes/name.".to_owned(),
            "windows_unsafe_suffix",
        ),
        (
            "unsafe-character",
            "notes/bad?.md".to_owned(),
            "unsafe_character",
        ),
        (
            "windows-device",
            "notes/CON.txt".to_owned(),
            "windows_device_name",
        ),
        (
            "over-4096-utf8-bytes",
            "a".repeat(4097),
            "invalid_length_or_utf8",
        ),
        (
            "trailing-slash",
            "notes/a.md/".to_owned(),
            "not_relative_posix",
        ),
        (
            "double-slash",
            "notes//a.md".to_owned(),
            "not_relative_posix",
        ),
        ("dot-segment", "notes/./a.md".to_owned(), "invalid_segment"),
        (
            "c1-control",
            "notes/\u{0085}a.md".to_owned(),
            "unsafe_character",
        ),
        (
            "trailing-space",
            "notes/name ".to_owned(),
            "windows_unsafe_suffix",
        ),
        (
            "windows-drive",
            r"C:\notes\a.md".to_owned(),
            "not_relative_posix",
        ),
        (
            "windows-unc",
            r"\\server\share\a.md".to_owned(),
            "not_relative_posix",
        ),
        (
            "windows-device-com1",
            "notes/com1".to_owned(),
            "windows_device_name",
        ),
        (
            "windows-device-lpt9-extension",
            "notes/LpT9.log".to_owned(),
            "windows_device_name",
        ),
    ];

    assert_eq!(cases.len(), 19);
    for (case, value, reason) in cases {
        let result = WorkspacePath::parse(&value);
        match result {
            Ok(_) => panic!("path fixture {case} unexpectedly passed"),
            Err(error) => {
                assert_eq!(error.field, "path", "fixture {case}");
                assert_eq!(error.reason, reason, "fixture {case}");
            }
        }
    }

    assert_validation_error(
        WorkspacePath::parse("notes/LPT9.txt"),
        "path",
        "windows_device_name",
    );
}

#[test]
fn metadata_enforces_bounds_and_directory_tombstone_rules() {
    let maximum = WorkspaceFileMetadata {
        size: MAX_BLOB_BYTES,
        modified_at_ms: 253_402_300_799_999,
        executable: true,
    };
    assert_eq!(maximum.validate(WorkspaceEntryKind::File), Ok(()));
    assert_eq!(maximum.validate(WorkspaceEntryKind::Symlink), Ok(()));

    assert_validation_error(
        WorkspaceFileMetadata {
            size: MAX_BLOB_BYTES + 1,
            modified_at_ms: 0,
            executable: false,
        }
        .validate(WorkspaceEntryKind::File),
        "metadata.size",
        "limit_exceeded",
    );
    for modified_at_ms in [-1, 253_402_300_800_000] {
        assert_validation_error(
            WorkspaceFileMetadata {
                size: 0,
                modified_at_ms,
                executable: false,
            }
            .validate(WorkspaceEntryKind::File),
            "metadata.modifiedAtMs",
            "out_of_range",
        );
    }

    for kind in [WorkspaceEntryKind::Directory, WorkspaceEntryKind::Tombstone] {
        assert_eq!(
            WorkspaceFileMetadata {
                size: 0,
                modified_at_ms: 0,
                executable: false,
            }
            .validate(kind),
            Ok(())
        );
        assert_validation_error(
            WorkspaceFileMetadata {
                size: 1,
                modified_at_ms: 0,
                executable: false,
            }
            .validate(kind),
            "metadata.size",
            "must_be_zero",
        );
        assert_validation_error(
            WorkspaceFileMetadata {
                size: 0,
                modified_at_ms: 0,
                executable: true,
            }
            .validate(kind),
            "metadata.executable",
            "must_be_false",
        );
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RequiredNullableFixture {
    value: RequiredNullable<String>,
}

#[test]
fn required_nullable_distinguishes_omission_null_and_value() {
    assert!(strict_json::from_slice::<RequiredNullableFixture>(br#"{}"#).is_err());

    let null = strict_json::from_slice::<RequiredNullableFixture>(br#"{"value":null}"#)
        .expect("explicit null");
    assert!(null.value.is_null());
    assert_eq!(null.value.as_ref(), RequiredNullable::Null);
    assert_eq!(serde_json::to_vec(&null).unwrap(), br#"{"value":null}"#);

    let value = strict_json::from_slice::<RequiredNullableFixture>(br#"{"value":"set"}"#)
        .expect("concrete value");
    assert_eq!(
        value.value.as_ref(),
        RequiredNullable::Value(&"set".to_owned())
    );
    assert_eq!(value.value.clone().into_option(), Some("set".to_owned()));
    assert_eq!(serde_json::to_vec(&value).unwrap(), br#"{"value":"set"}"#);
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OptionalNonNullFixture {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    value: Option<String>,
}

#[test]
fn optional_non_null_accepts_omission_or_value_but_rejects_null() {
    assert_eq!(
        strict_json::from_slice::<OptionalNonNullFixture>(br#"{}"#).unwrap(),
        OptionalNonNullFixture { value: None }
    );
    assert_eq!(
        strict_json::from_slice::<OptionalNonNullFixture>(br#"{"value":"set"}"#).unwrap(),
        OptionalNonNullFixture {
            value: Some("set".to_owned())
        }
    );
    assert!(strict_json::from_slice::<OptionalNonNullFixture>(br#"{"value":null}"#).is_err());
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct StrictNested {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct StrictFixture {
    boolean: bool,
    number: u32,
    text: String,
    nested: StrictNested,
    items: Vec<StrictNested>,
    map: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    optional: Option<StrictNested>,
}

const STRICT_VALID: &[u8] = br#"{
    "boolean":false,
    "number":0,
    "text":"",
    "nested":{"name":"nested"},
    "items":[{"name":"item"}],
    "map":{"key":"value"}
}"#;

#[test]
fn strict_json_accepts_one_complete_object() {
    strict_json::from_slice::<StrictFixture>(STRICT_VALID).expect("strict object");
}

#[test]
fn strict_json_rejects_non_object_trailing_and_recursive_duplicate_keys() {
    assert!(strict_json::from_slice::<serde_json::Value>(br#"[]"#).is_err());
    assert!(
        strict_json::from_slice::<StrictFixture>(
            br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{}} {}"#,
        )
        .is_err()
    );

    for raw in [
        br#"{"boolean":false,"boolean":true,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"one","name":"two"},"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[{"name":"one","name":"two"}],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{"key":"one","key":"two"}}"#.as_slice(),
    ] {
        assert!(strict_json::from_slice::<StrictFixture>(raw).is_err());
    }
}

#[test]
fn strict_json_rejects_unknown_and_missing_required_fields_recursively() {
    for raw in [
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{},"unexpected":true}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested","unexpected":true},"items":[],"map":{}}"#.as_slice(),
        br#"{"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"text":"","nested":{"name":"nested"},"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[{}],"map":{}}"#.as_slice(),
    ] {
        assert!(strict_json::from_slice::<StrictFixture>(raw).is_err());
    }
}

#[test]
fn strict_json_rejects_null_for_non_nullable_kinds_and_present_optionals() {
    for raw in [
        br#"{"boolean":null,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":null,"items":[],"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":null,"map":{}}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":null}"#.as_slice(),
        br#"{"boolean":false,"number":0,"text":"","nested":{"name":"nested"},"items":[],"map":{},"optional":null}"#.as_slice(),
    ] {
        assert!(strict_json::from_slice::<StrictFixture>(raw).is_err());
    }
}
