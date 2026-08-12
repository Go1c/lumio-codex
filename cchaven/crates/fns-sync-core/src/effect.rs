use fns_protocol::{
    OperationId, WorkspaceAckRequest, WorkspaceConflictResolvedRequest, WorkspaceContentHash,
    WorkspaceId, WorkspaceMutation,
};

use crate::{SyncError, canonical_json};

/// Effect emitted by the sync engine for the transport/system layer.
///
/// Mutation bodies are copied out of the durable outbox and therefore remain
/// immutable for the lifetime of a dispatch.  UploadBlob refers to the
/// immutable content-cache object named by the rejected mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncCommand {
    Mutation(WorkspaceMutation),
    ResolveConflict(WorkspaceConflictResolvedRequest),
    UploadBlob {
        workspace_id: WorkspaceId,
        operation_id: OperationId,
        content_hash: WorkspaceContentHash,
        size: u64,
    },
    DownloadBlob {
        workspace_id: WorkspaceId,
        operation_id: Option<OperationId>,
        content_hash: WorkspaceContentHash,
        size: u64,
    },
    SendAck(WorkspaceAckRequest),
}

impl SyncCommand {
    pub fn operation_id(&self) -> Option<OperationId> {
        match self {
            Self::Mutation(mutation) => Some(mutation.operation_id),
            Self::ResolveConflict(resolution) => Some(resolution.operation_id),
            Self::UploadBlob { operation_id, .. } => Some(*operation_id),
            Self::DownloadBlob { operation_id, .. } => *operation_id,
            Self::SendAck(_) => None,
        }
    }

    pub fn mutation(&self) -> Result<WorkspaceMutation, SyncError> {
        match self {
            Self::Mutation(mutation) => Ok(mutation.clone()),
            Self::ResolveConflict(_)
            | Self::UploadBlob { .. }
            | Self::DownloadBlob { .. }
            | Self::SendAck(_) => Err(SyncError::ProtocolInvariant {
                reason: "command_not_mutation",
            }),
        }
    }

    pub fn body_bytes(&self) -> Result<Vec<u8>, SyncError> {
        match self {
            Self::Mutation(mutation) => canonical_json(mutation),
            Self::ResolveConflict(resolution) => canonical_json(resolution),
            Self::SendAck(message) => canonical_json(message),
            Self::UploadBlob { .. } | Self::DownloadBlob { .. } => {
                Err(SyncError::ProtocolInvariant {
                    reason: "command_not_mutation",
                })
            }
        }
    }

    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Mutation(_))
    }
}
