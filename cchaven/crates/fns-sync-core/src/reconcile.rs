use std::collections::BTreeMap;

use fns_protocol::{
    RequiredNullable, WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspaceId,
    WorkspaceMutation, WorkspaceMutationKind, WorkspacePath, WorkspaceRevision,
};

use crate::model::{LocalDesiredEntry, LocalIntent};
use crate::{SyncError, canonical_json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DesiredOperation {
    Upsert {
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

impl DesiredOperation {
    pub(crate) fn paths(&self) -> Vec<&WorkspacePath> {
        match self {
            Self::Upsert { entry } => vec![&entry.path],
            Self::Delete { path } => vec![path],
            Self::Rename { from, to, .. } => vec![from, to],
        }
    }

    pub(crate) fn intent_for_path(&self, path: &WorkspacePath) -> LocalIntent {
        match self {
            Self::Upsert { entry } => LocalIntent::Desired {
                entry: entry.clone(),
            },
            Self::Delete { path: deleted } => LocalIntent::Delete {
                path: deleted.clone(),
            },
            Self::Rename {
                from,
                to,
                kind,
                content_hash,
                metadata,
            } => {
                debug_assert!(path == from || path == to);
                LocalIntent::Rename {
                    from: from.clone(),
                    to: to.clone(),
                    kind: *kind,
                    content_hash: content_hash.clone(),
                    metadata: metadata.clone(),
                }
            }
        }
    }
}

pub(crate) fn desired_from_intent(intent: &LocalIntent) -> DesiredOperation {
    match intent {
        LocalIntent::Desired { entry } => DesiredOperation::Upsert {
            entry: entry.clone(),
        },
        LocalIntent::Delete { path } => DesiredOperation::Delete { path: path.clone() },
        LocalIntent::Rename {
            from,
            to,
            kind,
            content_hash,
            metadata,
        } => DesiredOperation::Rename {
            from: from.clone(),
            to: to.clone(),
            kind: *kind,
            content_hash: content_hash.clone(),
            metadata: metadata.clone(),
        },
    }
}

pub(crate) fn encode_intent(intent: &LocalIntent) -> Result<Vec<u8>, SyncError> {
    canonical_json(intent)
}

pub(crate) fn decode_intent(body: &[u8]) -> Result<LocalIntent, SyncError> {
    serde_json::from_slice(body).map_err(|_| SyncError::CorruptState {
        table: "local_intents",
        field: "intent_json",
    })
}

pub(crate) fn desired_from_mutation(
    mutation: &WorkspaceMutation,
    rename_kind: Option<WorkspaceEntryKind>,
) -> DesiredOperation {
    match mutation.kind {
        WorkspaceMutationKind::UpsertFile => DesiredOperation::Upsert {
            entry: LocalDesiredEntry {
                path: mutation.path.clone(),
                kind: WorkspaceEntryKind::File,
                content_hash: mutation.content_hash.clone(),
                metadata: mutation.metadata.clone(),
            },
        },
        WorkspaceMutationKind::UpsertSymlink => DesiredOperation::Upsert {
            entry: LocalDesiredEntry {
                path: mutation.path.clone(),
                kind: WorkspaceEntryKind::Symlink,
                content_hash: mutation.content_hash.clone(),
                metadata: mutation.metadata.clone(),
            },
        },
        WorkspaceMutationKind::Mkdir => DesiredOperation::Upsert {
            entry: LocalDesiredEntry {
                path: mutation.path.clone(),
                kind: WorkspaceEntryKind::Directory,
                content_hash: RequiredNullable::Null,
                metadata: zero_metadata(),
            },
        },
        WorkspaceMutationKind::Delete => DesiredOperation::Delete {
            path: mutation.path.clone(),
        },
        WorkspaceMutationKind::Rename => {
            let to = mutation
                .new_path
                .clone()
                .expect("validated rename has target path");
            let kind = rename_kind.unwrap_or_else(|| {
                if mutation.content_hash.is_null() {
                    WorkspaceEntryKind::Directory
                } else {
                    WorkspaceEntryKind::File
                }
            });
            let (content_hash, metadata) = if kind == WorkspaceEntryKind::Directory {
                (RequiredNullable::Null, zero_metadata())
            } else {
                (mutation.content_hash.clone(), mutation.metadata.clone())
            };
            DesiredOperation::Rename {
                from: mutation.path.clone(),
                to,
                kind,
                content_hash,
                metadata,
            }
        }
    }
}

pub(crate) fn mutation_for_desired(
    desired: &DesiredOperation,
    workspace_id: WorkspaceId,
    client_id: fns_protocol::ClientId,
    operation_id: fns_protocol::OperationId,
    states: &BTreeMap<WorkspacePath, fns_protocol::WorkspacePathState>,
) -> WorkspaceMutation {
    let (path, base_path_revision, kind, content_hash, metadata, new_path, target_base) =
        match desired {
            DesiredOperation::Upsert { entry } => {
                let kind = match entry.kind {
                    WorkspaceEntryKind::File => WorkspaceMutationKind::UpsertFile,
                    WorkspaceEntryKind::Symlink => WorkspaceMutationKind::UpsertSymlink,
                    WorkspaceEntryKind::Directory => WorkspaceMutationKind::Mkdir,
                    WorkspaceEntryKind::Tombstone => WorkspaceMutationKind::Delete,
                };
                let (content_hash, metadata) = match kind {
                    WorkspaceMutationKind::Mkdir | WorkspaceMutationKind::Delete => {
                        (RequiredNullable::Null, zero_metadata())
                    }
                    _ => (entry.content_hash.clone(), entry.metadata.clone()),
                };
                (
                    entry.path.clone(),
                    if kind == WorkspaceMutationKind::UpsertFile
                        || kind == WorkspaceMutationKind::UpsertSymlink
                    {
                        states
                            .get(&entry.path)
                            .filter(|state| state.kind != WorkspaceEntryKind::Tombstone)
                            .map_or(WorkspaceRevision::ZERO, |state| state.path_revision)
                    } else {
                        states
                            .get(&entry.path)
                            .map_or(WorkspaceRevision::ZERO, |state| state.path_revision)
                    },
                    kind,
                    content_hash,
                    metadata,
                    None,
                    None,
                )
            }
            DesiredOperation::Delete { path } => (
                path.clone(),
                states
                    .get(path)
                    .map_or(WorkspaceRevision::ZERO, |state| state.path_revision),
                WorkspaceMutationKind::Delete,
                RequiredNullable::Null,
                zero_metadata(),
                None,
                None,
            ),
            DesiredOperation::Rename {
                from,
                to,
                kind,
                content_hash,
                metadata,
            } => (
                from.clone(),
                states
                    .get(from)
                    .map_or(WorkspaceRevision::ZERO, |state| state.path_revision),
                WorkspaceMutationKind::Rename,
                if *kind == WorkspaceEntryKind::Directory {
                    RequiredNullable::Null
                } else {
                    content_hash.clone()
                },
                if *kind == WorkspaceEntryKind::Directory {
                    zero_metadata()
                } else {
                    metadata.clone()
                },
                Some(to.clone()),
                Some(
                    states
                        .get(to)
                        .map_or(WorkspaceRevision::ZERO, |state| state.path_revision),
                ),
            ),
        };
    WorkspaceMutation {
        workspace_id,
        client_id,
        operation_id,
        path,
        base_path_revision,
        kind,
        content_hash,
        metadata,
        new_path,
        target_base_path_revision: target_base,
    }
}

pub(crate) fn mutation_matches_desired(
    mutation: &WorkspaceMutation,
    desired: &DesiredOperation,
) -> bool {
    let expected = desired_from_mutation(
        mutation,
        match desired {
            DesiredOperation::Rename { kind, .. } => Some(*kind),
            _ => None,
        },
    );
    expected == *desired
}

pub(crate) fn zero_metadata() -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 0,
        executable: false,
    }
}
