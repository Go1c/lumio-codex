pub mod agent;
pub mod boundary;
pub mod bug_package;
pub mod cleanup;
pub mod cli;
pub mod effect;
pub mod evidence;
pub mod harness;
pub mod manifest;
pub mod process;
pub mod profile;
pub mod scenario;
pub mod secret;
pub mod selftest;
pub mod snapshot;
pub mod soak;
pub mod stability;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("invalid harness configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("self-test profile rejected: {0}")]
    ProfileRejected(String),
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SQLite inspection failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("agent operation failed: {0}")]
    Agent(#[from] fns_agent::AgentError),
    #[error("process operation failed: {0}")]
    Process(&'static str),
    #[error("process operation failed: {0}")]
    ProcessDetail(String),
    #[error("process cleanup failed: {0}")]
    Cleanup(#[from] process::CleanupFailure),
    #[error("operation timed out: {0}")]
    Timeout(&'static str),
}

pub type Result<T> = std::result::Result<T, HarnessError>;

pub(crate) fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> HarnessError {
    HarnessError::Io {
        path: path.into(),
        source,
    }
}
