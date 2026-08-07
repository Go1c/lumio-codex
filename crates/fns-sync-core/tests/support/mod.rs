use std::path::PathBuf;

use fns_protocol::{
    RequiredNullable, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspaceId,
    WorkspaceMutation, WorkspaceMutationKind, WorkspacePath, WorkspaceRevision,
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
}
