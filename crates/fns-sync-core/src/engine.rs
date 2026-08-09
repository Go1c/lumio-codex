use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf, absolute};

use fns_fs::{
    ApplyId, AtomicWorkspaceWriter, ContentCache, ExpectedEntry, FsChange, FsOperation, HashCache,
    ObservedEntry, RootedWorkspace, SealedContentImport, StagedContentImport, SyncRuleConfig,
    SyncRules,
};
use fns_protocol::{
    OperationId, RequiredNullable, StreamId, WorkspaceAckRequest, WorkspaceConflictCreatedMessage,
    WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest, WorkspaceEntryKind,
    WorkspaceEventMessage, WorkspaceMutation, WorkspaceMutationAcceptedMessage,
    WorkspaceMutationRejectReason, WorkspaceMutationRejectedMessage, WorkspacePath,
    WorkspacePathState, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage,
    WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode, WorkspaceV2ErrorCode,
};

use crate::effect::SyncCommand;
use crate::error::SyncError;
use crate::model::{
    AppliedOperationReceiptKind, AppliedOperationRecord, ApplyCommitPlan, ApplyItemKind,
    ApplyJournalRecord, ApplyNamespace, ApplyStage, ConflictBlockedReason, ConflictRecord,
    ConflictResolutionReceipt, ConflictResolutionReceiptStatus, ConflictSideView, ConflictStatus,
    ConflictView, LocalDesiredEntry, OutboxBody, OutboxStage, PendingConflictResolutionView,
    RemoteApplyOperation, StreamConflictStatus, StreamItemStatus, StreamRevisionItemKind,
    WorkspaceCursor,
};
use crate::reconcile::{
    DesiredOperation, decode_intent, desired_from_intent, desired_from_mutation, encode_intent,
    mutation_for_desired, mutation_matches_desired, zero_metadata,
};
use crate::{SqliteState, canonical_json};

/// Limits CPU work per engine entry point and memory retained for blocked live events.
///
/// The per-call byte limit is soft for the first valid item so an item larger
/// than the budget can make progress by itself. Queue limits are hard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboundWorkLimits {
    /// Maximum number of staged or live items examined by one engine call.
    pub max_items_per_call: usize,
    /// Maximum serialized input bytes examined by one engine call.
    pub max_serialized_bytes_per_call: usize,
    /// Maximum number of live events retained while an earlier event is blocked.
    pub max_pending_live_items: usize,
    /// Maximum serialized bytes represented by retained live events.
    pub max_pending_live_serialized_bytes: usize,
}

impl Default for InboundWorkLimits {
    fn default() -> Self {
        Self {
            max_items_per_call: 64,
            max_serialized_bytes_per_call: 256 * 1024,
            max_pending_live_items: 256,
            max_pending_live_serialized_bytes: 4 * 1024 * 1024,
        }
    }
}

impl InboundWorkLimits {
    fn validate(self) -> Result<Self, SyncError> {
        if self.max_items_per_call == 0
            || self.max_serialized_bytes_per_call == 0
            || self.max_pending_live_items == 0
            || self.max_pending_live_serialized_bytes == 0
        {
            return Err(SyncError::InvalidConfiguration {
                reason: "invalid_inbound_work_limits",
            });
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct SyncEngineConfig {
    pub workspace_id: fns_protocol::WorkspaceId,
    pub client_id: fns_protocol::ClientId,
    pub workspace_root: PathBuf,
    pub state_root: PathBuf,
    sync_rules: SyncRuleConfig,
    inbound_work_limits: InboundWorkLimits,
    operation_ids: Vec<fns_protocol::OperationId>,
    timestamps: Vec<i64>,
}

impl SyncEngineConfig {
    pub fn new(
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        workspace_root: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
    ) -> Self {
        Self {
            workspace_id,
            client_id,
            workspace_root: workspace_root.as_ref().to_path_buf(),
            state_root: state_root.as_ref().to_path_buf(),
            sync_rules: SyncRuleConfig::default(),
            inbound_work_limits: InboundWorkLimits::default(),
            operation_ids: Vec::new(),
            timestamps: Vec::new(),
        }
    }

    pub fn with_operation_ids<I>(mut self, operation_ids: I) -> Self
    where
        I: IntoIterator<Item = fns_protocol::OperationId>,
    {
        self.operation_ids = operation_ids.into_iter().collect();
        self
    }

    pub fn with_timestamps<I>(mut self, timestamps: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.timestamps = timestamps.into_iter().collect();
        self
    }

    pub fn with_sync_rules(mut self, sync_rules: SyncRuleConfig) -> Self {
        self.sync_rules = sync_rules;
        self
    }

    pub fn with_inbound_work_limits(mut self, limits: InboundWorkLimits) -> Self {
        self.inbound_work_limits = limits;
        self
    }

    pub fn with_operation_id_sequence<I>(self, operation_ids: I) -> Self
    where
        I: IntoIterator<Item = fns_protocol::OperationId>,
    {
        self.with_operation_ids(operation_ids)
    }

    pub fn with_timestamp_sequence<I>(self, timestamps: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        self.with_timestamps(timestamps)
    }
}

pub struct SystemRuntime {
    pub(crate) workspace: RootedWorkspace,
    pub(crate) content_cache: ContentCache,
    pub(crate) writer: AtomicWorkspaceWriter,
    pub(crate) rules: SyncRules,
}

pub struct EngineRuntime {
    pub(crate) system: SystemRuntime,
    pub(crate) state: SqliteState,
}

pub struct SyncEngine {
    pub(crate) runtime: EngineRuntime,
    operation_ids: VecDeque<fns_protocol::OperationId>,
    timestamps: VecDeque<i64>,
    inbound_work_limits: InboundWorkLimits,
    /// Live revision items are kept in memory while their blobs are downloaded.
    /// They remain unacknowledged, so a reconnect replays them if the process
    /// stops before the download completes.
    pending_live_events: VecDeque<PendingLiveItem>,
    pending_live_serialized_bytes: usize,
    next_inbound_source: InboundWorkSource,
    closed: bool,
}

#[derive(Clone, Debug)]
struct PendingLiveItem {
    message: PendingLiveMessage,
    body_digest: [u8; 32],
    serialized_bytes: usize,
}

#[derive(Clone, Debug)]
enum PendingLiveMessage {
    Event(Box<WorkspaceEventMessage>),
    ConflictResolved(WorkspaceConflictResolvedMessage),
}

impl PendingLiveMessage {
    const fn revision(&self) -> fns_protocol::WorkspaceRevision {
        match self {
            Self::Event(message) => message.revision,
            Self::ConflictResolved(message) => message.revision,
        }
    }
}

struct PreservedReplacements {
    mutations: Vec<(WorkspaceMutation, i64)>,
    settled_paths: Vec<WorkspacePath>,
}

struct ConflictResolutionProposal {
    choice: fns_protocol::WorkspaceConflictChoice,
    path: WorkspacePath,
    content_hash: RequiredNullable<fns_protocol::WorkspaceContentHash>,
    metadata: fns_protocol::WorkspaceFileMetadata,
}

impl ConflictResolutionProposal {
    fn matches(&self, request: &WorkspaceConflictResolvedRequest) -> bool {
        request.choice == self.choice
            && request.path == self.path
            && request.content_hash == self.content_hash
            && request.metadata == self.metadata
    }

    fn into_request(
        self,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
        operation_id: OperationId,
        conflict_id: fns_protocol::ConflictId,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
    ) -> WorkspaceConflictResolvedRequest {
        WorkspaceConflictResolvedRequest {
            workspace_id,
            client_id,
            operation_id,
            conflict_id,
            conflict_revision,
            choice: self.choice,
            path: self.path,
            content_hash: self.content_hash,
            metadata: self.metadata,
        }
    }
}

fn decode_conflict_created(
    conflict: &ConflictRecord,
) -> Result<WorkspaceConflictCreatedMessage, SyncError> {
    let created: WorkspaceConflictCreatedMessage = serde_json::from_slice(&conflict.created_json)
        .map_err(|_| SyncError::CorruptState {
        table: "conflicts",
        field: "created_json",
    })?;
    created.validate().map_err(|_| SyncError::CorruptState {
        table: "conflicts",
        field: "created_json",
    })?;
    if created.workspace_id != conflict.workspace_id
        || created.conflict_id != conflict.conflict_id
        || created.conflict_revision != conflict.conflict_revision
        || canonical_json(&created)? != conflict.created_json
    {
        return Err(SyncError::CorruptState {
            table: "conflicts",
            field: "created_json",
        });
    }
    Ok(created)
}

fn decode_pending_resolution(
    conflict: &ConflictRecord,
    created: &WorkspaceConflictCreatedMessage,
    client_id: fns_protocol::ClientId,
) -> Result<Option<WorkspaceConflictResolvedRequest>, SyncError> {
    let Some(json) = conflict.resolution_json.as_deref() else {
        if conflict.resolution_digest.is_some() {
            return Err(SyncError::CorruptState {
                table: "conflicts",
                field: "resolution_digest",
            });
        }
        return Ok(None);
    };
    let request: WorkspaceConflictResolvedRequest =
        serde_json::from_slice(json).map_err(|_| SyncError::CorruptState {
            table: "conflicts",
            field: "resolution_json",
        })?;
    let digest = conflict.resolution_digest.ok_or(SyncError::CorruptState {
        table: "conflicts",
        field: "resolution_digest",
    })?;
    if canonical_json(&request)? != json
        || crate::body_digest(json) != digest
        || request.workspace_id != conflict.workspace_id
        || request.client_id != client_id
        || request.conflict_id != conflict.conflict_id
        || request.validate_against(created).is_err()
    {
        return Err(SyncError::CorruptState {
            table: "conflicts",
            field: "resolution_json",
        });
    }
    if request.choice == fns_protocol::WorkspaceConflictChoice::Merged {
        let candidate =
            request
                .content_hash
                .clone()
                .into_option()
                .ok_or(SyncError::CorruptState {
                    table: "conflicts",
                    field: "resolution_json",
                })?;
        if conflict.candidate_hash.as_deref() != Some(candidate.as_str()) {
            return Err(SyncError::CorruptState {
                table: "conflicts",
                field: "candidate_hash",
            });
        }
    } else if conflict.candidate_hash.is_some() {
        return Err(SyncError::CorruptState {
            table: "conflicts",
            field: "candidate_hash",
        });
    }
    Ok(Some(request))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ConflictCleanup {
    resolution_operation_id: Option<OperationId>,
    originating_operation_id: Option<OperationId>,
}

fn cleanup_conflict_resolution(
    tx: &mut crate::StateTransaction<'_>,
    conflict_id: fns_protocol::ConflictId,
    cleanup: ConflictCleanup,
) -> Result<(), SyncError> {
    if let Some(operation_id) = cleanup.resolution_operation_id {
        tx.remove_outbox(operation_id)?;
    }
    if let Some(operation_id) = cleanup.originating_operation_id {
        tx.remove_outbox(operation_id)?;
    }
    tx.remove_conflict(conflict_id)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SupersededEventSource {
    Live,
    Stream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConflictResolutionSource {
    Live,
    Stream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InboundWorkSource {
    Stream,
    Live,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppliedReceiptMatch {
    Missing,
    Exact,
    Legacy { body_digest: [u8; 32] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationAcceptanceMatch {
    Missing,
    Exact,
    LegacyUnbound,
}

impl InboundWorkSource {
    const fn other(self) -> Self {
        match self {
            Self::Stream => Self::Live,
            Self::Live => Self::Stream,
        }
    }
}

struct InboundWorkBudget {
    max_items: usize,
    max_serialized_bytes: usize,
    used_items: usize,
    used_serialized_bytes: usize,
}

impl InboundWorkBudget {
    fn new(limits: InboundWorkLimits) -> Self {
        Self {
            max_items: limits.max_items_per_call,
            max_serialized_bytes: limits.max_serialized_bytes_per_call,
            used_items: 0,
            used_serialized_bytes: 0,
        }
    }

    fn remaining_items(&self) -> usize {
        self.max_items - self.used_items
    }

    fn remaining_serialized_bytes(&self) -> usize {
        self.max_serialized_bytes
            .saturating_sub(self.used_serialized_bytes)
    }

    fn allows_oversized_first_item(&self) -> bool {
        self.used_items == 0
    }

    fn consume(&mut self, serialized_bytes: usize) -> bool {
        if self.used_items == self.max_items {
            return false;
        }
        let fits_bytes = serialized_bytes <= self.remaining_serialized_bytes();
        if !fits_bytes && !self.allows_oversized_first_item() {
            return false;
        }
        self.used_items += 1;
        self.used_serialized_bytes = self.used_serialized_bytes.saturating_add(serialized_bytes);
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResumeStep {
    Progressed,
    Blocked,
    Idle,
    BudgetExhausted,
}

impl SystemRuntime {
    pub fn workspace_root(&self) -> &Path {
        self.workspace.canonical_root()
    }

    pub fn open_blob(
        &self,
        hash: &fns_protocol::WorkspaceContentHash,
    ) -> Result<std::fs::File, SyncError> {
        self.content_cache
            .open_blob(hash)
            .map_err(SyncError::Filesystem)
    }
}

impl EngineRuntime {
    pub fn state(&self) -> &SqliteState {
        &self.state
    }

    pub fn system(&self) -> &SystemRuntime {
        &self.system
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationResult {
    Accepted(WorkspaceMutationAcceptedMessage),
    Rejected(WorkspaceMutationRejectedMessage),
}

impl SyncEngine {
    pub fn open(config: SyncEngineConfig) -> Result<Self, SyncError> {
        let inbound_work_limits = config.inbound_work_limits.validate()?;
        let workspace = RootedWorkspace::open(&config.workspace_root)?;
        let state_root = preflight_state_root(workspace.canonical_root(), &config.state_root)?;
        fs::create_dir_all(&state_root).map_err(|_| {
            SyncError::Filesystem(fns_fs::FsError::Io {
                operation: "create sync state root",
            })
        })?;
        let state_root = fs::canonicalize(&state_root).map_err(|_| {
            SyncError::Filesystem(fns_fs::FsError::Io {
                operation: "canonicalize sync state root",
            })
        })?;
        if paths_overlap(workspace.canonical_root(), &state_root) {
            return Err(SyncError::InvalidConfiguration {
                reason: "roots_overlap",
            });
        }
        let content_cache = ContentCache::open(&state_root)?;
        let writer = AtomicWorkspaceWriter::new(
            RootedWorkspace::open(workspace.canonical_root())?,
            ContentCache::open(&state_root)?,
        );
        let state = SqliteState::open(
            state_root.join("state.sqlite"),
            config.workspace_id,
            config.client_id,
        )?;
        let rules = SyncRules::compile(config.sync_rules).map_err(SyncError::Filesystem)?;
        let mut engine = Self {
            runtime: EngineRuntime {
                system: SystemRuntime {
                    workspace,
                    content_cache,
                    writer,
                    rules,
                },
                state,
            },
            operation_ids: config.operation_ids.into(),
            timestamps: config.timestamps.into(),
            inbound_work_limits,
            pending_live_events: VecDeque::new(),
            pending_live_serialized_bytes: 0,
            next_inbound_source: InboundWorkSource::Stream,
            closed: false,
        };
        engine.recover_apply_journals()?;
        Ok(engine)
    }

    pub fn new(config: SyncEngineConfig) -> Result<Self, SyncError> {
        Self::open(config)
    }

    pub fn state(&self) -> &SqliteState {
        &self.runtime.state
    }

    pub fn state_mut(&mut self) -> &mut SqliteState {
        &mut self.runtime.state
    }

    pub fn runtime(&self) -> &EngineRuntime {
        &self.runtime
    }

    pub fn cursor(&self) -> Result<WorkspaceCursor, SyncError> {
        self.runtime.state.cursor()
    }

    /// Returns the mode of the stream currently being assembled, if any.
    /// Transport uses this to hold live events that arrive after a snapshot
    /// end but before the snapshot acknowledgment has been confirmed.
    pub fn active_stream_mode(&self) -> Result<Option<WorkspaceSnapshotMode>, SyncError> {
        Ok(self.runtime.state.stream_state()?.map(|state| state.mode))
    }

    /// Return the revision whose active stream is durably complete and ready
    /// for its protocol Ack. A received End alone is insufficient while any
    /// item or apply journal remains unfinished.
    pub fn completed_stream_ack_revision(
        &self,
    ) -> Result<Option<fns_protocol::WorkspaceRevision>, SyncError> {
        self.ensure_open()?;
        let Some(active) = self.runtime.state.stream_state()? else {
            return Ok(None);
        };
        if !active.end_received {
            return Ok(None);
        }
        let summary = self.runtime.state.stream_table_summary(active.stream_id)?;
        let items_ready = match active.mode {
            WorkspaceSnapshotMode::Snapshot => {
                summary.entry_count == u64::from(active.expected_entry_count)
                    && summary.pending_entry_count == 0
                    && summary.revision_count == 0
            }
            WorkspaceSnapshotMode::Incremental => {
                summary.revision_count == u64::from(active.expected_event_count)
                    && summary.pending_revision_count == 0
                    && summary.entry_count == 0
            }
        };
        if !items_ready
            || summary.conflict_count != u64::from(active.expected_conflict_count)
            || self.runtime.state.has_apply_journals()?
        {
            return Ok(None);
        }
        let cursor = self.runtime.state.cursor()?;
        Ok((cursor.pending_ack_revision == Some(active.final_revision))
            .then_some(active.final_revision))
    }

    pub fn workspace_root(&self) -> &Path {
        self.runtime.system.workspace.canonical_root()
    }

    pub fn canonical_body<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, SyncError> {
        canonical_json(value)
    }

    pub fn stage_bytes(
        &mut self,
        expected: &fns_protocol::WorkspaceContentHash,
        bytes: &[u8],
    ) -> Result<(), SyncError> {
        self.ensure_open()?;
        self.runtime.system.content_cache.import(
            expected,
            bytes.len() as u64,
            std::io::Cursor::new(bytes),
        )?;
        Ok(())
    }

    pub fn open_blob(
        &self,
        hash: &fns_protocol::WorkspaceContentHash,
    ) -> Result<std::fs::File, SyncError> {
        self.ensure_open()?;
        self.runtime.system.open_blob(hash)
    }

    pub fn scan_and_record(&mut self) -> Result<(), SyncError> {
        self.ensure_open()?;
        let changes = self.scan_changes()?;
        self.record_local_changes(changes)
    }

    pub fn scan_changes(&mut self) -> Result<Vec<FsChange>, SyncError> {
        self.ensure_open()?;
        let scan = self
            .runtime
            .system
            .workspace
            .scan(&self.runtime.system.rules)?;
        if !scan.issues.is_empty() {
            return Err(SyncError::ScanIncomplete);
        }
        let mut current = BTreeMap::new();
        for observed in scan.entries {
            let entry = self.desired_entry_from_observed(&observed)?;
            current.insert(observed.path, entry);
        }
        let remote = self
            .runtime
            .state
            .path_states()?
            .into_iter()
            .map(|record| (record.path, record.state))
            .collect::<BTreeMap<_, _>>();

        let mut additions = Vec::new();
        let mut updates = Vec::new();
        for (path, entry) in &current {
            match remote.get(path) {
                None => additions.push((path.clone(), entry.clone())),
                Some(state) if state.kind == WorkspaceEntryKind::Tombstone => {
                    additions.push((path.clone(), entry.clone()))
                }
                Some(state) if !remote_matches_entry(state, entry) => {
                    updates.push((path.clone(), entry.clone()))
                }
                Some(_) => {}
            }
        }

        let mut deletions = remote
            .iter()
            .filter(|(path, state)| {
                state.kind != WorkspaceEntryKind::Tombstone && !current.contains_key(*path)
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        deletions.sort();

        let mut paired_additions = HashSet::new();
        let mut paired_deletions = HashSet::new();
        let mut renames = Vec::new();
        let mut directory_renames = Vec::new();
        let remote_directory_subtrees = deletions
            .iter()
            .filter(|path| {
                remote
                    .get(*path)
                    .is_some_and(|state| state.kind == WorkspaceEntryKind::Directory)
            })
            .map(|path| {
                (
                    path.clone(),
                    remote_directory_subtree_identity(path, &remote),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let current_directory_subtrees = additions
            .iter()
            .filter(|(_, entry)| entry.kind == WorkspaceEntryKind::Directory)
            .map(|(path, _)| {
                (
                    path.clone(),
                    current_directory_subtree_identity(path, &current),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let rename_candidates = deletions
            .iter()
            .map(|old_path| {
                let Some(old_state) = remote.get(old_path) else {
                    return Vec::new();
                };
                additions
                    .iter()
                    .enumerate()
                    .filter(|(_, (new_path, entry))| {
                        same_rename_identity(old_state, entry)
                            && (old_state.kind != WorkspaceEntryKind::Directory
                                || (remote_directory_subtrees.get(old_path)
                                    == current_directory_subtrees.get(new_path)
                                    && directory_subtrees_exact_match(
                                        old_path, new_path, &remote, &current,
                                    )))
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut reverse_candidate_counts = vec![0_usize; additions.len()];
        for candidates in &rename_candidates {
            for &add_index in candidates {
                reverse_candidate_counts[add_index] += 1;
            }
        }
        for (delete_index, old_path) in deletions.iter().enumerate() {
            let Some(old_state) = remote.get(old_path) else {
                continue;
            };
            let candidates = &rename_candidates[delete_index];
            if candidates.len() == 1 {
                let add_index = candidates[0];
                if reverse_candidate_counts[add_index] != 1 {
                    continue;
                }
                if directory_renames
                    .iter()
                    .any(|(from, _): &(WorkspacePath, WorkspacePath)| {
                        path_is_descendant(old_path, from)
                    })
                {
                    continue;
                }
                paired_additions.insert(add_index);
                paired_deletions.insert(delete_index);
                let new_path = additions[add_index].0.clone();
                renames.push(FsChange::Rename {
                    from: old_path.clone(),
                    to: new_path.clone(),
                });
                if old_state.kind == WorkspaceEntryKind::Directory
                    && additions[add_index].1.kind == WorkspaceEntryKind::Directory
                {
                    directory_renames.push((old_path.clone(), new_path));
                }
            }
        }

        let mut changes =
            additions
                .into_iter()
                .enumerate()
                .filter(|(index, (path, _))| {
                    !paired_additions.contains(index)
                        && !directory_renames.iter().any(
                            |(_, to): &(WorkspacePath, WorkspacePath)| path_is_descendant(path, to),
                        )
                })
                .map(|(_, (path, _))| FsChange::Create(path))
                .collect::<Vec<_>>();
        changes.extend(updates.into_iter().map(|(path, _)| FsChange::Update(path)));
        changes.extend(
            deletions
                .into_iter()
                .enumerate()
                .filter(|(index, path)| {
                    !paired_deletions.contains(index)
                        && !directory_renames.iter().any(
                            |(from, _): &(WorkspacePath, WorkspacePath)| {
                                path_is_descendant(path, from)
                            },
                        )
                })
                .map(|(_, path)| FsChange::Delete(path)),
        );
        renames.retain(|change| {
            let FsChange::Rename { from, .. } = change else {
                return true;
            };
            !directory_renames
                .iter()
                .any(|(directory_from, _)| path_is_descendant(from, directory_from))
        });
        renames.sort_by(|left, right| {
            let FsChange::Rename {
                from: left_from,
                to: left_to,
            } = left
            else {
                return std::cmp::Ordering::Equal;
            };
            let FsChange::Rename {
                from: right_from,
                to: right_to,
            } = right
            else {
                return std::cmp::Ordering::Equal;
            };
            left_from
                .cmp(right_from)
                .then_with(|| left_to.cmp(right_to))
        });
        changes.extend(renames);
        Ok(changes)
    }

    pub fn record_local_change(&mut self, change: FsChange) -> Result<(), SyncError> {
        self.record_local_changes([change])
    }

    pub fn local(&mut self, change: FsChange) -> Result<(), SyncError> {
        self.record_local_change(change)
    }

    pub fn local_changes<I>(&mut self, changes: I) -> Result<(), SyncError>
    where
        I: IntoIterator<Item = FsChange>,
    {
        self.record_local_changes(changes)
    }

    pub fn record_changes<I>(&mut self, changes: I) -> Result<(), SyncError>
    where
        I: IntoIterator<Item = FsChange>,
    {
        self.record_local_changes(changes)
    }

    pub fn record_local_changes<I>(&mut self, changes: I) -> Result<(), SyncError>
    where
        I: IntoIterator<Item = FsChange>,
    {
        self.ensure_open()?;
        let mut pending = changes.into_iter().collect::<VecDeque<_>>();
        while let Some(change) = pending.pop_front() {
            if change == FsChange::RescanRequired {
                pending.extend(self.scan_changes()?);
                continue;
            }
            let desired = self.desired_from_change(&change)?;
            if self.runtime.state.stream_state()?.is_some() {
                let states = self.path_state_map()?;
                if !desired_matches_remote(&desired, &states) {
                    self.defer_desired(desired)?;
                }
            } else {
                self.record_desired(desired)?;
            }
        }
        Ok(())
    }

    pub fn pending_commands(&mut self, limit: usize) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut commands = self.resume_inbound_work(limit, true)?;
        if commands.len() == limit {
            return Ok(commands);
        }
        if commands.is_empty()
            && self.runtime.state.stream_state()?.is_none()
            && self.pending_live_events.is_empty()
            && !self.runtime.state.has_apply_journals()?
        {
            self.materialize_local_intents()?;
        }
        let remaining = limit - commands.len();
        let records = self.runtime.state.pending_outbox_replay(remaining)?;
        for record in records {
            let command = match record.decoded_body().map_err(|_| SyncError::CorruptState {
                table: "outbox",
                field: "body_json",
            })? {
                OutboxBody::Mutation(mutation) => SyncCommand::Mutation(mutation),
                OutboxBody::ConflictResolution(resolution) => {
                    SyncCommand::ResolveConflict(resolution)
                }
            };
            commands.push(command);
        }
        if commands.len() < limit {
            for record in self.runtime.state.outbox()? {
                if record.stage != OutboxStage::AwaitingBlob {
                    continue;
                }
                let (workspace_id, operation_id, content_hash, size) =
                    match record.decoded_body().map_err(|_| SyncError::CorruptState {
                        table: "outbox",
                        field: "body_json",
                    })? {
                        OutboxBody::Mutation(mutation) => (
                            mutation.workspace_id,
                            mutation.operation_id,
                            mutation.content_hash,
                            mutation.metadata.size,
                        ),
                        OutboxBody::ConflictResolution(resolution) => (
                            resolution.workspace_id,
                            resolution.operation_id,
                            resolution.content_hash,
                            resolution.metadata.size,
                        ),
                    };
                let hash = content_hash
                    .into_option()
                    .ok_or(SyncError::ProtocolInvariant {
                        reason: "blob_without_content_hash",
                    })?;
                commands.push(SyncCommand::UploadBlob {
                    workspace_id,
                    operation_id,
                    content_hash: hash,
                    size,
                });
                if commands.len() == limit {
                    break;
                }
            }
        }
        if commands.len() < limit
            && let Some(revision) = self.runtime.state.cursor()?.pending_ack_revision
        {
            commands.push(SyncCommand::SendAck(WorkspaceAckRequest {
                workspace_id: self.runtime.state.workspace_id(),
                client_id: self.runtime.state.client_id(),
                revision,
            }));
        }
        Ok(commands)
    }

    /// Prepare durable outbox state for a fresh transport connection. Any
    /// upload whose prior Need/Begin/chunks/End exchange was interrupted is
    /// retried from its exact immutable Mutation body so the server can either
    /// accept existing CAS content or issue a fresh BlobRequired/BlobNeed.
    pub fn prepare_connection_attempt(&mut self) -> Result<usize, SyncError> {
        self.ensure_open()?;
        self.runtime.state.replay_awaiting_blob_mutations()
    }

    pub fn outbox(&self) -> Result<Vec<crate::OutboxRecord>, SyncError> {
        self.runtime.state.outbox()
    }

    pub fn list_conflicts(&self) -> Result<Vec<ConflictView>, SyncError> {
        self.ensure_open()?;
        let mut views = self
            .runtime
            .state
            .conflicts()?
            .iter()
            .map(|record| self.conflict_view(record))
            .collect::<Result<Vec<_>, _>>()?;
        views.sort_by_key(|view| view.conflict_id);
        Ok(views)
    }

    pub fn resolve_conflict(
        &mut self,
        conflict_id: fns_protocol::ConflictId,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
        choice: fns_protocol::WorkspaceConflictChoice,
    ) -> Result<ConflictResolutionReceipt, SyncError> {
        self.ensure_open()?;
        let conflict = self
            .runtime
            .state
            .conflict(conflict_id)?
            .ok_or(SyncError::ConflictUnavailable)?;
        let created = decode_conflict_created(&conflict)?;
        if conflict_revision != created.conflict_revision {
            return Err(SyncError::ConflictRevisionStale);
        }

        match conflict.status {
            ConflictStatus::Manual | ConflictStatus::Resolving => {}
            ConflictStatus::WaitingBlobs => {
                return Err(SyncError::ConflictResolutionBlocked {
                    reason: ConflictBlockedReason::WaitingBlobs,
                });
            }
            ConflictStatus::AutoReady => {
                return Err(SyncError::ConflictResolutionBlocked {
                    reason: ConflictBlockedReason::AutomaticResolutionPending,
                });
            }
            ConflictStatus::RefreshRequired => {
                return Err(SyncError::ConflictResolutionBlocked {
                    reason: ConflictBlockedReason::RefreshRequired,
                });
            }
        }

        let proposal = self.conflict_resolution_proposal(&created, choice)?;
        if conflict.status == ConflictStatus::Resolving {
            let existing =
                decode_pending_resolution(&conflict, &created, self.runtime.state.client_id())?
                    .ok_or(SyncError::CorruptState {
                        table: "conflicts",
                        field: "resolution_json",
                    })?;
            if !proposal.matches(&existing) {
                return Err(SyncError::ConflictResolutionChanged);
            }
            self.queue_conflict_resolution(existing.clone())?;
            return Ok(ConflictResolutionReceipt {
                status: ConflictResolutionReceiptStatus::Queued,
                operation_id: existing.operation_id,
            });
        }

        if conflict.resolution_json.is_some() || conflict.resolution_digest.is_some() {
            return Err(SyncError::CorruptState {
                table: "conflicts",
                field: "resolution_json",
            });
        }
        let request = proposal.into_request(
            self.runtime.state.workspace_id(),
            self.runtime.state.client_id(),
            self.next_operation_id()?,
            conflict_id,
            conflict_revision,
        );
        self.queue_conflict_resolution(request.clone())?;
        Ok(ConflictResolutionReceipt {
            status: ConflictResolutionReceiptStatus::Queued,
            operation_id: request.operation_id,
        })
    }

    fn conflict_view(&self, conflict: &ConflictRecord) -> Result<ConflictView, SyncError> {
        let created = decode_conflict_created(conflict)?;
        let pending_resolution =
            decode_pending_resolution(conflict, &created, self.runtime.state.client_id())?;
        if (conflict.status == ConflictStatus::Resolving) != pending_resolution.is_some()
            && conflict.status != ConflictStatus::RefreshRequired
        {
            return Err(SyncError::CorruptState {
                table: "conflicts",
                field: "resolution_json",
            });
        }
        let (can_resolve, blocked_reason) = match conflict.status {
            ConflictStatus::Manual => (true, None),
            ConflictStatus::WaitingBlobs => (false, Some(ConflictBlockedReason::WaitingBlobs)),
            ConflictStatus::AutoReady => (
                false,
                Some(ConflictBlockedReason::AutomaticResolutionPending),
            ),
            ConflictStatus::Resolving => (false, Some(ConflictBlockedReason::ResolutionPending)),
            ConflictStatus::RefreshRequired => {
                (false, Some(ConflictBlockedReason::RefreshRequired))
            }
        };
        Ok(ConflictView {
            conflict_id: created.conflict_id,
            conflict_revision: created.conflict_revision,
            path: created.path,
            kind: created.kind,
            status: conflict.status,
            ancestor: ConflictSideView::from(&created.ancestor),
            current: ConflictSideView::from(&created.current),
            incoming: ConflictSideView::from(&created.incoming),
            created_by_operation_id: created.created_by_operation_id,
            pending_resolution: pending_resolution.map(|request| {
                let is_delete = request.choice == fns_protocol::WorkspaceConflictChoice::Delete;
                PendingConflictResolutionView {
                    operation_id: request.operation_id,
                    choice: request.choice,
                    content_hash: request.content_hash.into_option(),
                    size: (!is_delete).then_some(request.metadata.size),
                }
            }),
            can_resolve,
            blocked_reason,
        })
    }

    fn conflict_resolution_proposal(
        &mut self,
        created: &WorkspaceConflictCreatedMessage,
        choice: fns_protocol::WorkspaceConflictChoice,
    ) -> Result<ConflictResolutionProposal, SyncError> {
        match choice {
            fns_protocol::WorkspaceConflictChoice::Current
            | fns_protocol::WorkspaceConflictChoice::Incoming => {
                let side = if choice == fns_protocol::WorkspaceConflictChoice::Current {
                    &created.current
                } else {
                    &created.incoming
                };
                if side.tombstone {
                    return Err(SyncError::ConflictResolutionBlocked {
                        reason: ConflictBlockedReason::SelectedSideDeleted,
                    });
                }
                let path = side
                    .path
                    .clone()
                    .into_option()
                    .ok_or(SyncError::CorruptState {
                        table: "conflicts",
                        field: "created_json",
                    })?;
                Ok(ConflictResolutionProposal {
                    choice,
                    path,
                    content_hash: side.content_hash.clone(),
                    metadata: side.metadata.clone(),
                })
            }
            fns_protocol::WorkspaceConflictChoice::Merged => {
                let observed = self.runtime.system.workspace.inspect(&created.path)?;
                if observed.is_none_or(|entry| entry.kind != WorkspaceEntryKind::File) {
                    return Err(SyncError::MergeRejected {
                        reason: "merged_file_required",
                    });
                }
                let descriptor = self.runtime.system.content_cache.stage_workspace_entry(
                    &self.runtime.system.workspace,
                    &created.path,
                    &mut self.runtime.state,
                )?;
                Ok(ConflictResolutionProposal {
                    choice,
                    path: created.path.clone(),
                    content_hash: RequiredNullable::Value(descriptor.content_hash),
                    metadata: descriptor.metadata,
                })
            }
            fns_protocol::WorkspaceConflictChoice::Delete => Ok(ConflictResolutionProposal {
                choice,
                path: created.path.clone(),
                content_hash: RequiredNullable::Null,
                metadata: fns_protocol::WorkspaceFileMetadata {
                    size: 0,
                    modified_at_ms: 0,
                    executable: false,
                },
            }),
        }
    }

    /// Validate and durably queue one immutable conflict resolution proposal.
    pub fn queue_conflict_resolution(
        &mut self,
        request: WorkspaceConflictResolvedRequest,
    ) -> Result<(), SyncError> {
        self.ensure_open()?;
        request
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict_resolution",
            })?;
        self.validate_identity(request.workspace_id, request.client_id)?;
        let mut conflict = self
            .runtime
            .state
            .conflict(request.conflict_id)?
            .ok_or(SyncError::ConflictUnavailable)?;
        let created: WorkspaceConflictCreatedMessage =
            serde_json::from_slice(&conflict.created_json).map_err(|_| {
                SyncError::CorruptState {
                    table: "conflicts",
                    field: "created_json",
                }
            })?;
        request.validate_against(&created).map_err(|error| {
            if error.reason == "conflict_revision_stale" {
                SyncError::ConflictRevisionStale
            } else {
                SyncError::ProtocolInvariant {
                    reason: "conflict_resolution_mismatch",
                }
            }
        })?;
        let resolution_json = canonical_json(&request)?;
        let resolution_digest = crate::body_digest(&resolution_json);
        if let Some(existing_json) = conflict.resolution_json.as_deref() {
            if existing_json != resolution_json
                || conflict.resolution_digest != Some(resolution_digest)
            {
                return Err(SyncError::OperationChanged);
            }
            if let Some(outbox) = self.runtime.state.outbox_entry(request.operation_id)?
                && (outbox.body_json != resolution_json || outbox.body_digest != resolution_digest)
            {
                return Err(SyncError::OperationChanged);
            }
            return Ok(());
        }
        if let Some(existing) = self.runtime.state.outbox_entry(request.operation_id)?
            && (existing.body_json != resolution_json || existing.body_digest != resolution_digest)
        {
            return Err(SyncError::OperationChanged);
        }
        if request.choice == fns_protocol::WorkspaceConflictChoice::Merged {
            let hash =
                request
                    .content_hash
                    .clone()
                    .into_option()
                    .ok_or(SyncError::ProtocolInvariant {
                        reason: "merged_resolution_without_content_hash",
                    })?;
            if !self.content_available(&hash, request.metadata.size)? {
                return Err(SyncError::MergeRejected {
                    reason: "merged_content_unavailable",
                });
            }
            conflict.candidate_hash = Some(hash.to_string());
        }
        conflict.status = ConflictStatus::Resolving;
        conflict.resolution_json = Some(resolution_json);
        conflict.resolution_digest = Some(resolution_digest);
        self.runtime.state.transaction(|tx| {
            tx.enqueue_conflict_resolution(&request)?;
            tx.put_conflict(&conflict)
        })
    }

    /// Settle only the durable request row for a correlated success response.
    /// The authoritative push remains responsible for tree/filesystem apply.
    pub fn conflict_resolution_accepted(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
    ) -> Result<(), SyncError> {
        self.ensure_open()?;
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict_resolution",
            })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "conflict_workspace_mismatch",
            });
        }
        let Some(conflict) = self.runtime.state.conflict(message.conflict_id)? else {
            let body_digest = crate::body_digest(&canonical_json(&message)?);
            return if self.conflict_resolution_receipt_match(&message, body_digest)?
                == AppliedReceiptMatch::Exact
            {
                Ok(())
            } else {
                Err(SyncError::ConflictUnavailable)
            };
        };
        let request_json =
            conflict
                .resolution_json
                .as_deref()
                .ok_or(SyncError::ProtocolInvariant {
                    reason: "conflict_resolution_not_outstanding",
                })?;
        let request: WorkspaceConflictResolvedRequest = serde_json::from_slice(request_json)
            .map_err(|_| SyncError::CorruptState {
                table: "conflicts",
                field: "resolution_json",
            })?;
        if !resolved_matches_request(&message, &request) {
            return Err(SyncError::OperationChanged);
        }
        match self.runtime.state.outbox_entry(request.operation_id)? {
            Some(outbox) if outbox.body_json == request_json => {
                self.runtime.state.remove_outbox(request.operation_id)
            }
            Some(_) => Err(SyncError::OperationChanged),
            None => Ok(()),
        }
    }

    /// Preserve or transition durable work after a correlated failure.
    pub fn conflict_resolution_rejected(
        &mut self,
        operation_id: OperationId,
        code: WorkspaceV2ErrorCode,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        let Some(outbox) = self.runtime.state.outbox_entry(operation_id)? else {
            // An authoritative resolution from another client can settle the
            // conflict after this request was sent but before its response is
            // received. The losing request then legitimately receives one of
            // these terminal errors after its durable row has been removed.
            if matches!(
                code,
                WorkspaceV2ErrorCode::ConflictRevisionStale
                    | WorkspaceV2ErrorCode::ConflictNotFound
            ) {
                return Ok(Vec::new());
            }
            return Err(SyncError::ProtocolInvariant {
                reason: "conflict_resolution_not_outstanding",
            });
        };
        let OutboxBody::ConflictResolution(request) =
            outbox.decoded_body().map_err(|_| SyncError::CorruptState {
                table: "outbox",
                field: "body_json",
            })?
        else {
            return Err(SyncError::ProtocolInvariant {
                reason: "operation_not_conflict_resolution",
            });
        };
        match code {
            WorkspaceV2ErrorCode::BlobRequired => {
                let hash = request.content_hash.clone().into_option().ok_or(
                    SyncError::ProtocolInvariant {
                        reason: "blob_required_without_content_hash",
                    },
                )?;
                self.runtime
                    .state
                    .set_outbox_stage(operation_id, OutboxStage::AwaitingBlob)?;
                Ok(vec![SyncCommand::UploadBlob {
                    workspace_id: request.workspace_id,
                    operation_id,
                    content_hash: hash,
                    size: request.metadata.size,
                }])
            }
            WorkspaceV2ErrorCode::ConflictRevisionStale
            | WorkspaceV2ErrorCode::ConflictNotFound => {
                self.runtime.state.transaction(|tx| {
                    tx.remove_outbox(operation_id)?;
                    tx.set_conflict_status(request.conflict_id, ConflictStatus::RefreshRequired)
                })?;
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn pending_outbox(&mut self, limit: usize) -> Result<Vec<SyncCommand>, SyncError> {
        self.pending_commands(limit)
    }

    pub fn mutation_result(
        &mut self,
        result: MutationResult,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        match result {
            MutationResult::Accepted(message) => self.mutation_accepted(message),
            MutationResult::Rejected(message) => self.mutation_rejected(message),
        }
    }

    pub fn handle_mutation_result(
        &mut self,
        result: MutationResult,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_result(result)
    }

    pub fn mutation_accepted(
        &mut self,
        accepted: WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        accepted
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_mutation_accepted",
            })?;
        let receipt_match = self.mutation_acceptance_match(&accepted)?;
        self.validate_identity(accepted.workspace_id, accepted.client_id)?;
        if receipt_match == MutationAcceptanceMatch::Exact {
            return Ok(Vec::new());
        }
        let record = self.runtime.state.outbox_entry(accepted.operation_id)?;
        let Some(record) = record else {
            return Err(SyncError::ProtocolInvariant {
                reason: "mutation_not_outstanding",
            });
        };
        let mutation = record
            .mutation()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "outbox_not_mutation",
            })?;
        validate_acceptance_shape(&mutation, &accepted)?;
        if receipt_match == MutationAcceptanceMatch::LegacyUnbound {
            let receipt = self
                .runtime
                .state
                .applied_operation(accepted.client_id, accepted.operation_id)?
                .ok_or(SyncError::CorruptState {
                    table: "applied_operations",
                    field: "receipt",
                })?;
            if receipt.body_digest != legacy_mutation_digest(&mutation)? {
                return Err(SyncError::OperationChanged);
            }
            self.runtime
                .state
                .transaction(|tx| tx.record_provisional_mutation_acceptance(&accepted))?;
            return Ok(Vec::new());
        }
        let operation_digest = applied_operation_digest(
            &mutation,
            &accepted.path_state,
            accepted.old_path_state.as_ref(),
            accepted.new_path_state.as_ref(),
        )?;
        let touched = mutation_paths(&mutation);
        let intents = self.deferred_operations(&touched)?;
        let settled_paths = paths_for_desired_operations(&touched, &intents);
        let mut states = self.path_state_map()?;
        if let Some(old) = &accepted.old_path_state {
            states.insert(old.path.clone(), old.clone());
        }
        if let Some(new) = &accepted.new_path_state {
            states.insert(new.path.clone(), new.clone());
        }
        states.insert(
            accepted.path_state.path.clone(),
            accepted.path_state.clone(),
        );
        let next_mutations = self.next_mutations(&intents, &states)?;
        let client_id = self.runtime.state.client_id();
        self.runtime.state.transaction(|tx| {
            if let Some(old) = &accepted.old_path_state {
                tx.put_path_state(old)?;
            }
            if let Some(new) = &accepted.new_path_state {
                tx.put_path_state(new)?;
            }
            tx.put_path_state(&accepted.path_state)?;
            tx.remove_outbox(accepted.operation_id)?;
            tx.record_mutation_applied_operation(
                client_id,
                accepted.operation_id,
                accepted.revision,
                operation_digest,
                &mutation,
                None,
            )?;
            for path in &settled_paths {
                tx.remove_local_intent(path)?;
            }
            for (mutation, timestamp) in &next_mutations {
                tx.enqueue_mutation_at(mutation, *timestamp)?;
            }
            Ok(())
        })?;
        Ok(Vec::new())
    }

    pub fn on_mutation_accepted(
        &mut self,
        accepted: WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_accepted(accepted)
    }

    pub fn mutation_result_accepted(
        &mut self,
        accepted: WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_accepted(accepted)
    }

    pub fn handle_mutation_accepted(
        &mut self,
        accepted: WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_accepted(accepted)
    }

    pub fn mutation_rejected(
        &mut self,
        rejected: WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        rejected
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_mutation_rejected",
            })?;
        self.validate_identity(rejected.workspace_id, rejected.client_id)?;
        if rejected.reason == WorkspaceMutationRejectReason::OperationReused {
            return Err(SyncError::ProtocolInvariant {
                reason: "operation_reused",
            });
        }
        let record = self
            .runtime
            .state
            .outbox_entry(rejected.operation_id)?
            .ok_or(SyncError::ProtocolInvariant {
                reason: "mutation_not_outstanding",
            })?;
        let mutation = record
            .mutation()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "outbox_not_mutation",
            })?;
        match rejected.reason {
            WorkspaceMutationRejectReason::BlobRequired => {
                let required_hash = rejected.required_hash.as_ref().into_option().ok_or(
                    SyncError::ProtocolInvariant {
                        reason: "blob_hash_missing",
                    },
                )?;
                if mutation.content_hash.as_ref() != RequiredNullable::Value(required_hash) {
                    return Err(SyncError::ProtocolInvariant {
                        reason: "blob_hash_mismatch",
                    });
                }
                self.runtime
                    .state
                    .set_outbox_stage(rejected.operation_id, OutboxStage::AwaitingBlob)?;
                Ok(Vec::new())
            }
            WorkspaceMutationRejectReason::ConflictCreated => {
                self.runtime
                    .state
                    .set_outbox_stage(rejected.operation_id, OutboxStage::BlockedConflict)?;
                Ok(Vec::new())
            }
            WorkspaceMutationRejectReason::StaleBaseRevision => {
                self.reconcile_stale(mutation, rejected.current_path_state)
            }
            WorkspaceMutationRejectReason::OperationReused => unreachable!(),
        }
    }

    pub fn on_mutation_rejected(
        &mut self,
        rejected: WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_rejected(rejected)
    }

    pub fn mutation_result_rejected(
        &mut self,
        rejected: WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_rejected(rejected)
    }

    pub fn handle_mutation_rejected(
        &mut self,
        rejected: WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.mutation_rejected(rejected)
    }

    /// Called after a blob upload completes successfully.
    /// Flips the outbox entry from AwaitingBlob → Dispatched so the next
    /// pending_commands cycle re-sends the exact mutation body. The transport
    /// must call this only after the server has acknowledged BlobEnd.
    pub fn blob_uploaded(&mut self, operation_id: OperationId) -> Result<(), SyncError> {
        self.ensure_open()?;
        let Some(record) = self.runtime.state.outbox_entry(operation_id)? else {
            return Err(SyncError::ProtocolInvariant {
                reason: "mutation_not_outstanding",
            });
        };
        match record.stage {
            OutboxStage::AwaitingBlob => self
                .runtime
                .state
                .set_outbox_stage(operation_id, OutboxStage::Dispatched),
            // A duplicated BlobEnd response is harmless after the first
            // completion; the durable mutation is already replayable.
            OutboxStage::Dispatched => Ok(()),
            OutboxStage::Queued | OutboxStage::BlockedConflict => {
                Err(SyncError::ProtocolInvariant {
                    reason: "blob_upload_not_awaiting",
                })
            }
        }
    }

    pub fn event(&mut self, event: WorkspaceEventMessage) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        event.validate().map_err(|_| SyncError::ProtocolInvariant {
            reason: "invalid_workspace_event",
        })?;
        if event.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "event_workspace_mismatch",
            });
        }
        let own_event = event.origin_client_id == self.runtime.state.client_id();
        match self.event_receipt_match(&event)? {
            AppliedReceiptMatch::Exact => {
                if !self.pending_live_revision_precedes_or_matches(event.revision) {
                    self.mark_live_event_applied(event.revision)?;
                    return Ok(Vec::new());
                }
            }
            AppliedReceiptMatch::Legacy { .. } => {}
            AppliedReceiptMatch::Missing
                if own_event
                    && self
                        .runtime
                        .state
                        .outbox_entry(event.operation_id)?
                        .is_none() =>
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_not_outstanding",
                });
            }
            AppliedReceiptMatch::Missing => {}
        }
        let event_body = canonical_json(&event)?;
        self.enqueue_live_message(PendingLiveMessage::Event(Box::new(event)), event_body)
    }

    fn enqueue_live_message(
        &mut self,
        message: PendingLiveMessage,
        body: Vec<u8>,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        let revision = message.revision();
        let body_digest = crate::body_digest(&body);
        if let Some(pending) = self
            .pending_live_events
            .iter()
            .find(|pending| pending.message.revision() == revision)
        {
            if pending.body_digest != body_digest
                || std::mem::discriminant(&pending.message) != std::mem::discriminant(&message)
            {
                return Err(SyncError::OperationChanged);
            }
            return self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, true);
        }
        if self
            .pending_live_events
            .back()
            .is_some_and(|pending| revision < pending.message.revision())
        {
            return Err(SyncError::StreamInvariant {
                reason: "live_revision_order",
            });
        }
        let next_serialized_bytes = self
            .pending_live_serialized_bytes
            .checked_add(body.len())
            .ok_or(SyncError::ResourceLimit {
                resource: "pending_live_events",
            })?;
        if self.pending_live_events.len() >= self.inbound_work_limits.max_pending_live_items
            || next_serialized_bytes > self.inbound_work_limits.max_pending_live_serialized_bytes
        {
            return Err(SyncError::ResourceLimit {
                resource: "pending_live_events",
            });
        }
        self.pending_live_events.push_back(PendingLiveItem {
            message,
            body_digest,
            serialized_bytes: body.len(),
        });
        self.pending_live_serialized_bytes = next_serialized_bytes;
        self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, true)
    }

    fn pending_live_revision_precedes_or_matches(
        &self,
        revision: fns_protocol::WorkspaceRevision,
    ) -> bool {
        self.pending_live_events
            .front()
            .is_some_and(|pending| pending.message.revision() <= revision)
    }

    fn mark_live_event_applied(
        &mut self,
        revision: fns_protocol::WorkspaceRevision,
    ) -> Result<(), SyncError> {
        let cursor = self.runtime.state.cursor()?;
        if revision <= cursor.last_ack_revision {
            return Ok(());
        }
        self.runtime.state.transaction(|tx| {
            if revision > cursor.last_applied_revision {
                tx.set_last_applied_revision(revision)?;
            }
            if cursor
                .pending_ack_revision
                .is_none_or(|pending| revision > pending)
            {
                tx.set_pending_ack(revision)?;
            }
            Ok(())
        })
    }

    pub fn on_event(
        &mut self,
        event: WorkspaceEventMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.event(event)
    }

    pub fn snapshot_begin(
        &mut self,
        message: WorkspaceSnapshotBeginMessage,
    ) -> Result<(), SyncError> {
        self.ensure_open()?;
        message.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_begin",
        })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let starts_new_stream = self
            .runtime
            .state
            .stream_state()?
            .is_none_or(|active| active.stream_id != message.stream_id);
        self.runtime.state.begin_stream(&message)?;
        if starts_new_stream {
            self.pending_live_events.clear();
            self.pending_live_serialized_bytes = 0;
            self.next_inbound_source = InboundWorkSource::Stream;
        }
        Ok(())
    }

    pub fn snapshot_entry(
        &mut self,
        message: WorkspaceSnapshotEntryMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        message.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_stream_entry",
        })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let status = self.status_for_state(&message.entry)?;
        let needs_download = status == StreamItemStatus::WaitingBlob;
        let staged = self.runtime.state.put_stream_entry(&message, status)?;
        if matches!(
            staged.status,
            StreamItemStatus::Applied | StreamItemStatus::Preserved
        ) {
            return Ok(Vec::new());
        }
        let mut commands =
            self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.entry) {
            push_download(
                &mut commands,
                self.inbound_work_limits.max_items_per_call,
                self.runtime.state.workspace_id(),
                None,
                hash,
                size,
            );
        }
        Ok(commands)
    }

    pub fn workspace_event(
        &mut self,
        message: WorkspaceEventMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_workspace_event",
            })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "event_workspace_mismatch",
            });
        }
        let body = canonical_json(&message)?;
        if let Some(existing) = self
            .runtime
            .state
            .stream_revision_item(message.stream_id, message.revision)?
        {
            if existing.event_index != Some(message.index) {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_revision_order",
                });
            }
            if existing.body_digest != crate::body_digest(&body) {
                return Err(SyncError::OperationChanged);
            }
            if existing.status == StreamItemStatus::Applied
                || existing.status == StreamItemStatus::Preserved
            {
                return Ok(Vec::new());
            }
        }
        if self.event_receipt_match(&message)? == AppliedReceiptMatch::Exact {
            self.runtime
                .state
                .put_stream_event(&message, StreamItemStatus::Applied)?;
            return self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false);
        }
        let diverged = self.event_is_diverged(&message)?;
        let status = if diverged {
            StreamItemStatus::Ready
        } else {
            self.status_for_state(&message.path_state)?
        };
        let needs_download = status == StreamItemStatus::WaitingBlob;
        self.runtime.state.put_stream_event(&message, status)?;
        let mut commands =
            self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.path_state) {
            push_download(
                &mut commands,
                self.inbound_work_limits.max_items_per_call,
                self.runtime.state.workspace_id(),
                Some(message.operation_id),
                hash,
                size,
            );
        }
        Ok(commands)
    }

    pub fn conflict_created(
        &mut self,
        message: WorkspaceConflictCreatedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict",
            })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        if let Some(active) = self.runtime.state.stream_state()? {
            self.runtime.state.put_stream_conflict(
                &message,
                StreamConflictStatus::Received,
                active.stream_id,
            )?;
            return self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false);
        }

        let created_json = canonical_json(&message)?;
        if let Some(existing) = self.runtime.state.conflict(message.conflict_id)? {
            if existing.conflict_revision == message.conflict_revision {
                return if existing.created_json == created_json {
                    Ok(Vec::new())
                } else {
                    Err(SyncError::OperationChanged)
                };
            }
            if existing.status != ConflictStatus::RefreshRequired {
                return Err(SyncError::OperationChanged);
            }
            let prior_resolution = existing
                .resolution_json
                .as_deref()
                .map(|json| {
                    serde_json::from_slice::<WorkspaceConflictResolvedRequest>(json).map_err(|_| {
                        SyncError::CorruptState {
                            table: "conflicts",
                            field: "resolution_json",
                        }
                    })
                })
                .transpose()?;
            let replacement = ConflictRecord {
                conflict_id: message.conflict_id,
                workspace_id: message.workspace_id,
                conflict_revision: message.conflict_revision,
                created_json,
                status: ConflictStatus::Manual,
                candidate_hash: None,
                resolution_json: None,
                resolution_digest: None,
            };
            self.runtime.state.transaction(|tx| {
                if let Some(resolution) = &prior_resolution {
                    tx.remove_outbox(resolution.operation_id)?;
                }
                tx.put_conflict(&replacement)
            })?;
            return Ok(Vec::new());
        }
        self.runtime
            .state
            .record_conflict(&message, ConflictStatus::Manual)?;
        Ok(Vec::new())
    }

    pub fn conflict_resolved(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_stream_conflict_resolution",
            })?;
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let Some(active) = self.runtime.state.stream_state()? else {
            let body_digest = crate::body_digest(&canonical_json(&message)?);
            let receipt_match = self.conflict_resolution_receipt_match(&message, body_digest)?;
            if receipt_match == AppliedReceiptMatch::Exact
                && !self.pending_live_revision_precedes_or_matches(message.revision)
            {
                self.mark_live_event_applied(message.revision)?;
                return Ok(Vec::new());
            }
            if receipt_match == AppliedReceiptMatch::Missing
                && message.revision <= self.runtime.state.cursor()?.last_applied_revision
            {
                return Err(SyncError::StreamInvariant {
                    reason: "live_revision_regression",
                });
            }
            let body = canonical_json(&message)?;
            return self.enqueue_live_message(PendingLiveMessage::ConflictResolved(message), body);
        };
        let stream_id = active.stream_id;
        if let Some(existing) = self
            .runtime
            .state
            .stream_revision_item(stream_id, message.revision)?
        {
            let body = canonical_json(&message)?;
            if existing.body_digest != crate::body_digest(&body) {
                return Err(SyncError::OperationChanged);
            }
            if existing.status == StreamItemStatus::Applied
                || existing.status == StreamItemStatus::Preserved
            {
                return Ok(Vec::new());
            }
        }
        let body_digest = crate::body_digest(&canonical_json(&message)?);
        if self.conflict_resolution_receipt_match(&message, body_digest)?
            == AppliedReceiptMatch::Exact
        {
            self.runtime.state.put_stream_conflict_resolved(
                &message,
                None,
                StreamItemStatus::Applied,
            )?;
            return self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false);
        }
        let status = self.status_for_state(&message.path_state)?;
        let needs_download = status == StreamItemStatus::WaitingBlob;
        self.runtime
            .state
            .put_stream_conflict_resolved(&message, None, status)?;
        let mut commands =
            self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.path_state) {
            push_download(
                &mut commands,
                self.inbound_work_limits.max_items_per_call,
                self.runtime.state.workspace_id(),
                Some(message.operation_id),
                hash,
                size,
            );
        }
        Ok(commands)
    }

    pub fn snapshot_end(
        &mut self,
        message: WorkspaceSnapshotEndMessage,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        let active = self
            .runtime
            .state
            .stream_state()?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?;
        let begin = WorkspaceSnapshotBeginMessage {
            workspace_id: active.workspace_id,
            stream_id: active.stream_id,
            mode: active.mode,
            from_revision: active.from_revision,
            final_revision: active.final_revision,
            entry_count: active.expected_entry_count,
            event_count: active.expected_event_count,
            conflict_count: active.expected_conflict_count,
        };
        message
            .validate_against(&begin)
            .map_err(|_| SyncError::StreamInvariant {
                reason: "end_mismatch",
            })?;
        self.runtime.state.set_stream_end_received(true)?;
        self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)
    }

    pub fn blob_available<R: Read>(
        &mut self,
        hash: fns_protocol::WorkspaceContentHash,
        size: u64,
        reader: R,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        self.runtime
            .system
            .content_cache
            .import(&hash, size, reader)?;
        self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)
    }

    pub fn begin_blob_import(
        &mut self,
        hash: fns_protocol::WorkspaceContentHash,
        size: u64,
    ) -> Result<StagedContentImport, SyncError> {
        self.ensure_open()?;
        Ok(self
            .runtime
            .system
            .content_cache
            .begin_staged_import(hash, size)?)
    }

    pub fn commit_blob_import(
        &mut self,
        sealed: SealedContentImport,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        sealed.commit()?;
        self.resume_inbound_work(self.inbound_work_limits.max_items_per_call, false)
    }

    pub fn ack_confirmed(&mut self, message: WorkspaceAckRequest) -> Result<(), SyncError> {
        self.ensure_open()?;
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_ack",
            })?;
        self.validate_identity(message.workspace_id, message.client_id)?;
        let cursor = self.runtime.state.cursor()?;
        let Some(pending) = cursor.pending_ack_revision else {
            if message.revision == cursor.last_ack_revision {
                return Ok(());
            }
            return Err(SyncError::ProtocolInvariant {
                reason: "ack_not_pending",
            });
        };
        if message.revision <= cursor.last_ack_revision
            || message.revision > pending
            || message.revision > cursor.last_applied_revision
        {
            return Err(SyncError::ProtocolInvariant {
                reason: "ack_mismatch",
            });
        }
        self.runtime.state.transaction(|tx| {
            tx.set_last_ack_revision(message.revision)?;
            if message.revision == pending {
                tx.clear_pending_ack()?;
                tx.clear_stream()?;
            }
            Ok(())
        })
    }

    pub fn close(&mut self) -> Result<(), SyncError> {
        if self.closed {
            return Ok(());
        }
        // Drop the durable SQLite connection before marking the engine closed;
        // the fixture can then safely discard this engine during reopen.
        self.runtime.state.close()?;
        self.closed = true;
        Ok(())
    }

    fn recover_apply_journals(&mut self) -> Result<(), SyncError> {
        for record in self.runtime.state.apply_journals()? {
            if record.workspace_id != self.runtime.state.workspace_id() {
                return Err(SyncError::CorruptState {
                    table: "apply_journal",
                    field: "workspace_id",
                });
            }
            if record.operation_body_digest == [0; 32] {
                if !is_migrated_legacy_apply_journal(&record) {
                    return Err(SyncError::CorruptState {
                        table: "apply_journal",
                        field: "operation_body_digest",
                    });
                }
                self.recover_legacy_apply_journal(&record)?;
                continue;
            }
            if crate::apply_journal_immutable_digest(&record) != record.operation_body_digest {
                return Err(SyncError::CorruptState {
                    table: "apply_journal",
                    field: "operation_body_digest",
                });
            }
            let operation: FsOperation = parse_apply_journal_json(
                &record.filesystem_operation_json,
                "filesystem_operation_json",
            )?;
            if record.preimage_json != record.filesystem_operation_json {
                return Err(SyncError::CorruptState {
                    table: "apply_journal",
                    field: "preimage_json",
                });
            }
            let operation_spec: RemoteApplyOperation =
                parse_apply_journal_json(&record.operation_json, "operation_json")?;
            let post_states: Vec<WorkspacePathState> =
                parse_apply_journal_json(&record.postimage_json, "postimage_json")?;
            let plan: ApplyCommitPlan =
                parse_apply_journal_json(&record.commit_json, "commit_json")?;
            self.validate_apply_commit_plan(&record, &plan)?;
            validate_apply_recovery_tuple(&operation_spec, &operation, &post_states, &plan)?;
            let recovering_legacy_live_gap = matches!(
                record.stage,
                ApplyStage::Prepared
                    | ApplyStage::FilesystemStarted
                    | ApplyStage::FilesystemApplied
            ) && matches!(
                &plan,
                ApplyCommitPlan::LiveEvent { event, .. }
                    if event.revision < self.runtime.state.cursor()?.last_applied_revision
            );
            if recovering_legacy_live_gap {
                self.runtime
                    .state
                    .preflight_legacy_live_event_gap(&record)?;
            }
            if record.stage == ApplyStage::Prepared
                && record.apply_namespace == ApplyNamespace::SnapshotEntry
            {
                self.runtime.state.remove_apply_journal(record.apply_id)?;
                continue;
            }

            let stage = if record.stage == ApplyStage::Prepared {
                self.runtime
                    .state
                    .set_apply_stage(record.apply_id, ApplyStage::FilesystemStarted)?;
                ApplyStage::FilesystemStarted
            } else {
                record.stage
            };
            let mut commit_record = record.clone();
            let mut recovered_status = StreamItemStatus::Applied;
            let mut preserve_local = false;
            let receipt = match stage {
                ApplyStage::Prepared => unreachable!(),
                ApplyStage::FilesystemStarted => {
                    let receipt = match self
                        .runtime
                        .system
                        .writer
                        .apply(record.apply_id, &operation)
                    {
                        Ok(receipt) => receipt,
                        Err(fns_fs::FsError::ContentMismatch) => {
                            self.runtime
                                .system
                                .writer
                                .abandon(record.apply_id, &operation)?;
                            preserve_local = true;
                            recovered_status = StreamItemStatus::Preserved;
                            fns_fs::ApplyReceipt {
                                apply_id: record.apply_id,
                                touched: Vec::new(),
                                postimages: Vec::new(),
                                postimage_hashes: Vec::new(),
                                cleanup_name: None,
                            }
                        }
                        Err(error) => return Err(SyncError::Filesystem(error)),
                    };
                    let receipt_json = canonical_json(&receipt)?;
                    self.runtime
                        .state
                        .set_apply_filesystem_applied(record.apply_id, &receipt_json)?;
                    commit_record.stage = ApplyStage::FilesystemApplied;
                    commit_record.filesystem_receipt_json = Some(receipt_json);
                    receipt
                }
                ApplyStage::FilesystemApplied
                | ApplyStage::DatabaseCommitted
                | ApplyStage::Finalized => {
                    let receipt_json = record.filesystem_receipt_json.as_deref().ok_or(
                        SyncError::CorruptState {
                            table: "apply_journal",
                            field: "filesystem_receipt_json",
                        },
                    )?;
                    let receipt: fns_fs::ApplyReceipt =
                        parse_apply_journal_json(receipt_json, "filesystem_receipt_json")?;
                    if receipt.apply_id != record.apply_id {
                        return Err(SyncError::CorruptState {
                            table: "apply_journal",
                            field: "filesystem_receipt_json",
                        });
                    }
                    if receipt.touched.is_empty()
                        || !self.apply_receipt_matches_workspace(&receipt)?
                    {
                        self.runtime
                            .system
                            .writer
                            .abandon(record.apply_id, &operation)?;
                        preserve_local = true;
                        recovered_status = StreamItemStatus::Preserved;
                    }
                    receipt
                }
            };
            if receipt.touched.is_empty() {
                recovered_status = StreamItemStatus::Preserved;
                preserve_local = true;
            }

            if recovering_legacy_live_gap {
                self.runtime
                    .state
                    .commit_legacy_live_event_gap(&commit_record)?;
            } else if matches!(
                stage,
                ApplyStage::FilesystemStarted | ApplyStage::FilesystemApplied
            ) {
                self.commit_recovered_apply(&plan, record.apply_id, recovered_status)?;
            }
            if preserve_local {
                self.preserve_recovered_local_intent(&operation)?;
            }
            if stage != ApplyStage::Finalized {
                self.runtime.system.writer.finalize(&receipt)?;
                self.runtime
                    .state
                    .set_apply_stage(record.apply_id, ApplyStage::Finalized)?;
            }
            self.runtime.state.remove_apply_journal(record.apply_id)?;
        }
        Ok(())
    }

    fn apply_receipt_matches_workspace(
        &mut self,
        receipt: &fns_fs::ApplyReceipt,
    ) -> Result<bool, SyncError> {
        if receipt.touched.len() != receipt.postimages.len()
            || receipt.touched.len() != receipt.postimage_hashes.len()
        {
            return Err(SyncError::CorruptState {
                table: "apply_journal",
                field: "filesystem_receipt_json",
            });
        }
        for ((path, expected), expected_hash) in receipt
            .touched
            .iter()
            .zip(&receipt.postimages)
            .zip(&receipt.postimage_hashes)
        {
            let observed = self.runtime.system.workspace.inspect(path)?;
            if observed.as_ref() != expected.as_ref() {
                return Ok(false);
            }
            if let (Some(observed), Some(expected_hash)) = (observed.as_ref(), expected_hash)
                && self.observed_content_hash(observed)?.as_ref() != Some(expected_hash)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn recover_legacy_apply_journal(
        &mut self,
        record: &ApplyJournalRecord,
    ) -> Result<(), SyncError> {
        let operation: RemoteApplyOperation = serde_json::from_slice(&record.operation_json)
            .map_err(|_| SyncError::CorruptState {
                table: "apply_journal",
                field: "operation_json",
            })?;
        let post_states: Vec<WorkspacePathState> = serde_json::from_slice(&record.postimage_json)
            .map_err(|_| SyncError::CorruptState {
            table: "apply_journal",
            field: "postimage_json",
        })?;
        let expected_states = match &operation {
            RemoteApplyOperation::Upsert { state } | RemoteApplyOperation::Delete { state } => {
                vec![state.clone()]
            }
            RemoteApplyOperation::Rename {
                old_state,
                new_state,
            } => vec![old_state.clone(), new_state.clone()],
        };
        if post_states != expected_states
            || post_states.iter().any(|state| state.validate().is_err())
        {
            return Err(SyncError::CorruptState {
                table: "apply_journal",
                field: "postimage_json",
            });
        }
        if record.stage == ApplyStage::FilesystemStarted {
            let filesystem_operation = legacy_filesystem_operation(&operation)?;
            self.runtime
                .system
                .writer
                .abandon(record.apply_id, &filesystem_operation)?;
        }
        self.runtime.state.remove_apply_journal(record.apply_id)?;
        Ok(())
    }

    fn validate_apply_commit_plan(
        &self,
        record: &ApplyJournalRecord,
        plan: &ApplyCommitPlan,
    ) -> Result<(), SyncError> {
        if record.apply_namespace != plan.namespace() {
            return Err(SyncError::CorruptState {
                table: "apply_journal",
                field: "apply_namespace",
            });
        }
        let valid = match plan {
            ApplyCommitPlan::SnapshotEntry { entry } => entry.validate().is_ok(),
            ApplyCommitPlan::StreamEvent { event, .. }
            | ApplyCommitPlan::LiveEvent { event, .. } => event.validate().is_ok(),
            ApplyCommitPlan::StreamConflictResolved { message }
            | ApplyCommitPlan::LiveConflictResolved { message } => message.validate().is_ok(),
        };
        if !valid {
            return Err(SyncError::CorruptState {
                table: "apply_journal",
                field: "commit_json",
            });
        }
        let exact = match plan {
            ApplyCommitPlan::SnapshotEntry { entry } => {
                record.item_kind == ApplyItemKind::Entry
                    && record.workspace_id == entry.workspace_id
                    && record.stream_id == entry.stream_id
                    && record.item_key == entry.entry.path.as_str()
            }
            ApplyCommitPlan::StreamEvent { event, .. } => {
                record.item_kind == ApplyItemKind::Event
                    && record.workspace_id == event.workspace_id
                    && record.stream_id == event.stream_id
                    && record.item_key == event.revision.to_string()
            }
            ApplyCommitPlan::LiveEvent { event, .. } => {
                record.item_kind == ApplyItemKind::Event
                    && record.workspace_id == event.workspace_id
                    && record.stream_id == event.stream_id
                    && record.item_key == event.revision.to_string()
            }
            ApplyCommitPlan::StreamConflictResolved { message } => {
                record.item_kind == ApplyItemKind::ConflictResolved
                    && record.workspace_id == message.workspace_id
                    && self
                        .runtime
                        .state
                        .stream_state()?
                        .is_some_and(|stream| stream.stream_id == record.stream_id)
                    && record.item_key == message.revision.to_string()
            }
            ApplyCommitPlan::LiveConflictResolved { message } => {
                record.item_kind == ApplyItemKind::ConflictResolved
                    && record.workspace_id == message.workspace_id
                    && record.stream_id == live_apply_stream_id()
                    && record.item_key == message.revision.to_string()
            }
        };
        if exact {
            Ok(())
        } else {
            Err(SyncError::CorruptState {
                table: "apply_journal",
                field: "commit_json",
            })
        }
    }

    fn commit_recovered_apply(
        &mut self,
        plan: &ApplyCommitPlan,
        apply_id: ApplyId,
        status: StreamItemStatus,
    ) -> Result<(), SyncError> {
        match plan {
            ApplyCommitPlan::SnapshotEntry { entry } => self.commit_entry_with_journal(
                entry,
                status,
                vec![entry.entry.clone()],
                Some(apply_id),
            ),
            ApplyCommitPlan::StreamEvent {
                event,
                remove_outbox,
            } => self.commit_event_with_journal(
                event,
                status,
                event_post_states(event),
                Some(event.operation_id),
                Some(applied_event_digest(event)?),
                *remove_outbox,
                Some(apply_id),
            ),
            ApplyCommitPlan::LiveEvent {
                event,
                remove_outbox,
            } => self.commit_live_event_with_journal(
                event,
                event_post_states(event),
                applied_event_digest(event)?,
                *remove_outbox,
                Some(apply_id),
            ),
            ApplyCommitPlan::StreamConflictResolved { message } => self
                .commit_conflict_resolved_with_journal(
                    message,
                    status,
                    ConflictResolutionSource::Stream,
                    Some(apply_id),
                ),
            ApplyCommitPlan::LiveConflictResolved { message } => self
                .commit_conflict_resolved_with_journal(
                    message,
                    status,
                    ConflictResolutionSource::Live,
                    Some(apply_id),
                ),
        }
    }

    fn preserve_recovered_local_intent(
        &mut self,
        operation: &FsOperation,
    ) -> Result<(), SyncError> {
        let snapshots = operation_directory_snapshots(operation);
        let mut changes = if snapshots.is_empty() {
            let paths = match operation {
                FsOperation::Rename { path, new_path, .. } => vec![path, new_path],
                FsOperation::UpsertFile { path, .. }
                | FsOperation::Mkdir { path, .. }
                | FsOperation::UpsertSymlink { path, .. }
                | FsOperation::Delete { path, .. } => vec![path],
            };
            paths
                .into_iter()
                .map(|path| {
                    self.runtime.system.workspace.inspect(path).map(|observed| {
                        if observed.is_some() {
                            FsChange::Update(path.clone())
                        } else {
                            FsChange::Delete(path.clone())
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            self.scan_changes()?
        };
        for (root, snapshot) in snapshots {
            if self.runtime.system.workspace.inspect(root)?.is_none() {
                continue;
            }
            for entry in &snapshot.entries {
                let path = WorkspacePath::parse(&format!(
                    "{}/{}",
                    root.as_str(),
                    entry.relative_path.as_str()
                ))
                .map_err(|_| SyncError::CorruptState {
                    table: "apply_journal",
                    field: "filesystem_operation_json",
                })?;
                if self.runtime.system.workspace.inspect(&path)?.is_none()
                    && !changes.contains(&FsChange::Delete(path.clone()))
                {
                    changes.push(FsChange::Delete(path));
                }
            }
        }
        self.record_local_changes(changes)
    }

    fn desired_entry_from_observed(
        &mut self,
        observed: &ObservedEntry,
    ) -> Result<LocalDesiredEntry, SyncError> {
        if observed.kind == WorkspaceEntryKind::Directory {
            return Ok(LocalDesiredEntry {
                path: observed.path.clone(),
                kind: observed.kind,
                content_hash: RequiredNullable::Null,
                metadata: zero_metadata(),
            });
        }
        let path = observed.path.clone();
        HashCache::invalidate(&mut self.runtime.state, &path).map_err(|_| {
            SyncError::Filesystem(fns_fs::FsError::Io {
                operation: "invalidate hash cache",
            })
        })?;
        let descriptor = self.runtime.system.content_cache.stage_workspace_entry(
            &self.runtime.system.workspace,
            &path,
            &mut self.runtime.state,
        )?;
        Ok(LocalDesiredEntry {
            path,
            kind: observed.kind,
            content_hash: RequiredNullable::Value(descriptor.content_hash),
            metadata: descriptor.metadata,
        })
    }

    fn desired_from_change(&mut self, change: &FsChange) -> Result<DesiredOperation, SyncError> {
        match change {
            FsChange::Create(path) | FsChange::Update(path) => {
                let observed =
                    self.runtime
                        .system
                        .workspace
                        .inspect(path)?
                        .ok_or(SyncError::Filesystem(fns_fs::FsError::Io {
                            operation: "observe local change",
                        }))?;
                Ok(DesiredOperation::Upsert {
                    entry: self.desired_entry_from_observed(&observed)?,
                })
            }
            FsChange::Delete(path) => {
                HashCache::invalidate(&mut self.runtime.state, path).map_err(|_| {
                    SyncError::Filesystem(fns_fs::FsError::Io {
                        operation: "invalidate hash cache",
                    })
                })?;
                Ok(DesiredOperation::Delete { path: path.clone() })
            }
            FsChange::Rename { from, to } => {
                HashCache::invalidate(&mut self.runtime.state, from).map_err(|_| {
                    SyncError::Filesystem(fns_fs::FsError::Io {
                        operation: "invalidate hash cache",
                    })
                })?;
                let observed =
                    self.runtime
                        .system
                        .workspace
                        .inspect(to)?
                        .ok_or(SyncError::Filesystem(fns_fs::FsError::Io {
                            operation: "observe local rename target",
                        }))?;
                let entry = self.desired_entry_from_observed(&observed)?;
                Ok(DesiredOperation::Rename {
                    from: from.clone(),
                    to: to.clone(),
                    kind: entry.kind,
                    content_hash: entry.content_hash,
                    metadata: entry.metadata,
                })
            }
            FsChange::RescanRequired => {
                unreachable!("rescan changes are expanded before reconciliation")
            }
        }
    }

    fn record_desired(&mut self, desired: DesiredOperation) -> Result<(), SyncError> {
        let states = self.path_state_map()?;
        let touched = desired.paths();
        let outbox = self.runtime.state.outbox()?;
        let existing = outbox.into_iter().find_map(|record| {
            let mutation = record.mutation().ok()?;
            if mutation_paths(&mutation)
                .iter()
                .any(|path| touched.iter().any(|candidate| candidate == &path))
            {
                Some((record, mutation))
            } else {
                None
            }
        });
        if let Some((record, mutation)) = existing {
            if mutation_matches_desired(&mutation, &desired) {
                return Ok(());
            }
            if record.stage == OutboxStage::Queued {
                let replacement = mutation_for_desired(
                    &desired,
                    self.runtime.state.workspace_id(),
                    self.runtime.state.client_id(),
                    self.next_operation_id()?,
                    &states,
                );
                let timestamp = self.next_timestamp();
                let paths = mutation_paths(&mutation);
                return self.runtime.state.transaction(|tx| {
                    tx.remove_outbox(record.operation_id)?;
                    tx.enqueue_mutation_at(&replacement, timestamp)?;
                    for path in &paths {
                        tx.remove_local_intent(path)?;
                    }
                    Ok(())
                });
            }
            let intent = desired.intent_for_path(touched[0]);
            let body = encode_intent(&intent)?;
            let timestamp = self.next_timestamp();
            let intent_paths = touched.to_vec();
            return self.runtime.state.transaction(|tx| {
                for path in &intent_paths {
                    tx.put_local_intent(path.as_str(), &body, timestamp)?;
                }
                Ok(())
            });
        }

        if let Some((merged, existing_paths)) = self.compact_deferred(&desired, &touched)? {
            let merged_paths = merged.paths().into_iter().cloned().collect::<Vec<_>>();
            let body = encode_intent(&merged.intent_for_path(&merged_paths[0]))?;
            let timestamp = self.next_timestamp();
            return self.runtime.state.transaction(|tx| {
                for path in &existing_paths {
                    tx.remove_local_intent(path)?;
                }
                for path in &merged_paths {
                    tx.put_local_intent(path.as_str(), &body, timestamp)?;
                }
                Ok(())
            });
        }

        if desired_matches_remote(&desired, &states) {
            let paths = touched.to_vec();
            return self.runtime.state.transaction(|tx| {
                for path in &paths {
                    tx.remove_local_intent(path)?;
                }
                Ok(())
            });
        }
        let mutation = mutation_for_desired(
            &desired,
            self.runtime.state.workspace_id(),
            self.runtime.state.client_id(),
            self.next_operation_id()?,
            &states,
        );
        mutation
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_local_mutation",
            })?;
        let timestamp = self.next_timestamp();
        let paths = touched.to_vec();
        self.runtime.state.transaction(|tx| {
            tx.enqueue_mutation_at(&mutation, timestamp)?;
            for path in &paths {
                tx.remove_local_intent(path)?;
            }
            Ok(())
        })
    }

    fn defer_desired(&mut self, desired: DesiredOperation) -> Result<(), SyncError> {
        let paths = desired.paths().into_iter().cloned().collect::<Vec<_>>();
        let body = encode_intent(&desired.intent_for_path(&paths[0]))?;
        let timestamp = self.next_timestamp();
        self.runtime.state.transaction(|tx| {
            for path in &paths {
                tx.put_local_intent(path.as_str(), &body, timestamp)?;
            }
            Ok(())
        })
    }

    fn materialize_local_intents(&mut self) -> Result<(), SyncError> {
        let intents = self.runtime.state.local_intents()?;
        if intents.is_empty() {
            return Ok(());
        }
        let states = self.path_state_map()?;
        for record in intents {
            let intent = decode_intent(&record.intent_json)?;
            let desired = desired_from_intent(&intent);
            if desired_matches_remote(&desired, &states) {
                self.runtime.state.remove_local_intent(&record.path)?;
                continue;
            }
            let touched = desired.paths();
            let has_outbox = self.runtime.state.outbox()?.into_iter().any(|outbox| {
                outbox
                    .mutation()
                    .map(|mutation| {
                        mutation_paths(&mutation)
                            .iter()
                            .any(|path| touched.iter().any(|candidate| candidate == &path))
                    })
                    .unwrap_or(false)
            });
            if has_outbox {
                continue;
            }
            let mutation = mutation_for_desired(
                &desired,
                self.runtime.state.workspace_id(),
                self.runtime.state.client_id(),
                self.next_operation_id()?,
                &states,
            );
            let timestamp = self.next_timestamp();
            self.runtime.state.transaction(|tx| {
                tx.enqueue_mutation_at(&mutation, timestamp)?;
                for path in &touched {
                    tx.remove_local_intent(path)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn deferred_operations(
        &self,
        touched: &[WorkspacePath],
    ) -> Result<Vec<DesiredOperation>, SyncError> {
        let mut operations = Vec::new();
        let mut seen = Vec::new();
        for intent_record in self.runtime.state.local_intents()? {
            if !touched.iter().any(|path| path == &intent_record.path) {
                continue;
            }
            let intent = decode_intent(&intent_record.intent_json)?;
            let desired = desired_from_intent(&intent);
            if !seen.contains(&desired) {
                seen.push(desired.clone());
                operations.push(desired);
            }
        }
        operations.sort_by_key(desired_operation_key);
        Ok(operations)
    }

    fn deferred_operations_intersecting(
        &self,
        touched: &[WorkspacePath],
    ) -> Result<Vec<DesiredOperation>, SyncError> {
        let mut available = Vec::new();
        for intent_record in self.runtime.state.local_intents()? {
            let intent = decode_intent(&intent_record.intent_json)?;
            let desired = desired_from_intent(&intent);
            if !available.contains(&desired) {
                available.push(desired);
            }
        }

        let mut intersecting = touched.to_vec();
        let mut operations = Vec::new();
        loop {
            let mut expanded = false;
            for operation in &available {
                if operations.contains(operation)
                    || !operation.paths().into_iter().any(|operation_path| {
                        intersecting
                            .iter()
                            .any(|path| paths_intersect(operation_path, path))
                    })
                {
                    continue;
                }
                intersecting.extend(operation.paths().into_iter().cloned());
                operations.push(operation.clone());
                expanded = true;
            }
            if !expanded {
                break;
            }
        }
        operations.sort_by_key(desired_operation_key);
        Ok(operations)
    }

    fn compact_deferred(
        &self,
        incoming: &DesiredOperation,
        touched: &[&WorkspacePath],
    ) -> Result<Option<(DesiredOperation, Vec<WorkspacePath>)>, SyncError> {
        let touched = touched
            .iter()
            .map(|path| (*path).clone())
            .collect::<Vec<_>>();
        let existing = self.deferred_operations(&touched)?;
        if existing.is_empty() {
            return Ok(None);
        }
        let mut merged = incoming.clone();
        let mut existing_paths = Vec::new();
        for current in existing {
            existing_paths.extend(current.paths().into_iter().cloned());
            merged = merge_deferred_desired(&current, &merged);
        }
        Ok(Some((merged, unique_paths(existing_paths))))
    }

    fn reconcile_stale(
        &mut self,
        mutation: WorkspaceMutation,
        current: RequiredNullable<WorkspacePathState>,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        if let RequiredNullable::Value(state) = &current
            && state.path != mutation.path
        {
            return Err(SyncError::ProtocolInvariant {
                reason: "stale_state_path_mismatch",
            });
        }
        let touched = mutation_paths(&mutation);
        let mut states = self.path_state_map()?;
        match &current {
            RequiredNullable::Null => {
                states.remove(&mutation.path);
            }
            RequiredNullable::Value(state) => {
                states.insert(state.path.clone(), state.clone());
            }
        }
        let intents = self.deferred_operations(&touched)?;
        let desired = if intents.is_empty() {
            vec![desired_from_mutation(&mutation, None)]
        } else {
            intents
        };
        let next_mutations = self.next_mutations(&desired, &states)?;
        let paths = paths_for_desired_operations(&touched, &desired);
        self.runtime.state.transaction(|tx| {
            match &current {
                RequiredNullable::Null => tx.remove_path_state(&mutation.path)?,
                RequiredNullable::Value(state) => tx.put_path_state(state)?,
            }
            tx.remove_outbox(mutation.operation_id)?;
            for path in &paths {
                tx.remove_local_intent(path)?;
            }
            for (next, timestamp) in &next_mutations {
                tx.enqueue_mutation_at(next, *timestamp)?;
            }
            Ok(())
        })?;
        Ok(Vec::new())
    }

    fn path_state_map(&self) -> Result<BTreeMap<WorkspacePath, WorkspacePathState>, SyncError> {
        Ok(self
            .runtime
            .state
            .path_states()?
            .into_iter()
            .map(|record| (record.path, record.state))
            .collect())
    }

    fn next_mutations(
        &mut self,
        desired: &[DesiredOperation],
        states: &BTreeMap<WorkspacePath, WorkspacePathState>,
    ) -> Result<Vec<(WorkspaceMutation, i64)>, SyncError> {
        let mut next = Vec::new();
        for desired in desired {
            if desired_matches_remote(desired, states) {
                continue;
            }
            let mutation = mutation_for_desired(
                desired,
                self.runtime.state.workspace_id(),
                self.runtime.state.client_id(),
                self.next_operation_id()?,
                states,
            );
            next.push((mutation, self.next_timestamp()));
        }
        Ok(next)
    }

    fn status_for_state(&self, state: &WorkspacePathState) -> Result<StreamItemStatus, SyncError> {
        let Some((hash, size)) = required_content(state) else {
            return Ok(StreamItemStatus::Ready);
        };
        if self.content_available(&hash, size)? {
            Ok(StreamItemStatus::Ready)
        } else {
            Ok(StreamItemStatus::WaitingBlob)
        }
    }

    fn content_available(
        &self,
        hash: &fns_protocol::WorkspaceContentHash,
        size: u64,
    ) -> Result<bool, SyncError> {
        let Ok(file) = self.runtime.system.content_cache.open_blob(hash) else {
            return Ok(false);
        };
        let mut reader = std::io::BufReader::new(file);
        let mut hasher = blake3::Hasher::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = reader.read(&mut buffer).map_err(|_| {
                SyncError::Filesystem(fns_fs::FsError::Io {
                    operation: "read content cache",
                })
            })?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(SyncError::ProtocolInvariant {
                    reason: "content_size_overflow",
                })?;
            hasher.update(&buffer[..count]);
        }
        let actual = fns_protocol::WorkspaceContentHash::parse(&format!(
            "blake3:{}",
            hasher.finalize().to_hex()
        ))
        .map_err(|_| SyncError::CorruptState {
            table: "content_cache",
            field: "content_hash",
        })?;
        Ok(total == size && actual == *hash)
    }

    fn resume_inbound_work(
        &mut self,
        command_limit: usize,
        reissue_waiting: bool,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        let mut commands = Vec::new();
        let mut budget = InboundWorkBudget::new(self.inbound_work_limits);
        let mut stream_unavailable = false;
        let mut live_unavailable = false;
        let mut source = self.next_inbound_source;
        // Rotate the first source across calls so a blocked source cannot
        // consume every one-command poll before the other source is observed.
        self.next_inbound_source = source.other();

        while budget.remaining_items() > 0 && !(stream_unavailable && live_unavailable) {
            let attempted_source = source;
            source = source.other();
            let unavailable = match attempted_source {
                InboundWorkSource::Stream => stream_unavailable,
                InboundWorkSource::Live => live_unavailable,
            };
            if unavailable {
                continue;
            }
            let step = match attempted_source {
                InboundWorkSource::Stream => self.resume_stream_step(
                    &mut budget,
                    &mut commands,
                    command_limit,
                    reissue_waiting,
                )?,
                InboundWorkSource::Live => self.resume_live_step(
                    &mut budget,
                    &mut commands,
                    command_limit,
                    reissue_waiting,
                )?,
            };
            if step != ResumeStep::Progressed {
                match attempted_source {
                    InboundWorkSource::Stream => stream_unavailable = true,
                    InboundWorkSource::Live => live_unavailable = true,
                }
            }
        }
        let _ = self.finish_stream_if_ready()?;
        Ok(commands)
    }

    fn resume_live_step(
        &mut self,
        budget: &mut InboundWorkBudget,
        commands: &mut Vec<SyncCommand>,
        command_limit: usize,
        reissue_waiting: bool,
    ) -> Result<ResumeStep, SyncError> {
        let Some(pending) = self.pending_live_events.front().cloned() else {
            return Ok(ResumeStep::Idle);
        };
        if !budget.consume(pending.serialized_bytes) {
            return Ok(ResumeStep::BudgetExhausted);
        }
        let required = match &pending.message {
            PendingLiveMessage::Event(event) => required_event_content(event),
            PendingLiveMessage::ConflictResolved(message) => required_content(&message.path_state),
        };
        if let Some((hash, size)) = required
            && !self.content_available(&hash, size)?
        {
            if reissue_waiting {
                let operation_id = match &pending.message {
                    PendingLiveMessage::Event(event) => event.operation_id,
                    PendingLiveMessage::ConflictResolved(message) => message.operation_id,
                };
                push_download(
                    commands,
                    command_limit,
                    self.runtime.state.workspace_id(),
                    Some(operation_id),
                    hash,
                    size,
                );
            }
            return Ok(ResumeStep::Blocked);
        }
        let serialized_bytes = pending.serialized_bytes;
        let body_digest = pending.body_digest;
        match pending.message {
            PendingLiveMessage::Event(event) => self.apply_live_event(*event)?,
            PendingLiveMessage::ConflictResolved(message) => {
                self.apply_live_conflict_resolved(message)?
            }
        }
        let applied = self
            .pending_live_events
            .pop_front()
            .ok_or(SyncError::ProtocolInvariant {
                reason: "pending_live_queue_changed",
            })?;
        if applied.body_digest != body_digest || applied.serialized_bytes != serialized_bytes {
            return Err(SyncError::ProtocolInvariant {
                reason: "pending_live_queue_changed",
            });
        }
        self.pending_live_serialized_bytes = self
            .pending_live_serialized_bytes
            .checked_sub(serialized_bytes)
            .ok_or(SyncError::ProtocolInvariant {
                reason: "pending_live_bytes_underflow",
            })?;
        Ok(ResumeStep::Progressed)
    }

    fn resume_stream_step(
        &mut self,
        budget: &mut InboundWorkBudget,
        commands: &mut Vec<SyncCommand>,
        command_limit: usize,
        reissue_waiting: bool,
    ) -> Result<ResumeStep, SyncError> {
        let Some(active) = self.runtime.state.stream_state()? else {
            return Ok(ResumeStep::Idle);
        };
        let remaining_items = budget.remaining_items().min(1);
        let remaining_bytes = budget.remaining_serialized_bytes();
        let allow_oversized = budget.allows_oversized_first_item();
        match active.mode {
            WorkspaceSnapshotMode::Snapshot => {
                let page = self.runtime.state.pending_stream_entries_page(
                    active.stream_id,
                    remaining_items,
                    remaining_bytes,
                    allow_oversized,
                )?;
                let Some(record) = page.items.into_iter().next() else {
                    if page.deferred_by_byte_budget {
                        return Ok(ResumeStep::BudgetExhausted);
                    }
                    return Ok(ResumeStep::Idle);
                };
                if !budget.consume(record.body_json.len()) {
                    return Ok(ResumeStep::BudgetExhausted);
                }
                let entry: WorkspaceSnapshotEntryMessage =
                    serde_json::from_slice(&record.body_json).map_err(|_| {
                        SyncError::CorruptState {
                            table: "stream_entries",
                            field: "body_json",
                        }
                    })?;
                if let Some((hash, size)) = required_content(&entry.entry)
                    && !self.content_available(&hash, size)?
                {
                    self.runtime
                        .state
                        .put_stream_entry(&entry, StreamItemStatus::WaitingBlob)?;
                    if reissue_waiting {
                        push_download(
                            commands,
                            command_limit,
                            self.runtime.state.workspace_id(),
                            None,
                            hash,
                            size,
                        );
                    }
                    return Ok(ResumeStep::Blocked);
                }
                if record.status != StreamItemStatus::Ready {
                    self.runtime
                        .state
                        .put_stream_entry(&entry, StreamItemStatus::Ready)?;
                }
                self.apply_snapshot_entry(entry)?;
            }
            WorkspaceSnapshotMode::Incremental => {
                let page = self.runtime.state.pending_stream_revision_items_page(
                    active.stream_id,
                    remaining_items,
                    remaining_bytes,
                    allow_oversized,
                )?;
                let Some(record) = page.items.into_iter().next() else {
                    if page.deferred_by_byte_budget {
                        return Ok(ResumeStep::BudgetExhausted);
                    }
                    return Ok(ResumeStep::Idle);
                };
                if !budget.consume(record.body_json.len()) {
                    return Ok(ResumeStep::BudgetExhausted);
                }
                match record.item_kind {
                    StreamRevisionItemKind::Event => {
                        let event: WorkspaceEventMessage =
                            serde_json::from_slice(&record.body_json).map_err(|_| {
                                SyncError::CorruptState {
                                    table: "stream_revision_items",
                                    field: "body_json",
                                }
                            })?;
                        if self.event_receipt_match(&event)? == AppliedReceiptMatch::Exact {
                            self.runtime
                                .state
                                .put_stream_event(&event, StreamItemStatus::Applied)?;
                        } else {
                            if record.status != StreamItemStatus::Ready {
                                if let Some((hash, size)) = required_content(&event.path_state)
                                    && !self.content_available(&hash, size)?
                                {
                                    self.runtime
                                        .state
                                        .put_stream_event(&event, StreamItemStatus::WaitingBlob)?;
                                    if reissue_waiting {
                                        push_download(
                                            commands,
                                            command_limit,
                                            self.runtime.state.workspace_id(),
                                            Some(event.operation_id),
                                            hash,
                                            size,
                                        );
                                    }
                                    return Ok(ResumeStep::Blocked);
                                }
                                self.runtime
                                    .state
                                    .put_stream_event(&event, StreamItemStatus::Ready)?;
                            }
                            self.apply_event(event)?;
                        }
                    }
                    StreamRevisionItemKind::ConflictResolved => {
                        let message: WorkspaceConflictResolvedMessage =
                            serde_json::from_slice(&record.body_json).map_err(|_| {
                                SyncError::CorruptState {
                                    table: "stream_revision_items",
                                    field: "body_json",
                                }
                            })?;
                        let body_digest = crate::body_digest(&canonical_json(&message)?);
                        if self.conflict_resolution_receipt_match(&message, body_digest)?
                            == AppliedReceiptMatch::Exact
                        {
                            self.runtime.state.put_stream_conflict_resolved(
                                &message,
                                None,
                                StreamItemStatus::Applied,
                            )?;
                        } else {
                            if let Some((hash, size)) = required_content(&message.path_state)
                                && !self.content_available(&hash, size)?
                            {
                                self.runtime.state.put_stream_conflict_resolved(
                                    &message,
                                    None,
                                    StreamItemStatus::WaitingBlob,
                                )?;
                                if reissue_waiting {
                                    push_download(
                                        commands,
                                        command_limit,
                                        self.runtime.state.workspace_id(),
                                        Some(message.operation_id),
                                        hash,
                                        size,
                                    );
                                }
                                return Ok(ResumeStep::Blocked);
                            }
                            if record.status != StreamItemStatus::Ready {
                                self.runtime.state.put_stream_conflict_resolved(
                                    &message,
                                    None,
                                    StreamItemStatus::Ready,
                                )?;
                            }
                            self.apply_conflict_resolved(message)?;
                        }
                    }
                }
            }
        }
        Ok(ResumeStep::Progressed)
    }

    fn apply_live_event(&mut self, event: WorkspaceEventMessage) -> Result<(), SyncError> {
        let operation_digest = applied_event_digest(&event)?;
        let mutation_body = canonical_json(&event.mutation)?;
        let own_event = event.origin_client_id == self.runtime.state.client_id();
        let outbox = self.runtime.state.outbox_entry(event.operation_id)?;
        let receipt_match = self.event_receipt_match(&event)?;
        if receipt_match == AppliedReceiptMatch::Exact {
            return self.mark_live_event_applied(event.revision);
        }
        if own_event {
            if let Some(record) = &outbox
                && record.body_json != mutation_body
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_body_mismatch",
                });
            }
            if receipt_match == AppliedReceiptMatch::Missing && outbox.is_none() {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_not_outstanding",
                });
            }
        }
        let remove_outbox = own_event && outbox.is_some();
        if let AppliedReceiptMatch::Legacy { body_digest } = receipt_match
            && self.runtime.state.cursor()?.last_applied_revision > event.revision
        {
            return self.settle_superseded_event(
                &event,
                operation_digest,
                body_digest,
                own_event,
                remove_outbox,
                SupersededEventSource::Live,
            );
        }
        let post_states = event_post_states(&event);
        let previous = self.path_states_for_event(&event)?;
        let observed = self.observed_for_event(&event)?;
        let post_matches = self.event_post_matches(&post_states, &observed)?;
        if post_matches {
            return self.commit_live_event(&event, post_states, operation_digest, remove_outbox);
        }
        if !self.event_baseline_matches(&previous, &observed)? {
            return self.preserve_live_event(
                &event,
                &observed,
                post_states,
                operation_digest,
                remove_outbox,
            );
        }
        let Some(operation) = self.operation_for_event(&event, &observed)? else {
            return self.commit_live_event(&event, post_states, operation_digest, remove_outbox);
        };
        let receipt = self.apply_with_journal(
            event.stream_id,
            ApplyItemKind::Event,
            event.revision.to_string(),
            RemoteApplyOperation::from_event(&event),
            post_states.clone(),
            ApplyCommitPlan::LiveEvent {
                event: event.clone(),
                remove_outbox,
            },
            operation,
        )?;
        self.commit_live_event_with_journal(
            &event,
            post_states,
            operation_digest,
            remove_outbox,
            Some(receipt.apply_id),
        )?;
        self.finish_apply_journal(&receipt)?;
        Ok(())
    }

    fn commit_live_event(
        &mut self,
        event: &WorkspaceEventMessage,
        post_states: Vec<WorkspacePathState>,
        operation_digest: [u8; 32],
        remove_outbox: bool,
    ) -> Result<(), SyncError> {
        self.commit_live_event_with_journal(
            event,
            post_states,
            operation_digest,
            remove_outbox,
            None,
        )
    }

    fn commit_live_event_with_journal(
        &mut self,
        event: &WorkspaceEventMessage,
        post_states: Vec<WorkspacePathState>,
        operation_digest: [u8; 32],
        remove_outbox: bool,
        apply_id: Option<ApplyId>,
    ) -> Result<(), SyncError> {
        let legacy_body_digest = match self.event_receipt_match(event)? {
            AppliedReceiptMatch::Legacy { body_digest } => Some(body_digest),
            AppliedReceiptMatch::Missing | AppliedReceiptMatch::Exact => None,
        };
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            tx.record_mutation_applied_operation(
                event.origin_client_id,
                event.operation_id,
                event.revision,
                operation_digest,
                &event.mutation,
                legacy_body_digest,
            )?;
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            tx.set_last_applied_revision(event.revision)?;
            tx.set_pending_ack(event.revision)?;
            if let Some(apply_id) = apply_id {
                tx.set_apply_stage(apply_id, ApplyStage::DatabaseCommitted)?;
            }
            Ok(())
        })?;
        if apply_id.is_some() {
            apply_failpoint("database_committed");
        }
        Ok(())
    }

    fn finish_stream_if_ready(&mut self) -> Result<bool, SyncError> {
        let Some(active) = self.runtime.state.stream_state()? else {
            return Ok(false);
        };
        if !active.end_received {
            return Ok(false);
        }
        let summary = self.runtime.state.stream_table_summary(active.stream_id)?;
        let items_ready = match active.mode {
            WorkspaceSnapshotMode::Snapshot => {
                summary.entry_count == u64::from(active.expected_entry_count)
                    && summary.pending_entry_count == 0
                    && summary.revision_count == 0
            }
            WorkspaceSnapshotMode::Incremental => {
                summary.revision_count == u64::from(active.expected_event_count)
                    && summary.pending_revision_count == 0
                    && summary.entry_count == 0
            }
        };
        if !items_ready || summary.conflict_count != u64::from(active.expected_conflict_count) {
            return Ok(false);
        }
        if self.runtime.state.has_apply_journals()? {
            return Ok(false);
        }
        if active.mode == WorkspaceSnapshotMode::Snapshot {
            self.reconcile_full_snapshot(active.stream_id)?;
        }
        let cursor = self.runtime.state.cursor()?;
        let needs_ack = active.final_revision > cursor.last_ack_revision;
        self.runtime.state.transaction(|tx| {
            tx.replace_authoritative_conflicts(active.stream_id)?;
            tx.set_last_applied_revision(active.final_revision)?;
            if needs_ack {
                tx.set_pending_ack(active.final_revision)?;
            } else {
                tx.clear_stream()?;
            }
            Ok(())
        })?;
        self.materialize_local_intents()?;
        Ok(true)
    }

    fn reconcile_full_snapshot(
        &mut self,
        stream_id: fns_protocol::StreamId,
    ) -> Result<(), SyncError> {
        let remote_states = self
            .runtime
            .state
            .path_states()?
            .into_iter()
            .map(|record| (record.path, record.state))
            .collect::<BTreeMap<_, _>>();
        for (path, state) in &remote_states {
            if self
                .runtime
                .state
                .snapshot_stream_contains_path(stream_id, path)?
            {
                continue;
            }
            self.runtime.state.remove_path_state(path)?;
            if self.runtime.system.workspace.inspect(path)?.is_some() {
                let desired = self.desired_from_current(path)?;
                self.queue_desired_with_states(desired, &BTreeMap::new())?;
            }
            let _ = state;
        }
        let scan = self
            .runtime
            .system
            .workspace
            .scan(&self.runtime.system.rules)?;
        if !scan.issues.is_empty() {
            return Err(SyncError::ScanIncomplete);
        }
        for observed in scan.entries {
            if self
                .runtime
                .state
                .snapshot_stream_contains_path(stream_id, &observed.path)?
            {
                continue;
            }
            let desired = DesiredOperation::Upsert {
                entry: self.desired_entry_from_observed(&observed)?,
            };
            self.queue_desired_with_states(desired, &BTreeMap::new())?;
        }
        Ok(())
    }

    fn apply_snapshot_entry(
        &mut self,
        entry: WorkspaceSnapshotEntryMessage,
    ) -> Result<(), SyncError> {
        let post = entry.entry.clone();
        let previous = self
            .runtime
            .state
            .path_state(post.path.as_str())?
            .map(|record| record.state);
        let observed = self.runtime.system.workspace.inspect(&post.path)?;
        let post_matches = self.observed_matches_post(&post, observed.as_ref())?;
        let baseline_matches =
            baseline_matches_observed(previous.as_ref(), observed.as_ref(), self)?;
        if post_matches {
            return self.commit_entry(&entry, StreamItemStatus::Applied, vec![post]);
        }
        if !baseline_matches && observed.is_some() {
            let desired = self.desired_from_current(&post.path)?;
            self.queue_desired_with_states(desired, &BTreeMap::new())?;
            return self.commit_entry(&entry, StreamItemStatus::Preserved, vec![post]);
        }
        if !baseline_matches && observed.is_none() && previous.is_some() {
            let desired = DesiredOperation::Delete {
                path: post.path.clone(),
            };
            self.queue_desired_with_states(desired, &BTreeMap::new())?;
            return self.commit_entry(&entry, StreamItemStatus::Preserved, vec![post]);
        }
        let Some(operation) = self.operation_for_state(&post, observed.as_ref())? else {
            return self.commit_entry(&entry, StreamItemStatus::Applied, vec![post]);
        };
        let receipt = self.apply_with_journal(
            entry.stream_id,
            ApplyItemKind::Entry,
            post.path.as_str().to_owned(),
            RemoteApplyOperation::from_state(&post),
            vec![post.clone()],
            ApplyCommitPlan::SnapshotEntry {
                entry: entry.clone(),
            },
            operation,
        )?;
        self.commit_entry_with_journal(
            &entry,
            StreamItemStatus::Applied,
            vec![post],
            Some(receipt.apply_id),
        )?;
        self.finish_apply_journal(&receipt)?;
        Ok(())
    }

    fn apply_event(&mut self, event: WorkspaceEventMessage) -> Result<(), SyncError> {
        let mutation_body = canonical_json(&event.mutation)?;
        let operation_digest = applied_event_digest(&event)?;
        let outbox = self.runtime.state.outbox_entry(event.operation_id)?;
        let receipt_match = self.event_receipt_match(&event)?;
        if receipt_match == AppliedReceiptMatch::Exact {
            return self
                .runtime
                .state
                .put_stream_event(&event, StreamItemStatus::Applied)
                .map(|_| ());
        }
        let own_event = event.origin_client_id == self.runtime.state.client_id();
        if own_event {
            if let Some(record) = &outbox
                && record.body_json != mutation_body
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_body_mismatch",
                });
            }
            if receipt_match == AppliedReceiptMatch::Missing && outbox.is_none() {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_not_outstanding",
                });
            }
        }
        let remove_outbox = own_event && outbox.is_some();
        if let AppliedReceiptMatch::Legacy { body_digest } = receipt_match
            && self.runtime.state.cursor()?.last_applied_revision > event.revision
        {
            return self.settle_superseded_event(
                &event,
                operation_digest,
                body_digest,
                own_event,
                remove_outbox,
                SupersededEventSource::Stream,
            );
        }
        if own_event && receipt_match == AppliedReceiptMatch::Missing {
            let post_states = event_post_states(&event);
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                post_states,
                Some(event.operation_id),
                Some(operation_digest),
                outbox.is_some(),
            );
        }
        let post_states = event_post_states(&event);
        let previous = self.path_states_for_event(&event)?;
        let observed = self.observed_for_event(&event)?;
        let post_matches = self.event_post_matches(&post_states, &observed)?;
        let baseline_matches = self.event_baseline_matches(&previous, &observed)?;
        if post_matches {
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                post_states,
                Some(event.operation_id),
                Some(operation_digest),
                remove_outbox,
            );
        }
        if !baseline_matches {
            self.preserve_event(
                &event,
                &previous,
                &observed,
                post_states,
                operation_digest,
                remove_outbox,
            )?;
            return Ok(());
        }
        let Some(operation) = self.operation_for_event(&event, &observed)? else {
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                post_states,
                Some(event.operation_id),
                Some(operation_digest),
                remove_outbox,
            );
        };
        let receipt = self.apply_with_journal(
            event.stream_id,
            ApplyItemKind::Event,
            event.revision.to_string(),
            RemoteApplyOperation::from_event(&event),
            post_states.clone(),
            ApplyCommitPlan::StreamEvent {
                event: event.clone(),
                remove_outbox,
            },
            operation,
        )?;
        self.commit_event_with_journal(
            &event,
            StreamItemStatus::Applied,
            post_states,
            Some(event.operation_id),
            Some(operation_digest),
            remove_outbox,
            Some(receipt.apply_id),
        )?;
        self.finish_apply_journal(&receipt)?;
        Ok(())
    }

    fn apply_conflict_resolved(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
    ) -> Result<(), SyncError> {
        self.apply_conflict_resolved_from(message, ConflictResolutionSource::Stream)
    }

    fn apply_live_conflict_resolved(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
    ) -> Result<(), SyncError> {
        self.apply_conflict_resolved_from(message, ConflictResolutionSource::Live)
    }

    fn apply_conflict_resolved_from(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
        source: ConflictResolutionSource,
    ) -> Result<(), SyncError> {
        let body_digest = crate::body_digest(&canonical_json(&message)?);
        let receipt_match = self.conflict_resolution_receipt_match(&message, body_digest)?;
        if receipt_match == AppliedReceiptMatch::Exact {
            return match source {
                ConflictResolutionSource::Stream => self
                    .runtime
                    .state
                    .put_stream_conflict_resolved(&message, None, StreamItemStatus::Applied)
                    .map(|_| ()),
                ConflictResolutionSource::Live => self.mark_live_event_applied(message.revision),
            };
        }
        if source == ConflictResolutionSource::Live
            && receipt_match == AppliedReceiptMatch::Missing
            && message.revision <= self.runtime.state.cursor()?.last_applied_revision
        {
            return Err(SyncError::StreamInvariant {
                reason: "live_revision_regression",
            });
        }
        if let AppliedReceiptMatch::Legacy {
            body_digest: legacy_body_digest,
        } = receipt_match
            && self.runtime.state.cursor()?.last_applied_revision > message.revision
        {
            return match source {
                ConflictResolutionSource::Stream => self.commit_superseded_conflict_resolution(
                    &message,
                    body_digest,
                    legacy_body_digest,
                ),
                ConflictResolutionSource::Live => self.commit_superseded_live_conflict_resolution(
                    &message,
                    body_digest,
                    legacy_body_digest,
                ),
            };
        }
        let path = message.path_state.path.clone();
        let previous = self
            .runtime
            .state
            .path_state(path.as_str())?
            .map(|record| record.state);
        let observed = self.runtime.system.workspace.inspect(&path)?;
        if self.observed_matches_post(&message.path_state, observed.as_ref())? {
            return self.commit_conflict_resolved_from(&message, StreamItemStatus::Applied, source);
        }
        if !baseline_matches_observed(previous.as_ref(), observed.as_ref(), self)? {
            let desired = self.desired_from_current(&path)?;
            let baseline = previous
                .clone()
                .map(|state| BTreeMap::from([(path.clone(), state)]))
                .unwrap_or_default();
            self.queue_desired_with_states(desired, &baseline)?;
            return self.commit_conflict_resolved_from(
                &message,
                StreamItemStatus::Preserved,
                source,
            );
        }
        let Some(operation) = self.operation_for_state(&message.path_state, observed.as_ref())?
        else {
            return self.commit_conflict_resolved_from(&message, StreamItemStatus::Applied, source);
        };
        let stream_id = match source {
            ConflictResolutionSource::Stream => {
                self.runtime
                    .state
                    .stream_state()?
                    .ok_or(SyncError::StreamInvariant {
                        reason: "no_active_stream",
                    })?
                    .stream_id
            }
            ConflictResolutionSource::Live => live_apply_stream_id(),
        };
        let receipt = self.apply_with_journal(
            stream_id,
            ApplyItemKind::ConflictResolved,
            message.revision.to_string(),
            RemoteApplyOperation::from_state(&message.path_state),
            vec![message.path_state.clone()],
            match source {
                ConflictResolutionSource::Stream => ApplyCommitPlan::StreamConflictResolved {
                    message: message.clone(),
                },
                ConflictResolutionSource::Live => ApplyCommitPlan::LiveConflictResolved {
                    message: message.clone(),
                },
            },
            operation,
        )?;
        self.commit_conflict_resolved_with_journal(
            &message,
            StreamItemStatus::Applied,
            source,
            Some(receipt.apply_id),
        )?;
        self.finish_apply_journal(&receipt)?;
        Ok(())
    }

    fn mutation_acceptance_match(
        &self,
        accepted: &WorkspaceMutationAcceptedMessage,
    ) -> Result<MutationAcceptanceMatch, SyncError> {
        if let Some(receipt) = self
            .runtime
            .state
            .applied_operation(accepted.client_id, accepted.operation_id)?
        {
            if receipt.revision != accepted.revision {
                return Err(SyncError::OperationChanged);
            }
            match receipt.receipt_kind {
                AppliedOperationReceiptKind::Legacy => {
                    if let Some(provisional) = self.runtime.state.provisional_mutation_acceptance(
                        accepted.client_id,
                        accepted.operation_id,
                    )? {
                        return if provisional == *accepted {
                            Ok(MutationAcceptanceMatch::Exact)
                        } else {
                            Err(SyncError::OperationChanged)
                        };
                    }
                    return Ok(MutationAcceptanceMatch::LegacyUnbound);
                }
                AppliedOperationReceiptKind::ConflictResolution => {
                    return Err(SyncError::OperationChanged);
                }
                AppliedOperationReceiptKind::MutationResult => {}
            }
            let mutation = self.mutation_from_receipt(&receipt)?;
            if mutation.workspace_id != accepted.workspace_id
                || validate_acceptance_shape(&mutation, accepted).is_err()
                || receipt.body_digest
                    != applied_operation_digest(
                        &mutation,
                        &accepted.path_state,
                        accepted.old_path_state.as_ref(),
                        accepted.new_path_state.as_ref(),
                    )?
            {
                return Err(SyncError::OperationChanged);
            }
            return Ok(MutationAcceptanceMatch::Exact);
        }

        let mut changed_replay_identities = Vec::new();
        for receipt in self
            .runtime
            .state
            .mutation_receipts_at_revision(accepted.revision)?
        {
            let mutation = self.mutation_from_receipt(&receipt)?;
            if validate_acceptance_shape(&mutation, accepted).is_ok()
                && receipt.body_digest
                    == applied_operation_digest(
                        &mutation,
                        &accepted.path_state,
                        accepted.old_path_state.as_ref(),
                        accepted.new_path_state.as_ref(),
                    )?
            {
                changed_replay_identities.push((receipt.origin_client_id, receipt.operation_id));
            }
        }
        for provisional in self
            .runtime
            .state
            .provisional_mutation_acceptances(accepted.revision)?
        {
            if same_acceptance_result(&provisional, accepted) {
                changed_replay_identities.push((provisional.client_id, provisional.operation_id));
            }
        }
        changed_replay_identities.sort_unstable();
        changed_replay_identities.dedup();
        if changed_replay_identities.len() == 1 {
            return Err(SyncError::OperationChanged);
        }
        Ok(MutationAcceptanceMatch::Missing)
    }

    fn event_receipt_match(
        &self,
        event: &WorkspaceEventMessage,
    ) -> Result<AppliedReceiptMatch, SyncError> {
        let Some(receipt) = self
            .runtime
            .state
            .applied_operation(event.origin_client_id, event.operation_id)?
        else {
            let event_acceptance = acceptance_from_event(event);
            let matching_provisionals = self
                .runtime
                .state
                .provisional_mutation_acceptances(event.revision)?
                .into_iter()
                .filter(|provisional| same_acceptance_result(provisional, &event_acceptance))
                .count();
            if matching_provisionals == 1 {
                return Err(SyncError::OperationChanged);
            }
            return Ok(AppliedReceiptMatch::Missing);
        };
        if receipt.revision != event.revision {
            return Err(SyncError::OperationChanged);
        }
        match receipt.receipt_kind {
            AppliedOperationReceiptKind::Legacy => {
                let legacy_digest = legacy_mutation_digest(&event.mutation)?;
                if receipt.body_digest != legacy_digest {
                    return Err(SyncError::OperationChanged);
                }
                if let Some(provisional) = self
                    .runtime
                    .state
                    .provisional_mutation_acceptance(event.origin_client_id, event.operation_id)?
                    && provisional != acceptance_from_event(event)
                {
                    return Err(SyncError::OperationChanged);
                }
                Ok(AppliedReceiptMatch::Legacy {
                    body_digest: legacy_digest,
                })
            }
            AppliedOperationReceiptKind::MutationResult => {
                if self.mutation_from_receipt(&receipt)? != event.mutation
                    || receipt.body_digest != applied_event_digest(event)?
                {
                    return Err(SyncError::OperationChanged);
                }
                Ok(AppliedReceiptMatch::Exact)
            }
            AppliedOperationReceiptKind::ConflictResolution => Err(SyncError::OperationChanged),
        }
    }

    fn conflict_resolution_receipt_match(
        &self,
        message: &WorkspaceConflictResolvedMessage,
        body_digest: [u8; 32],
    ) -> Result<AppliedReceiptMatch, SyncError> {
        let Some(receipt) = self
            .runtime
            .state
            .applied_operation(message.resolved_by_client_id, message.operation_id)?
        else {
            return Ok(AppliedReceiptMatch::Missing);
        };
        if receipt.revision != message.revision || receipt.body_digest != body_digest {
            return Err(SyncError::OperationChanged);
        }
        match receipt.receipt_kind {
            AppliedOperationReceiptKind::Legacy => Ok(AppliedReceiptMatch::Legacy {
                body_digest: receipt.body_digest,
            }),
            AppliedOperationReceiptKind::ConflictResolution => Ok(AppliedReceiptMatch::Exact),
            AppliedOperationReceiptKind::MutationResult => Err(SyncError::OperationChanged),
        }
    }

    fn mutation_from_receipt(
        &self,
        receipt: &AppliedOperationRecord,
    ) -> Result<WorkspaceMutation, SyncError> {
        let mutation_json = receipt
            .mutation_json
            .as_deref()
            .ok_or(SyncError::CorruptState {
                table: "applied_operations",
                field: "mutation_json",
            })?;
        let mutation: WorkspaceMutation =
            serde_json::from_slice(mutation_json).map_err(|_| SyncError::CorruptState {
                table: "applied_operations",
                field: "mutation_json",
            })?;
        mutation.validate().map_err(|_| SyncError::CorruptState {
            table: "applied_operations",
            field: "mutation_json",
        })?;
        if canonical_json(&mutation)? != mutation_json
            || mutation.client_id != receipt.origin_client_id
            || mutation.operation_id != receipt.operation_id
        {
            return Err(SyncError::CorruptState {
                table: "applied_operations",
                field: "mutation_json",
            });
        }
        Ok(mutation)
    }

    fn settle_superseded_event(
        &mut self,
        event: &WorkspaceEventMessage,
        operation_digest: [u8; 32],
        legacy_body_digest: [u8; 32],
        own_event: bool,
        remove_outbox: bool,
        source: SupersededEventSource,
    ) -> Result<(), SyncError> {
        let cursor = self.runtime.state.cursor()?;
        let (post_states, replacements) = if own_event {
            let post_states = self.non_regressing_event_post_states(event)?;
            let observed = self.observed_for_event(event)?;
            let replacements = self.preserved_replacements(event, &observed, &post_states)?;
            (post_states, replacements)
        } else {
            (
                Vec::new(),
                PreservedReplacements {
                    mutations: Vec::new(),
                    settled_paths: Vec::new(),
                },
            )
        };
        let stream_status = if replacements.mutations.is_empty() {
            StreamItemStatus::Applied
        } else {
            StreamItemStatus::Preserved
        };
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            tx.record_mutation_applied_operation(
                event.origin_client_id,
                event.operation_id,
                event.revision,
                operation_digest,
                &event.mutation,
                Some(legacy_body_digest),
            )?;
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            for path in &replacements.settled_paths {
                tx.remove_local_intent(path)?;
            }
            for (mutation, timestamp) in &replacements.mutations {
                tx.enqueue_mutation_at(mutation, *timestamp)?;
            }
            match source {
                SupersededEventSource::Live => {
                    if event.revision > cursor.last_ack_revision
                        && cursor
                            .pending_ack_revision
                            .is_none_or(|pending| event.revision > pending)
                    {
                        tx.set_pending_ack(event.revision)?;
                    }
                }
                SupersededEventSource::Stream => {
                    tx.put_stream_event(event, stream_status)?;
                }
            }
            Ok(())
        })
    }

    fn non_regressing_event_post_states(
        &self,
        event: &WorkspaceEventMessage,
    ) -> Result<Vec<WorkspacePathState>, SyncError> {
        event_post_states(event)
            .into_iter()
            .filter_map(|state| {
                let current = self.runtime.state.path_state(state.path.as_str());
                match current {
                    Ok(Some(current)) if current.state.path_revision > state.path_revision => None,
                    Ok(_) => Some(Ok(state)),
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    fn commit_superseded_conflict_resolution(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        body_digest: [u8; 32],
        legacy_body_digest: [u8; 32],
    ) -> Result<(), SyncError> {
        let cleanup = self.conflict_cleanup_operations(message)?;
        self.runtime.state.transaction(|tx| {
            tx.record_conflict_applied_operation(
                message.resolved_by_client_id,
                message.operation_id,
                message.revision,
                body_digest,
                Some(legacy_body_digest),
            )?;
            cleanup_conflict_resolution(tx, message.conflict_id, cleanup)?;
            tx.put_stream_conflict_resolved(message, StreamItemStatus::Applied)?;
            Ok(())
        })
    }

    fn commit_superseded_live_conflict_resolution(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        body_digest: [u8; 32],
        legacy_body_digest: [u8; 32],
    ) -> Result<(), SyncError> {
        let cleanup = self.conflict_cleanup_operations(message)?;
        self.runtime.state.transaction(|tx| {
            tx.record_conflict_applied_operation(
                message.resolved_by_client_id,
                message.operation_id,
                message.revision,
                body_digest,
                Some(legacy_body_digest),
            )?;
            cleanup_conflict_resolution(tx, message.conflict_id, cleanup)
        })
    }

    fn commit_entry(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
        post_states: Vec<WorkspacePathState>,
    ) -> Result<(), SyncError> {
        self.commit_entry_with_journal(entry, status, post_states, None)
    }

    fn commit_entry_with_journal(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
        post_states: Vec<WorkspacePathState>,
        apply_id: Option<ApplyId>,
    ) -> Result<(), SyncError> {
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            tx.put_stream_entry(entry, status)?;
            if let Some(apply_id) = apply_id {
                tx.set_apply_stage(apply_id, ApplyStage::DatabaseCommitted)?;
            }
            Ok(())
        })?;
        if apply_id.is_some() {
            apply_failpoint("database_committed");
        }
        Ok(())
    }

    fn commit_event(
        &mut self,
        event: &WorkspaceEventMessage,
        status: StreamItemStatus,
        post_states: Vec<WorkspacePathState>,
        operation_id: Option<fns_protocol::OperationId>,
        operation_digest: Option<[u8; 32]>,
        remove_outbox: bool,
    ) -> Result<(), SyncError> {
        self.commit_event_with_journal(
            event,
            status,
            post_states,
            operation_id,
            operation_digest,
            remove_outbox,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_event_with_journal(
        &mut self,
        event: &WorkspaceEventMessage,
        status: StreamItemStatus,
        post_states: Vec<WorkspacePathState>,
        operation_id: Option<fns_protocol::OperationId>,
        operation_digest: Option<[u8; 32]>,
        remove_outbox: bool,
        apply_id: Option<ApplyId>,
    ) -> Result<(), SyncError> {
        let legacy_body_digest = match self.event_receipt_match(event)? {
            AppliedReceiptMatch::Legacy { body_digest } => Some(body_digest),
            AppliedReceiptMatch::Missing | AppliedReceiptMatch::Exact => None,
        };
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            if let (Some(operation_id), Some(operation_digest)) = (operation_id, operation_digest) {
                tx.record_mutation_applied_operation(
                    event.origin_client_id,
                    operation_id,
                    event.revision,
                    operation_digest,
                    &event.mutation,
                    legacy_body_digest,
                )?;
            }
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            if status == StreamItemStatus::Applied {
                tx.set_last_applied_revision(event.revision)?;
            }
            tx.put_stream_event(event, status)?;
            if let Some(apply_id) = apply_id {
                tx.set_apply_stage(apply_id, ApplyStage::DatabaseCommitted)?;
            }
            Ok(())
        })?;
        if apply_id.is_some() {
            apply_failpoint("database_committed");
        }
        Ok(())
    }

    fn commit_conflict_resolved_from(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        status: StreamItemStatus,
        source: ConflictResolutionSource,
    ) -> Result<(), SyncError> {
        self.commit_conflict_resolved_with_journal(message, status, source, None)
    }

    fn commit_conflict_resolved_with_journal(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        status: StreamItemStatus,
        source: ConflictResolutionSource,
        apply_id: Option<ApplyId>,
    ) -> Result<(), SyncError> {
        let body_digest = crate::body_digest(&canonical_json(message)?);
        let legacy_body_digest =
            match self.conflict_resolution_receipt_match(message, body_digest)? {
                AppliedReceiptMatch::Legacy { body_digest } => Some(body_digest),
                AppliedReceiptMatch::Missing | AppliedReceiptMatch::Exact => None,
            };
        let cleanup = self.conflict_cleanup_operations(message)?;
        self.runtime.state.transaction(|tx| {
            tx.put_path_state(&message.path_state)?;
            tx.record_conflict_applied_operation(
                message.resolved_by_client_id,
                message.operation_id,
                message.revision,
                body_digest,
                legacy_body_digest,
            )?;
            cleanup_conflict_resolution(tx, message.conflict_id, cleanup)?;
            match source {
                ConflictResolutionSource::Stream => {
                    if status == StreamItemStatus::Applied {
                        tx.set_last_applied_revision(message.revision)?;
                    }
                    tx.put_stream_conflict_resolved(message, status)?;
                }
                ConflictResolutionSource::Live => {
                    tx.set_last_applied_revision(message.revision)?;
                    tx.set_pending_ack(message.revision)?;
                }
            }
            if let Some(apply_id) = apply_id {
                tx.set_apply_stage(apply_id, ApplyStage::DatabaseCommitted)?;
            }
            Ok(())
        })?;
        if apply_id.is_some() {
            apply_failpoint("database_committed");
        }
        Ok(())
    }

    fn conflict_cleanup_operations(
        &self,
        message: &WorkspaceConflictResolvedMessage,
    ) -> Result<ConflictCleanup, SyncError> {
        let Some(conflict) = self.runtime.state.conflict(message.conflict_id)? else {
            return Ok(ConflictCleanup::default());
        };
        if conflict.conflict_revision != message.conflict_revision {
            return Err(SyncError::OperationChanged);
        }
        let created: WorkspaceConflictCreatedMessage =
            serde_json::from_slice(&conflict.created_json).map_err(|_| {
                SyncError::CorruptState {
                    table: "conflicts",
                    field: "created_json",
                }
            })?;
        let resolution_operation_id = conflict
            .resolution_json
            .as_deref()
            .map(|json| {
                serde_json::from_slice::<WorkspaceConflictResolvedRequest>(json)
                    .map(|request| request.operation_id)
                    .map_err(|_| SyncError::CorruptState {
                        table: "conflicts",
                        field: "resolution_json",
                    })
            })
            .transpose()?;
        let originating_operation_id = self
            .runtime
            .state
            .outbox_entry(created.created_by_operation_id)?
            .filter(|record| record.stage == OutboxStage::BlockedConflict)
            .map(|_| created.created_by_operation_id);
        Ok(ConflictCleanup {
            resolution_operation_id,
            originating_operation_id,
        })
    }

    fn preserve_event(
        &mut self,
        event: &WorkspaceEventMessage,
        _previous: &[(WorkspacePath, Option<WorkspacePathState>)],
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
        post_states: Vec<WorkspacePathState>,
        operation_digest: [u8; 32],
        remove_outbox: bool,
    ) -> Result<(), SyncError> {
        let replacements = self.preserved_replacements(event, observed, &post_states)?;
        let legacy_body_digest = match self.event_receipt_match(event)? {
            AppliedReceiptMatch::Legacy { body_digest } => Some(body_digest),
            AppliedReceiptMatch::Missing | AppliedReceiptMatch::Exact => None,
        };
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            tx.record_mutation_applied_operation(
                event.origin_client_id,
                event.operation_id,
                event.revision,
                operation_digest,
                &event.mutation,
                legacy_body_digest,
            )?;
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            for path in &replacements.settled_paths {
                tx.remove_local_intent(path)?;
            }
            for (mutation, timestamp) in &replacements.mutations {
                tx.enqueue_mutation_at(mutation, *timestamp)?;
            }
            tx.put_stream_event(event, StreamItemStatus::Preserved)?;
            Ok(())
        })
    }

    fn preserve_live_event(
        &mut self,
        event: &WorkspaceEventMessage,
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
        post_states: Vec<WorkspacePathState>,
        operation_digest: [u8; 32],
        remove_outbox: bool,
    ) -> Result<(), SyncError> {
        let replacements = self.preserved_replacements(event, observed, &post_states)?;
        let legacy_body_digest = match self.event_receipt_match(event)? {
            AppliedReceiptMatch::Legacy { body_digest } => Some(body_digest),
            AppliedReceiptMatch::Missing | AppliedReceiptMatch::Exact => None,
        };
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            tx.record_mutation_applied_operation(
                event.origin_client_id,
                event.operation_id,
                event.revision,
                operation_digest,
                &event.mutation,
                legacy_body_digest,
            )?;
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            for path in &replacements.settled_paths {
                tx.remove_local_intent(path)?;
            }
            for (mutation, timestamp) in &replacements.mutations {
                tx.enqueue_mutation_at(mutation, *timestamp)?;
            }
            tx.set_last_applied_revision(event.revision)?;
            tx.set_pending_ack(event.revision)
        })
    }

    fn preserved_replacements(
        &mut self,
        event: &WorkspaceEventMessage,
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
        post_states: &[WorkspacePathState],
    ) -> Result<PreservedReplacements, SyncError> {
        let event_paths = mutation_paths(&event.mutation);
        let mut desired = self.deferred_operations_intersecting(&event_paths)?;
        for (path, observed) in observed {
            if desired
                .iter()
                .any(|operation| desired_operation_covers_path(operation, path))
            {
                continue;
            }
            let operation = if observed.is_some() {
                self.desired_from_current(path)?
            } else {
                DesiredOperation::Delete { path: path.clone() }
            };
            if !desired.contains(&operation) {
                desired.push(operation);
            }
        }
        let mut states = self.path_state_map()?;
        for state in post_states {
            states.insert(state.path.clone(), state.clone());
        }
        let replacement_desired = desired
            .iter()
            .map(|operation| replacement_desired_against_states(operation, &states))
            .collect::<Vec<_>>();
        let replacements = self.next_mutations(&replacement_desired, &states)?;
        let settled_paths = paths_for_desired_operations(&event_paths, &desired);
        Ok(PreservedReplacements {
            mutations: replacements,
            settled_paths,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_with_journal(
        &mut self,
        stream_id: fns_protocol::StreamId,
        item_kind: ApplyItemKind,
        item_key: String,
        operation_spec: RemoteApplyOperation,
        post_states: Vec<WorkspacePathState>,
        commit_plan: ApplyCommitPlan,
        operation: FsOperation,
    ) -> Result<fns_fs::ApplyReceipt, SyncError> {
        let apply_id = ApplyId(uuid::Uuid::new_v4());
        let operation_json = canonical_json(&operation_spec)?;
        let filesystem_operation_json = canonical_json(&operation)?;
        let commit_json = canonical_json(&commit_plan)?;
        let mut record = ApplyJournalRecord {
            apply_id,
            workspace_id: self.runtime.state.workspace_id(),
            stream_id,
            item_kind,
            item_key,
            apply_namespace: commit_plan.namespace(),
            operation_body_digest: [0; 32],
            operation_json,
            filesystem_operation_json: filesystem_operation_json.clone(),
            commit_json,
            preimage_json: filesystem_operation_json,
            postimage_json: canonical_json(&post_states)?,
            filesystem_receipt_json: None,
            stage: ApplyStage::Prepared,
        };
        record.operation_body_digest = crate::apply_journal_immutable_digest(&record);
        self.runtime.state.put_apply_journal(&record)?;
        apply_failpoint("prepared");
        self.runtime
            .state
            .set_apply_stage(apply_id, ApplyStage::FilesystemStarted)?;
        apply_failpoint("filesystem_started");
        let receipt = self
            .runtime
            .system
            .writer
            .apply(apply_id, &operation)
            .map_err(SyncError::Filesystem)?;
        let receipt_json = canonical_json(&receipt)?;
        self.runtime
            .state
            .set_apply_filesystem_applied(apply_id, &receipt_json)?;
        apply_failpoint("filesystem_applied");
        Ok(receipt)
    }

    fn finish_apply_journal(&mut self, receipt: &fns_fs::ApplyReceipt) -> Result<(), SyncError> {
        self.runtime.system.writer.finalize(receipt)?;
        apply_failpoint("filesystem_finalized");
        self.runtime
            .state
            .set_apply_stage(receipt.apply_id, ApplyStage::Finalized)?;
        apply_failpoint("finalized");
        self.runtime.state.remove_apply_journal(receipt.apply_id)
    }

    fn operation_for_state(
        &mut self,
        post: &WorkspacePathState,
        observed: Option<&ObservedEntry>,
    ) -> Result<Option<FsOperation>, SyncError> {
        let expected = self.expected_from_observed(observed)?;
        Ok(match post.kind {
            WorkspaceEntryKind::File => Some(FsOperation::UpsertFile {
                path: post.path.clone(),
                content_hash: post.content_hash.clone().into_option().ok_or(
                    SyncError::ProtocolInvariant {
                        reason: "file_content_hash_missing",
                    },
                )?,
                metadata: post.metadata.clone(),
                expected,
            }),
            WorkspaceEntryKind::Directory => Some(FsOperation::Mkdir {
                path: post.path.clone(),
                metadata: post.metadata.clone(),
                expected,
            }),
            WorkspaceEntryKind::Symlink => Some(FsOperation::UpsertSymlink {
                path: post.path.clone(),
                content_hash: post.content_hash.clone().into_option().ok_or(
                    SyncError::ProtocolInvariant {
                        reason: "symlink_content_hash_missing",
                    },
                )?,
                metadata: post.metadata.clone(),
                expected,
            }),
            WorkspaceEntryKind::Tombstone => {
                if matches!(expected, ExpectedEntry::Missing) {
                    None
                } else {
                    Some(FsOperation::Delete {
                        path: post.path.clone(),
                        expected,
                    })
                }
            }
        })
    }

    fn operation_for_event(
        &mut self,
        event: &WorkspaceEventMessage,
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
    ) -> Result<Option<FsOperation>, SyncError> {
        if event.mutation.kind != fns_protocol::WorkspaceMutationKind::Rename {
            return self.operation_for_state(
                &event.path_state,
                observed.first().and_then(|(_, value)| value.as_ref()),
            );
        }
        let new_path = event
            .mutation
            .new_path
            .clone()
            .ok_or(SyncError::ProtocolInvariant {
                reason: "rename_target_missing",
            })?;
        let source_observed = observed
            .iter()
            .find(|(path, _)| *path == event.mutation.path)
            .and_then(|(_, value)| value.as_ref());
        let target_observed = observed
            .iter()
            .find(|(path, _)| *path == new_path)
            .and_then(|(_, value)| value.as_ref());
        let Some(source_observed) = source_observed else {
            return Ok(None);
        };
        let source_expected = self.expected_from_observed(Some(source_observed))?;
        let target_expected = self.expected_from_observed(target_observed)?;
        let target_state = event
            .new_path_state
            .as_ref()
            .ok_or(SyncError::ProtocolInvariant {
                reason: "rename_target_state_missing",
            })?;
        Ok(Some(FsOperation::Rename {
            path: event.mutation.path.clone(),
            new_path,
            content_hash: target_state.content_hash.clone().into_option(),
            metadata: target_state.metadata.clone(),
            source_expected,
            target_expected,
        }))
    }

    fn observed_matches_post(
        &mut self,
        state: &WorkspacePathState,
        observed: Option<&ObservedEntry>,
    ) -> Result<bool, SyncError> {
        let Some(observed) = observed else {
            return Ok(state.kind == WorkspaceEntryKind::Tombstone);
        };
        if observed.kind != state.kind || state.kind == WorkspaceEntryKind::Tombstone {
            return Ok(false);
        }
        let hash = self.observed_content_hash(observed)?;
        Ok(state.content_hash.as_ref().into_option() == hash.as_ref()
            && (state.kind == WorkspaceEntryKind::Directory
                || observed.metadata.size == state.metadata.size))
    }

    fn path_states_for_event(
        &self,
        event: &WorkspaceEventMessage,
    ) -> Result<Vec<(WorkspacePath, Option<WorkspacePathState>)>, SyncError> {
        let mut paths = vec![event.mutation.path.clone()];
        if let Some(path) = &event.mutation.new_path {
            paths.push(path.clone());
        }
        paths
            .into_iter()
            .map(|path| {
                let state = self
                    .runtime
                    .state
                    .path_state(path.as_str())?
                    .map(|record| record.state);
                Ok((path, state))
            })
            .collect()
    }

    fn observed_for_event(
        &self,
        event: &WorkspaceEventMessage,
    ) -> Result<Vec<(WorkspacePath, Option<ObservedEntry>)>, SyncError> {
        let mut paths = vec![event.mutation.path.clone()];
        if let Some(path) = &event.mutation.new_path {
            paths.push(path.clone());
        }
        paths
            .into_iter()
            .map(|path| {
                let observed = self.runtime.system.workspace.inspect(&path)?;
                Ok((path, observed))
            })
            .collect()
    }

    fn event_is_diverged(&mut self, event: &WorkspaceEventMessage) -> Result<bool, SyncError> {
        let previous = self.path_states_for_event(event)?;
        let observed = self.observed_for_event(event)?;
        if self.event_post_matches(&event_post_states(event), &observed)? {
            return Ok(false);
        }
        Ok(!self.event_baseline_matches(&previous, &observed)?)
    }

    fn event_post_matches(
        &mut self,
        states: &[WorkspacePathState],
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
    ) -> Result<bool, SyncError> {
        for state in states {
            let current = observed
                .iter()
                .find(|(path, _)| *path == state.path)
                .and_then(|(_, value)| value.as_ref());
            if !self.observed_matches_post(state, current)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn event_baseline_matches(
        &mut self,
        previous: &[(WorkspacePath, Option<WorkspacePathState>)],
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
    ) -> Result<bool, SyncError> {
        for (path, state) in previous {
            let current = observed
                .iter()
                .find(|(candidate, _)| candidate == path)
                .and_then(|(_, value)| value.as_ref());
            if !baseline_matches_observed(state.as_ref(), current, self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn observed_content_hash(
        &mut self,
        observed: &ObservedEntry,
    ) -> Result<Option<fns_protocol::WorkspaceContentHash>, SyncError> {
        if observed.kind == WorkspaceEntryKind::Directory {
            return Ok(None);
        }
        Ok(self
            .desired_entry_from_observed(observed)?
            .content_hash
            .into_option())
    }

    fn expected_from_observed(
        &mut self,
        observed: Option<&ObservedEntry>,
    ) -> Result<ExpectedEntry, SyncError> {
        let Some(observed) = observed else {
            return Ok(ExpectedEntry::Missing);
        };
        Ok(ExpectedEntry::Present {
            kind: observed.kind,
            content_hash: self.observed_content_hash(observed)?,
            directory_snapshot: (observed.kind == WorkspaceEntryKind::Directory)
                .then(|| {
                    self.runtime
                        .system
                        .workspace
                        .directory_snapshot(&observed.path)
                })
                .transpose()?,
            fingerprint: observed.fingerprint.clone(),
        })
    }

    fn desired_from_current(
        &mut self,
        path: &WorkspacePath,
    ) -> Result<DesiredOperation, SyncError> {
        let Some(observed) = self.runtime.system.workspace.inspect(path)? else {
            return Ok(DesiredOperation::Delete { path: path.clone() });
        };
        Ok(DesiredOperation::Upsert {
            entry: self.desired_entry_from_observed(&observed)?,
        })
    }

    fn queue_desired_with_states(
        &mut self,
        desired: DesiredOperation,
        states: &BTreeMap<WorkspacePath, WorkspacePathState>,
    ) -> Result<(), SyncError> {
        let touched = desired.paths().into_iter().cloned().collect::<Vec<_>>();
        let existing = self.runtime.state.outbox()?.into_iter().find_map(|record| {
            let mutation = record.mutation().ok()?;
            mutation_paths(&mutation)
                .iter()
                .any(|path| touched.iter().any(|candidate| candidate == path))
                .then_some((record, mutation))
        });
        if let Some((record, mutation)) = existing {
            if record.stage == OutboxStage::Queued && mutation_matches_desired(&mutation, &desired)
            {
                return Ok(());
            }
            let intent = desired.intent_for_path(&touched[0]);
            let body = encode_intent(&intent)?;
            let timestamps = touched
                .iter()
                .map(|_| self.next_timestamp())
                .collect::<Vec<_>>();
            return self.runtime.state.transaction(|tx| {
                for (path, timestamp) in touched.iter().zip(timestamps) {
                    tx.put_local_intent(path.as_str(), &body, timestamp)?;
                }
                Ok(())
            });
        }
        let mutation = mutation_for_desired(
            &desired,
            self.runtime.state.workspace_id(),
            self.runtime.state.client_id(),
            self.next_operation_id()?,
            states,
        );
        let timestamp = self.next_timestamp();
        self.runtime.state.transaction(|tx| {
            tx.enqueue_mutation_at(&mutation, timestamp)?;
            Ok(())
        })
    }

    fn ensure_open(&self) -> Result<(), SyncError> {
        if self.closed {
            return Err(SyncError::ProtocolInvariant {
                reason: "engine_closed",
            });
        }
        Ok(())
    }

    fn validate_identity(
        &self,
        workspace_id: fns_protocol::WorkspaceId,
        client_id: fns_protocol::ClientId,
    ) -> Result<(), SyncError> {
        if workspace_id != self.runtime.state.workspace_id()
            || client_id != self.runtime.state.client_id()
        {
            return Err(SyncError::ProtocolInvariant {
                reason: "mutation_result_identity_mismatch",
            });
        }
        Ok(())
    }

    fn next_operation_id(&mut self) -> Result<fns_protocol::OperationId, SyncError> {
        while let Some(operation_id) = self.operation_ids.pop_front() {
            if self.runtime.state.outbox_entry(operation_id)?.is_none()
                && self
                    .runtime
                    .state
                    .applied_operation(self.runtime.state.client_id(), operation_id)?
                    .is_none()
            {
                return Ok(operation_id);
            }
        }
        Ok(
            fns_protocol::OperationId::parse(&uuid::Uuid::new_v4().to_string())
                .expect("uuid v4 is canonical"),
        )
    }

    fn next_timestamp(&mut self) -> i64 {
        self.timestamps
            .pop_front()
            .unwrap_or_else(crate::ids::now_ms)
    }
}

#[cfg(debug_assertions)]
fn apply_failpoint(stage: &str) {
    if std::env::var_os("FNS_SYNC_APPLY_FAILPOINT").as_deref() == Some(std::ffi::OsStr::new(stage))
    {
        std::process::abort();
    }
}

#[cfg(not(debug_assertions))]
fn apply_failpoint(_stage: &str) {}

fn parse_apply_journal_json<T>(bytes: &[u8], field: &'static str) -> Result<T, SyncError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value = serde_json::from_slice(bytes).map_err(|_| SyncError::CorruptState {
        table: "apply_journal",
        field,
    })?;
    if canonical_json(&value)? != bytes {
        return Err(SyncError::CorruptState {
            table: "apply_journal",
            field,
        });
    }
    Ok(value)
}

impl RemoteApplyOperation {
    fn from_state(state: &WorkspacePathState) -> Self {
        if state.kind == WorkspaceEntryKind::Tombstone {
            Self::Delete {
                state: state.clone(),
            }
        } else {
            Self::Upsert {
                state: state.clone(),
            }
        }
    }

    fn from_event(event: &WorkspaceEventMessage) -> Self {
        if event.mutation.kind == fns_protocol::WorkspaceMutationKind::Rename {
            Self::Rename {
                old_state: event
                    .old_path_state
                    .clone()
                    .expect("validated rename old state"),
                new_state: event
                    .new_path_state
                    .clone()
                    .expect("validated rename new state"),
            }
        } else {
            Self::from_state(&event.path_state)
        }
    }
}

fn validate_apply_recovery_tuple(
    operation: &RemoteApplyOperation,
    filesystem_operation: &FsOperation,
    post_states: &[WorkspacePathState],
    plan: &ApplyCommitPlan,
) -> Result<(), SyncError> {
    let (expected_operation, expected_post_states) = match plan {
        ApplyCommitPlan::SnapshotEntry { entry } => (
            RemoteApplyOperation::from_state(&entry.entry),
            vec![entry.entry.clone()],
        ),
        ApplyCommitPlan::StreamEvent { event, .. } | ApplyCommitPlan::LiveEvent { event, .. } => (
            RemoteApplyOperation::from_event(event),
            event_post_states(event),
        ),
        ApplyCommitPlan::StreamConflictResolved { message }
        | ApplyCommitPlan::LiveConflictResolved { message } => (
            RemoteApplyOperation::from_state(&message.path_state),
            vec![message.path_state.clone()],
        ),
    };
    if operation != &expected_operation {
        return Err(SyncError::CorruptState {
            table: "apply_journal",
            field: "operation_json",
        });
    }
    if post_states != expected_post_states
        || post_states.iter().any(|state| state.validate().is_err())
    {
        return Err(SyncError::CorruptState {
            table: "apply_journal",
            field: "postimage_json",
        });
    }
    if !filesystem_operation_matches_remote(filesystem_operation, operation) {
        return Err(SyncError::CorruptState {
            table: "apply_journal",
            field: "filesystem_operation_json",
        });
    }
    Ok(())
}

fn filesystem_operation_matches_remote(
    filesystem_operation: &FsOperation,
    operation: &RemoteApplyOperation,
) -> bool {
    match (filesystem_operation, operation) {
        (
            FsOperation::UpsertFile {
                path,
                content_hash,
                metadata,
                ..
            },
            RemoteApplyOperation::Upsert { state },
        ) => {
            state.kind == WorkspaceEntryKind::File
                && *path == state.path
                && state.content_hash.as_ref().into_option() == Some(content_hash)
                && *metadata == state.metadata
        }
        (FsOperation::Mkdir { path, metadata, .. }, RemoteApplyOperation::Upsert { state }) => {
            state.kind == WorkspaceEntryKind::Directory
                && *path == state.path
                && *metadata == state.metadata
        }
        (
            FsOperation::UpsertSymlink {
                path,
                content_hash,
                metadata,
                ..
            },
            RemoteApplyOperation::Upsert { state },
        ) => {
            state.kind == WorkspaceEntryKind::Symlink
                && *path == state.path
                && state.content_hash.as_ref().into_option() == Some(content_hash)
                && *metadata == state.metadata
        }
        (FsOperation::Delete { path, .. }, RemoteApplyOperation::Delete { state }) => {
            *path == state.path && state.kind == WorkspaceEntryKind::Tombstone && state.tombstone
        }
        (
            FsOperation::Rename {
                path,
                new_path,
                content_hash,
                metadata,
                ..
            },
            RemoteApplyOperation::Rename {
                old_state,
                new_state,
            },
        ) => {
            *path == old_state.path
                && *new_path == new_state.path
                && old_state.kind == WorkspaceEntryKind::Tombstone
                && old_state.tombstone
                && new_state.kind != WorkspaceEntryKind::Tombstone
                && !new_state.tombstone
                && *content_hash == new_state.content_hash.clone().into_option()
                && *metadata == new_state.metadata
        }
        _ => false,
    }
}

fn operation_directory_snapshots(
    operation: &FsOperation,
) -> Vec<(&WorkspacePath, &fns_fs::DirectorySnapshot)> {
    fn snapshot(expected: &ExpectedEntry) -> Option<&fns_fs::DirectorySnapshot> {
        match expected {
            ExpectedEntry::Present {
                directory_snapshot: Some(snapshot),
                ..
            } => Some(snapshot),
            ExpectedEntry::Missing | ExpectedEntry::Present { .. } => None,
        }
    }

    match operation {
        FsOperation::Delete { path, expected } => snapshot(expected)
            .map(|snapshot| vec![(path, snapshot)])
            .unwrap_or_default(),
        FsOperation::Rename {
            path,
            new_path,
            source_expected,
            target_expected,
            ..
        } => {
            let mut snapshots = Vec::new();
            if let Some(snapshot) = snapshot(source_expected) {
                snapshots.push((path, snapshot));
            }
            if let Some(snapshot) = snapshot(target_expected) {
                snapshots.push((new_path, snapshot));
            }
            snapshots
        }
        FsOperation::UpsertFile { .. }
        | FsOperation::Mkdir { .. }
        | FsOperation::UpsertSymlink { .. } => Vec::new(),
    }
}

fn required_content(
    state: &WorkspacePathState,
) -> Option<(fns_protocol::WorkspaceContentHash, u64)> {
    match state.kind {
        WorkspaceEntryKind::File | WorkspaceEntryKind::Symlink => state
            .content_hash
            .clone()
            .into_option()
            .map(|hash| (hash, state.metadata.size)),
        WorkspaceEntryKind::Directory | WorkspaceEntryKind::Tombstone => None,
    }
}

fn required_event_content(
    event: &WorkspaceEventMessage,
) -> Option<(fns_protocol::WorkspaceContentHash, u64)> {
    event_post_states(event)
        .into_iter()
        .find_map(|state| required_content(&state))
}

fn legacy_filesystem_operation(operation: &RemoteApplyOperation) -> Result<FsOperation, SyncError> {
    let invalid = || SyncError::CorruptState {
        table: "apply_journal",
        field: "operation_json",
    };
    Ok(match operation {
        RemoteApplyOperation::Upsert { state } => match state.kind {
            WorkspaceEntryKind::File => FsOperation::UpsertFile {
                path: state.path.clone(),
                content_hash: state
                    .content_hash
                    .clone()
                    .into_option()
                    .ok_or_else(invalid)?,
                metadata: state.metadata.clone(),
                expected: ExpectedEntry::Missing,
            },
            WorkspaceEntryKind::Directory => FsOperation::Mkdir {
                path: state.path.clone(),
                metadata: state.metadata.clone(),
                expected: ExpectedEntry::Missing,
            },
            WorkspaceEntryKind::Symlink => FsOperation::UpsertSymlink {
                path: state.path.clone(),
                content_hash: state
                    .content_hash
                    .clone()
                    .into_option()
                    .ok_or_else(invalid)?,
                metadata: state.metadata.clone(),
                expected: ExpectedEntry::Missing,
            },
            WorkspaceEntryKind::Tombstone => return Err(invalid()),
        },
        RemoteApplyOperation::Delete { state } => FsOperation::Delete {
            path: state.path.clone(),
            expected: ExpectedEntry::Missing,
        },
        RemoteApplyOperation::Rename {
            old_state,
            new_state,
        } => FsOperation::Rename {
            path: old_state.path.clone(),
            new_path: new_state.path.clone(),
            content_hash: new_state.content_hash.clone().into_option(),
            metadata: new_state.metadata.clone(),
            source_expected: ExpectedEntry::Missing,
            target_expected: ExpectedEntry::Missing,
        },
    })
}

fn is_migrated_legacy_apply_journal(record: &ApplyJournalRecord) -> bool {
    record.commit_json.is_empty()
        && (record.filesystem_operation_json.is_empty()
            || record.filesystem_operation_json == record.operation_json)
        && record.filesystem_receipt_json.is_none()
        && matches!(
            record.stage,
            ApplyStage::Prepared | ApplyStage::FilesystemStarted
        )
        && matches!(
            (record.item_kind, record.apply_namespace),
            (ApplyItemKind::Entry, ApplyNamespace::SnapshotEntry)
                | (ApplyItemKind::Event, ApplyNamespace::StreamEvent)
                | (
                    ApplyItemKind::ConflictResolved,
                    ApplyNamespace::StreamConflictResolved
                )
        )
}

fn push_download(
    commands: &mut Vec<SyncCommand>,
    limit: usize,
    workspace_id: fns_protocol::WorkspaceId,
    operation_id: Option<fns_protocol::OperationId>,
    content_hash: fns_protocol::WorkspaceContentHash,
    size: u64,
) {
    if commands.len() >= limit
        || commands.iter().any(|command| {
            matches!(command, SyncCommand::DownloadBlob { content_hash: existing, .. } if *existing == content_hash)
        })
    {
        return;
    }
    commands.push(SyncCommand::DownloadBlob {
        workspace_id,
        operation_id,
        content_hash,
        size,
    });
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedOperationResult<'a> {
    mutation: &'a WorkspaceMutation,
    path_state: &'a WorkspacePathState,
    old_path_state: Option<&'a WorkspacePathState>,
    new_path_state: Option<&'a WorkspacePathState>,
}

fn applied_operation_digest(
    mutation: &WorkspaceMutation,
    path_state: &WorkspacePathState,
    old_path_state: Option<&WorkspacePathState>,
    new_path_state: Option<&WorkspacePathState>,
) -> Result<[u8; 32], SyncError> {
    let result = AppliedOperationResult {
        mutation,
        path_state,
        old_path_state,
        new_path_state,
    };
    Ok(crate::body_digest(&canonical_json(&result)?))
}

fn legacy_mutation_digest(mutation: &WorkspaceMutation) -> Result<[u8; 32], SyncError> {
    Ok(crate::body_digest(&canonical_json(mutation)?))
}

fn applied_event_digest(event: &WorkspaceEventMessage) -> Result<[u8; 32], SyncError> {
    applied_operation_digest(
        &event.mutation,
        &event.path_state,
        event.old_path_state.as_ref(),
        event.new_path_state.as_ref(),
    )
}

fn acceptance_from_event(event: &WorkspaceEventMessage) -> WorkspaceMutationAcceptedMessage {
    WorkspaceMutationAcceptedMessage {
        workspace_id: event.workspace_id,
        client_id: event.origin_client_id,
        operation_id: event.operation_id,
        revision: event.revision,
        path_state: event.path_state.clone(),
        old_path_state: event.old_path_state.clone(),
        new_path_state: event.new_path_state.clone(),
    }
}

fn same_acceptance_result(
    left: &WorkspaceMutationAcceptedMessage,
    right: &WorkspaceMutationAcceptedMessage,
) -> bool {
    left.revision == right.revision
        && left.path_state == right.path_state
        && left.old_path_state == right.old_path_state
        && left.new_path_state == right.new_path_state
}

fn event_post_states(event: &WorkspaceEventMessage) -> Vec<WorkspacePathState> {
    if event.mutation.kind == fns_protocol::WorkspaceMutationKind::Rename {
        vec![
            event
                .old_path_state
                .clone()
                .expect("validated rename old state"),
            event
                .new_path_state
                .clone()
                .expect("validated rename new state"),
        ]
    } else {
        vec![event.path_state.clone()]
    }
}

fn baseline_matches_observed(
    baseline: Option<&WorkspacePathState>,
    observed: Option<&ObservedEntry>,
    engine: &mut SyncEngine,
) -> Result<bool, SyncError> {
    let Some(baseline) = baseline else {
        return Ok(observed.is_none());
    };
    if baseline.kind == WorkspaceEntryKind::Tombstone || baseline.tombstone {
        return Ok(observed.is_none());
    }
    let Some(observed) = observed else {
        return Ok(false);
    };
    if observed.kind != baseline.kind || observed.metadata != baseline.metadata {
        return Ok(false);
    }
    let hash = engine.observed_content_hash(observed)?;
    Ok(baseline.content_hash.as_ref().into_option() == hash.as_ref())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn preflight_state_root(workspace_root: &Path, configured: &Path) -> Result<PathBuf, SyncError> {
    let absolute_workspace = absolute(workspace_root).map_err(|_| {
        SyncError::Filesystem(fns_fs::FsError::Io {
            operation: "resolve workspace root",
        })
    })?;
    let absolute_configured = absolute(configured).map_err(|_| {
        SyncError::Filesystem(fns_fs::FsError::Io {
            operation: "resolve sync state root",
        })
    })?;
    if paths_overlap(&absolute_workspace, &absolute_configured) {
        return Err(SyncError::InvalidConfiguration {
            reason: "roots_overlap",
        });
    }

    let mut ancestor = absolute_configured.clone();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(SyncError::Filesystem(fns_fs::FsError::Io {
                        operation: "resolve sync state root",
                    }));
                };
                missing.push(name.to_os_string());
                if !ancestor.pop() {
                    return Err(SyncError::Filesystem(fns_fs::FsError::Io {
                        operation: "resolve sync state root",
                    }));
                }
            }
            Err(_) => {
                return Err(SyncError::Filesystem(fns_fs::FsError::Io {
                    operation: "stat sync state root",
                }));
            }
        }
    }
    let mut candidate = fs::canonicalize(&ancestor).map_err(|_| {
        SyncError::Filesystem(fns_fs::FsError::Io {
            operation: "canonicalize sync state root parent",
        })
    })?;
    for component in missing.iter().rev() {
        candidate.push(component);
    }
    if paths_overlap(workspace_root, &candidate) {
        return Err(SyncError::InvalidConfiguration {
            reason: "roots_overlap",
        });
    }
    Ok(candidate)
}

fn path_is_descendant(path: &WorkspacePath, ancestor: &WorkspacePath) -> bool {
    path.as_str()
        .strip_prefix(ancestor.as_str())
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn paths_intersect(left: &WorkspacePath, right: &WorkspacePath) -> bool {
    left == right || path_is_descendant(left, right) || path_is_descendant(right, left)
}

fn desired_operation_covers_path(desired: &DesiredOperation, path: &WorkspacePath) -> bool {
    match desired {
        DesiredOperation::Rename {
            from,
            to,
            kind: WorkspaceEntryKind::Directory,
            ..
        } => {
            path == from
                || path == to
                || path_is_descendant(path, from)
                || path_is_descendant(path, to)
        }
        _ => desired.paths().contains(&path),
    }
}

fn replacement_desired_against_states(
    desired: &DesiredOperation,
    states: &BTreeMap<WorkspacePath, WorkspacePathState>,
) -> DesiredOperation {
    let DesiredOperation::Rename {
        from,
        to,
        kind,
        content_hash,
        metadata,
    } = desired
    else {
        return desired.clone();
    };
    if desired_matches_remote(desired, states)
        || states
            .get(from)
            .is_some_and(|state| state.kind != WorkspaceEntryKind::Tombstone && !state.tombstone)
    {
        return desired.clone();
    }
    DesiredOperation::Upsert {
        entry: LocalDesiredEntry {
            path: to.clone(),
            kind: *kind,
            content_hash: content_hash.clone(),
            metadata: metadata.clone(),
        },
    }
}

fn desired_operation_key(desired: &DesiredOperation) -> (String, String, u8) {
    match desired {
        DesiredOperation::Upsert { entry } => (entry.path.as_str().to_owned(), String::new(), 0),
        DesiredOperation::Delete { path } => (path.as_str().to_owned(), String::new(), 1),
        DesiredOperation::Rename { from, to, .. } => {
            (from.as_str().to_owned(), to.as_str().to_owned(), 2)
        }
    }
}

fn merge_deferred_desired(
    current: &DesiredOperation,
    incoming: &DesiredOperation,
) -> DesiredOperation {
    match (current, incoming) {
        (DesiredOperation::Rename { from, to, .. }, DesiredOperation::Upsert { entry })
            if entry.path == *to =>
        {
            DesiredOperation::Rename {
                from: from.clone(),
                to: to.clone(),
                kind: entry.kind,
                content_hash: entry.content_hash.clone(),
                metadata: entry.metadata.clone(),
            }
        }
        (
            DesiredOperation::Rename { from, to, .. },
            DesiredOperation::Rename {
                from: incoming_from,
                to: incoming_to,
                kind,
                content_hash,
                metadata,
            },
        ) if *incoming_from == *to => DesiredOperation::Rename {
            from: from.clone(),
            to: incoming_to.clone(),
            kind: *kind,
            content_hash: content_hash.clone(),
            metadata: metadata.clone(),
        },
        (DesiredOperation::Rename { from, to, .. }, DesiredOperation::Delete { path })
            if *path == *to =>
        {
            DesiredOperation::Delete { path: from.clone() }
        }
        _ => incoming.clone(),
    }
}

fn paths_for_desired_operations(
    touched: &[WorkspacePath],
    desired: &[DesiredOperation],
) -> Vec<WorkspacePath> {
    let mut paths = touched.to_vec();
    for operation in desired {
        paths.extend(operation.paths().into_iter().cloned());
    }
    unique_paths(paths)
}

fn unique_paths(paths: Vec<WorkspacePath>) -> Vec<WorkspacePath> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

fn remote_matches_entry(state: &WorkspacePathState, entry: &LocalDesiredEntry) -> bool {
    state.path == entry.path
        && state.kind == entry.kind
        && state.content_hash == entry.content_hash
        && state.metadata == entry.metadata
        && state.tombstone == (entry.kind == WorkspaceEntryKind::Tombstone)
}

fn same_rename_identity(state: &WorkspacePathState, entry: &LocalDesiredEntry) -> bool {
    state.kind == entry.kind
        && state.content_hash == entry.content_hash
        && state.metadata.size == entry.metadata.size
        && state.metadata.executable == entry.metadata.executable
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectorySubtreeSummary {
    digest: [u8; 32],
    entry_count: usize,
}

fn remote_directory_subtree_identity(
    root: &WorkspacePath,
    remote: &BTreeMap<WorkspacePath, WorkspacePathState>,
) -> DirectorySubtreeSummary {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fns-directory-subtree-summary-v1\0");
    let mut entry_count = 0;
    for (path, state) in remote
        .iter()
        .filter(|(_, state)| state.kind != WorkspaceEntryKind::Tombstone && !state.tombstone)
    {
        let Some(relative) = descendant_suffix(path, root) else {
            continue;
        };
        update_directory_subtree_summary(
            &mut hasher,
            relative,
            state.kind,
            &state.content_hash,
            state.metadata.size,
            state.metadata.executable,
        );
        entry_count += 1;
    }
    DirectorySubtreeSummary {
        digest: *hasher.finalize().as_bytes(),
        entry_count,
    }
}

fn current_directory_subtree_identity(
    root: &WorkspacePath,
    current: &BTreeMap<WorkspacePath, LocalDesiredEntry>,
) -> DirectorySubtreeSummary {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fns-directory-subtree-summary-v1\0");
    let mut entry_count = 0;
    for (path, entry) in current {
        let Some(relative) = descendant_suffix(path, root) else {
            continue;
        };
        update_directory_subtree_summary(
            &mut hasher,
            relative,
            entry.kind,
            &entry.content_hash,
            entry.metadata.size,
            entry.metadata.executable,
        );
        entry_count += 1;
    }
    DirectorySubtreeSummary {
        digest: *hasher.finalize().as_bytes(),
        entry_count,
    }
}

fn update_directory_subtree_summary(
    hasher: &mut blake3::Hasher,
    relative: &str,
    kind: WorkspaceEntryKind,
    content_hash: &RequiredNullable<fns_protocol::WorkspaceContentHash>,
    size: u64,
    executable: bool,
) {
    update_length_prefixed(hasher, relative.as_bytes());
    let kind = match kind {
        WorkspaceEntryKind::File => 1,
        WorkspaceEntryKind::Directory => 2,
        WorkspaceEntryKind::Symlink => 3,
        WorkspaceEntryKind::Tombstone => 4,
    };
    hasher.update(&[kind]);
    match content_hash {
        RequiredNullable::Null => {
            hasher.update(&[0]);
        }
        RequiredNullable::Value(hash) => {
            hasher.update(&[1]);
            update_length_prefixed(hasher, hash.as_str().as_bytes());
        }
    }
    hasher.update(&size.to_le_bytes());
    hasher.update(&[u8::from(executable)]);
}

fn update_length_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn directory_subtrees_exact_match(
    old_root: &WorkspacePath,
    new_root: &WorkspacePath,
    remote: &BTreeMap<WorkspacePath, WorkspacePathState>,
    current: &BTreeMap<WorkspacePath, LocalDesiredEntry>,
) -> bool {
    let mut old_entries = remote
        .iter()
        .filter(|(_, state)| state.kind != WorkspaceEntryKind::Tombstone && !state.tombstone)
        .filter_map(|(path, state)| {
            descendant_suffix(path, old_root).map(|relative| (relative, state))
        });
    let mut new_entries = current.iter().filter_map(|(path, entry)| {
        descendant_suffix(path, new_root).map(|relative| (relative, entry))
    });
    loop {
        match (old_entries.next(), new_entries.next()) {
            (None, None) => return true,
            (Some((old_relative, old)), Some((new_relative, new)))
                if old_relative == new_relative
                    && old.kind == new.kind
                    && old.content_hash == new.content_hash
                    && old.metadata.size == new.metadata.size
                    && old.metadata.executable == new.metadata.executable => {}
            _ => return false,
        }
    }
}

fn descendant_suffix<'a>(path: &'a WorkspacePath, root: &WorkspacePath) -> Option<&'a str> {
    path.as_str().strip_prefix(root.as_str())?.strip_prefix('/')
}

fn desired_matches_remote(
    desired: &DesiredOperation,
    states: &BTreeMap<WorkspacePath, WorkspacePathState>,
) -> bool {
    match desired {
        DesiredOperation::Upsert { entry } => states
            .get(&entry.path)
            .is_some_and(|state| remote_matches_entry(state, entry)),
        DesiredOperation::Delete { path } => states.get(path).is_none_or(|state| {
            state.kind == WorkspaceEntryKind::Tombstone
                && state.tombstone
                && state.content_hash.is_null()
        }),
        DesiredOperation::Rename {
            from,
            to,
            kind,
            content_hash,
            metadata,
        } => {
            let source_deleted = states.get(from).is_some_and(|state| {
                state.kind == WorkspaceEntryKind::Tombstone && state.tombstone
            });
            let target = states.get(to).is_some_and(|state| {
                state.kind == *kind
                    && state.content_hash == *content_hash
                    && state.metadata == *metadata
                    && state.tombstone == (*kind == WorkspaceEntryKind::Tombstone)
            });
            source_deleted && target
        }
    }
}

fn mutation_paths(mutation: &WorkspaceMutation) -> Vec<WorkspacePath> {
    let mut paths = vec![mutation.path.clone()];
    if let Some(new_path) = &mutation.new_path {
        paths.push(new_path.clone());
    }
    paths
}

fn resolved_matches_request(
    message: &WorkspaceConflictResolvedMessage,
    request: &WorkspaceConflictResolvedRequest,
) -> bool {
    let identity_matches = message.workspace_id == request.workspace_id
        && message.conflict_id == request.conflict_id
        && message.conflict_revision == request.conflict_revision
        && message.operation_id == request.operation_id
        && message.resolved_by_client_id == request.client_id
        && message.choice == request.choice
        && message.path_state.path == request.path;
    identity_matches
        && (request.choice == fns_protocol::WorkspaceConflictChoice::Current
            || (message.path_state.content_hash == request.content_hash
                && message.path_state.metadata == request.metadata
                && message.path_state.tombstone
                    == (request.choice == fns_protocol::WorkspaceConflictChoice::Delete)))
}

fn live_apply_stream_id() -> StreamId {
    StreamId::parse("00000000-0000-4000-8000-0000000000c0").expect("static live apply stream id")
}

fn validate_acceptance_shape(
    mutation: &WorkspaceMutation,
    accepted: &WorkspaceMutationAcceptedMessage,
) -> Result<(), SyncError> {
    if mutation.kind == fns_protocol::WorkspaceMutationKind::Rename {
        let (Some(old), Some(new), Some(expected_new)) = (
            &accepted.old_path_state,
            &accepted.new_path_state,
            &mutation.new_path,
        ) else {
            return Err(SyncError::ProtocolInvariant {
                reason: "rename_acceptance_pair_missing",
            });
        };
        if old.path != mutation.path || new.path != *expected_new || accepted.path_state != *new {
            return Err(SyncError::ProtocolInvariant {
                reason: "rename_acceptance_path_mismatch",
            });
        }
    } else if accepted.old_path_state.is_some()
        || accepted.new_path_state.is_some()
        || accepted.path_state.path != mutation.path
    {
        return Err(SyncError::ProtocolInvariant {
            reason: "mutation_acceptance_path_mismatch",
        });
    }
    Ok(())
}
