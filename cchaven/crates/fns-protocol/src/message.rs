use serde::{Deserialize, Serialize};

use crate::revision::WorkspaceConflictRevision;
use crate::{
    BLOB_CHUNK_BYTES, ClientId, ConflictId, MAX_BLOB_BYTES, OperationId, RequiredNullable,
    StreamId, TransferId, WorkspaceBlobDirection, WorkspaceConflictChoice, WorkspaceConflictKind,
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspaceId,
    WorkspaceMutationKind, WorkspaceMutationRejectReason, WorkspacePath, WorkspaceRevision,
    WorkspaceSnapshotMode, WorkspaceValidationError, deserialize_optional_non_null,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceHelloRequest {
    pub protocol_version: String,
    pub client_id: ClientId,
    pub client_version: String,
    pub capabilities: Vec<String>,
}

impl WorkspaceHelloRequest {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.protocol_version != "2" {
            return Err(validation_error("protocolVersion", "unsupported"));
        }
        if self.client_version.is_empty() {
            return Err(validation_error("clientVersion", "required"));
        }
        let expected = ["binary_chunks", "conflicts", "snapshot_v1"];
        if self.capabilities.len() != expected.len()
            || self.capabilities.iter().map(String::as_str).ne(expected)
        {
            return Err(validation_error("capabilities", "required_set"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceHelloResponse {
    pub protocol_version: String,
    pub server_version: String,
    pub max_control_frame_bytes: u32,
    pub max_binary_chunk_bytes: u32,
    pub max_blob_bytes: u64,
    pub max_transfers_per_connection: u32,
    pub heartbeat_seconds: u32,
}

impl WorkspaceHelloResponse {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.protocol_version != "2" || self.server_version.is_empty() {
            return Err(validation_error("hello", "invalid_version"));
        }
        if self.max_control_frame_bytes != 65_536
            || self.max_binary_chunk_bytes != BLOB_CHUNK_BYTES
            || self.max_blob_bytes != MAX_BLOB_BYTES
            || self.max_transfers_per_connection != 4
            || self.heartbeat_seconds != 25
        {
            return Err(validation_error("hello", "invalid_limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSubscribeRequest {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub last_ack_revision: WorkspaceRevision,
}

impl WorkspaceSubscribeRequest {
    pub const fn validate(&self) -> Result<(), WorkspaceValidationError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePathState {
    pub path: WorkspacePath,
    pub path_revision: WorkspaceRevision,
    pub kind: WorkspaceEntryKind,
    pub content_hash: RequiredNullable<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
    pub tombstone: bool,
}

impl WorkspacePathState {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.tombstone != (self.kind == WorkspaceEntryKind::Tombstone) {
            return Err(validation_error("tombstone", "kind_mismatch"));
        }
        match self.kind {
            WorkspaceEntryKind::File | WorkspaceEntryKind::Symlink => {
                if self.content_hash.is_null() {
                    return Err(validation_error("contentHash", "required_for_kind"));
                }
            }
            WorkspaceEntryKind::Directory | WorkspaceEntryKind::Tombstone => {
                if !self.content_hash.is_null() {
                    return Err(validation_error("contentHash", "must_be_null_for_kind"));
                }
            }
        }
        self.metadata.validate(self.kind)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSnapshotBeginMessage {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub mode: WorkspaceSnapshotMode,
    pub from_revision: WorkspaceRevision,
    pub final_revision: WorkspaceRevision,
    pub entry_count: u32,
    pub event_count: u32,
    pub conflict_count: u32,
}

impl WorkspaceSnapshotBeginMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.final_revision < self.from_revision {
            return Err(validation_error("finalRevision", "before_from_revision"));
        }
        match self.mode {
            WorkspaceSnapshotMode::Snapshot if self.event_count != 0 => {
                Err(validation_error("eventCount", "must_be_zero_for_snapshot"))
            }
            WorkspaceSnapshotMode::Incremental if self.entry_count != 0 => Err(validation_error(
                "entryCount",
                "must_be_zero_for_incremental",
            )),
            WorkspaceSnapshotMode::Snapshot | WorkspaceSnapshotMode::Incremental => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSnapshotEntryMessage {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub index: u32,
    pub entry: WorkspacePathState,
}

impl WorkspaceSnapshotEntryMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        self.entry.validate()
    }

    pub fn validate_at(&self, expected_index: u32) -> Result<(), WorkspaceValidationError> {
        if self.index != expected_index {
            return Err(validation_error("index", "stream_gap"));
        }
        self.validate()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSnapshotEndMessage {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub mode: WorkspaceSnapshotMode,
    pub delivered_count: u32,
    pub final_revision: WorkspaceRevision,
}

impl WorkspaceSnapshotEndMessage {
    pub const fn validate(&self) -> Result<(), WorkspaceValidationError> {
        Ok(())
    }

    pub fn validate_against(
        &self,
        begin: &WorkspaceSnapshotBeginMessage,
    ) -> Result<(), WorkspaceValidationError> {
        begin.validate()?;
        if self.workspace_id != begin.workspace_id
            || self.stream_id != begin.stream_id
            || self.mode != begin.mode
            || self.final_revision != begin.final_revision
        {
            return Err(validation_error("snapshotEnd", "begin_mismatch"));
        }
        let selected_count = match begin.mode {
            WorkspaceSnapshotMode::Snapshot => begin.entry_count,
            WorkspaceSnapshotMode::Incremental => begin.event_count,
        };
        let expected = selected_count
            .checked_add(begin.conflict_count)
            .ok_or_else(|| validation_error("deliveredCount", "count_overflow"))?;
        if self.delivered_count != expected {
            return Err(validation_error("deliveredCount", "count_mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMutation {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub operation_id: OperationId,
    pub path: WorkspacePath,
    pub base_path_revision: WorkspaceRevision,
    pub kind: WorkspaceMutationKind,
    pub content_hash: RequiredNullable<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub new_path: Option<WorkspacePath>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub target_base_path_revision: Option<WorkspaceRevision>,
}

impl WorkspaceMutation {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.kind == WorkspaceMutationKind::Rename {
            let new_path = self
                .new_path
                .as_ref()
                .ok_or_else(|| validation_error("newPath", "required_for_rename"))?;
            if self.target_base_path_revision.is_none() {
                return Err(validation_error(
                    "targetBasePathRevision",
                    "required_for_rename",
                ));
            }
            if new_path == &self.path {
                return Err(validation_error("newPath", "same_as_path"));
            }
            let descendant_prefix = format!("{}/", self.path.as_str());
            if new_path.as_str().starts_with(&descendant_prefix) {
                return Err(validation_error("newPath", "directory_into_child"));
            }
        } else {
            if self.new_path.is_some() {
                return Err(validation_error("newPath", "forbidden_for_kind"));
            }
            if self.target_base_path_revision.is_some() {
                return Err(validation_error(
                    "targetBasePathRevision",
                    "forbidden_for_kind",
                ));
            }
        }

        match self.kind {
            WorkspaceMutationKind::UpsertFile | WorkspaceMutationKind::UpsertSymlink => {
                if self.content_hash.is_null() {
                    return Err(validation_error("contentHash", "required_for_kind"));
                }
                let entry_kind = if self.kind == WorkspaceMutationKind::UpsertFile {
                    WorkspaceEntryKind::File
                } else {
                    WorkspaceEntryKind::Symlink
                };
                self.metadata.validate(entry_kind)
            }
            WorkspaceMutationKind::Mkdir | WorkspaceMutationKind::Delete => {
                if !self.content_hash.is_null() {
                    return Err(validation_error("contentHash", "must_be_null_for_kind"));
                }
                let entry_kind = if self.kind == WorkspaceMutationKind::Mkdir {
                    WorkspaceEntryKind::Directory
                } else {
                    WorkspaceEntryKind::Tombstone
                };
                self.metadata.validate(entry_kind)
            }
            WorkspaceMutationKind::Rename => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMutationAcceptedMessage {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub operation_id: OperationId,
    pub revision: WorkspaceRevision,
    pub path_state: WorkspacePathState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub old_path_state: Option<WorkspacePathState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub new_path_state: Option<WorkspacePathState>,
}

impl WorkspaceMutationAcceptedMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.revision == WorkspaceRevision::ZERO
            || self.path_state.path_revision != self.revision
        {
            return Err(validation_error("revision", "path_state_mismatch"));
        }
        self.path_state.validate()?;
        if self.old_path_state.is_some() != self.new_path_state.is_some() {
            return Err(validation_error("pathState", "rename_pair_required"));
        }
        if let (Some(old), Some(new)) = (&self.old_path_state, &self.new_path_state) {
            old.validate()?;
            new.validate()?;
            if old.path == new.path {
                return Err(validation_error(
                    "oldPathState.path",
                    "rename_path_required",
                ));
            }
            if old.path_revision != self.revision {
                return Err(validation_error(
                    "oldPathState.pathRevision",
                    "revision_mismatch",
                ));
            }
            if new.path_revision != self.revision {
                return Err(validation_error(
                    "newPathState.pathRevision",
                    "revision_mismatch",
                ));
            }
            if self.path_state != *new {
                return Err(validation_error("pathState", "new_path_state_mismatch"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMutationRejectedMessage {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub operation_id: OperationId,
    pub reason: WorkspaceMutationRejectReason,
    pub current_path_state: RequiredNullable<WorkspacePathState>,
    pub conflict_id: RequiredNullable<ConflictId>,
    pub required_hash: RequiredNullable<WorkspaceContentHash>,
}

impl WorkspaceMutationRejectedMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if let RequiredNullable::Value(state) = &self.current_path_state {
            state.validate()?;
        }
        match self.reason {
            WorkspaceMutationRejectReason::StaleBaseRevision
            | WorkspaceMutationRejectReason::OperationReused => {
                if !self.conflict_id.is_null() || !self.required_hash.is_null() {
                    return Err(validation_error("reason", "payload_mismatch"));
                }
            }
            WorkspaceMutationRejectReason::BlobRequired => {
                if self.required_hash.is_null() || !self.conflict_id.is_null() {
                    return Err(validation_error("requiredHash", "required_for_reason"));
                }
            }
            WorkspaceMutationRejectReason::ConflictCreated => {
                if self.conflict_id.is_null() {
                    return Err(validation_error("conflictId", "required_for_reason"));
                }
                if !self.required_hash.is_null() {
                    return Err(validation_error("requiredHash", "forbidden_for_reason"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceEventMessage {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub index: u32,
    pub revision: WorkspaceRevision,
    pub operation_id: OperationId,
    pub origin_client_id: ClientId,
    pub mutation: WorkspaceMutation,
    pub path_state: WorkspacePathState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub old_path_state: Option<WorkspacePathState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub new_path_state: Option<WorkspacePathState>,
}

impl WorkspaceEventMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        self.mutation.validate()?;
        if self.mutation.workspace_id != self.workspace_id
            || self.mutation.operation_id != self.operation_id
            || self.mutation.client_id != self.origin_client_id
        {
            return Err(validation_error("mutation", "event_identity_mismatch"));
        }
        if self.path_state.path_revision != self.revision {
            return Err(validation_error(
                "pathState.pathRevision",
                "revision_mismatch",
            ));
        }
        self.path_state.validate()?;

        if self.mutation.kind == WorkspaceMutationKind::Rename {
            let (Some(old), Some(new), Some(expected_new_path)) = (
                &self.old_path_state,
                &self.new_path_state,
                &self.mutation.new_path,
            ) else {
                return Err(validation_error("pathState", "rename_pair_required"));
            };
            old.validate()?;
            new.validate()?;
            if old.path != self.mutation.path {
                return Err(validation_error(
                    "oldPathState.path",
                    "mutation_path_mismatch",
                ));
            }
            if new.path != *expected_new_path {
                return Err(validation_error(
                    "newPathState.path",
                    "mutation_new_path_mismatch",
                ));
            }
            if old.path_revision != self.revision {
                return Err(validation_error(
                    "oldPathState.pathRevision",
                    "revision_mismatch",
                ));
            }
            if new.path_revision != self.revision {
                return Err(validation_error(
                    "newPathState.pathRevision",
                    "revision_mismatch",
                ));
            }
            if self.path_state != *new {
                return Err(validation_error("pathState", "new_path_state_mismatch"));
            }
        } else if self.old_path_state.is_some() || self.new_path_state.is_some() {
            return Err(validation_error("pathState", "forbidden_for_kind"));
        }
        Ok(())
    }

    pub fn validate_after(
        &self,
        previous_index: u32,
        previous_revision: WorkspaceRevision,
    ) -> Result<(), WorkspaceValidationError> {
        if previous_index.checked_add(1) != Some(self.index) {
            return Err(validation_error("index", "stream_gap"));
        }
        if self.revision <= previous_revision {
            return Err(validation_error("revision", "not_strictly_increasing"));
        }
        self.validate()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAckRequest {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub revision: WorkspaceRevision,
}

impl WorkspaceAckRequest {
    pub const fn validate(&self) -> Result<(), WorkspaceValidationError> {
        Ok(())
    }

    pub fn validate_between(
        &self,
        previous: WorkspaceRevision,
        delivered: WorkspaceRevision,
    ) -> Result<(), WorkspaceValidationError> {
        if self.revision <= previous {
            return Err(validation_error("revision", "ack_regression"));
        }
        if self.revision > delivered {
            return Err(validation_error("revision", "ack_overshoot"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBlobNeedUploadPush {
    pub workspace_id: WorkspaceId,
    pub direction: WorkspaceBlobDirection,
    pub operation_id: OperationId,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
}

impl WorkspaceBlobNeedUploadPush {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.direction != WorkspaceBlobDirection::Upload {
            return Err(validation_error("direction", "must_be_upload"));
        }
        validate_blob_size(self.size)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBlobNeedDownloadRequest {
    pub workspace_id: WorkspaceId,
    pub direction: WorkspaceBlobDirection,
    pub operation_id: RequiredNullable<OperationId>,
    pub content_hash: WorkspaceContentHash,
    pub size: RequiredNullable<u64>,
}

impl WorkspaceBlobNeedDownloadRequest {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.direction != WorkspaceBlobDirection::Download {
            return Err(validation_error("direction", "must_be_download"));
        }
        validate_required_null("operationId", &self.operation_id)?;
        validate_required_null("size", &self.size)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBlobNeedDownloadResponse {
    pub workspace_id: WorkspaceId,
    pub direction: WorkspaceBlobDirection,
    pub operation_id: RequiredNullable<OperationId>,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
}

impl WorkspaceBlobNeedDownloadResponse {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.direction != WorkspaceBlobDirection::Download {
            return Err(validation_error("direction", "must_be_download"));
        }
        validate_required_null("operationId", &self.operation_id)?;
        validate_blob_size(self.size)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBlobBeginMessage {
    pub workspace_id: WorkspaceId,
    pub transfer_id: TransferId,
    pub direction: WorkspaceBlobDirection,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
    pub chunk_size: u32,
    pub chunk_count: u64,
}

impl WorkspaceBlobBeginMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        validate_blob_size(self.size)?;
        if self.chunk_size != BLOB_CHUNK_BYTES {
            return Err(validation_error("chunkSize", "must_equal_limit"));
        }
        if self.chunk_count != blob_chunk_count(self.size) {
            return Err(validation_error("chunkCount", "arithmetic_mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBlobEndMessage {
    pub workspace_id: WorkspaceId,
    pub transfer_id: TransferId,
    pub direction: WorkspaceBlobDirection,
    pub content_hash: WorkspaceContentHash,
    pub size: u64,
    pub chunk_count: u64,
}

impl WorkspaceBlobEndMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        validate_blob_size(self.size)?;
        if self.chunk_count != blob_chunk_count(self.size) {
            return Err(validation_error("chunkCount", "arithmetic_mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConflictSide {
    pub path: RequiredNullable<WorkspacePath>,
    pub path_revision: WorkspaceRevision,
    pub content_hash: RequiredNullable<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
    pub tombstone: bool,
}

impl WorkspaceConflictSide {
    fn validate_with_field(&self, field: &str) -> Result<(), WorkspaceValidationError> {
        if self.path.is_null() && !self.tombstone {
            return Err(validation_error(
                &format!("{field}.path"),
                "null_requires_tombstone",
            ));
        }
        if self.tombstone {
            if !self.content_hash.is_null() {
                return Err(validation_error(
                    &format!("{field}.contentHash"),
                    "must_be_null_for_tombstone",
                ));
            }
            return self.metadata.validate(WorkspaceEntryKind::Tombstone);
        }
        if self.content_hash.is_null() {
            return self.metadata.validate(WorkspaceEntryKind::Directory);
        }
        self.metadata.validate(WorkspaceEntryKind::File)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConflictCreatedMessage {
    pub workspace_id: WorkspaceId,
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub path: WorkspacePath,
    pub kind: WorkspaceConflictKind,
    pub ancestor: WorkspaceConflictSide,
    pub current: WorkspaceConflictSide,
    pub incoming: WorkspaceConflictSide,
    pub created_by_operation_id: OperationId,
}

impl WorkspaceConflictCreatedMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        self.ancestor.validate_with_field("ancestor")?;
        self.current.validate_with_field("current")?;
        self.incoming.validate_with_field("incoming")?;
        match self.kind {
            WorkspaceConflictKind::Content | WorkspaceConflictKind::Binary => {
                if !side_is_live_file_at(&self.current, &self.path) {
                    return Err(validation_error("current", "kind_mismatch"));
                }
                if !side_is_live_file_at(&self.incoming, &self.path) {
                    return Err(validation_error("incoming", "kind_mismatch"));
                }
            }
            WorkspaceConflictKind::DeleteModify => {
                if self.current.tombstone == self.incoming.tombstone {
                    return Err(validation_error("incoming", "kind_mismatch"));
                }
                let live = if self.current.tombstone {
                    &self.incoming
                } else {
                    &self.current
                };
                if !side_is_live_file_at(live, &self.path) {
                    return Err(validation_error("incoming", "kind_mismatch"));
                }
            }
            WorkspaceConflictKind::Rename => {
                let (RequiredNullable::Value(current), RequiredNullable::Value(incoming)) =
                    (&self.current.path, &self.incoming.path)
                else {
                    return Err(validation_error("incoming", "kind_mismatch"));
                };
                if self.current.tombstone || self.incoming.tombstone {
                    return Err(validation_error("incoming", "kind_mismatch"));
                }
                if current == incoming {
                    return Err(validation_error("incoming.path", "rename_path_required"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConflictResolvedRequest {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub operation_id: OperationId,
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub choice: WorkspaceConflictChoice,
    pub path: WorkspacePath,
    pub content_hash: RequiredNullable<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
}

impl WorkspaceConflictResolvedRequest {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        match self.choice {
            WorkspaceConflictChoice::Merged => {
                if self.content_hash.is_null() {
                    return Err(validation_error("contentHash", "required_for_merged"));
                }
                self.metadata.validate(WorkspaceEntryKind::File)
            }
            WorkspaceConflictChoice::Delete => validate_delete_resolution(self),
            WorkspaceConflictChoice::Current | WorkspaceConflictChoice::Incoming => Ok(()),
        }
    }

    pub fn validate_against(
        &self,
        created: &WorkspaceConflictCreatedMessage,
    ) -> Result<(), WorkspaceValidationError> {
        if self.conflict_revision != created.conflict_revision {
            return Err(validation_error(
                "conflictRevision",
                "conflict_revision_stale",
            ));
        }
        created.validate()?;
        if self.workspace_id != created.workspace_id || self.conflict_id != created.conflict_id {
            return Err(validation_error("conflictId", "conflict_mismatch"));
        }
        match self.choice {
            WorkspaceConflictChoice::Current => validate_side_replay(self, &created.current),
            WorkspaceConflictChoice::Incoming => validate_side_replay(self, &created.incoming),
            WorkspaceConflictChoice::Merged => {
                if self.path != created.path {
                    return Err(validation_error("path", "conflict_path_mismatch"));
                }
                self.validate()
            }
            WorkspaceConflictChoice::Delete => {
                if self.path != created.path {
                    return Err(validation_error("path", "conflict_path_mismatch"));
                }
                self.validate()
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConflictResolvedMessage {
    pub workspace_id: WorkspaceId,
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub operation_id: OperationId,
    pub revision: WorkspaceRevision,
    pub choice: WorkspaceConflictChoice,
    pub path_state: WorkspacePathState,
    pub resolved_by_client_id: ClientId,
}

impl WorkspaceConflictResolvedMessage {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.revision == WorkspaceRevision::ZERO {
            return Err(validation_error("revision", "must_be_positive"));
        }
        if self.path_state.path_revision != self.revision {
            return Err(validation_error(
                "pathState.pathRevision",
                "revision_mismatch",
            ));
        }
        self.path_state.validate()
    }
}

fn validate_required_null<T>(
    field: &str,
    value: &RequiredNullable<T>,
) -> Result<(), WorkspaceValidationError> {
    if value.is_null() {
        Ok(())
    } else {
        Err(validation_error(field, "must_be_null"))
    }
}

fn validate_blob_size(size: u64) -> Result<(), WorkspaceValidationError> {
    if size > MAX_BLOB_BYTES {
        Err(validation_error("size", "limit_exceeded"))
    } else {
        Ok(())
    }
}

const fn blob_chunk_count(size: u64) -> u64 {
    if size == 0 {
        0
    } else {
        (size - 1) / BLOB_CHUNK_BYTES as u64 + 1
    }
}

fn side_is_live_file_at(side: &WorkspaceConflictSide, path: &WorkspacePath) -> bool {
    !side.tombstone
        && matches!(&side.path, RequiredNullable::Value(side_path) if side_path == path)
        && !side.content_hash.is_null()
}

fn validate_side_replay(
    request: &WorkspaceConflictResolvedRequest,
    side: &WorkspaceConflictSide,
) -> Result<(), WorkspaceValidationError> {
    let matches = !side.tombstone
        && matches!(&side.path, RequiredNullable::Value(path) if path == &request.path)
        && request.content_hash == side.content_hash
        && request.metadata == side.metadata;
    if matches {
        Ok(())
    } else {
        Err(validation_error("choice", "side_mismatch"))
    }
}

fn validate_delete_resolution(
    request: &WorkspaceConflictResolvedRequest,
) -> Result<(), WorkspaceValidationError> {
    if !request.content_hash.is_null() {
        return Err(validation_error("contentHash", "must_be_null_for_delete"));
    }
    if request.metadata.size != 0 {
        return Err(validation_error("metadata.size", "must_be_zero_for_delete"));
    }
    if request.metadata.modified_at_ms != 0 {
        return Err(validation_error(
            "metadata.modifiedAtMs",
            "must_be_zero_for_delete",
        ));
    }
    if request.metadata.executable {
        return Err(validation_error(
            "metadata.executable",
            "must_be_false_for_delete",
        ));
    }
    Ok(())
}

fn validation_error(field: &str, reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}
