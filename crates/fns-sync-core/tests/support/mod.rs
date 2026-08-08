#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use fns_protocol::{
    ConflictId, RequiredNullable, WorkspaceAckRequest, WorkspaceConflictChoice,
    WorkspaceConflictCreatedMessage, WorkspaceConflictKind, WorkspaceConflictResolvedMessage,
    WorkspaceConflictSide, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage,
    WorkspaceFileMetadata, WorkspaceId, WorkspaceMutation, WorkspaceMutationKind, WorkspacePath,
    WorkspaceRevision, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage,
    WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode,
};
use tempfile::TempDir;

use fns_sync_core::{SqliteState, SyncCommand, SyncEngine, SyncEngineConfig};

pub struct StateFixture {
    _dir: TempDir,
    workspace_id: WorkspaceId,
    client_id: fns_protocol::ClientId,
}

impl StateFixture {
    pub fn new() -> Self {
        Self {
            _dir: tempfile::tempdir().expect("fixture directory"),
            workspace_id: WorkspaceId::parse("10000000-0000-4000-8000-000000000001")
                .expect("workspace id"),
            client_id: fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002")
                .expect("client id"),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self._dir.path().join("state.sqlite")
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    pub fn client_id(&self) -> fns_protocol::ClientId {
        self.client_id
    }

    pub fn open(&self) -> SqliteState {
        SqliteState::open(self.db_path(), self.workspace_id, self.client_id).expect("state")
    }

    pub fn mutation(&self, path: &str) -> WorkspaceMutation {
        let workspace_path = WorkspacePath::parse(path).expect("mutation path");
        WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: self.client_id,
            operation_id: fns_protocol::OperationId::parse("10000000-0000-4000-8000-000000000003")
                .expect("operation id"),
            path: workspace_path,
            base_path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceMutationKind::UpsertFile,
            content_hash: RequiredNullable::Value(
                WorkspaceContentHash::parse(
                    "blake3:0000000000000000000000000000000000000000000000000000000000000000",
                )
                .expect("content hash"),
            ),
            metadata: WorkspaceFileMetadata {
                size: 0,
                modified_at_ms: 0,
                executable: false,
            },
            new_path: None,
            target_base_path_revision: None,
        }
    }

    pub fn path_state(&self, path: &str, revision: u64) -> fns_protocol::WorkspacePathState {
        fns_protocol::WorkspacePathState {
            path: WorkspacePath::parse(path).expect("path state path"),
            path_revision: WorkspaceRevision::new(revision),
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(
                WorkspaceContentHash::parse(
                    "blake3:0000000000000000000000000000000000000000000000000000000000000000",
                )
                .expect("content hash"),
            ),
            metadata: WorkspaceFileMetadata {
                size: 0,
                modified_at_ms: 0,
                executable: false,
            },
            tombstone: false,
        }
    }

    pub fn event(
        &self,
        stream_id: fns_protocol::StreamId,
        index: u32,
        revision: u64,
        path: &str,
    ) -> WorkspaceEventMessage {
        let mutation = self.mutation(path);
        WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id: mutation.operation_id,
            origin_client_id: self.client_id,
            mutation,
            path_state: self.path_state(path, revision),
            old_path_state: None,
            new_path_state: None,
        }
    }

    pub fn snapshot_entry(
        &self,
        stream_id: fns_protocol::StreamId,
        index: u32,
        path: &str,
        revision: u64,
    ) -> WorkspaceSnapshotEntryMessage {
        WorkspaceSnapshotEntryMessage {
            workspace_id: self.workspace_id,
            stream_id,
            index,
            entry: self.path_state(path, revision),
        }
    }

    pub fn conflict_created(&self, path: &str) -> WorkspaceConflictCreatedMessage {
        self.conflict_created_with_id(path, "10000000-0000-4000-8000-000000000030")
    }

    pub fn conflict_created_with_id(
        &self,
        path: &str,
        conflict_id: &str,
    ) -> WorkspaceConflictCreatedMessage {
        let path = WorkspacePath::parse(path).expect("conflict path");
        let hash = WorkspaceContentHash::parse(
            "blake3:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("conflict hash");
        let metadata = WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false,
        };
        let side = WorkspaceConflictSide {
            path: RequiredNullable::Value(path.clone()),
            path_revision: WorkspaceRevision::new(1),
            content_hash: RequiredNullable::Value(hash),
            metadata,
            tombstone: false,
        };
        WorkspaceConflictCreatedMessage {
            workspace_id: self.workspace_id,
            conflict_id: ConflictId::parse(conflict_id).expect("conflict id"),
            conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1")
                .expect("conflict revision"),
            path: path.clone(),
            kind: WorkspaceConflictKind::Content,
            ancestor: side.clone(),
            current: side.clone(),
            incoming: side,
            created_by_operation_id: self.mutation(path.as_str()).operation_id,
        }
    }

    pub fn conflict_resolved(&self, revision: u64, path: &str) -> WorkspaceConflictResolvedMessage {
        WorkspaceConflictResolvedMessage {
            workspace_id: self.workspace_id,
            conflict_id: ConflictId::parse("10000000-0000-4000-8000-000000000030")
                .expect("conflict id"),
            conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1")
                .expect("conflict revision"),
            operation_id: self.mutation(path).operation_id,
            revision: WorkspaceRevision::new(revision),
            choice: WorkspaceConflictChoice::Current,
            path_state: self.path_state(path, revision),
            resolved_by_client_id: self.client_id,
        }
    }
}

pub struct EngineFixture {
    pub engine: SyncEngine,
    pub workspace: TempDir,
    pub state: TempDir,
    workspace_id: WorkspaceId,
    client_id: fns_protocol::ClientId,
    stream_id: fns_protocol::StreamId,
    blobs: RefCell<BTreeMap<WorkspaceContentHash, Vec<u8>>>,
    retained_commands: Vec<SyncCommand>,
}

impl EngineFixture {
    pub fn new() -> Self {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let state = tempfile::tempdir().expect("state directory");
        let workspace_id =
            WorkspaceId::parse("10000000-0000-4000-8000-000000000001").expect("workspace id");
        let client_id = fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002")
            .expect("client id");
        let stream_id = fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000050")
            .expect("stream id");
        let operation_ids = (101_u32..=140)
            .map(|number| {
                fns_protocol::OperationId::parse(&format!("10000000-0000-4000-8000-{number:012}"))
                    .expect("operation id")
            })
            .collect::<Vec<_>>();
        let timestamps = (0_i64..40)
            .map(|offset| 1_800_000_000_000_i64 + offset)
            .collect::<Vec<_>>();
        let config = SyncEngineConfig::new(workspace_id, client_id, workspace.path(), state.path())
            .with_operation_ids(operation_ids)
            .with_timestamps(timestamps);
        let engine = SyncEngine::open(config).expect("engine");
        Self {
            engine,
            workspace,
            state,
            workspace_id,
            client_id,
            stream_id,
            blobs: RefCell::new(BTreeMap::new()),
            retained_commands: Vec::new(),
        }
    }

    pub fn path(&self, path: &str) -> PathBuf {
        self.workspace.path().join(path)
    }

    pub fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.path(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("workspace parent");
        }
        fs::write(path, bytes).expect("workspace write");
    }

    pub fn remove(&self, path: &str) {
        let path = self.path(path);
        if path.is_dir() {
            fs::remove_dir_all(path).expect("workspace remove directory");
        } else {
            fs::remove_file(path).expect("workspace remove file");
        }
    }

    pub fn rename(&self, from: &str, to: &str) {
        let destination = self.path(to);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("workspace rename parent");
        }
        fs::rename(self.path(from), destination).expect("workspace rename");
    }

    pub fn seed_remote_file(&mut self, path: &str, revision: u64, bytes: &[u8]) {
        self.write(path, bytes);
        let content_hash = fns_protocol::WorkspaceContentHash::parse(&format!(
            "blake3:{}",
            blake3::hash(bytes).to_hex()
        ))
        .expect("content hash");
        self.engine
            .stage_bytes(&content_hash, bytes)
            .expect("stage remote bytes");
        let modified_at_ms = fs::metadata(self.path(path))
            .expect("seed metadata")
            .modified()
            .ok()
            .and_then(|modified| {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis() as i64)
            })
            .unwrap_or(0);
        self.engine
            .state_mut()
            .put_path_state(&fns_protocol::WorkspacePathState {
                path: WorkspacePath::parse(path).expect("path state path"),
                path_revision: WorkspaceRevision::new(revision),
                kind: WorkspaceEntryKind::File,
                content_hash: RequiredNullable::Value(content_hash),
                metadata: WorkspaceFileMetadata {
                    size: bytes.len() as u64,
                    modified_at_ms,
                    executable: false,
                },
                tombstone: false,
            })
            .expect("remote path state");
    }

    pub fn seed_remote_directory(&mut self, path: &str, revision: u64) {
        fs::create_dir_all(self.path(path)).expect("seed directory");
        let modified_at_ms = fs::metadata(self.path(path))
            .expect("seed directory metadata")
            .modified()
            .ok()
            .and_then(|modified| {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis() as i64)
            })
            .unwrap_or(0);
        self.engine
            .state_mut()
            .put_path_state(&fns_protocol::WorkspacePathState {
                path: WorkspacePath::parse(path).expect("path state path"),
                path_revision: WorkspaceRevision::new(revision),
                kind: WorkspaceEntryKind::Directory,
                content_hash: RequiredNullable::Null,
                metadata: WorkspaceFileMetadata {
                    size: 0,
                    modified_at_ms,
                    executable: false,
                },
                tombstone: false,
            })
            .expect("remote directory state");
    }

    pub fn record_all_changes(&mut self) -> Vec<SyncCommand> {
        self.engine.scan_and_record().expect("scan and record");
        self.engine.pending_commands(16).expect("dispatch commands")
    }

    pub fn record_all_changes_and_close(&mut self) -> Vec<SyncCommand> {
        let commands = self.record_all_changes();
        self.engine.close().expect("close engine");
        commands
    }

    pub fn incremental_begin(
        &self,
        from: u64,
        final_revision: u64,
        event_count: u32,
        conflict_count: u32,
    ) -> WorkspaceSnapshotBeginMessage {
        let message = WorkspaceSnapshotBeginMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::new(from),
            final_revision: WorkspaceRevision::new(final_revision),
            entry_count: 0,
            event_count,
            conflict_count,
        };
        message.validate().expect("valid incremental begin");
        message
    }

    pub fn snapshot_begin(
        &self,
        final_revision: u64,
        entry_count: u32,
        conflict_count: u32,
    ) -> WorkspaceSnapshotBeginMessage {
        let message = WorkspaceSnapshotBeginMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            from_revision: WorkspaceRevision::ZERO,
            final_revision: WorkspaceRevision::new(final_revision),
            entry_count,
            event_count: 0,
            conflict_count,
        };
        message.validate().expect("valid snapshot begin");
        message
    }

    pub fn incremental_end(
        &self,
        final_revision: u64,
        event_count: u32,
        conflict_count: u32,
    ) -> WorkspaceSnapshotEndMessage {
        WorkspaceSnapshotEndMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            mode: WorkspaceSnapshotMode::Incremental,
            delivered_count: event_count.checked_add(conflict_count).expect("end count"),
            final_revision: WorkspaceRevision::new(final_revision),
        }
    }

    pub fn snapshot_end(
        &self,
        final_revision: u64,
        entry_count: u32,
        conflict_count: u32,
    ) -> WorkspaceSnapshotEndMessage {
        WorkspaceSnapshotEndMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            mode: WorkspaceSnapshotMode::Snapshot,
            delivered_count: entry_count.checked_add(conflict_count).expect("end count"),
            final_revision: WorkspaceRevision::new(final_revision),
        }
    }

    pub fn ack(&self, revision: u64) -> WorkspaceAckRequest {
        let message = WorkspaceAckRequest {
            workspace_id: self.workspace_id,
            client_id: self.client_id,
            revision: WorkspaceRevision::new(revision),
        };
        message.validate().expect("valid ack");
        message
    }

    pub fn stream_id(&self) -> fns_protocol::StreamId {
        self.stream_id
    }

    pub fn snapshot_file_entry(
        &self,
        index: u32,
        revision: u64,
        path: &str,
        bytes: &[u8],
    ) -> WorkspaceSnapshotEntryMessage {
        let content_hash = hash(bytes);
        self.blobs
            .borrow_mut()
            .insert(content_hash.clone(), bytes.to_vec());
        let entry = WorkspaceSnapshotEntryMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            entry: path_state(
                path,
                revision,
                RequiredNullable::Value(content_hash),
                file_metadata(bytes.len() as u64),
                WorkspaceEntryKind::File,
            ),
        };
        entry.validate().expect("valid snapshot file entry");
        entry
    }

    pub fn remote_update_event(
        &self,
        index: u32,
        revision: u64,
        path: &str,
        bytes: &[u8],
    ) -> WorkspaceEventMessage {
        let operation_id = operation_id(200 + index);
        let path = workspace_path(path);
        let content_hash = hash(bytes);
        self.blobs
            .borrow_mut()
            .insert(content_hash.clone(), bytes.to_vec());
        let metadata = file_metadata(bytes.len() as u64);
        let mutation = WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: remote_client_id(),
            operation_id,
            path: path.clone(),
            base_path_revision: WorkspaceRevision::new(revision.saturating_sub(1)),
            kind: WorkspaceMutationKind::UpsertFile,
            content_hash: RequiredNullable::Value(content_hash.clone()),
            metadata: metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        };
        let message = WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id,
            origin_client_id: remote_client_id(),
            mutation,
            path_state: path_state(
                path.as_str(),
                revision,
                RequiredNullable::Value(content_hash),
                metadata,
                WorkspaceEntryKind::File,
            ),
            old_path_state: None,
            new_path_state: None,
        };
        message.validate().expect("valid remote update event");
        message
    }

    pub fn remote_delete_event(
        &self,
        index: u32,
        revision: u64,
        path: &str,
    ) -> WorkspaceEventMessage {
        let operation_id = operation_id(200 + index);
        let path = workspace_path(path);
        let metadata = zero_metadata();
        let mutation = WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: remote_client_id(),
            operation_id,
            path: path.clone(),
            base_path_revision: WorkspaceRevision::new(revision.saturating_sub(1)),
            kind: WorkspaceMutationKind::Delete,
            content_hash: RequiredNullable::Null,
            metadata: metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        };
        let message = WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id,
            origin_client_id: remote_client_id(),
            mutation,
            path_state: path_state(
                path.as_str(),
                revision,
                RequiredNullable::Null,
                metadata,
                WorkspaceEntryKind::Tombstone,
            ),
            old_path_state: None,
            new_path_state: None,
        };
        message.validate().expect("valid remote delete event");
        message
    }

    pub fn remote_conflict_resolved(
        &self,
        revision: u64,
        path: &str,
    ) -> WorkspaceConflictResolvedMessage {
        let message = WorkspaceConflictResolvedMessage {
            workspace_id: self.workspace_id,
            conflict_id: conflict_id("10000000-0000-4000-8000-000000000030"),
            conflict_revision: conflict_revision("1"),
            operation_id: operation_id(230),
            revision: WorkspaceRevision::new(revision),
            choice: WorkspaceConflictChoice::Current,
            path_state: path_state(
                path,
                revision,
                RequiredNullable::Value(hash(b"current")),
                file_metadata(7),
                WorkspaceEntryKind::File,
            ),
            resolved_by_client_id: remote_client_id(),
        };
        message
            .validate()
            .expect("valid remote conflict resolution");
        message
    }

    pub fn remote_conflict_created(
        &self,
        conflict_id_value: &str,
        conflict_revision_value: &str,
        path: &str,
    ) -> WorkspaceConflictCreatedMessage {
        let path = workspace_path(path);
        let current_hash = hash(b"current");
        let incoming_hash = hash(b"incoming");
        let message = WorkspaceConflictCreatedMessage {
            workspace_id: self.workspace_id,
            conflict_id: conflict_id(conflict_id_value),
            conflict_revision: conflict_revision(conflict_revision_value),
            path: path.clone(),
            kind: WorkspaceConflictKind::Content,
            ancestor: conflict_side(path.clone(), hash(b"ancestor"), 8),
            current: conflict_side(path.clone(), current_hash, 7),
            incoming: conflict_side(path, incoming_hash, 8),
            created_by_operation_id: operation_id(240),
        };
        message.validate().expect("valid remote conflict");
        message
    }

    pub fn remote_rename_event(
        &self,
        index: u32,
        revision: u64,
        from: &str,
        to: &str,
    ) -> WorkspaceEventMessage {
        let from = workspace_path(from);
        let to = workspace_path(to);
        let operation_id = operation_id(200 + index);
        let old_state = path_state(
            from.as_str(),
            revision,
            RequiredNullable::Null,
            zero_metadata(),
            WorkspaceEntryKind::Tombstone,
        );
        let new_state = path_state(
            to.as_str(),
            revision,
            RequiredNullable::Null,
            zero_metadata(),
            WorkspaceEntryKind::Directory,
        );
        let mutation = WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: remote_client_id(),
            operation_id,
            path: from,
            base_path_revision: WorkspaceRevision::new(revision.saturating_sub(1)),
            kind: WorkspaceMutationKind::Rename,
            content_hash: RequiredNullable::Null,
            metadata: zero_metadata(),
            new_path: Some(to),
            target_base_path_revision: Some(WorkspaceRevision::ZERO),
        };
        let message = WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id,
            origin_client_id: remote_client_id(),
            mutation,
            path_state: new_state.clone(),
            old_path_state: Some(old_state),
            new_path_state: Some(new_state),
        };
        message.validate().expect("valid remote rename event");
        message
    }

    pub fn remote_mkdir_event(
        &self,
        index: u32,
        revision: u64,
        path: &str,
    ) -> WorkspaceEventMessage {
        let operation_id = operation_id(200 + index);
        let path = workspace_path(path);
        let metadata = zero_metadata();
        let mutation = WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: remote_client_id(),
            operation_id,
            path: path.clone(),
            base_path_revision: WorkspaceRevision::new(revision.saturating_sub(1)),
            kind: WorkspaceMutationKind::Mkdir,
            content_hash: RequiredNullable::Null,
            metadata: metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        };
        let message = WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id,
            origin_client_id: remote_client_id(),
            mutation,
            path_state: path_state(
                path.as_str(),
                revision,
                RequiredNullable::Null,
                metadata,
                WorkspaceEntryKind::Directory,
            ),
            old_path_state: None,
            new_path_state: None,
        };
        message.validate().expect("valid remote mkdir event");
        message
    }

    pub fn remote_symlink_event(
        &self,
        index: u32,
        revision: u64,
        path: &str,
        target: &[u8],
    ) -> WorkspaceEventMessage {
        let operation_id = operation_id(200 + index);
        let path = workspace_path(path);
        let content_hash = hash(target);
        self.blobs
            .borrow_mut()
            .insert(content_hash.clone(), target.to_vec());
        let metadata = file_metadata(target.len() as u64);
        let mutation = WorkspaceMutation {
            workspace_id: self.workspace_id,
            client_id: remote_client_id(),
            operation_id,
            path: path.clone(),
            base_path_revision: WorkspaceRevision::new(revision.saturating_sub(1)),
            kind: WorkspaceMutationKind::UpsertSymlink,
            content_hash: RequiredNullable::Value(content_hash.clone()),
            metadata: metadata.clone(),
            new_path: None,
            target_base_path_revision: None,
        };
        let message = WorkspaceEventMessage {
            workspace_id: self.workspace_id,
            stream_id: self.stream_id,
            index,
            revision: WorkspaceRevision::new(revision),
            operation_id,
            origin_client_id: remote_client_id(),
            mutation,
            path_state: path_state(
                path.as_str(),
                revision,
                RequiredNullable::Value(content_hash),
                metadata,
                WorkspaceEntryKind::Symlink,
            ),
            old_path_state: None,
            new_path_state: None,
        };
        message.validate().expect("valid remote symlink event");
        message
    }

    pub fn provide_requested_blobs(&mut self) {
        loop {
            let commands = self.engine.pending_commands(64).expect("pending commands");
            let mut downloaded = false;
            for command in commands {
                match command {
                    SyncCommand::DownloadBlob {
                        content_hash, size, ..
                    } => {
                        downloaded = true;
                        let bytes = self
                            .blobs
                            .borrow()
                            .get(&content_hash)
                            .expect("requested blob")
                            .clone();
                        assert_eq!(bytes.len() as u64, size);
                        self.engine
                            .blob_available(content_hash, size, Cursor::new(bytes))
                            .expect("blob available");
                    }
                    other => self.retained_commands.push(other),
                }
            }
            if !downloaded {
                break;
            }
        }
    }

    pub fn retained_commands(&self) -> &[SyncCommand] {
        &self.retained_commands
    }

    pub fn reopen(self) -> Self {
        let Self {
            engine,
            workspace,
            state,
            workspace_id,
            client_id,
            stream_id,
            blobs,
            retained_commands,
        } = self;
        drop(engine);
        let operation_ids = (101_u32..=140)
            .map(|number| {
                fns_protocol::OperationId::parse(&format!("10000000-0000-4000-8000-{number:012}"))
                    .expect("operation id")
            })
            .collect::<Vec<_>>();
        let timestamps = (0_i64..40)
            .map(|offset| 1_800_000_000_000_i64 + offset)
            .collect::<Vec<_>>();
        let config = SyncEngineConfig::new(workspace_id, client_id, workspace.path(), state.path());
        let config = config
            .with_operation_ids(operation_ids)
            .with_timestamps(timestamps);
        let engine = SyncEngine::open(config).expect("reopen engine");
        Self {
            engine,
            workspace,
            state,
            workspace_id,
            client_id,
            stream_id,
            blobs,
            retained_commands,
        }
    }
}

pub fn mutation_kinds(commands: &[SyncCommand]) -> Vec<WorkspaceMutationKind> {
    commands
        .iter()
        .map(|command| match command {
            SyncCommand::Mutation(mutation) => mutation.kind,
            SyncCommand::UploadBlob { .. }
            | SyncCommand::DownloadBlob { .. }
            | SyncCommand::SendAck(_) => panic!("expected mutation command"),
        })
        .collect()
}

pub fn base_revision(command: &SyncCommand) -> u64 {
    match command {
        SyncCommand::Mutation(mutation) => mutation.base_path_revision.get(),
        SyncCommand::UploadBlob { .. }
        | SyncCommand::DownloadBlob { .. }
        | SyncCommand::SendAck(_) => panic!("expected mutation command"),
    }
}

pub fn rename_revisions(command: &SyncCommand) -> (u64, u64) {
    match command {
        SyncCommand::Mutation(mutation) if mutation.kind == WorkspaceMutationKind::Rename => (
            mutation.base_path_revision.get(),
            mutation
                .target_base_path_revision
                .expect("rename target revision")
                .get(),
        ),
        SyncCommand::Mutation(_) => panic!("expected rename command"),
        SyncCommand::UploadBlob { .. }
        | SyncCommand::DownloadBlob { .. }
        | SyncCommand::SendAck(_) => panic!("expected mutation command"),
    }
}

pub fn path_state(
    path: &str,
    revision: u64,
    content_hash: RequiredNullable<WorkspaceContentHash>,
    metadata: WorkspaceFileMetadata,
    kind: WorkspaceEntryKind,
) -> fns_protocol::WorkspacePathState {
    fns_protocol::WorkspacePathState {
        path: WorkspacePath::parse(path).expect("path state path"),
        path_revision: WorkspaceRevision::new(revision),
        kind,
        content_hash,
        metadata,
        tombstone: kind == WorkspaceEntryKind::Tombstone,
    }
}

pub fn file_metadata(size: u64) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size,
        modified_at_ms: 0,
        executable: false,
    }
}

pub fn workspace_path(path: &str) -> WorkspacePath {
    WorkspacePath::parse(path).expect("workspace path")
}

pub fn is_download(command: &SyncCommand) -> bool {
    matches!(command, SyncCommand::DownloadBlob { .. })
}

pub fn ack_revisions(commands: &[SyncCommand]) -> Vec<u64> {
    commands
        .iter()
        .filter_map(|command| match command {
            SyncCommand::SendAck(message) => Some(message.revision.get()),
            _ => None,
        })
        .collect()
}

pub fn hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex()))
        .expect("content hash")
}

pub fn operation_id(value: u32) -> fns_protocol::OperationId {
    fns_protocol::OperationId::parse(&format!("10000000-0000-4000-8000-{value:012}"))
        .expect("operation id")
}

pub fn conflict_id(value: &str) -> ConflictId {
    ConflictId::parse(value).expect("conflict id")
}

pub fn conflict_revision(value: &str) -> fns_protocol::revision::WorkspaceConflictRevision {
    fns_protocol::revision::WorkspaceConflictRevision::parse(value).expect("conflict revision")
}

pub fn remote_client_id() -> fns_protocol::ClientId {
    fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000004").expect("remote client id")
}

pub fn zero_metadata() -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 0,
        executable: false,
    }
}

pub fn state_kind_for_entry(kind: WorkspaceEntryKind) -> WorkspaceEntryKind {
    kind
}

fn conflict_side(
    path: WorkspacePath,
    content_hash: WorkspaceContentHash,
    size: u64,
) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Value(path),
        path_revision: WorkspaceRevision::new(1),
        content_hash: RequiredNullable::Value(content_hash),
        metadata: file_metadata(size),
        tombstone: false,
    }
}

pub fn self_event_from_mutation(
    fixture: &EngineFixture,
    index: u32,
    revision: u64,
    mutation: WorkspaceMutation,
) -> WorkspaceEventMessage {
    let (path_state, old_path_state, new_path_state) = match mutation.kind {
        WorkspaceMutationKind::UpsertFile => (
            path_state(
                mutation.path.as_str(),
                revision,
                mutation.content_hash.clone(),
                mutation.metadata.clone(),
                WorkspaceEntryKind::File,
            ),
            None,
            None,
        ),
        WorkspaceMutationKind::UpsertSymlink => (
            path_state(
                mutation.path.as_str(),
                revision,
                mutation.content_hash.clone(),
                mutation.metadata.clone(),
                WorkspaceEntryKind::Symlink,
            ),
            None,
            None,
        ),
        WorkspaceMutationKind::Mkdir => (
            path_state(
                mutation.path.as_str(),
                revision,
                RequiredNullable::Null,
                zero_metadata(),
                WorkspaceEntryKind::Directory,
            ),
            None,
            None,
        ),
        WorkspaceMutationKind::Delete => (
            path_state(
                mutation.path.as_str(),
                revision,
                RequiredNullable::Null,
                zero_metadata(),
                WorkspaceEntryKind::Tombstone,
            ),
            None,
            None,
        ),
        WorkspaceMutationKind::Rename => panic!("self rename event is not used by this task"),
    };
    let event = WorkspaceEventMessage {
        workspace_id: fixture.workspace_id,
        stream_id: fixture.stream_id,
        index,
        revision: WorkspaceRevision::new(revision),
        operation_id: mutation.operation_id,
        origin_client_id: fixture.client_id,
        mutation,
        path_state,
        old_path_state,
        new_path_state,
    };
    event.validate().expect("valid self event");
    event
}
