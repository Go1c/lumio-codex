use std::collections::BTreeSet;

use fns_protocol::{
    ACTION_FLOW_SPECS, ClientId, ConflictId, DecodedEnvelope, MAX_ACTION_BYTES, MAX_BLOB_BYTES,
    MAX_CONTROL_FRAME_BYTES, MessageBody, MessageBodyKind, OperationId, ProtocolDecodeError,
    ProtocolEncodeError, RequestId, RequiredNullable, StreamId, TransferId, WorkspaceAckRequest,
    WorkspaceAction, WorkspaceBlobBeginMessage, WorkspaceBlobDirection, WorkspaceBlobEndMessage,
    WorkspaceBlobNeedDownloadRequest, WorkspaceBlobNeedDownloadResponse,
    WorkspaceBlobNeedUploadPush, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest,
    WorkspaceConflictSide, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage,
    WorkspaceFileMetadata, WorkspaceFlow, WorkspaceHelloRequest, WorkspaceHelloResponse,
    WorkspaceId, WorkspaceMutation, WorkspaceMutationAcceptedMessage, WorkspaceMutationKind,
    WorkspaceMutationRejectReason, WorkspaceMutationRejectedMessage, WorkspacePath,
    WorkspacePathState, WorkspaceRevision, WorkspaceSnapshotBeginMessage,
    WorkspaceSnapshotEndMessage, WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode,
    WorkspaceSubscribeRequest, WorkspaceV2Error, WorkspaceV2ErrorCode, WorkspaceV2FieldError,
    WorkspaceValidationError, decode_data, decode_server_text_frame, decode_text_frame,
    encode_failure, encode_request, encode_success, encode_unknown_action_failure, strict_json,
};
use serde_json::Value;

const WORKSPACE_ID: &str = "10000000-0000-4000-8000-000000000002";
const CLIENT_ID: &str = "10000000-0000-4000-8000-000000000001";
const OPERATION_ID: &str = "10000000-0000-4000-8000-000000000004";
const REQUEST_ID: &str = "10000000-0000-4000-8000-000000000006";
const STREAM_ID: &str = "10000000-0000-4000-8000-000000000003";
const TRANSFER_ID: &str = "10000000-0000-4000-8000-000000000009";
const CONFLICT_ID: &str = "10000000-0000-4000-8000-000000000005";
const HASH: &str = "blake3:abababababababababababababababababababababababababababababababab";

const HELLO_REQUEST_JSON: &str = r#"{"protocolVersion":"2","clientId":"10000000-0000-4000-8000-000000000001","clientVersion":"1.0.0","capabilities":["binary_chunks","conflicts","snapshot_v1"]}"#;
const HELLO_RESPONSE_JSON: &str = r#"{"protocolVersion":"2","serverVersion":"2.0.0","maxControlFrameBytes":65536,"maxBinaryChunkBytes":1048576,"maxBlobBytes":5368709120,"maxTransfersPerConnection":4,"heartbeatSeconds":25}"#;
const SUBSCRIBE_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","lastAckRevision":"0"}"#;
const SNAPSHOT_BEGIN_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","mode":"snapshot","fromRevision":"0","finalRevision":"1","entryCount":1,"eventCount":0}"#;
const SNAPSHOT_ENTRY_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","index":0,"entry":{"path":"notes/café.md","pathRevision":"1","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}"#;
const SNAPSHOT_END_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","mode":"snapshot","deliveredCount":1,"finalRevision":"1"}"#;
const MUTATION_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"0","kind":"upsert_file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false}}"#;
const MUTATION_ACCEPTED_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","revision":"1","pathState":{"path":"notes/a.md","pathRevision":"1","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}"#;
const MUTATION_REJECTED_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","reason":"operation_reused","currentPathState":null,"conflictId":null,"requiredHash":null}"#;
const EVENT_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","index":1,"revision":"2","operationId":"10000000-0000-4000-8000-000000000004","originClientId":"10000000-0000-4000-8000-000000000001","mutation":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"1","kind":"upsert_file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false}},"pathState":{"path":"notes/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}"#;
const ACK_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","revision":"1"}"#;
const BLOB_NEED_DOWNLOAD_REQUEST_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","direction":"download","operationId":null,"contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":null}"#;
const BLOB_NEED_DOWNLOAD_RESPONSE_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","direction":"download","operationId":null,"contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":0}"#;
const BLOB_NEED_UPLOAD_PUSH_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","direction":"upload","operationId":"10000000-0000-4000-8000-000000000004","contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":0}"#;
const BLOB_BEGIN_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","transferId":"10000000-0000-4000-8000-000000000009","direction":"upload","contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":0,"chunkSize":1048576,"chunkCount":0}"#;
const BLOB_END_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","transferId":"10000000-0000-4000-8000-000000000009","direction":"download","contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":7,"chunkCount":1}"#;
const CONFLICT_CREATED_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","conflictId":"10000000-0000-4000-8000-000000000005","conflictRevision":"7","path":"notes/a.md","kind":"content","ancestor":{"path":"notes/a.md","pathRevision":"3","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"current":{"path":"notes/a.md","pathRevision":"6","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"incoming":{"path":"notes/a.md","pathRevision":"5","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"createdByOperationId":"10000000-0000-4000-8000-000000000004"}"#;
const CONFLICT_RESOLVED_REQUEST_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","conflictId":"10000000-0000-4000-8000-000000000005","conflictRevision":"7","choice":"merged","path":"notes/a.md","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":8,"modifiedAtMs":2,"executable":false}}"#;
const CONFLICT_RESOLVED_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","conflictId":"10000000-0000-4000-8000-000000000005","conflictRevision":"7","operationId":"10000000-0000-4000-8000-000000000004","revision":"8","choice":"merged","pathState":{"path":"notes/a.md","pathRevision":"8","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":8,"modifiedAtMs":2,"executable":false},"tombstone":false},"resolvedByClientId":"10000000-0000-4000-8000-000000000001"}"#;

const MUTATION_SCHEMA_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"1","kind":"rename","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"newPath":"archive/a.md","targetBasePathRevision":"0"}"#;
const MUTATION_ACCEPTED_SCHEMA_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","revision":"2","pathState":{"path":"archive/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"oldPathState":{"path":"notes/a.md","pathRevision":"2","kind":"tombstone","contentHash":null,"metadata":{"size":0,"modifiedAtMs":0,"executable":false},"tombstone":true},"newPathState":{"path":"archive/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}"#;
const MUTATION_REJECTED_SCHEMA_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","reason":"stale_base_revision","currentPathState":{"path":"notes/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"conflictId":null,"requiredHash":null}"#;
const EVENT_SCHEMA_JSON: &str = r#"{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","index":1,"revision":"2","operationId":"10000000-0000-4000-8000-000000000004","originClientId":"10000000-0000-4000-8000-000000000001","mutation":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000001","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"1","kind":"rename","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"newPath":"archive/a.md","targetBasePathRevision":"0"},"pathState":{"path":"archive/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"oldPathState":{"path":"notes/a.md","pathRevision":"2","kind":"tombstone","contentHash":null,"metadata":{"size":0,"modifiedAtMs":0,"executable":false},"tombstone":true},"newPathState":{"path":"archive/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}"#;

const HASH_JSON: &str =
    r#""blake3:abababababababababababababababababababababababababababababababab""#;
const OPERATION_ID_JSON: &str = r#""10000000-0000-4000-8000-000000000004""#;
const CONFLICT_ID_JSON: &str = r#""10000000-0000-4000-8000-000000000005""#;
const PATH_JSON: &str = r#""notes/a.md""#;
const SIZE_JSON: &str = "7";
const PATH_STATE_JSON: &str = r#"{"path":"notes/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WirePresence {
    RequiredNonNull,
    RequiredNullable { representative: &'static str },
    OptionalNonNull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FieldRule {
    path: &'static str,
    presence: WirePresence,
}

const fn required_non_null(path: &'static str) -> FieldRule {
    FieldRule {
        path,
        presence: WirePresence::RequiredNonNull,
    }
}

const fn required_nullable(path: &'static str, representative: &'static str) -> FieldRule {
    FieldRule {
        path,
        presence: WirePresence::RequiredNullable { representative },
    }
}

const fn optional_non_null(path: &'static str) -> FieldRule {
    FieldRule {
        path,
        presence: WirePresence::OptionalNonNull,
    }
}

#[derive(Clone, Copy)]
struct BodySchemaCase<'a> {
    name: &'static str,
    action: &'static str,
    flow: WorkspaceFlow,
    kind: MessageBodyKind,
    canonical: &'static str,
    fields: &'a [FieldRule],
}

fn for_each_body_schema_case(mut visit: impl FnMut(BodySchemaCase<'_>)) {
    use MessageBodyKind as Kind;
    use WorkspaceFlow::{ClientRequest, ServerPush, ServerResponse};

    let cases = [
        BodySchemaCase {
            name: "hello-request",
            action: "WorkspaceHello",
            flow: ClientRequest,
            kind: Kind::HelloRequest,
            canonical: HELLO_REQUEST_JSON,
            fields: &[
                required_non_null("protocolVersion"),
                required_non_null("clientId"),
                required_non_null("clientVersion"),
                required_non_null("capabilities"),
            ],
        },
        BodySchemaCase {
            name: "hello-response",
            action: "WorkspaceHello",
            flow: ServerResponse,
            kind: Kind::HelloResponse,
            canonical: HELLO_RESPONSE_JSON,
            fields: &[
                required_non_null("protocolVersion"),
                required_non_null("serverVersion"),
                required_non_null("maxControlFrameBytes"),
                required_non_null("maxBinaryChunkBytes"),
                required_non_null("maxBlobBytes"),
                required_non_null("maxTransfersPerConnection"),
                required_non_null("heartbeatSeconds"),
            ],
        },
        BodySchemaCase {
            name: "subscribe-request",
            action: "WorkspaceSubscribe",
            flow: ClientRequest,
            kind: Kind::SubscribeRequest,
            canonical: SUBSCRIBE_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("lastAckRevision"),
            ],
        },
        BodySchemaCase {
            name: "snapshot-begin",
            action: "WorkspaceSnapshotBegin",
            flow: ServerPush,
            kind: Kind::SnapshotBegin,
            canonical: SNAPSHOT_BEGIN_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("streamId"),
                required_non_null("mode"),
                required_non_null("fromRevision"),
                required_non_null("finalRevision"),
                required_non_null("entryCount"),
                required_non_null("eventCount"),
            ],
        },
        BodySchemaCase {
            name: "snapshot-entry",
            action: "WorkspaceSnapshotEntry",
            flow: ServerPush,
            kind: Kind::SnapshotEntry,
            canonical: SNAPSHOT_ENTRY_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("streamId"),
                required_non_null("index"),
                required_non_null("entry"),
                required_non_null("entry.path"),
                required_non_null("entry.pathRevision"),
                required_non_null("entry.kind"),
                required_nullable("entry.contentHash", HASH_JSON),
                required_non_null("entry.metadata"),
                required_non_null("entry.metadata.size"),
                required_non_null("entry.metadata.modifiedAtMs"),
                required_non_null("entry.metadata.executable"),
                required_non_null("entry.tombstone"),
            ],
        },
        BodySchemaCase {
            name: "snapshot-end",
            action: "WorkspaceSnapshotEnd",
            flow: ServerPush,
            kind: Kind::SnapshotEnd,
            canonical: SNAPSHOT_END_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("streamId"),
                required_non_null("mode"),
                required_non_null("deliveredCount"),
                required_non_null("finalRevision"),
            ],
        },
        BodySchemaCase {
            name: "mutation",
            action: "WorkspaceMutation",
            flow: ClientRequest,
            kind: Kind::Mutation,
            canonical: MUTATION_SCHEMA_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("operationId"),
                required_non_null("path"),
                required_non_null("basePathRevision"),
                required_non_null("kind"),
                required_nullable("contentHash", HASH_JSON),
                required_non_null("metadata"),
                required_non_null("metadata.size"),
                required_non_null("metadata.modifiedAtMs"),
                required_non_null("metadata.executable"),
                optional_non_null("newPath"),
                optional_non_null("targetBasePathRevision"),
            ],
        },
        BodySchemaCase {
            name: "mutation-accepted",
            action: "WorkspaceMutationAccepted",
            flow: ServerResponse,
            kind: Kind::MutationAccepted,
            canonical: MUTATION_ACCEPTED_SCHEMA_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("operationId"),
                required_non_null("revision"),
                required_non_null("pathState"),
                required_non_null("pathState.path"),
                required_non_null("pathState.pathRevision"),
                required_non_null("pathState.kind"),
                required_nullable("pathState.contentHash", HASH_JSON),
                required_non_null("pathState.metadata"),
                required_non_null("pathState.metadata.size"),
                required_non_null("pathState.metadata.modifiedAtMs"),
                required_non_null("pathState.metadata.executable"),
                required_non_null("pathState.tombstone"),
                optional_non_null("oldPathState"),
                required_non_null("oldPathState.path"),
                required_non_null("oldPathState.pathRevision"),
                required_non_null("oldPathState.kind"),
                required_nullable("oldPathState.contentHash", HASH_JSON),
                required_non_null("oldPathState.metadata"),
                required_non_null("oldPathState.metadata.size"),
                required_non_null("oldPathState.metadata.modifiedAtMs"),
                required_non_null("oldPathState.metadata.executable"),
                required_non_null("oldPathState.tombstone"),
                optional_non_null("newPathState"),
                required_non_null("newPathState.path"),
                required_non_null("newPathState.pathRevision"),
                required_non_null("newPathState.kind"),
                required_nullable("newPathState.contentHash", HASH_JSON),
                required_non_null("newPathState.metadata"),
                required_non_null("newPathState.metadata.size"),
                required_non_null("newPathState.metadata.modifiedAtMs"),
                required_non_null("newPathState.metadata.executable"),
                required_non_null("newPathState.tombstone"),
            ],
        },
        BodySchemaCase {
            name: "mutation-rejected",
            action: "WorkspaceMutationRejected",
            flow: ServerResponse,
            kind: Kind::MutationRejected,
            canonical: MUTATION_REJECTED_SCHEMA_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("operationId"),
                required_non_null("reason"),
                required_nullable("currentPathState", PATH_STATE_JSON),
                required_non_null("currentPathState.path"),
                required_non_null("currentPathState.pathRevision"),
                required_non_null("currentPathState.kind"),
                required_nullable("currentPathState.contentHash", HASH_JSON),
                required_non_null("currentPathState.metadata"),
                required_non_null("currentPathState.metadata.size"),
                required_non_null("currentPathState.metadata.modifiedAtMs"),
                required_non_null("currentPathState.metadata.executable"),
                required_non_null("currentPathState.tombstone"),
                required_nullable("conflictId", CONFLICT_ID_JSON),
                required_nullable("requiredHash", HASH_JSON),
            ],
        },
        BodySchemaCase {
            name: "event",
            action: "WorkspaceEvent",
            flow: ServerPush,
            kind: Kind::Event,
            canonical: EVENT_SCHEMA_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("streamId"),
                required_non_null("index"),
                required_non_null("revision"),
                required_non_null("operationId"),
                required_non_null("originClientId"),
                required_non_null("mutation"),
                required_non_null("mutation.workspaceId"),
                required_non_null("mutation.clientId"),
                required_non_null("mutation.operationId"),
                required_non_null("mutation.path"),
                required_non_null("mutation.basePathRevision"),
                required_non_null("mutation.kind"),
                required_nullable("mutation.contentHash", HASH_JSON),
                required_non_null("mutation.metadata"),
                required_non_null("mutation.metadata.size"),
                required_non_null("mutation.metadata.modifiedAtMs"),
                required_non_null("mutation.metadata.executable"),
                optional_non_null("mutation.newPath"),
                optional_non_null("mutation.targetBasePathRevision"),
                required_non_null("pathState"),
                required_non_null("pathState.path"),
                required_non_null("pathState.pathRevision"),
                required_non_null("pathState.kind"),
                required_nullable("pathState.contentHash", HASH_JSON),
                required_non_null("pathState.metadata"),
                required_non_null("pathState.metadata.size"),
                required_non_null("pathState.metadata.modifiedAtMs"),
                required_non_null("pathState.metadata.executable"),
                required_non_null("pathState.tombstone"),
                optional_non_null("oldPathState"),
                required_non_null("oldPathState.path"),
                required_non_null("oldPathState.pathRevision"),
                required_non_null("oldPathState.kind"),
                required_nullable("oldPathState.contentHash", HASH_JSON),
                required_non_null("oldPathState.metadata"),
                required_non_null("oldPathState.metadata.size"),
                required_non_null("oldPathState.metadata.modifiedAtMs"),
                required_non_null("oldPathState.metadata.executable"),
                required_non_null("oldPathState.tombstone"),
                optional_non_null("newPathState"),
                required_non_null("newPathState.path"),
                required_non_null("newPathState.pathRevision"),
                required_non_null("newPathState.kind"),
                required_nullable("newPathState.contentHash", HASH_JSON),
                required_non_null("newPathState.metadata"),
                required_non_null("newPathState.metadata.size"),
                required_non_null("newPathState.metadata.modifiedAtMs"),
                required_non_null("newPathState.metadata.executable"),
                required_non_null("newPathState.tombstone"),
            ],
        },
        BodySchemaCase {
            name: "ack",
            action: "WorkspaceAck",
            flow: ClientRequest,
            kind: Kind::Ack,
            canonical: ACK_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("revision"),
            ],
        },
        BodySchemaCase {
            name: "blob-need-download-request",
            action: "WorkspaceBlobNeed",
            flow: ClientRequest,
            kind: Kind::BlobNeedDownloadRequest,
            canonical: BLOB_NEED_DOWNLOAD_REQUEST_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("direction"),
                required_nullable("operationId", OPERATION_ID_JSON),
                required_non_null("contentHash"),
                required_nullable("size", SIZE_JSON),
            ],
        },
        BodySchemaCase {
            name: "blob-need-download-response",
            action: "WorkspaceBlobNeed",
            flow: ServerResponse,
            kind: Kind::BlobNeedDownloadResponse,
            canonical: BLOB_NEED_DOWNLOAD_RESPONSE_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("direction"),
                required_nullable("operationId", OPERATION_ID_JSON),
                required_non_null("contentHash"),
                required_non_null("size"),
            ],
        },
        BodySchemaCase {
            name: "blob-need-upload-push",
            action: "WorkspaceBlobNeed",
            flow: ServerPush,
            kind: Kind::BlobNeedUploadPush,
            canonical: BLOB_NEED_UPLOAD_PUSH_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("direction"),
                required_non_null("operationId"),
                required_non_null("contentHash"),
                required_non_null("size"),
            ],
        },
        BodySchemaCase {
            name: "blob-begin",
            action: "WorkspaceBlobBegin",
            flow: ClientRequest,
            kind: Kind::BlobBegin,
            canonical: BLOB_BEGIN_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("transferId"),
                required_non_null("direction"),
                required_non_null("contentHash"),
                required_non_null("size"),
                required_non_null("chunkSize"),
                required_non_null("chunkCount"),
            ],
        },
        BodySchemaCase {
            name: "blob-end",
            action: "WorkspaceBlobEnd",
            flow: ClientRequest,
            kind: Kind::BlobEnd,
            canonical: BLOB_END_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("transferId"),
                required_non_null("direction"),
                required_non_null("contentHash"),
                required_non_null("size"),
                required_non_null("chunkCount"),
            ],
        },
        BodySchemaCase {
            name: "conflict-created",
            action: "WorkspaceConflictCreated",
            flow: ServerPush,
            kind: Kind::ConflictCreated,
            canonical: CONFLICT_CREATED_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("conflictId"),
                required_non_null("conflictRevision"),
                required_non_null("path"),
                required_non_null("kind"),
                required_non_null("ancestor"),
                required_nullable("ancestor.path", PATH_JSON),
                required_non_null("ancestor.pathRevision"),
                required_nullable("ancestor.contentHash", HASH_JSON),
                required_non_null("ancestor.metadata"),
                required_non_null("ancestor.metadata.size"),
                required_non_null("ancestor.metadata.modifiedAtMs"),
                required_non_null("ancestor.metadata.executable"),
                required_non_null("ancestor.tombstone"),
                required_non_null("current"),
                required_nullable("current.path", PATH_JSON),
                required_non_null("current.pathRevision"),
                required_nullable("current.contentHash", HASH_JSON),
                required_non_null("current.metadata"),
                required_non_null("current.metadata.size"),
                required_non_null("current.metadata.modifiedAtMs"),
                required_non_null("current.metadata.executable"),
                required_non_null("current.tombstone"),
                required_non_null("incoming"),
                required_nullable("incoming.path", PATH_JSON),
                required_non_null("incoming.pathRevision"),
                required_nullable("incoming.contentHash", HASH_JSON),
                required_non_null("incoming.metadata"),
                required_non_null("incoming.metadata.size"),
                required_non_null("incoming.metadata.modifiedAtMs"),
                required_non_null("incoming.metadata.executable"),
                required_non_null("incoming.tombstone"),
                required_non_null("createdByOperationId"),
            ],
        },
        BodySchemaCase {
            name: "conflict-resolved-request",
            action: "WorkspaceConflictResolved",
            flow: ClientRequest,
            kind: Kind::ConflictResolvedRequest,
            canonical: CONFLICT_RESOLVED_REQUEST_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("clientId"),
                required_non_null("operationId"),
                required_non_null("conflictId"),
                required_non_null("conflictRevision"),
                required_non_null("choice"),
                required_non_null("path"),
                required_nullable("contentHash", HASH_JSON),
                required_non_null("metadata"),
                required_non_null("metadata.size"),
                required_non_null("metadata.modifiedAtMs"),
                required_non_null("metadata.executable"),
            ],
        },
        BodySchemaCase {
            name: "conflict-resolved",
            action: "WorkspaceConflictResolved",
            flow: ServerResponse,
            kind: Kind::ConflictResolved,
            canonical: CONFLICT_RESOLVED_JSON,
            fields: &[
                required_non_null("workspaceId"),
                required_non_null("conflictId"),
                required_non_null("conflictRevision"),
                required_non_null("operationId"),
                required_non_null("revision"),
                required_non_null("choice"),
                required_non_null("pathState"),
                required_non_null("pathState.path"),
                required_non_null("pathState.pathRevision"),
                required_non_null("pathState.kind"),
                required_nullable("pathState.contentHash", HASH_JSON),
                required_non_null("pathState.metadata"),
                required_non_null("pathState.metadata.size"),
                required_non_null("pathState.metadata.modifiedAtMs"),
                required_non_null("pathState.metadata.executable"),
                required_non_null("pathState.tombstone"),
                required_non_null("resolvedByClientId"),
            ],
        },
    ];
    assert_eq!(cases.len(), 19);
    for case in cases {
        visit(case);
    }
}

#[derive(Clone, Copy)]
struct LegalRow {
    action: &'static str,
    flow: WorkspaceFlow,
    kind: MessageBodyKind,
    data: &'static str,
}

fn legal_rows() -> [LegalRow; 25] {
    use MessageBodyKind as Kind;
    use WorkspaceFlow::{ClientRequest, ServerPush, ServerResponse};

    [
        LegalRow {
            action: "WorkspaceHello",
            flow: ClientRequest,
            kind: Kind::HelloRequest,
            data: HELLO_REQUEST_JSON,
        },
        LegalRow {
            action: "WorkspaceHello",
            flow: ServerResponse,
            kind: Kind::HelloResponse,
            data: HELLO_RESPONSE_JSON,
        },
        LegalRow {
            action: "WorkspaceSubscribe",
            flow: ClientRequest,
            kind: Kind::SubscribeRequest,
            data: SUBSCRIBE_JSON,
        },
        LegalRow {
            action: "WorkspaceSnapshotBegin",
            flow: ServerPush,
            kind: Kind::SnapshotBegin,
            data: SNAPSHOT_BEGIN_JSON,
        },
        LegalRow {
            action: "WorkspaceSnapshotEntry",
            flow: ServerPush,
            kind: Kind::SnapshotEntry,
            data: SNAPSHOT_ENTRY_JSON,
        },
        LegalRow {
            action: "WorkspaceSnapshotEnd",
            flow: ServerPush,
            kind: Kind::SnapshotEnd,
            data: SNAPSHOT_END_JSON,
        },
        LegalRow {
            action: "WorkspaceMutation",
            flow: ClientRequest,
            kind: Kind::Mutation,
            data: MUTATION_JSON,
        },
        LegalRow {
            action: "WorkspaceMutationAccepted",
            flow: ServerResponse,
            kind: Kind::MutationAccepted,
            data: MUTATION_ACCEPTED_JSON,
        },
        LegalRow {
            action: "WorkspaceMutationRejected",
            flow: ServerResponse,
            kind: Kind::MutationRejected,
            data: MUTATION_REJECTED_JSON,
        },
        LegalRow {
            action: "WorkspaceEvent",
            flow: ServerPush,
            kind: Kind::Event,
            data: EVENT_JSON,
        },
        LegalRow {
            action: "WorkspaceAck",
            flow: ClientRequest,
            kind: Kind::Ack,
            data: ACK_JSON,
        },
        LegalRow {
            action: "WorkspaceAck",
            flow: ServerResponse,
            kind: Kind::Ack,
            data: ACK_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobNeed",
            flow: ClientRequest,
            kind: Kind::BlobNeedDownloadRequest,
            data: BLOB_NEED_DOWNLOAD_REQUEST_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobNeed",
            flow: ServerResponse,
            kind: Kind::BlobNeedDownloadResponse,
            data: BLOB_NEED_DOWNLOAD_RESPONSE_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobNeed",
            flow: ServerPush,
            kind: Kind::BlobNeedUploadPush,
            data: BLOB_NEED_UPLOAD_PUSH_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobBegin",
            flow: ClientRequest,
            kind: Kind::BlobBegin,
            data: BLOB_BEGIN_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobBegin",
            flow: ServerResponse,
            kind: Kind::BlobBegin,
            data: BLOB_BEGIN_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobBegin",
            flow: ServerPush,
            kind: Kind::BlobBegin,
            data: BLOB_BEGIN_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobEnd",
            flow: ClientRequest,
            kind: Kind::BlobEnd,
            data: BLOB_END_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobEnd",
            flow: ServerResponse,
            kind: Kind::BlobEnd,
            data: BLOB_END_JSON,
        },
        LegalRow {
            action: "WorkspaceBlobEnd",
            flow: ServerPush,
            kind: Kind::BlobEnd,
            data: BLOB_END_JSON,
        },
        LegalRow {
            action: "WorkspaceConflictCreated",
            flow: ServerPush,
            kind: Kind::ConflictCreated,
            data: CONFLICT_CREATED_JSON,
        },
        LegalRow {
            action: "WorkspaceConflictResolved",
            flow: ClientRequest,
            kind: Kind::ConflictResolvedRequest,
            data: CONFLICT_RESOLVED_REQUEST_JSON,
        },
        LegalRow {
            action: "WorkspaceConflictResolved",
            flow: ServerResponse,
            kind: Kind::ConflictResolved,
            data: CONFLICT_RESOLVED_JSON,
        },
        LegalRow {
            action: "WorkspaceConflictResolved",
            flow: ServerPush,
            kind: Kind::ConflictResolved,
            data: CONFLICT_RESOLVED_JSON,
        },
    ]
}

fn action(token: &str) -> WorkspaceAction {
    token.parse().expect("registered action")
}

fn request_id() -> RequestId {
    RequestId::parse(REQUEST_ID).unwrap()
}

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

fn assert_decode_error<T>(result: Result<T, ProtocolDecodeError>, field: &str, reason: &str) {
    match result {
        Ok(_) => panic!("expected {field}: {reason}"),
        Err(error) => {
            assert_eq!(error.field, field);
            assert_eq!(error.reason, reason);
        }
    }
}

fn assert_encode_error<T>(result: Result<T, ProtocolEncodeError>, field: &str, reason: &str) {
    match result {
        Ok(_) => panic!("expected {field}: {reason}"),
        Err(error) => {
            assert_eq!(error.field, field);
            assert_eq!(error.reason, reason);
        }
    }
}

#[test]
fn registry_has_the_exact_ordered_actions_and_25_declared_rows() {
    let expected = [
        "WorkspaceHello",
        "WorkspaceSubscribe",
        "WorkspaceSnapshotBegin",
        "WorkspaceSnapshotEntry",
        "WorkspaceSnapshotEnd",
        "WorkspaceMutation",
        "WorkspaceMutationAccepted",
        "WorkspaceMutationRejected",
        "WorkspaceEvent",
        "WorkspaceAck",
        "WorkspaceBlobNeed",
        "WorkspaceBlobBegin",
        "WorkspaceBlobEnd",
        "WorkspaceConflictCreated",
        "WorkspaceConflictResolved",
    ];
    assert_eq!(WorkspaceAction::ALL.len(), 15);
    assert_eq!(WorkspaceAction::ALL.map(WorkspaceAction::as_str), expected);
    assert_eq!(ACTION_FLOW_SPECS.len(), 25);

    let expected_rows = legal_rows()
        .map(|row| (action(row.action), row.flow, row.kind))
        .into_iter()
        .collect::<BTreeSet<_>>();
    let actual_rows = ACTION_FLOW_SPECS
        .iter()
        .map(|spec| (spec.action, spec.flow, spec.body_kind))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_rows, expected_rows);
}

#[test]
fn registry_decodes_every_legal_body_and_rejects_every_undeclared_pair() {
    let legal = legal_rows();
    for row in legal {
        let body = decode_data(action(row.action), row.flow, row.data.as_bytes()).unwrap();
        assert_eq!(body.kind(), row.kind, "{}/{}", row.action, row.flow);
        assert_eq!(serde_json::to_string(&body).unwrap(), row.data);
        body.validate().unwrap();
    }

    for registered_action in WorkspaceAction::ALL {
        for flow in WorkspaceFlow::ALL {
            let declared = legal
                .iter()
                .any(|row| action(row.action) == registered_action && row.flow == flow);
            if !declared {
                assert_decode_error(
                    decode_data(registered_action, flow, b"{}"),
                    "flow",
                    "flow_not_allowed",
                );
            }
        }
    }
}

#[test]
fn action_parsing_is_closed_and_unknown_tokens_never_become_variants() {
    for registered in WorkspaceAction::ALL {
        let token = registered.as_str();
        assert_eq!(token.parse::<WorkspaceAction>().unwrap(), registered);
        assert_eq!(
            serde_json::to_string(&registered).unwrap(),
            format!("\"{token}\"")
        );
        assert_eq!(
            serde_json::from_str::<WorkspaceAction>(&format!("\"{token}\"")).unwrap(),
            registered
        );
    }

    assert_validation_error(
        "WorkspaceFuture1".parse::<WorkspaceAction>(),
        "action",
        "unknown_action",
    );
    assert!(serde_json::from_str::<WorkspaceAction>(r#""WorkspaceFuture1""#).is_err());
    assert_decode_error(
        decode_text_frame(
            br#"WorkspaceFuture1|{"requestId":"10000000-0000-4000-8000-000000000006","data":{}}"#,
            WorkspaceFlow::ClientRequest,
        ),
        "action",
        "unknown_action",
    );
}

#[test]
fn text_framing_uses_first_pipe_safe_tokens_and_locked_size_limits() {
    let with_pipe = HELLO_REQUEST_JSON.replace("1.0.0", "1.0|0");
    let frame = format!("WorkspaceHello|{{\"requestId\":\"{REQUEST_ID}\",\"data\":{with_pipe}}}");
    let decoded = decode_text_frame(frame.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
    match decoded.envelope {
        DecodedEnvelope::Request {
            body: MessageBody::HelloRequest(body),
            ..
        } => {
            assert_eq!(body.client_version, "1.0|0");
        }
        other => panic!("unexpected envelope: {other:?}"),
    }

    assert_decode_error(
        decode_text_frame(b"WorkspaceHello", WorkspaceFlow::ClientRequest),
        "frame",
        "missing_separator",
    );
    assert_decode_error(
        decode_text_frame(b"\xff|{}", WorkspaceFlow::ClientRequest),
        "action",
        "invalid_utf8",
    );
    assert_decode_error(
        decode_text_frame(b"Workspace-Hello|{}", WorkspaceFlow::ClientRequest),
        "action",
        "invalid_token",
    );

    let max_safe_unknown = format!("{}|{{}}", "A".repeat(MAX_ACTION_BYTES));
    assert_decode_error(
        decode_text_frame(max_safe_unknown.as_bytes(), WorkspaceFlow::ClientRequest),
        "action",
        "unknown_action",
    );
    let too_long_action = format!("{}|{{}}", "A".repeat(MAX_ACTION_BYTES + 1));
    assert_decode_error(
        decode_text_frame(too_long_action.as_bytes(), WorkspaceFlow::ClientRequest),
        "action",
        "invalid_token",
    );

    let prefix = format!(
        "WorkspaceHello|{{\"requestId\":\"{REQUEST_ID}\",\"data\":{{\"protocolVersion\":\"2\",\"clientId\":\"{CLIENT_ID}\",\"clientVersion\":\""
    );
    let suffix = "\",\"capabilities\":[\"binary_chunks\",\"conflicts\",\"snapshot_v1\"]}}";
    let padding = "x".repeat(MAX_CONTROL_FRAME_BYTES - prefix.len() - suffix.len());
    let maximum = format!("{prefix}{padding}{suffix}");
    assert_eq!(maximum.len(), MAX_CONTROL_FRAME_BYTES);
    decode_text_frame(maximum.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();

    let oversized = format!("{maximum}x");
    assert_decode_error(
        decode_text_frame(oversized.as_bytes(), WorkspaceFlow::ClientRequest),
        "frame",
        "too_large",
    );
}

#[test]
fn envelopes_enforce_request_success_failure_and_push_presence() {
    for raw in [
        r#"WorkspaceHello|{"data":{}}"#,
        &format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}"}}"#),
        &format!(
            r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":true,"data":{HELLO_REQUEST_JSON}}}"#
        ),
        &format!(r#"WorkspaceHello|{{"requestId":null,"data":{HELLO_REQUEST_JSON}}}"#),
    ] {
        assert!(decode_text_frame(raw.as_bytes(), WorkspaceFlow::ClientRequest).is_err());
    }

    for raw in [
        format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":true}}"#),
        format!(
            r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":true,"data":{HELLO_RESPONSE_JSON},"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
        ),
        format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":false}}"#),
        format!(
            r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":false,"data":{HELLO_RESPONSE_JSON},"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
        ),
    ] {
        assert!(decode_text_frame(raw.as_bytes(), WorkspaceFlow::ServerResponse).is_err());
    }

    let push = format!(r#"WorkspaceSnapshotBegin|{{"status":true,"data":{SNAPSHOT_BEGIN_JSON}}}"#);
    let decoded = decode_text_frame(push.as_bytes(), WorkspaceFlow::ServerPush).unwrap();
    assert!(matches!(
        decoded.envelope,
        DecodedEnvelope::Success {
            request_id: None,
            ..
        }
    ));
    for raw in [
        format!(r#"WorkspaceSnapshotBegin|{{"requestId":"{REQUEST_ID}","status":true,"data":{SNAPSHOT_BEGIN_JSON}}}"#),
        r#"WorkspaceSnapshotBegin|{"status":false,"error":{"code":"invalid_request","message":"invalid request","retryable":false}}"#.to_owned(),
    ] {
        assert!(decode_text_frame(raw.as_bytes(), WorkspaceFlow::ServerPush).is_err());
    }
}

#[test]
fn server_decoder_infers_subscribe_stream_and_same_action_failure_once() {
    assert!(!ACTION_FLOW_SPECS.iter().any(|spec| {
        spec.action == WorkspaceAction::WorkspaceSubscribe
            && spec.flow == WorkspaceFlow::ServerResponse
    }));
    let request =
        format!(r#"WorkspaceSubscribe|{{"requestId":"{REQUEST_ID}","data":{SUBSCRIBE_JSON}}}"#);
    let decoded = decode_text_frame(request.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
    assert_eq!(decoded.action, WorkspaceAction::WorkspaceSubscribe);
    assert_eq!(decoded.flow, WorkspaceFlow::ClientRequest);
    assert!(matches!(
        decoded.envelope,
        DecodedEnvelope::Request {
            request_id: actual_request_id,
            body: MessageBody::SubscribeRequest(_),
        } if actual_request_id == request_id()
    ));

    let continuation =
        format!(r#"WorkspaceSnapshotBegin|{{"status":true,"data":{SNAPSHOT_BEGIN_JSON}}}"#);
    let decoded = decode_server_text_frame(continuation.as_bytes()).unwrap();
    assert_eq!(decoded.action, WorkspaceAction::WorkspaceSnapshotBegin);
    assert_eq!(decoded.flow, WorkspaceFlow::ServerPush);
    assert!(matches!(
        decoded.envelope,
        DecodedEnvelope::Success {
            request_id: None,
            body: MessageBody::SnapshotBegin(_),
        }
    ));

    let failure = format!(
        r#"WorkspaceSubscribe|{{"requestId":"{REQUEST_ID}","status":false,"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
    );
    let decoded = decode_server_text_frame(failure.as_bytes()).unwrap();
    assert_eq!(decoded.action, WorkspaceAction::WorkspaceSubscribe);
    assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse);
    assert!(matches!(
        decoded.envelope,
        DecodedEnvelope::Failure {
            request_id: Some(actual_request_id),
            error,
        } if actual_request_id == request_id()
            && error.code == WorkspaceV2ErrorCode::InvalidRequest
    ));
}

#[test]
fn server_decoder_correlates_mutation_results_across_distinct_actions() {
    for (action, data, expected_kind) in [
        (
            WorkspaceAction::WorkspaceMutationAccepted,
            MUTATION_ACCEPTED_JSON,
            MessageBodyKind::MutationAccepted,
        ),
        (
            WorkspaceAction::WorkspaceMutationRejected,
            MUTATION_REJECTED_JSON,
            MessageBodyKind::MutationRejected,
        ),
    ] {
        assert_ne!(action, WorkspaceAction::WorkspaceMutation);
        let frame = format!(
            r#"{}|{{"requestId":"{REQUEST_ID}","status":true,"data":{data}}}"#,
            action.as_str()
        );
        let decoded = decode_server_text_frame(frame.as_bytes()).unwrap();
        assert_eq!(decoded.action, action);
        assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse);
        assert!(matches!(
            decoded.envelope,
            DecodedEnvelope::Success {
                request_id: Some(actual_request_id),
                body,
            } if actual_request_id == request_id() && body.kind() == expected_kind
        ));
    }
}

#[test]
fn server_decoder_keeps_mutation_protocol_failure_on_request_action() {
    let frame = format!(
        r#"WorkspaceMutation|{{"requestId":"{REQUEST_ID}","status":false,"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
    );
    let decoded = decode_server_text_frame(frame.as_bytes()).unwrap();
    assert_eq!(decoded.action, WorkspaceAction::WorkspaceMutation);
    assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse);
    assert!(matches!(
        decoded.envelope,
        DecodedEnvelope::Failure {
            request_id: Some(actual_request_id),
            error,
        } if actual_request_id == request_id()
            && error.code == WorkspaceV2ErrorCode::InvalidRequest
    ));
}

#[test]
fn server_decoder_infers_all_dual_response_push_actions_once() {
    let rows = [
        (
            WorkspaceAction::WorkspaceBlobNeed,
            BLOB_NEED_DOWNLOAD_RESPONSE_JSON,
            BLOB_NEED_UPLOAD_PUSH_JSON,
            MessageBodyKind::BlobNeedDownloadResponse,
            MessageBodyKind::BlobNeedUploadPush,
        ),
        (
            WorkspaceAction::WorkspaceBlobBegin,
            BLOB_BEGIN_JSON,
            BLOB_BEGIN_JSON,
            MessageBodyKind::BlobBegin,
            MessageBodyKind::BlobBegin,
        ),
        (
            WorkspaceAction::WorkspaceBlobEnd,
            BLOB_END_JSON,
            BLOB_END_JSON,
            MessageBodyKind::BlobEnd,
            MessageBodyKind::BlobEnd,
        ),
        (
            WorkspaceAction::WorkspaceConflictResolved,
            CONFLICT_RESOLVED_JSON,
            CONFLICT_RESOLVED_JSON,
            MessageBodyKind::ConflictResolved,
            MessageBodyKind::ConflictResolved,
        ),
    ];

    for (action, response_data, push_data, response_kind, push_kind) in rows {
        let response = format!(
            r#"{}|{{"requestId":"{REQUEST_ID}","status":true,"data":{response_data}}}"#,
            action.as_str()
        );
        let decoded = decode_server_text_frame(response.as_bytes()).unwrap();
        assert_eq!(decoded.action, action);
        assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse);
        assert!(matches!(
            decoded.envelope,
            DecodedEnvelope::Success {
                request_id: Some(actual_request_id),
                body,
            } if actual_request_id == request_id() && body.kind() == response_kind
        ));

        let push = format!(
            r#"{}|{{"status":true,"data":{push_data}}}"#,
            action.as_str()
        );
        let decoded = decode_server_text_frame(push.as_bytes()).unwrap();
        assert_eq!(decoded.action, action);
        assert_eq!(decoded.flow, WorkspaceFlow::ServerPush);
        assert!(matches!(
            decoded.envelope,
            DecodedEnvelope::Success {
                request_id: None,
                body,
            } if body.kind() == push_kind
        ));
    }
}

#[test]
fn server_decoder_rejects_missing_status_and_illegal_envelopes() {
    for (request_id_member, expected_request_id) in [
        (
            format!(r#""requestId":"{REQUEST_ID}","#),
            Some(request_id()),
        ),
        (String::new(), None),
    ] {
        let frame = format!(
            r#"WorkspaceSubscribe|{{{request_id_member}"status":false,"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
        );
        let decoded = decode_server_text_frame(frame.as_bytes()).unwrap();
        assert_eq!(decoded.action, WorkspaceAction::WorkspaceSubscribe);
        assert_eq!(decoded.flow, WorkspaceFlow::ServerResponse);
        assert!(matches!(
            decoded.envelope,
            DecodedEnvelope::Failure {
                request_id,
                error,
            } if request_id == expected_request_id
                && error.code == WorkspaceV2ErrorCode::InvalidRequest
        ));
    }

    let invalid = [
        format!(r#"WorkspaceSnapshotBegin|{{"data":{SNAPSHOT_BEGIN_JSON}}}"#),
        format!(r#"WorkspaceSnapshotBegin|{{"status":null,"data":{SNAPSHOT_BEGIN_JSON}}}"#),
        format!(
            r#"WorkspaceHello|{{"requestId":null,"status":true,"data":{HELLO_RESPONSE_JSON}}}"#
        ),
        format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":true}}"#),
        format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":false}}"#),
        format!(
            r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","status":false,"data":{HELLO_RESPONSE_JSON},"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
        ),
        format!(
            r#"WorkspaceSnapshotBegin|{{"requestId":"{REQUEST_ID}","status":true,"data":{SNAPSHOT_BEGIN_JSON}}}"#
        ),
    ];
    for frame in invalid {
        assert!(
            decode_server_text_frame(frame.as_bytes()).is_err(),
            "{frame}"
        );
    }
}

#[test]
fn encoders_are_registry_checked_and_preserve_failure_edge_cases() {
    let hello_request = decode_data(
        action("WorkspaceHello"),
        WorkspaceFlow::ClientRequest,
        HELLO_REQUEST_JSON.as_bytes(),
    )
    .unwrap();
    let request = encode_request(action("WorkspaceHello"), request_id(), hello_request).unwrap();
    assert_eq!(
        String::from_utf8(request).unwrap(),
        format!(r#"WorkspaceHello|{{"requestId":"{REQUEST_ID}","data":{HELLO_REQUEST_JSON}}}"#)
    );

    let begin = decode_data(
        action("WorkspaceSnapshotBegin"),
        WorkspaceFlow::ServerPush,
        SNAPSHOT_BEGIN_JSON.as_bytes(),
    )
    .unwrap();
    assert_encode_error(
        encode_success(
            action("WorkspaceSnapshotBegin"),
            WorkspaceFlow::ServerResponse,
            Some(request_id()),
            begin.clone(),
        ),
        "flow",
        "flow_not_allowed",
    );
    let failure = WorkspaceV2Error::new(WorkspaceV2ErrorCode::InvalidRequest, vec![]);
    let encoded = encode_failure(
        action("WorkspaceSnapshotBegin"),
        Some(request_id()),
        failure,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(encoded).unwrap(),
        format!(
            r#"WorkspaceSnapshotBegin|{{"requestId":"{REQUEST_ID}","status":false,"error":{{"code":"invalid_request","message":"invalid request","retryable":false}}}}"#
        )
    );

    let ack = decode_data(
        action("WorkspaceAck"),
        WorkspaceFlow::ServerResponse,
        ACK_JSON.as_bytes(),
    )
    .unwrap();
    assert_encode_error(
        encode_success(
            action("WorkspaceHello"),
            WorkspaceFlow::ServerResponse,
            Some(request_id()),
            ack,
        ),
        "data",
        "type_mismatch",
    );
    assert_encode_error(
        encode_success(
            action("WorkspaceSnapshotBegin"),
            WorkspaceFlow::ServerPush,
            Some(request_id()),
            begin,
        ),
        "requestId",
        "forbidden_for_push",
    );
}

#[test]
fn safe_unknown_action_failure_is_the_only_unknown_echo_path() {
    let encoded = encode_unknown_action_failure("WorkspaceFuture1", Some(request_id())).unwrap();
    assert_eq!(
        String::from_utf8(encoded).unwrap(),
        format!(
            r#"WorkspaceFuture1|{{"requestId":"{REQUEST_ID}","status":false,"error":{{"code":"unknown_action","message":"unknown workspace action","retryable":false}}}}"#
        )
    );
    assert_eq!(WorkspaceAction::ALL.len(), 15);
    assert!(
        WorkspaceAction::ALL
            .iter()
            .all(|item| item.as_str() != "WorkspaceFuture1")
    );

    for unsafe_token in [
        "",
        "1Workspace",
        "Workspace-Future",
        "WorkspaceFuture|injected",
        "WorkspaceFuturé",
    ] {
        assert_encode_error(
            encode_unknown_action_failure(unsafe_token, None),
            "action",
            "invalid_token",
        );
    }
    assert_encode_error(
        encode_unknown_action_failure("WorkspaceHello", None),
        "action",
        "registered_action",
    );
}

#[test]
fn all_24_errors_have_exact_messages_retryability_and_field_presence() {
    let expected = [
        ("invalid_frame", "invalid control frame", false),
        ("invalid_json", "invalid JSON payload", false),
        ("unknown_action", "unknown workspace action", false),
        ("unauthenticated", "authentication required", false),
        ("forbidden", "workspace access forbidden", false),
        ("invalid_request", "invalid request", false),
        ("invalid_revision", "invalid workspace revision", false),
        ("invalid_hash", "invalid content hash", false),
        (
            "invalid_path",
            "path must be a canonical workspace-relative POSIX path",
            false,
        ),
        ("workspace_not_found", "workspace not found", false),
        (
            "workspace_limit_exceeded",
            "workspace limit exceeded",
            false,
        ),
        ("client_not_registered", "client not registered", false),
        ("stale_base_revision", "base revision is stale", false),
        ("operation_reused", "operation identifier was reused", false),
        ("blob_required", "blob upload required", false),
        ("blob_not_found", "blob not found", false),
        ("blob_hash_mismatch", "blob hash mismatch", false),
        ("blob_size_mismatch", "blob size mismatch", false),
        (
            "blob_transfer_out_of_order",
            "blob transfer is out of order",
            false,
        ),
        ("blob_limit_exceeded", "blob transfer limit exceeded", false),
        ("conflict_not_found", "conflict not found", false),
        (
            "conflict_revision_stale",
            "conflict revision is stale",
            false,
        ),
        ("server_busy", "server is busy", true),
        ("internal", "internal server error", true),
    ];
    assert_eq!(WorkspaceV2ErrorCode::ALL.len(), 24);
    for (index, (wire, message, retryable)) in expected.into_iter().enumerate() {
        let code = WorkspaceV2ErrorCode::ALL[index];
        assert_eq!(code.as_str(), wire);
        let error = WorkspaceV2Error::new(code, vec![]);
        assert_eq!(error.message, message);
        assert_eq!(error.retryable, retryable);
        assert_eq!(
            serde_json::to_string(&error).unwrap(),
            format!(r#"{{"code":"{wire}","message":"{message}","retryable":{retryable}}}"#)
        );
    }

    let with_field = WorkspaceV2Error::new(
        WorkspaceV2ErrorCode::InvalidPath,
        vec![WorkspaceV2FieldError {
            field: "data.path".to_owned(),
            reason: "invalid_segment".to_owned(),
        }],
    );
    assert_eq!(
        serde_json::to_string(&with_field).unwrap(),
        r#"{"code":"invalid_path","message":"path must be a canonical workspace-relative POSIX path","retryable":false,"fields":[{"field":"data.path","reason":"invalid_segment"}]}"#
    );
    assert!(strict_json::from_slice::<WorkspaceV2Error>(br#"{"code":"invalid_path","message":"path must be a canonical workspace-relative POSIX path","retryable":false,"fields":null}"#).is_err());
}

#[test]
fn every_body_round_trips_exact_keys_and_rejects_unknown_missing_and_null() {
    let mut unique = BTreeSet::new();
    for row in legal_rows() {
        if !unique.insert(row.kind) {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(row.data).unwrap();
        let object = parsed.as_object().unwrap();
        let first_key = object.keys().next().unwrap().clone();

        let mut unknown = parsed.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), Value::Bool(true));
        assert_decode_error(
            decode_data(
                action(row.action),
                row.flow,
                &serde_json::to_vec(&unknown).unwrap(),
            ),
            "data",
            "invalid_json",
        );

        let mut missing = parsed.clone();
        missing.as_object_mut().unwrap().remove(&first_key);
        assert_decode_error(
            decode_data(
                action(row.action),
                row.flow,
                &serde_json::to_vec(&missing).unwrap(),
            ),
            "data",
            "invalid_json",
        );

        let mut explicit_null = parsed;
        explicit_null
            .as_object_mut()
            .unwrap()
            .insert(first_key, Value::Null);
        assert_decode_error(
            decode_data(
                action(row.action),
                row.flow,
                &serde_json::to_vec(&explicit_null).unwrap(),
            ),
            "data",
            "invalid_json",
        );
    }

    let rename = valid_rename_mutation();
    let encoded = serde_json::to_value(&rename).unwrap();
    for optional in ["newPath", "targetBasePathRevision"] {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(optional);
        strict_json::from_slice::<WorkspaceMutation>(&serde_json::to_vec(&missing).unwrap())
            .unwrap();

        let mut null = encoded.clone();
        null.as_object_mut()
            .unwrap()
            .insert(optional.to_owned(), Value::Null);
        assert!(
            strict_json::from_slice::<WorkspaceMutation>(&serde_json::to_vec(&null).unwrap())
                .is_err()
        );
    }
}

#[test]
fn literal_body_schema_oracle_enforces_every_recursive_field_contract() {
    let mut case_count = 0;
    let mut covered_kinds = BTreeSet::new();
    for_each_body_schema_case(|case| {
        case_count += 1;
        assert!(
            covered_kinds.insert(case.kind),
            "{} duplicates body kind {:?}",
            case.name,
            case.kind,
        );
        let canonical = serde_json::from_str::<Value>(case.canonical).unwrap();
        let expected_paths = case
            .fields
            .iter()
            .map(|field| field.path.to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            expected_paths.len(),
            case.fields.len(),
            "{} has duplicate literal field rules",
            case.name,
        );
        assert_eq!(
            json_field_paths(&canonical),
            expected_paths,
            "{} literal schema does not cover its complete canonical object",
            case.name,
        );

        let body = decode_schema_case(&case, &canonical).unwrap();
        assert_eq!(body.kind(), case.kind, "{} body kind", case.name);
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            case.canonical,
            "{} canonical round trip",
            case.name,
        );

        for rule in case.fields {
            match rule.presence {
                WirePresence::RequiredNonNull => {
                    let mut missing = canonical.clone();
                    remove_json_path(&mut missing, rule.path);
                    assert!(
                        decode_schema_case(&case, &missing).is_err(),
                        "{} {} accepted omission despite required-non-null",
                        case.name,
                        rule.path,
                    );

                    let mut explicit_null = canonical.clone();
                    set_json_path(&mut explicit_null, rule.path, Value::Null);
                    assert!(
                        decode_schema_case(&case, &explicit_null).is_err(),
                        "{} {} accepted null despite required-non-null",
                        case.name,
                        rule.path,
                    );
                }
                WirePresence::RequiredNullable { representative } => {
                    let mut missing = canonical.clone();
                    remove_json_path(&mut missing, rule.path);
                    assert!(
                        decode_schema_case(&case, &missing).is_err(),
                        "{} {} accepted omission despite required-nullable",
                        case.name,
                        rule.path,
                    );

                    let mut explicit_null = canonical.clone();
                    set_json_path(&mut explicit_null, rule.path, Value::Null);
                    decode_schema_case(&case, &explicit_null).unwrap_or_else(|error| {
                        panic!(
                            "{} {} rejected null despite required-nullable: {error}",
                            case.name, rule.path,
                        )
                    });

                    let representative = serde_json::from_str(representative).unwrap();
                    let mut with_value = canonical.clone();
                    set_json_path(&mut with_value, rule.path, representative);
                    decode_schema_case(&case, &with_value).unwrap_or_else(|error| {
                        panic!(
                            "{} {} rejected representative value despite required-nullable: {error}",
                            case.name, rule.path,
                        )
                    });
                }
                WirePresence::OptionalNonNull => {
                    let mut missing = canonical.clone();
                    remove_json_path(&mut missing, rule.path);
                    decode_schema_case(&case, &missing).unwrap_or_else(|error| {
                        panic!(
                            "{} {} rejected omission despite optional-non-null: {error}",
                            case.name, rule.path,
                        )
                    });

                    let mut explicit_null = canonical.clone();
                    set_json_path(&mut explicit_null, rule.path, Value::Null);
                    assert!(
                        decode_schema_case(&case, &explicit_null).is_err(),
                        "{} {} accepted null despite optional-non-null",
                        case.name,
                        rule.path,
                    );
                }
            }
        }

        for object_path in json_object_paths(&canonical) {
            let mut unknown = canonical.clone();
            insert_unknown_field(&mut unknown, &object_path);
            assert!(
                decode_schema_case(&case, &unknown).is_err(),
                "{} accepted unknown field in object {object_path:?}",
                case.name,
            );
        }
    });
    assert_eq!(case_count, 19);
    assert_eq!(
        covered_kinds.len(),
        19,
        "the literal oracle must cover each distinct body kind exactly once",
    );
}

fn decode_schema_case(
    case: &BodySchemaCase<'_>,
    value: &Value,
) -> Result<MessageBody, ProtocolDecodeError> {
    decode_data(
        action(case.action),
        case.flow,
        &serde_json::to_vec(value).unwrap(),
    )
}

fn json_field_paths(value: &Value) -> BTreeSet<String> {
    fn visit(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        let Value::Object(object) = value else {
            return;
        };
        for (key, child) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            paths.insert(path.clone());
            visit(child, &path, paths);
        }
    }

    let mut paths = BTreeSet::new();
    visit(value, "", &mut paths);
    paths
}

fn json_object_paths(value: &Value) -> BTreeSet<String> {
    fn visit(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        let Value::Object(object) = value else {
            return;
        };
        paths.insert(prefix.to_owned());
        for (key, child) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            visit(child, &path, paths);
        }
    }

    let mut paths = BTreeSet::new();
    visit(value, "", &mut paths);
    paths
}

fn json_value_at_path_mut<'a>(value: &'a mut Value, path: &[&str]) -> &'a mut Value {
    let mut current = value;
    for segment in path {
        current = current
            .as_object_mut()
            .and_then(|object| object.get_mut(*segment))
            .unwrap_or_else(|| panic!("literal schema path segment {segment:?} is missing"));
    }
    current
}

fn remove_json_path(value: &mut Value, path: &str) {
    let mut segments = path.split('.').collect::<Vec<_>>();
    let field = segments.pop().expect("field path");
    let parent = json_value_at_path_mut(value, &segments);
    assert!(
        parent.as_object_mut().unwrap().remove(field).is_some(),
        "literal schema path {path:?} is missing",
    );
}

fn set_json_path(value: &mut Value, path: &str, replacement: Value) {
    let mut segments = path.split('.').collect::<Vec<_>>();
    let field = segments.pop().expect("field path");
    let parent = json_value_at_path_mut(value, &segments);
    assert!(
        parent
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), replacement)
            .is_some(),
        "literal schema path {path:?} is missing",
    );
}

fn insert_unknown_field(value: &mut Value, object_path: &str) {
    let segments = if object_path.is_empty() {
        Vec::new()
    } else {
        object_path.split('.').collect::<Vec<_>>()
    };
    let object = json_value_at_path_mut(value, &segments)
        .as_object_mut()
        .expect("literal object path");
    assert!(
        object
            .insert("unexpected".to_owned(), Value::Bool(true))
            .is_none(),
    );
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse(WORKSPACE_ID).unwrap()
}
fn client_id() -> ClientId {
    ClientId::parse(CLIENT_ID).unwrap()
}
fn operation_id() -> OperationId {
    OperationId::parse(OPERATION_ID).unwrap()
}
fn stream_id() -> StreamId {
    StreamId::parse(STREAM_ID).unwrap()
}
fn transfer_id() -> TransferId {
    TransferId::parse(TRANSFER_ID).unwrap()
}
fn conflict_id() -> ConflictId {
    ConflictId::parse(CONFLICT_ID).unwrap()
}
fn revision(value: u64) -> WorkspaceRevision {
    WorkspaceRevision::new(value)
}
fn path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).unwrap()
}
fn hash() -> WorkspaceContentHash {
    WorkspaceContentHash::parse(HASH).unwrap()
}
fn metadata(size: u64) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size,
        modified_at_ms: if size == 0 { 0 } else { 1 },
        executable: false,
    }
}
fn live_state(value: &str, rev: u64) -> WorkspacePathState {
    WorkspacePathState {
        path: path(value),
        path_revision: revision(rev),
        kind: WorkspaceEntryKind::File,
        content_hash: RequiredNullable::Value(hash()),
        metadata: metadata(3),
        tombstone: false,
    }
}
fn tombstone_state(value: &str, rev: u64) -> WorkspacePathState {
    WorkspacePathState {
        path: path(value),
        path_revision: revision(rev),
        kind: WorkspaceEntryKind::Tombstone,
        content_hash: RequiredNullable::Null,
        metadata: metadata(0),
        tombstone: true,
    }
}
fn valid_mutation() -> WorkspaceMutation {
    WorkspaceMutation {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: operation_id(),
        path: path("notes/a.md"),
        base_path_revision: revision(1),
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: RequiredNullable::Value(hash()),
        metadata: metadata(3),
        new_path: None,
        target_base_path_revision: None,
    }
}
fn valid_rename_mutation() -> WorkspaceMutation {
    WorkspaceMutation {
        kind: WorkspaceMutationKind::Rename,
        new_path: Some(path("archive/a.md")),
        target_base_path_revision: Some(revision(0)),
        ..valid_mutation()
    }
}

#[test]
fn hello_path_state_and_snapshot_validators_have_exact_boundaries() {
    let hello = WorkspaceHelloRequest {
        protocol_version: "2".to_owned(),
        client_id: client_id(),
        client_version: "1.0.0".to_owned(),
        capabilities: vec![
            "binary_chunks".to_owned(),
            "conflicts".to_owned(),
            "snapshot_v1".to_owned(),
        ],
    };
    hello.validate().unwrap();
    let mut invalid = hello.clone();
    invalid.capabilities.swap(0, 1);
    assert_validation_error(invalid.validate(), "capabilities", "required_set");

    let response = WorkspaceHelloResponse {
        protocol_version: "2".to_owned(),
        server_version: "2.0.0".to_owned(),
        max_control_frame_bytes: 65_536,
        max_binary_chunk_bytes: 1_048_576,
        max_blob_bytes: MAX_BLOB_BYTES,
        max_transfers_per_connection: 4,
        heartbeat_seconds: 25,
    };
    response.validate().unwrap();
    let mut invalid_response = response;
    invalid_response.heartbeat_seconds = 26;
    assert_validation_error(invalid_response.validate(), "hello", "invalid_limits");

    let state = live_state("notes/a.md", 1);
    state.validate().unwrap();
    let mut invalid_state = state.clone();
    invalid_state.tombstone = true;
    assert_validation_error(invalid_state.validate(), "tombstone", "kind_mismatch");

    let begin = WorkspaceSnapshotBeginMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        mode: WorkspaceSnapshotMode::Snapshot,
        from_revision: revision(0),
        final_revision: revision(1),
        entry_count: 1,
        event_count: 0,
    };
    begin.validate().unwrap();
    let mut invalid_begin = begin.clone();
    invalid_begin.event_count = 1;
    assert_validation_error(
        invalid_begin.validate(),
        "eventCount",
        "must_be_zero_for_snapshot",
    );

    let entry = WorkspaceSnapshotEntryMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        index: 0,
        entry: state,
    };
    entry.validate_at(0).unwrap();
    assert_validation_error(entry.validate_at(1), "index", "stream_gap");

    let end = WorkspaceSnapshotEndMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        mode: WorkspaceSnapshotMode::Snapshot,
        delivered_count: 1,
        final_revision: revision(1),
    };
    end.validate_against(&begin).unwrap();
    let mut invalid_end = end;
    invalid_end.delivered_count = 0;
    assert_validation_error(
        invalid_end.validate_against(&begin),
        "deliveredCount",
        "count_mismatch",
    );
}

#[test]
fn mutation_result_event_and_ack_validators_preserve_rename_state() {
    let rename = valid_rename_mutation();
    rename.validate().unwrap();
    let mut child = rename.clone();
    child.new_path = Some(path("notes/a.md/child"));
    assert_validation_error(child.validate(), "newPath", "directory_into_child");

    let old = tombstone_state("notes/a.md", 2);
    let new = live_state("archive/a.md", 2);
    let accepted = WorkspaceMutationAcceptedMessage {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: operation_id(),
        revision: revision(2),
        path_state: new.clone(),
        old_path_state: Some(old.clone()),
        new_path_state: Some(new.clone()),
    };
    accepted.validate().unwrap();
    let mut invalid_accepted = accepted.clone();
    invalid_accepted.path_state.metadata.size = 4;
    assert_validation_error(
        invalid_accepted.validate(),
        "pathState",
        "new_path_state_mismatch",
    );

    let rejected = WorkspaceMutationRejectedMessage {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: operation_id(),
        reason: WorkspaceMutationRejectReason::BlobRequired,
        current_path_state: RequiredNullable::Null,
        conflict_id: RequiredNullable::Null,
        required_hash: RequiredNullable::Value(hash()),
    };
    rejected.validate().unwrap();
    let mut invalid_rejected = rejected;
    invalid_rejected.required_hash = RequiredNullable::Null;
    assert_validation_error(
        invalid_rejected.validate(),
        "requiredHash",
        "required_for_reason",
    );

    let event = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        index: 1,
        revision: revision(2),
        operation_id: operation_id(),
        origin_client_id: client_id(),
        mutation: rename,
        path_state: new.clone(),
        old_path_state: Some(old),
        new_path_state: Some(new),
    };
    event.validate_after(0, revision(1)).unwrap();
    assert_validation_error(event.validate_after(1, revision(1)), "index", "stream_gap");
    let mut overflow = event.clone();
    overflow.index = u32::MAX;
    assert_validation_error(
        overflow.validate_after(u32::MAX, revision(1)),
        "index",
        "stream_gap",
    );

    let ack = WorkspaceAckRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        revision: revision(2),
    };
    ack.validate_between(revision(1), revision(2)).unwrap();
    assert_validation_error(
        ack.validate_between(revision(2), revision(2)),
        "revision",
        "ack_regression",
    );
}

#[test]
fn blob_need_begin_end_validators_preserve_required_null_and_download_ack_data() {
    let request = WorkspaceBlobNeedDownloadRequest {
        workspace_id: workspace_id(),
        direction: WorkspaceBlobDirection::Download,
        operation_id: RequiredNullable::Null,
        content_hash: hash(),
        size: RequiredNullable::Null,
    };
    request.validate().unwrap();
    let mut invalid_request = request;
    invalid_request.operation_id = RequiredNullable::Value(operation_id());
    assert_validation_error(invalid_request.validate(), "operationId", "must_be_null");

    let response = WorkspaceBlobNeedDownloadResponse {
        workspace_id: workspace_id(),
        direction: WorkspaceBlobDirection::Download,
        operation_id: RequiredNullable::Null,
        content_hash: hash(),
        size: 0,
    };
    response.validate().unwrap();
    let mut invalid_response = response;
    invalid_response.operation_id = RequiredNullable::Value(operation_id());
    assert_validation_error(invalid_response.validate(), "operationId", "must_be_null");

    let upload = WorkspaceBlobNeedUploadPush {
        workspace_id: workspace_id(),
        direction: WorkspaceBlobDirection::Upload,
        operation_id: operation_id(),
        content_hash: hash(),
        size: 0,
    };
    upload.validate().unwrap();
    let mut invalid_upload = upload;
    invalid_upload.direction = WorkspaceBlobDirection::Download;
    assert_validation_error(invalid_upload.validate(), "direction", "must_be_upload");

    let begin = WorkspaceBlobBeginMessage {
        workspace_id: workspace_id(),
        transfer_id: transfer_id(),
        direction: WorkspaceBlobDirection::Download,
        content_hash: hash(),
        size: 1_048_577,
        chunk_size: 1_048_576,
        chunk_count: 2,
    };
    begin.validate().unwrap();
    let mut invalid_begin = begin;
    invalid_begin.chunk_count = 1;
    assert_validation_error(
        invalid_begin.validate(),
        "chunkCount",
        "arithmetic_mismatch",
    );

    let download_end = WorkspaceBlobEndMessage {
        workspace_id: workspace_id(),
        transfer_id: transfer_id(),
        direction: WorkspaceBlobDirection::Download,
        content_hash: hash(),
        size: 7,
        chunk_count: 1,
    };
    download_end.validate().unwrap();
    let body = MessageBody::BlobEnd(download_end.clone());
    let request = encode_request(action("WorkspaceBlobEnd"), request_id(), body.clone()).unwrap();
    assert!(
        String::from_utf8(request)
            .unwrap()
            .contains(r#""direction":"download""#)
    );
    encode_success(
        action("WorkspaceBlobEnd"),
        WorkspaceFlow::ServerResponse,
        Some(request_id()),
        body,
    )
    .unwrap();
    let mut invalid_end = download_end;
    invalid_end.chunk_count = 0;
    assert_validation_error(invalid_end.validate(), "chunkCount", "arithmetic_mismatch");
}

fn live_side(value: &str, rev: u64) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Value(path(value)),
        path_revision: revision(rev),
        content_hash: RequiredNullable::Value(hash()),
        metadata: metadata(3),
        tombstone: false,
    }
}
fn tombstone_side(rev: u64) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Null,
        path_revision: revision(rev),
        content_hash: RequiredNullable::Null,
        metadata: metadata(0),
        tombstone: true,
    }
}
fn created_conflict(kind: WorkspaceConflictKind) -> WorkspaceConflictCreatedMessage {
    let mut created = WorkspaceConflictCreatedMessage {
        workspace_id: workspace_id(),
        conflict_id: conflict_id(),
        conflict_revision: revision(7),
        path: path("notes/a.md"),
        kind,
        ancestor: live_side("notes/a.md", 3),
        current: live_side("notes/a.md", 6),
        incoming: live_side("notes/a.md", 5),
        created_by_operation_id: operation_id(),
    };
    match kind {
        WorkspaceConflictKind::DeleteModify => created.incoming = tombstone_side(5),
        WorkspaceConflictKind::Rename => created.incoming = live_side("archive/a.md", 5),
        WorkspaceConflictKind::Content | WorkspaceConflictKind::Binary => {}
    }
    created
}

#[test]
fn conflict_validators_cover_all_four_kinds_and_choices_with_stale_first() {
    for kind in [
        WorkspaceConflictKind::Content,
        WorkspaceConflictKind::DeleteModify,
        WorkspaceConflictKind::Rename,
        WorkspaceConflictKind::Binary,
    ] {
        created_conflict(kind).validate().unwrap();
    }
    let mut invalid_kind = created_conflict(WorkspaceConflictKind::Rename);
    invalid_kind.incoming = invalid_kind.current.clone();
    assert_validation_error(
        invalid_kind.validate(),
        "incoming.path",
        "rename_path_required",
    );

    let created = created_conflict(WorkspaceConflictKind::Content);
    let mut request = WorkspaceConflictResolvedRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        operation_id: operation_id(),
        conflict_id: conflict_id(),
        conflict_revision: revision(7),
        choice: WorkspaceConflictChoice::Current,
        path: path("notes/a.md"),
        content_hash: RequiredNullable::Value(hash()),
        metadata: metadata(3),
    };
    request.validate_against(&created).unwrap();
    request.choice = WorkspaceConflictChoice::Incoming;
    request.validate_against(&created).unwrap();
    request.choice = WorkspaceConflictChoice::Merged;
    request.metadata = WorkspaceFileMetadata {
        size: 8,
        modified_at_ms: 2,
        executable: false,
    };
    request.validate_against(&created).unwrap();
    request.choice = WorkspaceConflictChoice::Delete;
    request.content_hash = RequiredNullable::Null;
    request.metadata = metadata(0);
    request.validate_against(&created).unwrap();

    request.conflict_revision = revision(6);
    request.content_hash = RequiredNullable::Value(hash());
    request.metadata.size = 99;
    assert_validation_error(
        request.validate_against(&created),
        "conflictRevision",
        "conflict_revision_stale",
    );

    let resolved = WorkspaceConflictResolvedMessage {
        workspace_id: workspace_id(),
        conflict_id: conflict_id(),
        conflict_revision: revision(7),
        operation_id: operation_id(),
        revision: revision(8),
        choice: WorkspaceConflictChoice::Merged,
        path_state: live_state("notes/a.md", 8),
        resolved_by_client_id: client_id(),
    };
    resolved.validate().unwrap();
    let mut invalid_resolved = resolved;
    invalid_resolved.path_state.path_revision = revision(7);
    assert_validation_error(
        invalid_resolved.validate(),
        "pathState.pathRevision",
        "revision_mismatch",
    );
}

#[test]
fn subscribe_body_has_an_explicit_validator() {
    let subscribe = WorkspaceSubscribeRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        last_ack_revision: revision(0),
    };
    subscribe.validate().unwrap();
}
