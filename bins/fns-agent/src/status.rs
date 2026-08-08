//! Runtime status: atomic status file read/write and JSON schema.

use crate::error::AgentErrorCode;

use std::path::Path;

/// Agent runtime status written to `state_dir/runtime-status.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStatus {
    pub schema_version: String,
    pub running: bool,
    pub phase: crate::error::AgentPhase,
    pub pid: Option<u32>,
    pub connected: bool,
    pub workspace_id: fns_protocol::WorkspaceId,
    pub last_ack_revision: fns_protocol::WorkspaceRevision,
    pub pending_commands: u64,
    pub queued_watcher_batches: usize,
    pub active_transfers: usize,
    pub reconnect_attempt: u32,
    pub last_error_code: Option<AgentErrorCode>,
    pub updated_at_ms: i64,
}

impl AgentStatus {
    /// Create a stopped status for a workspace.
    pub fn stopped(workspace_id: fns_protocol::WorkspaceId) -> Self {
        Self {
            schema_version: "fns-agent-status/1".into(),
            running: false,
            phase: crate::error::AgentPhase::Stopped,
            pid: None,
            connected: false,
            workspace_id,
            last_ack_revision: fns_protocol::WorkspaceRevision::ZERO,
            pending_commands: 0,
            queued_watcher_batches: 0,
            active_transfers: 0,
            reconnect_attempt: 0,
            last_error_code: None,
            updated_at_ms: 0,
        }
    }

    /// Write status atomically to a file.
    pub fn write_to(&self, path: &Path) -> Result<(), std::io::Error> {
        fns_platform::atomic_write_private_json(path, &self)
            .map_err(|_| std::io::Error::other("write failed"))
    }

    /// Read status from a file. Returns stopped if the file doesn't exist.
    pub fn read_or_stored(path: &Path, workspace_id: fns_protocol::WorkspaceId) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).unwrap_or_else(|_| Self::stopped(workspace_id))
            }
            Err(_) => Self::stopped(workspace_id),
        }
    }
}
