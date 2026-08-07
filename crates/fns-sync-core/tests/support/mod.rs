use std::path::PathBuf;

use fns_protocol::{
    ConflictId, RequiredNullable, WorkspaceConflictChoice, WorkspaceConflictCreatedMessage,
    WorkspaceConflictKind, WorkspaceConflictResolvedMessage, WorkspaceConflictSide,
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceFileMetadata,
    WorkspaceId, WorkspaceMutation, WorkspaceMutationKind, WorkspacePath, WorkspaceRevision,
    WorkspaceSnapshotEntryMessage,
};
use tempfile::TempDir;

use fns_sync_core::SqliteState;

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
