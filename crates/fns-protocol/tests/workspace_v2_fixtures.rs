use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use fns_protocol::{
    ACTION_FLOW_SPECS, DecodedEnvelope, DecodedFrame, MessageBody, RequestId, RequiredNullable,
    WorkspaceAction, WorkspaceContentHash, WorkspaceFlow, WorkspacePath, WorkspaceRevision,
    WorkspaceSnapshotMode, WorkspaceV2ErrorCode, WorkspaceValidationError, decode_data,
    decode_server_text_frame, decode_text_frame, deserialize_optional_non_null, encode_failure,
    encode_request, encode_success,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, value::RawValue};
use sha2::{Digest, Sha256};

const SOURCE_COMMIT: &str = "ba4caa45bb766dc4f1bc983e134d6b272a70cd05";
const MANIFEST_SHA256: &str = "86f52715e7827ac99873850961ee84ffd99610a5f0009b16033d5706b18f9e7e";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureManifest {
    schema_version: String,
    actions: Vec<WorkspaceAction>,
    files: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlFixtureRow {
    case: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    sequence: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    step: Option<u32>,
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    frame: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ErrorFixtureRow {
    case: String,
    action: WorkspaceAction,
    frame: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvalidFixtureRow {
    case: String,
    value: Box<RawValue>,
    field: String,
    reason: String,
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace-sync-v2")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("invalid row in {}: {error}", path.display()))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDataEnvelope<'a> {
    #[serde(default, borrow)]
    data: Option<&'a RawValue>,
}

fn raw_data(frame: &str) -> Option<&RawValue> {
    let (_, envelope) = frame.split_once('|').expect("fixture frame has a pipe");
    serde_json::from_str::<RawDataEnvelope<'_>>(envelope)
        .expect("fixture envelope is JSON")
        .data
}

fn decoded_request_id(decoded: &DecodedFrame) -> Option<RequestId> {
    match &decoded.envelope {
        DecodedEnvelope::Request { request_id, .. } => Some(*request_id),
        DecodedEnvelope::Success { request_id, .. }
        | DecodedEnvelope::Failure { request_id, .. } => *request_id,
    }
}

fn decoded_body(decoded: &DecodedFrame) -> Option<&MessageBody> {
    match &decoded.envelope {
        DecodedEnvelope::Request { body, .. } | DecodedEnvelope::Success { body, .. } => Some(body),
        DecodedEnvelope::Failure { .. } => None,
    }
}

fn decode_control_fixture(row: &ControlFixtureRow) -> DecodedFrame {
    match row.flow {
        WorkspaceFlow::ClientRequest => {
            decode_text_frame(row.frame.as_bytes(), WorkspaceFlow::ClientRequest)
        }
        WorkspaceFlow::ServerResponse | WorkspaceFlow::ServerPush => {
            decode_server_text_frame(row.frame.as_bytes())
        }
    }
    .unwrap_or_else(|error| panic!("{}: {error}", row.case))
}

fn fixture_sequence(name: &str) -> Vec<(ControlFixtureRow, DecodedFrame)> {
    let controls: Vec<ControlFixtureRow> =
        read_jsonl(&fixture_root().join("valid/control-frames.jsonl"));
    let mut rows = controls
        .into_iter()
        .filter(|row| row.sequence.as_deref() == Some(name))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.step);
    rows.into_iter()
        .map(|row| {
            let decoded = decode_control_fixture(&row);
            (row, decoded)
        })
        .collect()
}

#[derive(Clone)]
enum JsonPathPart {
    Key(String),
    Index(usize),
}

fn collect_object_key_paths(
    value: &Value,
    prefix: &mut Vec<JsonPathPart>,
    paths: &mut Vec<Vec<JsonPathPart>>,
) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                prefix.push(JsonPathPart::Key(key.clone()));
                paths.push(prefix.clone());
                collect_object_key_paths(child, prefix, paths);
                prefix.pop();
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter().enumerate() {
                prefix.push(JsonPathPart::Index(index));
                collect_object_key_paths(child, prefix, paths);
                prefix.pop();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn parent_mut<'a>(value: &'a mut Value, path: &[JsonPathPart]) -> &'a mut Value {
    let mut current = value;
    for part in &path[..path.len() - 1] {
        current = match part {
            JsonPathPart::Key(key) => current
                .as_object_mut()
                .and_then(|object| object.get_mut(key))
                .expect("recorded object path exists"),
            JsonPathPart::Index(index) => current
                .as_array_mut()
                .and_then(|array| array.get_mut(*index))
                .expect("recorded array path exists"),
        };
    }
    current
}

fn remove_json_key(value: &mut Value, path: &[JsonPathPart]) {
    let JsonPathPart::Key(key) = path.last().expect("key path is nonempty") else {
        unreachable!("only object-key paths are recorded")
    };
    parent_mut(value, path)
        .as_object_mut()
        .expect("recorded parent is an object")
        .remove(key);
}

fn set_json_null(value: &mut Value, path: &[JsonPathPart]) {
    let JsonPathPart::Key(key) = path.last().expect("key path is nonempty") else {
        unreachable!("only object-key paths are recorded")
    };
    *parent_mut(value, path)
        .as_object_mut()
        .and_then(|object| object.get_mut(key))
        .expect("recorded key exists") = Value::Null;
}

fn optional_non_null_key(path: &[JsonPathPart]) -> bool {
    matches!(
        path.last(),
        Some(JsonPathPart::Key(key))
            if matches!(
                key.as_str(),
                "newPath" | "targetBasePathRevision" | "oldPathState" | "newPathState"
            )
    )
}

fn json_path(path: &[JsonPathPart]) -> String {
    let mut rendered = String::new();
    for part in path {
        match part {
            JsonPathPart::Key(key) => {
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(key);
            }
            JsonPathPart::Index(index) => {
                rendered.push('[');
                rendered.push_str(&index.to_string());
                rendered.push(']');
            }
        }
    }
    rendered
}

fn required_nullable_path(
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    path: &[JsonPathPart],
) -> bool {
    use WorkspaceAction::{
        WorkspaceBlobNeed, WorkspaceConflictCreated, WorkspaceConflictResolved, WorkspaceEvent,
        WorkspaceMutation, WorkspaceMutationAccepted, WorkspaceMutationRejected,
        WorkspaceSnapshotEntry,
    };
    use WorkspaceFlow::{ClientRequest, ServerPush, ServerResponse};

    let path = json_path(path);
    matches!(
        (action, flow, path.as_str()),
        (WorkspaceSnapshotEntry, ServerPush, "entry.contentHash")
            | (WorkspaceMutation, ClientRequest, "contentHash")
            | (
                WorkspaceMutationAccepted,
                ServerResponse,
                "pathState.contentHash" | "oldPathState.contentHash" | "newPathState.contentHash"
            )
            | (
                WorkspaceMutationRejected,
                ServerResponse,
                "currentPathState" | "currentPathState.contentHash" | "conflictId" | "requiredHash"
            )
            | (
                WorkspaceEvent,
                ServerPush,
                "mutation.contentHash"
                    | "pathState.contentHash"
                    | "oldPathState.contentHash"
                    | "newPathState.contentHash"
            )
            | (WorkspaceBlobNeed, ClientRequest, "operationId" | "size")
            | (WorkspaceBlobNeed, ServerResponse, "operationId")
            | (
                WorkspaceConflictCreated,
                ServerPush,
                "ancestor.path"
                    | "ancestor.contentHash"
                    | "current.path"
                    | "current.contentHash"
                    | "incoming.path"
                    | "incoming.contentHash"
            )
            | (WorkspaceConflictResolved, ClientRequest, "contentHash")
            | (
                WorkspaceConflictResolved,
                ServerResponse | ServerPush,
                "pathState.contentHash"
            )
    )
}

fn assert_validation_error<T>(
    result: Result<T, WorkspaceValidationError>,
    expected: &InvalidFixtureRow,
) {
    let error = match result {
        Ok(_) => panic!("{} was accepted", expected.case),
        Err(error) => error,
    };
    assert_eq!(error.field, expected.field, "{}", expected.case);
    assert_eq!(error.reason, expected.reason, "{}", expected.case);
}

fn key_path(path: &str) -> Vec<JsonPathPart> {
    path.split('.')
        .map(|key| JsonPathPart::Key(key.to_owned()))
        .collect()
}

#[test]
fn fixture_source_provenance_is_exact() {
    let source_sha_path = fixture_root()
        .parent()
        .expect("fixture archive has a parent")
        .join("SOURCE_MANIFEST_SHA256");
    assert_eq!(
        fs::read(source_sha_path).unwrap(),
        format!("{MANIFEST_SHA256}\n").as_bytes(),
        "source manifest pin must be the exact SHA-256 plus one newline",
    );

    let readme =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .unwrap();
    assert!(readme.lines().any(|line| {
        line == format!("Workspace sync v2 authority: `fast-note-sync-service@{SOURCE_COMMIT}`.")
    }));
    assert!(
        readme.lines().any(|line| {
            line == format!("Source fixture manifest SHA-256: `{MANIFEST_SHA256}`.")
        })
    );
}

#[test]
fn snapshot_conflict_sequences_are_complete_ordered_and_strict() {
    use WorkspaceAction::{
        WorkspaceConflictCreated, WorkspaceConflictResolved, WorkspaceEvent,
        WorkspaceSnapshotBegin, WorkspaceSnapshotEnd, WorkspaceSnapshotEntry,
    };

    let full = fixture_sequence("snapshot-full-conflicts");
    assert_eq!(full.len(), 4, "full snapshot sequence length");
    assert_eq!(
        full.iter().map(|(row, _)| row.action).collect::<Vec<_>>(),
        [
            WorkspaceSnapshotBegin,
            WorkspaceSnapshotEntry,
            WorkspaceConflictCreated,
            WorkspaceSnapshotEnd,
        ]
    );
    assert!(
        full.iter()
            .all(|(row, _)| row.flow == WorkspaceFlow::ServerPush)
    );
    let Some(MessageBody::SnapshotBegin(full_begin)) = decoded_body(&full[0].1) else {
        panic!("full snapshot starts with Begin")
    };
    let Some(MessageBody::SnapshotEntry(full_entry)) = decoded_body(&full[1].1) else {
        panic!("full snapshot contains an entry")
    };
    let Some(MessageBody::ConflictCreated(full_conflict)) = decoded_body(&full[2].1) else {
        panic!("full snapshot contains its authoritative conflict")
    };
    let Some(MessageBody::SnapshotEnd(full_end)) = decoded_body(&full[3].1) else {
        panic!("full snapshot ends with End")
    };
    assert_eq!(full_begin.mode, WorkspaceSnapshotMode::Snapshot);
    assert_eq!(full_begin.entry_count, 1);
    assert_eq!(full_begin.event_count, 0);
    assert_eq!(full_begin.conflict_count, 1);
    assert_eq!(full_entry.workspace_id, full_begin.workspace_id);
    assert_eq!(full_entry.stream_id, full_begin.stream_id);
    full_entry.validate_at(0).unwrap();
    assert_eq!(full_conflict.workspace_id, full_begin.workspace_id);
    full_conflict.validate().unwrap();
    assert_eq!(full_end.delivered_count, 2);
    full_end.validate_against(full_begin).unwrap();

    let incremental = fixture_sequence("snapshot-incremental-conflicts");
    assert_eq!(incremental.len(), 7, "incremental snapshot sequence length");
    assert_eq!(
        incremental
            .iter()
            .map(|(row, _)| row.action)
            .collect::<Vec<_>>(),
        [
            WorkspaceSnapshotBegin,
            WorkspaceEvent,
            WorkspaceConflictResolved,
            WorkspaceEvent,
            WorkspaceConflictCreated,
            WorkspaceConflictCreated,
            WorkspaceSnapshotEnd,
        ]
    );
    assert!(
        incremental
            .iter()
            .all(|(row, _)| row.flow == WorkspaceFlow::ServerPush)
    );
    let Some(MessageBody::SnapshotBegin(incremental_begin)) = decoded_body(&incremental[0].1)
    else {
        panic!("incremental snapshot starts with Begin")
    };
    let Some(MessageBody::Event(first_event)) = decoded_body(&incremental[1].1) else {
        panic!("incremental snapshot contains its first Event")
    };
    let Some(MessageBody::ConflictResolved(resolved)) = decoded_body(&incremental[2].1) else {
        panic!("incremental snapshot contains ConflictResolved")
    };
    let Some(MessageBody::Event(second_event)) = decoded_body(&incremental[3].1) else {
        panic!("incremental snapshot contains its second Event")
    };
    let Some(MessageBody::ConflictCreated(first_conflict)) = decoded_body(&incremental[4].1) else {
        panic!("incremental snapshot contains its first ConflictCreated")
    };
    let Some(MessageBody::ConflictCreated(second_conflict)) = decoded_body(&incremental[5].1)
    else {
        panic!("incremental snapshot contains its second ConflictCreated")
    };
    let Some(MessageBody::SnapshotEnd(incremental_end)) = decoded_body(&incremental[6].1) else {
        panic!("incremental snapshot ends with End")
    };
    assert_eq!(incremental_begin.mode, WorkspaceSnapshotMode::Incremental);
    assert_eq!(incremental_begin.entry_count, 0);
    assert_eq!(incremental_begin.event_count, 3);
    assert_eq!(incremental_begin.conflict_count, 2);
    first_event
        .validate_after(0, incremental_begin.from_revision)
        .unwrap();
    resolved.validate().unwrap();
    second_event
        .validate_after(first_event.index, resolved.revision)
        .unwrap();
    assert_eq!(second_event.index, first_event.index + 1);
    assert!(first_event.revision < resolved.revision);
    assert!(resolved.revision < second_event.revision);
    assert_eq!(second_event.revision, incremental_begin.final_revision);
    assert!(first_conflict.conflict_id < second_conflict.conflict_id);
    assert_eq!(first_conflict.workspace_id, incremental_begin.workspace_id);
    assert_eq!(second_conflict.workspace_id, incremental_begin.workspace_id);
    first_conflict.validate().unwrap();
    second_conflict.validate().unwrap();
    assert_eq!(incremental_end.delivered_count, 5);
    incremental_end.validate_against(incremental_begin).unwrap();

    let controls: Vec<ControlFixtureRow> =
        read_jsonl(&fixture_root().join("valid/control-frames.jsonl"));
    let created = controls
        .iter()
        .filter(|row| row.action == WorkspaceConflictCreated)
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 4);
    for row in created {
        assert_eq!(row.flow, WorkspaceFlow::ServerPush, "{}", row.case);
        let data = raw_data(&row.frame).expect("ConflictCreated fixture has data");
        let object = serde_json::from_str::<Value>(data.get())
            .unwrap()
            .as_object()
            .expect("ConflictCreated data is an object")
            .clone();
        for (field, value) in [
            (
                "streamId",
                Value::String("10000000-0000-4000-8000-000000000003".to_owned()),
            ),
            ("index", Value::from(0)),
        ] {
            assert!(!object.contains_key(field), "{} has {field}", row.case);
            let mut injected = object.clone();
            injected.insert(field.to_owned(), value);
            assert!(
                decode_data(
                    WorkspaceConflictCreated,
                    WorkspaceFlow::ServerPush,
                    &serde_json::to_vec(&Value::Object(injected)).unwrap(),
                )
                .is_err(),
                "{} accepted injected {field}",
                row.case,
            );
        }
    }
}

#[test]
fn manifest_and_rows_qualifies_required_nullable_paths() {
    use WorkspaceAction::{WorkspaceBlobNeed, WorkspaceConflictCreated, WorkspaceMutation};
    use WorkspaceFlow::{ClientRequest, ServerPush};

    assert!(!required_nullable_path(
        WorkspaceMutation,
        ClientRequest,
        &key_path("operationId")
    ));
    assert!(!required_nullable_path(
        WorkspaceMutation,
        ClientRequest,
        &key_path("path")
    ));
    assert!(!required_nullable_path(
        WorkspaceMutation,
        ClientRequest,
        &key_path("metadata.size")
    ));
    assert!(required_nullable_path(
        WorkspaceBlobNeed,
        ClientRequest,
        &key_path("operationId")
    ));
    assert!(required_nullable_path(
        WorkspaceBlobNeed,
        ClientRequest,
        &key_path("size")
    ));
    assert!(required_nullable_path(
        WorkspaceConflictCreated,
        ServerPush,
        &key_path("ancestor.path")
    ));
    assert!(required_nullable_path(
        WorkspaceConflictCreated,
        ServerPush,
        &key_path("current.contentHash")
    ));
    assert!(!required_nullable_path(
        WorkspaceConflictCreated,
        ServerPush,
        &key_path("path")
    ));

    let controls: Vec<ControlFixtureRow> =
        read_jsonl(&fixture_root().join("valid/control-frames.jsonl"));
    let fixture_data = |case: &str| {
        let row = controls
            .iter()
            .find(|row| row.case == case)
            .unwrap_or_else(|| panic!("missing fixture case {case}"));
        let data = raw_data(&row.frame).expect("control fixture has data");
        (
            row.action,
            row.flow,
            serde_json::from_str::<Value>(data.get()).unwrap(),
        )
    };

    let (action, flow, mutation) = fixture_data("mutation-request");
    for path in ["operationId", "path", "metadata.size"] {
        let mut nulled = mutation.clone();
        set_json_null(&mut nulled, &key_path(path));
        assert!(
            decode_data(action, flow, &serde_json::to_vec(&nulled).unwrap()).is_err(),
            "Mutation {path} accepted null"
        );
    }

    let (action, flow, blob_need) = fixture_data("blob-need-download-request-required-null");
    decode_data(action, flow, &serde_json::to_vec(&blob_need).unwrap())
        .expect("BlobNeed download request accepts exact operationId/size nulls");

    let (action, flow, conflict) = fixture_data("conflict-created-content-push");
    for path in ["ancestor.path", "current.contentHash"] {
        let mut nulled = conflict.clone();
        set_json_null(&mut nulled, &key_path(path));
        decode_data(action, flow, &serde_json::to_vec(&nulled).unwrap())
            .unwrap_or_else(|error| panic!("conflict side {path} rejected null: {error}"));
    }
    let mut nulled_root = conflict;
    set_json_null(&mut nulled_root, &key_path("path"));
    assert!(
        decode_data(action, flow, &serde_json::to_vec(&nulled_root).unwrap()).is_err(),
        "ConflictCreated root path accepted null"
    );
}

#[test]
fn manifest_and_rows() {
    let expected_files = BTreeMap::from([
        (
            "binary/header-vectors.json".to_owned(),
            "cf7459afdcfa1c15094ef7c73ab7bc95ff23bb353013dbf32ea24e1bd34ac0b2".to_owned(),
        ),
        (
            "invalid/hashes.jsonl".to_owned(),
            "d166aabf61c48cb6cd578eef07606a59661a261e550340bf4012b1ce170582c9".to_owned(),
        ),
        (
            "invalid/paths.jsonl".to_owned(),
            "2355070c84bd5fad4a88471428345ab53f4195a0ce09289a187a1ab169c846ae".to_owned(),
        ),
        (
            "invalid/revisions.jsonl".to_owned(),
            "f31b2410f77e0df8ccdc7a95346704fd440ae3d0e1b7cc5de72164bf988478fb".to_owned(),
        ),
        (
            "valid/control-frames.jsonl".to_owned(),
            "37d388e43d10865d29421ba3e2b4f2a6cf2f8d33cf1cb6d016533545e86df85d".to_owned(),
        ),
        (
            "valid/error-envelopes.jsonl".to_owned(),
            "d67bc4bfde9e3122c41897215b64f5cc96acfbb301f69d2e6095c22c0dc7b2fd".to_owned(),
        ),
    ]);

    let root = fixture_root();
    let manifest_bytes = fs::read(root.join("manifest.json")).unwrap();
    let manifest: FixtureManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest.schema_version, "workspace-sync-v2-fixtures/1");
    assert_eq!(manifest.actions, WorkspaceAction::ALL);
    assert_eq!(manifest.files, expected_files);
    assert_eq!(sha256_hex(&manifest_bytes), MANIFEST_SHA256);
    let source_sha =
        fs::read_to_string(root.parent().unwrap().join("SOURCE_MANIFEST_SHA256")).unwrap();
    assert_eq!(source_sha, format!("{MANIFEST_SHA256}\n"));
    for (relative, expected_sha) in &manifest.files {
        assert_eq!(
            sha256_hex(&fs::read(root.join(relative)).unwrap()),
            *expected_sha,
            "{relative}"
        );
    }

    let controls: Vec<ControlFixtureRow> = read_jsonl(&root.join("valid/control-frames.jsonl"));
    let errors: Vec<ErrorFixtureRow> = read_jsonl(&root.join("valid/error-envelopes.jsonl"));
    let invalid_revisions: Vec<InvalidFixtureRow> =
        read_jsonl(&root.join("invalid/revisions.jsonl"));
    let invalid_hashes: Vec<InvalidFixtureRow> = read_jsonl(&root.join("invalid/hashes.jsonl"));
    let invalid_paths: Vec<InvalidFixtureRow> = read_jsonl(&root.join("invalid/paths.jsonl"));
    assert_eq!(controls.len(), 51);
    assert_eq!(errors.len(), 24);
    assert_eq!(invalid_revisions.len(), 5);
    assert_eq!(invalid_hashes.len(), 5);
    assert_eq!(invalid_paths.len(), 19);

    let mut replayed = Vec::with_capacity(controls.len());
    let mut covered = BTreeSet::new();
    let mut canonical = BTreeMap::new();
    for row in controls {
        let (wire_action, _) = row.frame.split_once('|').expect("fixture frame has a pipe");
        assert_eq!(wire_action, row.action.as_str(), "{}", row.case);
        let decoded = match row.flow {
            WorkspaceFlow::ClientRequest => {
                decode_text_frame(row.frame.as_bytes(), WorkspaceFlow::ClientRequest)
            }
            WorkspaceFlow::ServerResponse | WorkspaceFlow::ServerPush => {
                decode_server_text_frame(row.frame.as_bytes())
            }
        }
        .unwrap_or_else(|error| panic!("{}: {error}", row.case));
        assert_eq!(decoded.action, row.action, "{}", row.case);
        assert_eq!(decoded.flow, row.flow, "{}", row.case);

        let encoded = if let Some(raw) = raw_data(&row.frame) {
            let registry_body = decode_data(row.action, row.flow, raw.get().as_bytes())
                .unwrap_or_else(|error| panic!("{} registry decode: {error}", row.case));
            registry_body
                .validate()
                .unwrap_or_else(|error| panic!("{} body validation: {error}", row.case));
            assert_eq!(Some(&registry_body), decoded_body(&decoded), "{}", row.case);
            let key = (row.action, row.flow, registry_body.kind());
            covered.insert(key);
            canonical.entry(key).or_insert_with(|| {
                (
                    row.case.clone(),
                    serde_json::from_str::<Value>(raw.get()).unwrap(),
                )
            });
            match &decoded.envelope {
                DecodedEnvelope::Request { request_id, body } => {
                    encode_request(row.action, *request_id, body.clone())
                }
                DecodedEnvelope::Success { request_id, body } => {
                    encode_success(row.action, row.flow, *request_id, body.clone())
                }
                DecodedEnvelope::Failure { .. } => unreachable!("data fixture is not a failure"),
            }
            .unwrap()
        } else {
            let DecodedEnvelope::Failure { request_id, error } = &decoded.envelope else {
                panic!("{} has no data but is not a failure", row.case);
            };
            error
                .validate()
                .unwrap_or_else(|validation| panic!("{}: {validation}", row.case));
            encode_failure(row.action, *request_id, error.clone()).unwrap()
        };
        assert_eq!(encoded, row.frame.as_bytes(), "{}", row.case);
        replayed.push((row, decoded));
    }

    let registry = ACTION_FLOW_SPECS
        .iter()
        .map(|spec| (spec.action, spec.flow, spec.body_kind))
        .collect::<BTreeSet<_>>();
    assert_eq!(covered, registry);
    assert_eq!(canonical.len(), 25);

    for ((action, flow, _), (case, data)) in canonical {
        let mut paths = Vec::new();
        collect_object_key_paths(&data, &mut Vec::new(), &mut paths);
        for path in paths {
            let mut omitted = data.clone();
            remove_json_key(&mut omitted, &path);
            let omitted = serde_json::to_vec(&omitted).unwrap();
            if optional_non_null_key(&path) {
                decode_data(action, flow, &omitted)
                    .unwrap_or_else(|error| panic!("{case} optional omission: {error}"));
            } else {
                assert!(
                    decode_data(action, flow, &omitted).is_err(),
                    "{case} required omission was accepted"
                );
            }

            let mut nulled = data.clone();
            set_json_null(&mut nulled, &path);
            let nulled = serde_json::to_vec(&nulled).unwrap();
            if required_nullable_path(action, flow, &path) {
                decode_data(action, flow, &nulled)
                    .unwrap_or_else(|error| panic!("{case} nullable path rejected null: {error}"));
            } else {
                assert!(
                    decode_data(action, flow, &nulled).is_err(),
                    "{case} non-nullable path accepted null"
                );
            }
        }
    }

    let mut seen_error_codes = BTreeSet::new();
    for row in errors {
        let decoded = decode_server_text_frame(row.frame.as_bytes())
            .unwrap_or_else(|error| panic!("{}: {error}", row.case));
        assert_eq!(decoded.action, row.action, "{}", row.case);
        assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse, "{}", row.case);
        let DecodedEnvelope::Failure { request_id, error } = decoded.envelope else {
            panic!("{} is not a failure", row.case);
        };
        assert_eq!(row.case.replace('-', "_"), error.code.as_str());
        assert_eq!(error.message, error.code.message());
        assert_eq!(error.retryable, error.code.retryable());
        if error.code == WorkspaceV2ErrorCode::BlobRequired {
            assert!(!error.retryable);
        }
        assert!(seen_error_codes.insert(error.code));
        assert_eq!(
            encode_failure(row.action, request_id, error).unwrap(),
            row.frame.as_bytes(),
            "{}",
            row.case
        );
    }
    assert_eq!(
        seen_error_codes,
        WorkspaceV2ErrorCode::ALL.into_iter().collect()
    );

    for row in &invalid_revisions {
        assert_validation_error(
            WorkspaceRevision::decode_json(row.value.get().as_bytes()),
            row,
        );
    }
    for row in &invalid_hashes {
        assert_validation_error(
            WorkspaceContentHash::decode_json(row.value.get().as_bytes()),
            row,
        );
    }
    for row in &invalid_paths {
        assert_validation_error(WorkspacePath::decode_json(row.value.get().as_bytes()), row);
    }

    let mut sequences: BTreeMap<&str, Vec<(u32, &ControlFixtureRow, &DecodedFrame)>> =
        BTreeMap::new();
    for (row, decoded) in &replayed {
        match (&row.sequence, row.step) {
            (Some(sequence), Some(step)) => {
                sequences
                    .entry(sequence.as_str())
                    .or_default()
                    .push((step, row, decoded));
            }
            (None, None) => {}
            _ => panic!("{} has only one sequence coordinate", row.case),
        }
    }
    for rows in sequences.values_mut() {
        rows.sort_by_key(|(step, _, _)| *step);
    }
    let expected_steps = BTreeMap::from([
        ("merged-conflict-reconnect-missing", 3_u32),
        ("merged-conflict-stale", 2_u32),
        ("merged-conflict-upload", 9_u32),
        ("snapshot-full-conflicts", 4_u32),
        ("snapshot-incremental-conflicts", 7_u32),
    ]);
    assert_eq!(
        sequences
            .iter()
            .map(|(name, rows)| (*name, rows.len() as u32))
            .collect::<BTreeMap<_, _>>(),
        expected_steps
    );
    for rows in sequences.values() {
        assert_eq!(
            rows.iter().map(|(step, _, _)| *step).collect::<Vec<_>>(),
            (1..=rows.len() as u32).collect::<Vec<_>>()
        );
        for window in rows.windows(2) {
            let (_, request_row, request) = window[0];
            if request_row.flow == WorkspaceFlow::ClientRequest {
                let (_, response_row, response) = window[1];
                assert_eq!(response_row.flow, WorkspaceFlow::ServerResponse);
                assert_eq!(response_row.action, request_row.action);
                assert_eq!(decoded_request_id(response), decoded_request_id(request));
            }
        }
    }

    let by_case = |case: &str| {
        replayed
            .iter()
            .find(|(row, _)| row.case == case)
            .unwrap_or_else(|| panic!("missing fixture case {case}"))
    };
    let (subscribe_row, subscribe) = by_case("subscribe-request");
    let (snapshot_row, snapshot) = by_case("snapshot-begin-push");
    assert_eq!(subscribe_row.action, WorkspaceAction::WorkspaceSubscribe);
    assert_eq!(snapshot_row.action, WorkspaceAction::WorkspaceSnapshotBegin);
    assert!(decoded_request_id(subscribe).is_some());
    assert_eq!(decoded_request_id(snapshot), None);
    let (mutation_row, mutation) = by_case("mutation-request");
    let (accepted_row, accepted) = by_case("mutation-accepted-response");
    assert_ne!(mutation_row.action, accepted_row.action);
    assert_eq!(decoded_request_id(mutation), decoded_request_id(accepted));

    for sequence_name in [
        "merged-conflict-upload",
        "merged-conflict-reconnect-missing",
    ] {
        let rows = &sequences[sequence_name];
        let blob_need_index = rows
            .iter()
            .position(|(_, row, _)| row.action == WorkspaceAction::WorkspaceBlobNeed)
            .expect("blob-retry sequence has BlobNeed");
        let (_, _, preceding) = rows[blob_need_index - 1];
        assert!(matches!(
            preceding.envelope,
            DecodedEnvelope::Failure {
                ref error,
                ..
            } if error.code == WorkspaceV2ErrorCode::BlobRequired
        ));
        let (_, _, retry) = rows[0];
        let (_, _, blob_need) = rows[blob_need_index];
        let Some(MessageBody::ConflictResolvedRequest(retry_body)) = decoded_body(retry) else {
            panic!("{sequence_name} starts with a conflict resolution request");
        };
        let Some(MessageBody::BlobNeedUploadPush(blob_need_body)) = decoded_body(blob_need) else {
            panic!("{sequence_name} contains an upload BlobNeed");
        };
        assert_eq!(blob_need_body.operation_id, retry_body.operation_id);
        assert_eq!(
            retry_body.content_hash.as_ref(),
            RequiredNullable::Value(&blob_need_body.content_hash)
        );
    }

    let (_, initial) = by_case("merged-upload-resolve-initial");
    let retries = [
        "merged-upload-resolve-retry",
        "merged-reconnect-resolve-retry",
        "merged-stale-resolve-retry",
    ];
    for retry_case in retries {
        let (_, retry) = by_case(retry_case);
        assert_ne!(decoded_request_id(initial), decoded_request_id(retry));
        assert_eq!(
            serde_json::to_vec(decoded_body(initial).unwrap()).unwrap(),
            serde_json::to_vec(decoded_body(retry).unwrap()).unwrap(),
            "{retry_case}"
        );
    }
    let stale = sequences["merged-conflict-stale"]
        .last()
        .expect("stale sequence is nonempty")
        .2;
    assert!(matches!(
        stale.envelope,
        DecodedEnvelope::Failure {
            ref error,
            ..
        } if error.code == WorkspaceV2ErrorCode::ConflictRevisionStale
    ));
}
