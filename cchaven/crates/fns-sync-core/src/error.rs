use thiserror::Error;

/// Errors returned by the durable synchronization state store.
///
/// The variants intentionally contain only stable, non-sensitive labels. In
/// particular, SQLite errors are classified before they cross this boundary
/// so a database filename or SQL statement cannot be exposed to callers.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SyncError {
    #[error("invalid configuration: {reason}")]
    InvalidConfiguration { reason: &'static str },
    #[error("storage unavailable")]
    StorageUnavailable,
    #[error("corrupt state in {table}.{field}")]
    CorruptState {
        table: &'static str,
        field: &'static str,
    },
    #[error("operation body changed")]
    OperationChanged,
    #[error("stream invariant violated: {reason}")]
    StreamInvariant { reason: &'static str },
    #[error("protocol invariant violated: {reason}")]
    ProtocolInvariant { reason: &'static str },
    #[error("conflict unavailable")]
    ConflictUnavailable,
    #[error("conflict revision is stale")]
    ConflictRevisionStale,
    #[error("conflict resolution changed")]
    ConflictResolutionChanged,
    #[error("conflict cannot be resolved: {reason}")]
    ConflictResolutionBlocked {
        reason: crate::model::ConflictBlockedReason,
    },
    #[error("merge rejected: {reason}")]
    MergeRejected { reason: &'static str },
    #[error("resource limit exceeded: {resource}")]
    ResourceLimit { resource: &'static str },
    #[error("filesystem operation failed")]
    Filesystem(#[source] fns_fs::FsError),
    #[error("workspace scan incomplete")]
    ScanIncomplete,
}

impl From<fns_fs::FsError> for SyncError {
    fn from(error: fns_fs::FsError) -> Self {
        Self::Filesystem(error)
    }
}

pub(crate) fn storage_error<T>(_error: T) -> SyncError {
    SyncError::StorageUnavailable
}

pub(crate) fn corrupt(table: &'static str, field: &'static str) -> SyncError {
    SyncError::CorruptState { table, field }
}
