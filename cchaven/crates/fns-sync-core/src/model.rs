use fns_protocol::revision::WorkspaceConflictRevision;
use fns_protocol::{
    ClientId, ConflictId, OperationId, RequiredNullable, StreamId, WorkspaceConflictChoice,
    WorkspaceConflictKind, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata,
    WorkspaceId, WorkspacePath, WorkspaceRevision, WorkspaceSnapshotMode,
};

macro_rules! stage_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Deserialize,
            serde::Serialize,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

stage_enum! {
    /// Durable stage of a local operation in the client outbox.
    OutboxStage {
        Queued => "queued",
        Dispatched => "dispatched",
        AwaitingBlob => "awaiting_blob",
        BlockedConflict => "blocked_conflict"
    }
}

stage_enum! {
    /// Durable stage of a received stream item.
    StreamItemStatus {
        Received => "received",
        WaitingBlob => "waiting_blob",
        Ready => "ready",
        Applied => "applied",
        Preserved => "preserved"
    }
}

stage_enum! {
    /// Durable stage of a received stream conflict.
    StreamConflictStatus {
        Received => "received",
        Replaced => "replaced",
        Pruned => "pruned"
    }
}

stage_enum! {
    /// Kind of item represented by an apply journal row.
    ApplyItemKind {
        Entry => "entry",
        Event => "event",
        ConflictResolved => "conflict_resolved"
    }
}

stage_enum! {
    /// Namespace of the authoritative item coordinated by an apply journal.
    ApplyNamespace {
        SnapshotEntry => "snapshot_entry",
        StreamEvent => "stream_event",
        LiveEvent => "live_event",
        StreamConflictResolved => "stream_conflict_resolved",
        LiveConflictResolved => "live_conflict_resolved"
    }
}

stage_enum! {
    /// Filesystem/apply journal checkpoint.
    ApplyStage {
        Prepared => "prepared",
        FilesystemStarted => "filesystem_started",
        FilesystemApplied => "filesystem_applied",
        DatabaseCommitted => "database_committed",
        Finalized => "finalized"
    }
}

stage_enum! {
    /// Durable conflict lifecycle state.
    ConflictStatus {
        WaitingBlobs => "waiting_blobs",
        Manual => "manual",
        AutoReady => "auto_ready",
        Resolving => "resolving",
        RefreshRequired => "refresh_required"
    }
}

stage_enum! {
    /// Kind of revision item stored in the incremental stream table.
    StreamRevisionItemKind {
        Event => "event",
        ConflictResolved => "conflict_resolved"
    }
}

pub type StreamEntryStatus = StreamItemStatus;
pub type StreamRevisionItemStatus = StreamItemStatus;
pub type StreamMode = WorkspaceSnapshotMode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceCursor {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
    pub last_ack_revision: WorkspaceRevision,
    pub last_applied_revision: WorkspaceRevision,
    pub pending_ack_revision: Option<WorkspaceRevision>,
    pub pending_segment_ack_revision: Option<WorkspaceRevision>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathStateRecord {
    pub workspace_id: WorkspaceId,
    pub path: WorkspacePath,
    pub state_json: Vec<u8>,
    pub state_digest: [u8; 32],
    pub state: fns_protocol::WorkspacePathState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxRecord {
    pub client_id: ClientId,
    pub operation_id: OperationId,
    pub workspace_id: WorkspaceId,
    pub body_json: Vec<u8>,
    pub body_digest: [u8; 32],
    pub stage: OutboxStage,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboxBody {
    Mutation(fns_protocol::WorkspaceMutation),
    ConflictResolution(fns_protocol::WorkspaceConflictResolvedRequest),
}

impl OutboxRecord {
    pub fn mutation(&self) -> Result<fns_protocol::WorkspaceMutation, serde_json::Error> {
        serde_json::from_slice(&self.body_json)
    }

    pub fn body(&self) -> &[u8] {
        &self.body_json
    }

    pub fn decoded_body(&self) -> Result<OutboxBody, serde_json::Error> {
        match serde_json::from_slice(&self.body_json) {
            Ok(mutation) => Ok(OutboxBody::Mutation(mutation)),
            Err(_) => serde_json::from_slice(&self.body_json).map(OutboxBody::ConflictResolution),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalIntentRecord {
    pub workspace_id: WorkspaceId,
    pub path: WorkspacePath,
    pub intent_json: Vec<u8>,
    pub updated_at_ms: i64,
}

/// Stable desired state captured from a filesystem observation.  It deliberately
/// contains no server revision: revisions belong to the remote path state at
/// reconciliation time, while this value remains the local postimage that can
/// be replayed after a crash.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalDesiredEntry {
    pub path: WorkspacePath,
    pub kind: WorkspaceEntryKind,
    pub content_hash: RequiredNullable<WorkspaceContentHash>,
    pub metadata: WorkspaceFileMetadata,
}

/// Durable deferred local intent.  Rename rows are written under both paths so
/// a watcher event touching either side remains visible until its dispatched
/// operation is settled.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "intent", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalIntent {
    Desired {
        entry: LocalDesiredEntry,
    },
    Delete {
        path: WorkspacePath,
    },
    Rename {
        from: WorkspacePath,
        to: WorkspacePath,
        kind: WorkspaceEntryKind,
        content_hash: RequiredNullable<WorkspaceContentHash>,
        metadata: WorkspaceFileMetadata,
    },
}

impl LocalIntent {
    pub fn paths(&self) -> Vec<&WorkspacePath> {
        match self {
            Self::Desired { entry } => vec![&entry.path],
            Self::Delete { path } => vec![path],
            Self::Rename { from, to, .. } => vec![from, to],
        }
    }

    pub fn desired_entry(&self) -> Option<LocalDesiredEntry> {
        match self {
            Self::Desired { entry } => Some(entry.clone()),
            Self::Delete { .. } => None,
            Self::Rename {
                to,
                kind,
                content_hash,
                metadata,
                ..
            } => Some(LocalDesiredEntry {
                path: to.clone(),
                kind: *kind,
                content_hash: content_hash.clone(),
                metadata: metadata.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamStateRecord {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub mode: WorkspaceSnapshotMode,
    pub from_revision: WorkspaceRevision,
    pub final_revision: WorkspaceRevision,
    pub expected_entry_count: u32,
    pub expected_event_count: u32,
    pub expected_conflict_count: u32,
    pub next_event_index: u32,
    pub end_received: bool,
}

pub type StreamState = StreamStateRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamEntryRecord {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub entry_index: u32,
    pub body_json: Vec<u8>,
    pub body_digest: [u8; 32],
    pub status: StreamItemStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRevisionItemRecord {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub revision: WorkspaceRevision,
    pub item_kind: StreamRevisionItemKind,
    pub body_json: Vec<u8>,
    pub body_digest: [u8; 32],
    pub event_index: Option<u32>,
    pub status: StreamItemStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamConflictRecord {
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub created_json: Vec<u8>,
    pub status: StreamConflictStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyJournalRecord {
    pub apply_id: fns_fs::ApplyId,
    pub workspace_id: WorkspaceId,
    pub stream_id: StreamId,
    pub item_kind: ApplyItemKind,
    pub item_key: String,
    pub apply_namespace: ApplyNamespace,
    pub operation_body_digest: [u8; 32],
    pub operation_json: Vec<u8>,
    pub filesystem_operation_json: Vec<u8>,
    pub commit_json: Vec<u8>,
    pub preimage_json: Vec<u8>,
    pub postimage_json: Vec<u8>,
    pub filesystem_receipt_json: Option<Vec<u8>>,
    pub stage: ApplyStage,
}

/// Exact database-side action paired with a journaled filesystem apply.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "namespace", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplyCommitPlan {
    SnapshotEntry {
        entry: fns_protocol::WorkspaceSnapshotEntryMessage,
    },
    StreamEvent {
        event: fns_protocol::WorkspaceEventMessage,
        remove_outbox: bool,
    },
    LiveEvent {
        event: fns_protocol::WorkspaceEventMessage,
        remove_outbox: bool,
    },
    StreamConflictResolved {
        message: fns_protocol::WorkspaceConflictResolvedMessage,
    },
    LiveConflictResolved {
        message: fns_protocol::WorkspaceConflictResolvedMessage,
    },
}

impl ApplyCommitPlan {
    pub const fn namespace(&self) -> ApplyNamespace {
        match self {
            Self::SnapshotEntry { .. } => ApplyNamespace::SnapshotEntry,
            Self::StreamEvent { .. } => ApplyNamespace::StreamEvent,
            Self::LiveEvent { .. } => ApplyNamespace::LiveEvent,
            Self::StreamConflictResolved { .. } => ApplyNamespace::StreamConflictResolved,
            Self::LiveConflictResolved { .. } => ApplyNamespace::LiveConflictResolved,
        }
    }
}

/// Canonical remote postimage persisted in an apply-journal row before any
/// filesystem mutation begins.  The journal stores this wire-independent
/// description so a resumed engine can prove which path state the operation
/// was meant to materialize.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteApplyOperation {
    Upsert {
        state: fns_protocol::WorkspacePathState,
    },
    Delete {
        state: fns_protocol::WorkspacePathState,
    },
    Rename {
        old_state: fns_protocol::WorkspacePathState,
        new_state: fns_protocol::WorkspacePathState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedOperationRecord {
    pub origin_client_id: ClientId,
    pub operation_id: OperationId,
    pub revision: WorkspaceRevision,
    pub body_digest: [u8; 32],
    pub receipt_kind: AppliedOperationReceiptKind,
    pub mutation_json: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppliedOperationReceiptKind {
    Legacy,
    MutationResult,
    ConflictResolution,
}

impl AppliedOperationReceiptKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::MutationResult => "mutation_result",
            Self::ConflictResolution => "conflict_resolution",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy" => Some(Self::Legacy),
            "mutation_result" => Some(Self::MutationResult),
            "conflict_resolution" => Some(Self::ConflictResolution),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictRecord {
    pub conflict_id: ConflictId,
    pub workspace_id: WorkspaceId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub created_json: Vec<u8>,
    pub status: ConflictStatus,
    pub candidate_hash: Option<String>,
    pub resolution_json: Option<Vec<u8>>,
    pub resolution_digest: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictBlockedReason {
    WaitingBlobs,
    AutomaticResolutionPending,
    ResolutionPending,
    RefreshRequired,
    SelectedSideDeleted,
}

impl ConflictBlockedReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WaitingBlobs => "waiting_blobs",
            Self::AutomaticResolutionPending => "automatic_resolution_pending",
            Self::ResolutionPending => "resolution_pending",
            Self::RefreshRequired => "refresh_required",
            Self::SelectedSideDeleted => "selected_side_deleted",
        }
    }
}

impl std::fmt::Display for ConflictBlockedReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictSideView {
    pub path: Option<WorkspacePath>,
    pub path_revision: WorkspaceRevision,
    pub content_hash: Option<WorkspaceContentHash>,
    pub size: u64,
    pub modified_at_ms: i64,
    pub executable: bool,
    pub tombstone: bool,
}

impl From<&fns_protocol::WorkspaceConflictSide> for ConflictSideView {
    fn from(side: &fns_protocol::WorkspaceConflictSide) -> Self {
        Self {
            path: side.path.clone().into_option(),
            path_revision: side.path_revision,
            content_hash: side.content_hash.clone().into_option(),
            size: side.metadata.size,
            modified_at_ms: side.metadata.modified_at_ms,
            executable: side.metadata.executable,
            tombstone: side.tombstone,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingConflictResolutionView {
    pub operation_id: OperationId,
    pub choice: WorkspaceConflictChoice,
    pub content_hash: Option<WorkspaceContentHash>,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictView {
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub path: WorkspacePath,
    pub kind: WorkspaceConflictKind,
    pub status: ConflictStatus,
    pub ancestor: ConflictSideView,
    pub current: ConflictSideView,
    pub incoming: ConflictSideView,
    pub created_by_operation_id: OperationId,
    pub pending_resolution: Option<PendingConflictResolutionView>,
    pub can_resolve: bool,
    pub blocked_reason: Option<ConflictBlockedReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictResolutionInput {
    pub conflict_id: ConflictId,
    pub conflict_revision: WorkspaceConflictRevision,
    pub choice: WorkspaceConflictChoice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolutionReceiptStatus {
    Queued,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictResolutionReceipt {
    pub status: ConflictResolutionReceiptStatus,
    pub operation_id: OperationId,
}

pub type PathState = PathStateRecord;
pub type OutboxEntry = OutboxRecord;
pub type ApplyJournal = ApplyJournalRecord;
pub type Conflict = ConflictRecord;
