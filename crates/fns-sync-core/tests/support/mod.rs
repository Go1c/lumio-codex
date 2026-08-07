#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use fns_protocol::{
    ConflictId, RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictResolvedMessage, WorkspaceConflictSide,
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceFileMetadata,
    WorkspaceId, WorkspaceMutation, WorkspaceMutationKind, WorkspacePath, WorkspaceRevision,
    WorkspaceSnapshotEntryMessage,
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
}

impl EngineFixture {
    pub fn new() -> Self {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let state = tempfile::tempdir().expect("state directory");
        let workspace_id =
            WorkspaceId::parse("10000000-0000-4000-8000-000000000001").expect("workspace id");
        let client_id = fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002")
            .expect("client id");
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

    pub fn record_all_changes(&mut self) -> Vec<SyncCommand> {
        self.engine.scan_and_record().expect("scan and record");
        self.engine.pending_commands(16).expect("dispatch commands")
    }

    pub fn record_all_changes_and_close(&mut self) -> Vec<SyncCommand> {
        let commands = self.record_all_changes();
        self.engine.close().expect("close engine");
        commands
    }

    pub fn reopen(self) -> Self {
        let Self {
            engine,
            workspace,
            state,
            workspace_id,
            client_id,
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
        }
    }
}

pub fn mutation_kinds(commands: &[SyncCommand]) -> Vec<WorkspaceMutationKind> {
    commands
        .iter()
        .map(|command| match command {
            SyncCommand::Mutation(mutation) => mutation.kind,
            SyncCommand::UploadBlob { .. } => panic!("expected mutation command"),
        })
        .collect()
}

pub fn base_revision(command: &SyncCommand) -> u64 {
    match command {
        SyncCommand::Mutation(mutation) => mutation.base_path_revision.get(),
        SyncCommand::UploadBlob { .. } => panic!("expected mutation command"),
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
        SyncCommand::UploadBlob { .. } => panic!("expected mutation command"),
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
