use fns_protocol::WorkspacePath;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FsError {
    #[error("invalid synchronization rule at index {index}: {reason}")]
    InvalidRule { index: usize, reason: String },
    #[error("workspace root is not a directory")]
    RootNotDirectory,
    #[error("workspace root must not be a symlink")]
    RootSymlink,
    #[error("invalid workspace path: {reason}")]
    InvalidPath { reason: String },
    #[error("workspace path escapes the canonical root")]
    PathEscape,
    #[error("workspace path collision: {path}")]
    PathCollision { path: WorkspacePath },
    #[error("unsupported symbolic link")]
    UnsupportedSymlink,
    #[error("workspace file changed while being read: {path}")]
    UnstableFile { path: WorkspacePath },
    #[error("content hash does not match the expected value")]
    ContentMismatch,
    #[error("content size does not match the expected value")]
    SizeMismatch,
    #[error("filesystem queue disconnected")]
    QueueDisconnected,
    #[error("filesystem I/O operation failed: {operation}")]
    Io { operation: &'static str },
}
