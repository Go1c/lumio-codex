use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use fns_fs::{
    ContentCache, FsChange, HashCache, ObservedEntry, RootedWorkspace, SyncRuleConfig, SyncRules,
};
use fns_protocol::{
    RequiredNullable, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceMutation,
    WorkspaceMutationAcceptedMessage, WorkspaceMutationRejectReason,
    WorkspaceMutationRejectedMessage, WorkspacePath, WorkspacePathState,
};

use crate::effect::SyncCommand;
use crate::error::SyncError;
use crate::model::{LocalDesiredEntry, OutboxStage};
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
        fs::create_dir_all(&config.state_root).map_err(|_| {
            SyncError::Filesystem(fns_fs::FsError::Io {
                operation: "create sync state root",
            })
        })?;
        let state_root = fs::canonicalize(&config.state_root).map_err(|_| {
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
        let state = SqliteState::open(
            state_root.join("state.sqlite"),
            config.workspace_id,
            config.client_id,
        )?;
        let rules = SyncRules::compile(SyncRuleConfig::default()).map_err(SyncError::Filesystem)?;
        Ok(Self {
            runtime: EngineRuntime {
                system: SystemRuntime {
                    workspace,
                    content_cache,
                    rules,
                },
                state,
            },
            operation_ids: config.operation_ids.into(),
            timestamps: config.timestamps.into(),
        })
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
        self.runtime.system.open_blob(hash)
    }

    pub fn scan_and_record(&mut self) -> Result<(), SyncError> {
        let changes = self.scan_changes()?;
        self.record_local_changes(changes)
    }

    pub fn scan_changes(&mut self) -> Result<Vec<FsChange>, SyncError> {
        let scan = self
            .runtime
            .system
            .workspace
            .scan(&self.runtime.system.rules)?;
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
                paired_additions.insert(add_index);
                paired_deletions.insert(delete_index);
                renames.push(FsChange::Rename {
                    from: old_path.clone(),
                    to: additions[add_index].0.clone(),
                });
            }
        }

        let mut changes = additions
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !paired_additions.contains(index))
            .map(|(_, (path, _))| FsChange::Create(path))
            .collect::<Vec<_>>();
        changes.extend(updates.into_iter().map(|(path, _)| FsChange::Update(path)));
        changes.extend(
            deletions
                .into_iter()
                .enumerate()
                .filter(|(index, _)| !paired_deletions.contains(index))
                .map(|(_, path)| FsChange::Delete(path)),
        );
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
        if limit == 0 {
            return Ok(Vec::new());
        }
        let records = self.runtime.state.pending_outbox_replay(limit)?;
        let mut commands = Vec::new();
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
                let mutation = record.mutation().map_err(|_| SyncError::CorruptState {
                    table: "outbox",
                    field: "body_json",
                })?;
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
        accepted
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_mutation_accepted",
            })?;
        self.validate_identity(accepted.workspace_id, accepted.client_id)?;
        let record = self.runtime.state.outbox_entry(accepted.operation_id)?;
        let Some(record) = record else {
            if self
                .runtime
                .state
                .applied_operation(self.runtime.state.client_id(), accepted.operation_id)?
                .is_some()
            {
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
        let next = intents
            .into_iter()
            .find(|desired| !desired_matches_remote(desired, &states));
        let next_mutation = match next.as_ref() {
            Some(desired) => Some(mutation_for_desired(
                desired,
                self.runtime.state.workspace_id(),
                self.runtime.state.client_id(),
                self.next_operation_id()?,
                &states,
            )),
            None => None,
        };
        let timestamp = next_mutation.as_ref().map(|_| self.next_timestamp());
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
            for path in &touched {
                tx.remove_local_intent(path)?;
            }
            if let (Some(mutation), Some(timestamp)) = (&next_mutation, timestamp) {
                tx.enqueue_mutation_at(mutation, timestamp)?;
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
            if self
                .runtime
                .state
                .applied_operation(self.runtime.state.client_id(), event.operation_id)?
                .is_some()
            {
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

    pub fn close(self) -> Result<(), SyncError> {
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
            let mutation_paths = mutation_paths(&mutation);
            let intent_paths = touched.to_vec();
            return self.runtime.state.transaction(|tx| {
                for path in &mutation_paths {
                    tx.remove_local_intent(path)?;
                }
                for path in &intent_paths {
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
        Ok(operations)
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
        let desired = intents
            .into_iter()
            .next()
            .unwrap_or_else(|| desired_from_mutation(&mutation, None));
        let next = if !desired_matches_remote(&desired, &states) {
            Some(mutation_for_desired(
                &desired,
                self.runtime.state.workspace_id(),
                self.runtime.state.client_id(),
                self.next_operation_id()?,
                &states,
            ))
        } else {
            None
        };
        let timestamp = next.as_ref().map(|_| self.next_timestamp());
        let paths = touched.clone();
        self.runtime.state.transaction(|tx| {
            match &current {
                RequiredNullable::Null => tx.remove_path_state(&mutation.path)?,
                RequiredNullable::Value(state) => tx.put_path_state(state)?,
            }
            tx.remove_outbox(mutation.operation_id)?;
            for path in &paths {
                tx.remove_local_intent(path)?;
            }
            if let (Some(next), Some(timestamp)) = (&next, timestamp) {
                tx.enqueue_mutation_at(next, timestamp)?;
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
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
