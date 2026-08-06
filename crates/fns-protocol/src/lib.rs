pub mod error;
pub mod nullable;
pub mod revision;
pub mod strict_json;
pub mod value;

pub use error::WorkspaceValidationError;
pub use nullable::{RequiredNullable, deserialize_optional_non_null};
pub use revision::WorkspaceRevision;
pub use value::{
    ClientId, ConflictId, OperationId, RequestId, StreamId, TransferId, WorkspaceBlobDirection,
    WorkspaceConflictChoice, WorkspaceConflictKind, WorkspaceContentHash, WorkspaceEntryKind,
    WorkspaceFileMetadata, WorkspaceId, WorkspaceMutationKind, WorkspaceMutationRejectReason,
    WorkspacePath, WorkspaceSnapshotMode,
};

pub const MAX_CONTROL_FRAME_BYTES: usize = 65_536;
pub const MAX_ACTION_BYTES: usize = 64;
pub const BLOB_HEADER_LEN: usize = 64;
pub const BLOB_CHUNK_BYTES: u32 = 1_048_576;
pub const MAX_BLOB_BYTES: u64 = 5_368_709_120;
