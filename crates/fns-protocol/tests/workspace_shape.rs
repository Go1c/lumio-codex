use std::fmt::Debug;
use std::{fs, path::PathBuf};

use fns_protocol::{
    ACTION_FLOW_SPECS, BLOB_CHUNK_BYTES, BLOB_HEADER_LEN, MAX_ACTION_BYTES, MAX_BLOB_BYTES,
    MAX_CONTROL_FRAME_BYTES, MessageBody, WorkspaceAction, WorkspaceBlobDirection,
    WorkspaceConflictChoice, WorkspaceConflictKind, WorkspaceEntryKind, WorkspaceFlow,
    WorkspaceMutationKind, WorkspaceMutationRejectReason, WorkspaceSnapshotMode,
    WorkspaceValidationError,
};
use serde::{Serialize, de::DeserializeOwned};

#[test]
fn protocol_limits_are_public_and_locked() {
    assert_eq!(MAX_CONTROL_FRAME_BYTES, 65_536);
    assert_eq!(MAX_ACTION_BYTES, 64);
    assert_eq!(BLOB_HEADER_LEN, 64);
    assert_eq!(BLOB_CHUNK_BYTES, 1_048_576);
    assert_eq!(MAX_BLOB_BYTES, 5_368_709_120);
}

#[test]
fn validation_error_has_stable_fields_and_display() {
    let error = WorkspaceValidationError {
        field: "path".to_owned(),
        reason: "invalid_segment".to_owned(),
    };
    assert_eq!(error.to_string(), "path: invalid_segment");
    assert_eq!(
        error,
        WorkspaceValidationError {
            field: "path".to_owned(),
            reason: "invalid_segment".to_owned(),
        }
    );
}

fn assert_wire_name<T>(value: T, name: &str)
where
    T: Debug + DeserializeOwned + Eq + Serialize,
{
    let raw = format!("\"{name}\"");
    assert_eq!(serde_json::to_string(&value).unwrap(), raw);
    assert_eq!(serde_json::from_str::<T>(&raw).unwrap(), value);
}

#[test]
fn primitive_enums_use_the_exact_wire_names() {
    for (value, name) in [
        (WorkspaceEntryKind::File, "file"),
        (WorkspaceEntryKind::Directory, "directory"),
        (WorkspaceEntryKind::Symlink, "symlink"),
        (WorkspaceEntryKind::Tombstone, "tombstone"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (WorkspaceMutationKind::UpsertFile, "upsert_file"),
        (WorkspaceMutationKind::Mkdir, "mkdir"),
        (WorkspaceMutationKind::UpsertSymlink, "upsert_symlink"),
        (WorkspaceMutationKind::Delete, "delete"),
        (WorkspaceMutationKind::Rename, "rename"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (WorkspaceSnapshotMode::Snapshot, "snapshot"),
        (WorkspaceSnapshotMode::Incremental, "incremental"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (WorkspaceBlobDirection::Upload, "upload"),
        (WorkspaceBlobDirection::Download, "download"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (WorkspaceConflictKind::Content, "content"),
        (WorkspaceConflictKind::DeleteModify, "delete_modify"),
        (WorkspaceConflictKind::Rename, "rename"),
        (WorkspaceConflictKind::Binary, "binary"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (WorkspaceConflictChoice::Current, "current"),
        (WorkspaceConflictChoice::Incoming, "incoming"),
        (WorkspaceConflictChoice::Merged, "merged"),
        (WorkspaceConflictChoice::Delete, "delete"),
    ] {
        assert_wire_name(value, name);
    }

    for (value, name) in [
        (
            WorkspaceMutationRejectReason::StaleBaseRevision,
            "stale_base_revision",
        ),
        (
            WorkspaceMutationRejectReason::OperationReused,
            "operation_reused",
        ),
        (WorkspaceMutationRejectReason::BlobRequired, "blob_required"),
        (
            WorkspaceMutationRejectReason::ConflictCreated,
            "conflict_created",
        ),
    ] {
        assert_wire_name(value, name);
    }
}

#[test]
fn action_contract_is_public_and_has_locked_cardinality() {
    assert_eq!(WorkspaceAction::ALL.len(), 15);
    assert_eq!(WorkspaceFlow::ALL.len(), 3);
    assert_eq!(ACTION_FLOW_SPECS.len(), 25);

    let kind = std::mem::size_of::<MessageBody>();
    assert!(kind > 0);
}

#[test]
fn ci_contract_requires_three_platforms_and_the_locked_quality_gate() {
    let protocol_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repository_root = protocol_crate
        .parent()
        .and_then(|path| path.parent())
        .expect("protocol crate must be nested below the repository root");
    let workflow = fs::read_to_string(repository_root.join(".github/workflows/rust.yml"))
        .expect("Rust CI workflow must exist");

    for required in [
        "os: [ubuntu-latest, macos-latest, windows-latest]",
        "toolchain: 1.94.0",
        "cargo test --locked -p fns-protocol",
        "cargo check --locked --workspace --all-targets",
        "cargo fmt --all -- --check",
        "cargo clippy --locked --workspace --all-targets -- -D warnings",
        "cargo test --locked --workspace",
    ] {
        assert!(
            workflow.contains(required),
            "missing CI command: {required}"
        );
    }
}

#[test]
fn fixture_bytes_are_marked_binary_for_cross_platform_checkout() {
    let protocol_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repository_root = protocol_crate
        .parent()
        .and_then(|path| path.parent())
        .expect("protocol crate must be nested below the repository root");
    let attributes = fs::read_to_string(repository_root.join(".gitattributes"))
        .expect("fixture attributes must exist");

    assert!(
        attributes
            .lines()
            .any(|line| line == "crates/fns-protocol/tests/fixtures/** -text"),
        "workspace fixture bytes must be marked binary"
    );
}
