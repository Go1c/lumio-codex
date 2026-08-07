use fns_protocol::{OperationId, WorkspaceContentHash, WorkspaceId, WorkspaceMutation};

use crate::{SyncError, canonical_json};

/// Effect emitted by the sync engine for the transport/system layer.
///
/// Mutation bodies are copied out of the durable outbox and therefore remain
/// immutable for the lifetime of a dispatch.  UploadBlob refers to the
/// immutable content-cache object named by the rejected mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncCommand {
    Mutation(WorkspaceMutation),
    UploadBlob {
        workspace_id: WorkspaceId,
        operation_id: OperationId,
        content_hash: WorkspaceContentHash,
        size: u64,
    },
}

impl SyncCommand {
    pub const fn operation_id(&self) -> OperationId {
        match self {
            Self::Mutation(mutation) => mutation.operation_id,
            Self::UploadBlob { operation_id, .. } => *operation_id,
        }
    }

    pub fn mutation(&self) -> Result<WorkspaceMutation, SyncError> {
        match self {
            Self::Mutation(mutation) => Ok(mutation.clone()),
            Self::UploadBlob { .. } => Err(SyncError::ProtocolInvariant {
                reason: "command_not_mutation",
            }),
        }
    }

    pub fn body_bytes(&self) -> Result<Vec<u8>, SyncError> {
        match self {
            Self::Mutation(mutation) => canonical_json(mutation),
            Self::UploadBlob { .. } => Err(SyncError::ProtocolInvariant {
                reason: "command_not_mutation",
            }),
        }
    }

    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Mutation(_))
    }
}
