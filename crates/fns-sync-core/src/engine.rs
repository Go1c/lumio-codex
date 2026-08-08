use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf, absolute};

use fns_fs::{
    ApplyId, AtomicWorkspaceWriter, ContentCache, ExpectedEntry, FsChange, FsOperation, HashCache,
    ObservedEntry, RootedWorkspace, SyncRuleConfig, SyncRules,
};
use fns_protocol::{
    RequiredNullable, WorkspaceAckRequest, WorkspaceConflictCreatedMessage,
    WorkspaceConflictResolvedMessage, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceMutation,
    WorkspaceMutationAcceptedMessage, WorkspaceMutationRejectReason,
    WorkspaceMutationRejectedMessage, WorkspacePath, WorkspacePathState,
    WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage, WorkspaceSnapshotEntryMessage,
    WorkspaceSnapshotMode,
};

use crate::effect::SyncCommand;
use crate::error::SyncError;
use crate::model::{
    ApplyItemKind, ApplyJournalRecord, ApplyStage, LocalDesiredEntry, OutboxBody, OutboxStage,
    RemoteApplyOperation, StreamConflictStatus, StreamItemStatus, StreamRevisionItemKind,
    WorkspaceCursor,
};
use crate::reconcile::{
    DesiredOperation, decode_intent, desired_from_intent, desired_from_mutation, encode_intent,
    mutation_for_desired, mutation_matches_desired, zero_metadata,
};
use crate::{SqliteState, canonical_json};

#[derive(Clone, Debug)]
pub struct SyncEngineConfig {
    pub workspace_id: fns_protocol::WorkspaceId,
    pub client_id: fns_protocol::ClientId,
    pub workspace_root: PathBuf,
    pub state_root: PathBuf,
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
    closed: bool,
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
        let rules = SyncRules::compile(SyncRuleConfig::default()).map_err(SyncError::Filesystem)?;
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
        for (delete_index, old_path) in deletions.iter().enumerate() {
            let Some(old_state) = remote.get(old_path) else {
                continue;
            };
            let candidates = additions
                .iter()
                .enumerate()
                .filter(|(index, (_, entry))| {
                    !paired_additions.contains(index) && same_rename_identity(old_state, entry)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if candidates.len() == 1 {
                let add_index = candidates[0];
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
            self.record_desired(desired)?;
        }
        Ok(())
    }

    pub fn pending_commands(&mut self, limit: usize) -> Result<Vec<SyncCommand>, SyncError> {
        self.ensure_open()?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut commands = self.resume_stream_commands(limit, true)?;
        if commands.len() == limit {
            return Ok(commands);
        }
        let remaining = limit - commands.len();
        let records = self.runtime.state.pending_outbox_replay(remaining)?;
        for record in records {
            let mutation = record.mutation().map_err(|_| SyncError::CorruptState {
                table: "outbox",
                field: "body_json",
            })?;
            commands.push(SyncCommand::Mutation(mutation));
        }
        if commands.len() < limit {
            for record in self.runtime.state.outbox()? {
                if record.stage != OutboxStage::AwaitingBlob {
                    continue;
                }
                let OutboxBody::Mutation(mutation) =
                    record.decoded_body().map_err(|_| SyncError::CorruptState {
                        table: "outbox",
                        field: "body_json",
                    })?
                else {
                    continue;
                };
                let hash = mutation.content_hash.clone().into_option().ok_or(
                    SyncError::ProtocolInvariant {
                        reason: "blob_without_content_hash",
                    },
                )?;
                commands.push(SyncCommand::UploadBlob {
                    workspace_id: mutation.workspace_id,
                    operation_id: mutation.operation_id,
                    content_hash: hash,
                    size: mutation.metadata.size,
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

    pub fn outbox(&self) -> Result<Vec<crate::OutboxRecord>, SyncError> {
        self.runtime.state.outbox()
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
        self.validate_identity(accepted.workspace_id, accepted.client_id)?;
        let record = self.runtime.state.outbox_entry(accepted.operation_id)?;
        let Some(record) = record else {
            if let Some(receipt) = self
                .runtime
                .state
                .applied_operation(self.runtime.state.client_id(), accepted.operation_id)?
            {
                if receipt.revision != accepted.revision {
                    return Err(SyncError::OperationChanged);
                }
                return Ok(Vec::new());
            }
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
        let body_digest = record.body_digest;
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
            tx.record_applied_operation(
                client_id,
                accepted.operation_id,
                accepted.revision,
                body_digest,
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
        if event.origin_client_id == self.runtime.state.client_id() {
            if let Some(record) = self.runtime.state.outbox_entry(event.operation_id)? {
                let expected = canonical_json(&event.mutation)?;
                if expected != record.body_json {
                    return Err(SyncError::ProtocolInvariant {
                        reason: "event_operation_body_mismatch",
                    });
                }
                return self.mutation_accepted(WorkspaceMutationAcceptedMessage {
                    workspace_id: event.workspace_id,
                    client_id: event.origin_client_id,
                    operation_id: event.operation_id,
                    revision: event.revision,
                    path_state: event.path_state,
                    old_path_state: event.old_path_state,
                    new_path_state: event.new_path_state,
                });
            }
            if let Some(receipt) = self
                .runtime
                .state
                .applied_operation(self.runtime.state.client_id(), event.operation_id)?
            {
                let body = canonical_json(&event.mutation)?;
                if receipt.revision != event.revision
                    || receipt.body_digest != crate::body_digest(&body)
                {
                    return Err(SyncError::OperationChanged);
                }
                return Ok(Vec::new());
            }
            return Err(SyncError::ProtocolInvariant {
                reason: "event_operation_not_outstanding",
            });
        }
        self.runtime.state.transaction(|tx| {
            if let Some(old) = &event.old_path_state {
                tx.put_path_state(old)?;
            }
            if let Some(new) = &event.new_path_state {
                tx.put_path_state(new)?;
            }
            tx.put_path_state(&event.path_state)
        })?;
        Ok(Vec::new())
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
        self.runtime.state.begin_stream(&message)?;
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
        self.runtime.state.put_stream_entry(&message, status)?;
        let mut commands = self.resume_stream_commands(usize::MAX, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.entry) {
            push_download(
                &mut commands,
                usize::MAX,
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
        let body_digest = crate::body_digest(&canonical_json(&message.mutation)?);
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
        if let Some(receipt) = self
            .runtime
            .state
            .applied_operation(message.origin_client_id, message.operation_id)?
            && (receipt.revision != message.revision || receipt.body_digest != body_digest)
        {
            return Err(SyncError::OperationChanged);
        }
        let diverged = self.event_is_diverged(&message)?;
        let status = if diverged {
            StreamItemStatus::Ready
        } else {
            self.status_for_state(&message.path_state)?
        };
        let needs_download = status == StreamItemStatus::WaitingBlob;
        self.runtime.state.put_stream_event(&message, status)?;
        let mut commands = self.resume_stream_commands(usize::MAX, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.path_state) {
            push_download(
                &mut commands,
                usize::MAX,
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
        if message.workspace_id != self.runtime.state.workspace_id() {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let stream_id = self
            .runtime
            .state
            .stream_state()?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?
            .stream_id;
        self.runtime.state.put_stream_conflict(
            &message,
            StreamConflictStatus::Received,
            stream_id,
        )?;
        self.resume_stream_commands(usize::MAX, false)
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
        let stream_id = self
            .runtime
            .state
            .stream_state()?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?
            .stream_id;
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
        let status = self.status_for_state(&message.path_state)?;
        let needs_download = status == StreamItemStatus::WaitingBlob;
        self.runtime
            .state
            .put_stream_conflict_resolved(&message, None, status)?;
        let mut commands = self.resume_stream_commands(usize::MAX, false)?;
        if needs_download && let Some((hash, size)) = required_content(&message.path_state) {
            push_download(
                &mut commands,
                usize::MAX,
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
        self.resume_stream_commands(usize::MAX, false)
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
        self.resume_stream_commands(usize::MAX, false)
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
        if message.revision != pending || message.revision > cursor.last_applied_revision {
            return Err(SyncError::ProtocolInvariant {
                reason: "ack_mismatch",
            });
        }
        self.runtime.state.transaction(|tx| {
            tx.set_last_ack_revision(message.revision)?;
            tx.clear_pending_ack()?;
            tx.clear_stream()?;
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
            if record.stage == ApplyStage::Prepared {
                self.runtime.state.remove_apply_journal(record.apply_id)?;
                continue;
            }
            let operation: RemoteApplyOperation = serde_json::from_slice(&record.operation_json)
                .map_err(|_| SyncError::CorruptState {
                    table: "apply_journal",
                    field: "operation_json",
                })?;
            let post_states: Vec<WorkspacePathState> =
                serde_json::from_slice(&record.postimage_json).map_err(|_| {
                    SyncError::CorruptState {
                        table: "apply_journal",
                        field: "postimage_json",
                    }
                })?;
            let postimage = post_states.iter().try_fold(true, |matches, state| {
                if !matches {
                    return Ok(false);
                }
                let observed = self.runtime.system.workspace.inspect(&state.path)?;
                self.observed_matches_post(state, observed.as_ref())
            })?;
            if !postimage {
                continue;
            }
            let receipt = fns_fs::ApplyReceipt {
                apply_id: record.apply_id,
                touched: post_states.iter().map(|state| state.path.clone()).collect(),
                postimages: Vec::new(),
                postimage_hashes: Vec::new(),
                cleanup_name: recovery_cleanup_name(record.apply_id, &operation),
            };
            self.runtime.system.writer.finalize(&receipt)?;
            self.runtime.state.remove_apply_journal(record.apply_id)?;
        }
        Ok(())
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

    fn resume_stream_commands(
        &mut self,
        limit: usize,
        reissue_waiting: bool,
    ) -> Result<Vec<SyncCommand>, SyncError> {
        let mut commands = Vec::new();
        loop {
            let Some(active) = self.runtime.state.stream_state()? else {
                break;
            };
            let mut blocked = false;
            let mut progressed = false;
            match active.mode {
                WorkspaceSnapshotMode::Snapshot => {
                    let entries = self.runtime.state.stream_entries(active.stream_id)?;
                    for record in entries {
                        if matches!(
                            record.status,
                            StreamItemStatus::Applied | StreamItemStatus::Preserved
                        ) {
                            continue;
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
                                    &mut commands,
                                    limit,
                                    self.runtime.state.workspace_id(),
                                    None,
                                    hash,
                                    size,
                                );
                            }
                            blocked = true;
                            break;
                        }
                        if record.status != StreamItemStatus::Ready {
                            self.runtime
                                .state
                                .put_stream_entry(&entry, StreamItemStatus::Ready)?;
                        }
                        self.apply_snapshot_entry(entry)?;
                        progressed = true;
                    }
                }
                WorkspaceSnapshotMode::Incremental => {
                    let items = self.runtime.state.stream_revision_items(active.stream_id)?;
                    for record in items {
                        if matches!(
                            record.status,
                            StreamItemStatus::Applied | StreamItemStatus::Preserved
                        ) {
                            continue;
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
                                if record.status != StreamItemStatus::Ready {
                                    if let Some((hash, size)) = required_content(&event.path_state)
                                        && !self.content_available(&hash, size)?
                                    {
                                        self.runtime.state.put_stream_event(
                                            &event,
                                            StreamItemStatus::WaitingBlob,
                                        )?;
                                        if reissue_waiting {
                                            push_download(
                                                &mut commands,
                                                limit,
                                                self.runtime.state.workspace_id(),
                                                Some(event.operation_id),
                                                hash,
                                                size,
                                            );
                                        }
                                        blocked = true;
                                        break;
                                    }
                                    self.runtime
                                        .state
                                        .put_stream_event(&event, StreamItemStatus::Ready)?;
                                }
                                self.apply_event(event)?;
                                progressed = true;
                            }
                            StreamRevisionItemKind::ConflictResolved => {
                                let message: WorkspaceConflictResolvedMessage =
                                    serde_json::from_slice(&record.body_json).map_err(|_| {
                                        SyncError::CorruptState {
                                            table: "stream_revision_items",
                                            field: "body_json",
                                        }
                                    })?;
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
                                            &mut commands,
                                            limit,
                                            self.runtime.state.workspace_id(),
                                            Some(message.operation_id),
                                            hash,
                                            size,
                                        );
                                    }
                                    blocked = true;
                                    break;
                                }
                                if record.status != StreamItemStatus::Ready {
                                    self.runtime.state.put_stream_conflict_resolved(
                                        &message,
                                        None,
                                        StreamItemStatus::Ready,
                                    )?;
                                }
                                self.apply_conflict_resolved(message)?;
                                progressed = true;
                            }
                        }
                    }
                }
            }
            let _ = self.finish_stream_if_ready()?;
            if blocked || !progressed || commands.len() >= limit {
                break;
            }
        }
        Ok(commands)
    }

    fn finish_stream_if_ready(&mut self) -> Result<bool, SyncError> {
        let Some(active) = self.runtime.state.stream_state()? else {
            return Ok(false);
        };
        if !active.end_received {
            return Ok(false);
        }
        let entries = self.runtime.state.stream_entries(active.stream_id)?;
        let revisions = self.runtime.state.stream_revision_items(active.stream_id)?;
        let conflicts = self.runtime.state.stream_conflicts(active.stream_id)?;
        let items_ready = match active.mode {
            WorkspaceSnapshotMode::Snapshot => {
                entries.len() == active.expected_entry_count as usize
                    && revisions.is_empty()
                    && entries.iter().all(|entry| {
                        matches!(
                            entry.status,
                            StreamItemStatus::Applied | StreamItemStatus::Preserved
                        )
                    })
            }
            WorkspaceSnapshotMode::Incremental => {
                revisions.len() == active.expected_event_count as usize
                    && entries.is_empty()
                    && revisions.iter().all(|item| {
                        matches!(
                            item.status,
                            StreamItemStatus::Applied | StreamItemStatus::Preserved
                        )
                    })
            }
        };
        if !items_ready || conflicts.len() != active.expected_conflict_count as usize {
            return Ok(false);
        }
        if !self.runtime.state.apply_journals()?.is_empty() {
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
        Ok(true)
    }

    fn reconcile_full_snapshot(
        &mut self,
        stream_id: fns_protocol::StreamId,
    ) -> Result<(), SyncError> {
        let entries = self.runtime.state.stream_entries(stream_id)?;
        let incoming = entries
            .iter()
            .map(|entry| {
                serde_json::from_slice::<WorkspaceSnapshotEntryMessage>(&entry.body_json)
                    .map(|message| message.entry.path)
                    .map_err(|_| SyncError::CorruptState {
                        table: "stream_entries",
                        field: "body_json",
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let incoming = incoming.into_iter().collect::<HashSet<_>>();
        let remote_states = self
            .runtime
            .state
            .path_states()?
            .into_iter()
            .map(|record| (record.path, record.state))
            .collect::<BTreeMap<_, _>>();
        for (path, state) in &remote_states {
            if incoming.contains(path) {
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
            if incoming.contains(&observed.path) {
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
            previous,
            vec![post.clone()],
            operation,
        )?;
        self.commit_entry(&entry, StreamItemStatus::Applied, vec![post])?;
        self.runtime.system.writer.finalize(&receipt)?;
        self.runtime.state.remove_apply_journal(receipt.apply_id)?;
        Ok(())
    }

    fn apply_event(&mut self, event: WorkspaceEventMessage) -> Result<(), SyncError> {
        let mutation_body = canonical_json(&event.mutation)?;
        let mutation_digest = crate::body_digest(&mutation_body);
        let outbox = self.runtime.state.outbox_entry(event.operation_id)?;
        if event.origin_client_id == self.runtime.state.client_id() {
            if let Some(record) = &outbox
                && record.body_json != mutation_body
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_body_mismatch",
                });
            }
            if outbox.is_none()
                && self
                    .runtime
                    .state
                    .applied_operation(event.origin_client_id, event.operation_id)?
                    .is_none()
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "event_operation_not_outstanding",
                });
            }
            let post_states = event_post_states(&event);
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                post_states,
                Some(event.operation_id),
                Some(mutation_digest),
                outbox.is_some(),
            );
        }
        if let Some(receipt) = self
            .runtime
            .state
            .applied_operation(event.origin_client_id, event.operation_id)?
        {
            if receipt.revision != event.revision || receipt.body_digest != mutation_digest {
                return Err(SyncError::OperationChanged);
            }
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                event_post_states(&event),
                None,
                None,
                false,
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
                Some(mutation_digest),
                false,
            );
        }
        if !baseline_matches {
            self.preserve_event(&event, &previous, &observed, post_states, mutation_digest)?;
            return Ok(());
        }
        let Some(operation) = self.operation_for_event(&event, &observed)? else {
            return self.commit_event(
                &event,
                StreamItemStatus::Applied,
                post_states,
                Some(event.operation_id),
                Some(mutation_digest),
                false,
            );
        };
        let receipt = self.apply_with_journal(
            event.stream_id,
            ApplyItemKind::Event,
            event.revision.to_string(),
            RemoteApplyOperation::from_event(&event),
            previous.first().and_then(|(_, state)| state.clone()),
            post_states.clone(),
            operation,
        )?;
        self.commit_event(
            &event,
            StreamItemStatus::Applied,
            post_states,
            Some(event.operation_id),
            Some(mutation_digest),
            false,
        )?;
        self.runtime.system.writer.finalize(&receipt)?;
        self.runtime.state.remove_apply_journal(receipt.apply_id)?;
        Ok(())
    }

    fn apply_conflict_resolved(
        &mut self,
        message: WorkspaceConflictResolvedMessage,
    ) -> Result<(), SyncError> {
        let path = message.path_state.path.clone();
        let previous = self
            .runtime
            .state
            .path_state(path.as_str())?
            .map(|record| record.state);
        let observed = self.runtime.system.workspace.inspect(&path)?;
        if self.observed_matches_post(&message.path_state, observed.as_ref())? {
            return self.commit_conflict_resolved(&message, StreamItemStatus::Applied);
        }
        if !baseline_matches_observed(previous.as_ref(), observed.as_ref(), self)? {
            let desired = self.desired_from_current(&path)?;
            let baseline = previous
                .clone()
                .map(|state| BTreeMap::from([(path.clone(), state)]))
                .unwrap_or_default();
            self.queue_desired_with_states(desired, &baseline)?;
            return self.commit_conflict_resolved(&message, StreamItemStatus::Preserved);
        }
        let Some(operation) = self.operation_for_state(&message.path_state, observed.as_ref())?
        else {
            return self.commit_conflict_resolved(&message, StreamItemStatus::Applied);
        };
        let stream_id = self
            .runtime
            .state
            .stream_state()?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?
            .stream_id;
        let receipt = self.apply_with_journal(
            stream_id,
            ApplyItemKind::ConflictResolved,
            message.revision.to_string(),
            RemoteApplyOperation::from_state(&message.path_state),
            previous,
            vec![message.path_state.clone()],
            operation,
        )?;
        self.commit_conflict_resolved(&message, StreamItemStatus::Applied)?;
        self.runtime.system.writer.finalize(&receipt)?;
        self.runtime.state.remove_apply_journal(receipt.apply_id)?;
        Ok(())
    }

    fn commit_entry(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
        post_states: Vec<WorkspacePathState>,
    ) -> Result<(), SyncError> {
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            if status == StreamItemStatus::Applied {
                tx.set_last_applied_revision(entry.entry.path_revision)?;
            }
            tx.put_stream_entry(entry, status)?;
            Ok(())
        })
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
        self.runtime.state.transaction(|tx| {
            for state in &post_states {
                tx.put_path_state(state)?;
            }
            if let (Some(operation_id), Some(operation_digest)) = (operation_id, operation_digest) {
                tx.record_applied_operation(
                    event.origin_client_id,
                    operation_id,
                    event.revision,
                    operation_digest,
                )?;
            }
            if remove_outbox {
                tx.remove_outbox(event.operation_id)?;
            }
            if status == StreamItemStatus::Applied {
                tx.set_last_applied_revision(event.revision)?;
            }
            tx.put_stream_event(event, status)?;
            Ok(())
        })
    }

    fn commit_conflict_resolved(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        status: StreamItemStatus,
    ) -> Result<(), SyncError> {
        self.runtime.state.transaction(|tx| {
            tx.put_path_state(&message.path_state)?;
            tx.record_applied_operation(
                message.resolved_by_client_id,
                message.operation_id,
                message.revision,
                crate::body_digest(&canonical_json(message)?),
            )?;
            if status == StreamItemStatus::Applied {
                tx.set_last_applied_revision(message.revision)?;
            }
            tx.put_stream_conflict_resolved(message, status)?;
            Ok(())
        })
    }

    fn preserve_event(
        &mut self,
        event: &WorkspaceEventMessage,
        previous: &[(WorkspacePath, Option<WorkspacePathState>)],
        observed: &[(WorkspacePath, Option<ObservedEntry>)],
        post_states: Vec<WorkspacePathState>,
        mutation_digest: [u8; 32],
    ) -> Result<(), SyncError> {
        for (path, observed) in observed {
            let desired = if observed.is_some() {
                self.desired_from_current(path)?
            } else {
                DesiredOperation::Delete { path: path.clone() }
            };
            let baseline = previous
                .iter()
                .filter_map(|(candidate, state)| (candidate == path).then_some(state.clone()))
                .flatten()
                .map(|state| (state.path.clone(), state))
                .collect::<BTreeMap<_, _>>();
            self.queue_desired_with_states(desired, &baseline)?;
        }
        self.commit_event(
            event,
            StreamItemStatus::Preserved,
            post_states,
            Some(event.operation_id),
            Some(mutation_digest),
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_with_journal(
        &mut self,
        stream_id: fns_protocol::StreamId,
        item_kind: ApplyItemKind,
        item_key: String,
        operation_spec: RemoteApplyOperation,
        previous: Option<WorkspacePathState>,
        post_states: Vec<WorkspacePathState>,
        operation: FsOperation,
    ) -> Result<fns_fs::ApplyReceipt, SyncError> {
        let apply_id = ApplyId(uuid::Uuid::new_v4());
        let record = ApplyJournalRecord {
            apply_id,
            workspace_id: self.runtime.state.workspace_id(),
            stream_id,
            item_kind,
            item_key,
            operation_json: canonical_json(&operation_spec)?,
            preimage_json: canonical_json(&previous)?,
            postimage_json: canonical_json(&post_states)?,
            stage: ApplyStage::Prepared,
        };
        self.runtime.state.put_apply_journal(&record)?;
        self.runtime
            .state
            .set_apply_stage(apply_id, ApplyStage::FilesystemStarted)?;
        self.runtime
            .system
            .writer
            .apply(apply_id, &operation)
            .map_err(SyncError::Filesystem)
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

fn recovery_cleanup_name(apply_id: ApplyId, operation: &RemoteApplyOperation) -> Option<String> {
    let (path, prefix) = match operation {
        RemoteApplyOperation::Upsert { .. } => return None,
        RemoteApplyOperation::Delete { state } => (&state.path, ".fns-delete-"),
        RemoteApplyOperation::Rename { new_state, .. } => (&new_state.path, ".fns-delete-"),
    };
    let name = format!("{prefix}{}", apply_id.0);
    Some(
        path.as_str()
            .rsplit_once('/')
            .map_or_else(|| name.clone(), |(parent, _)| format!("{parent}/{name}")),
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
