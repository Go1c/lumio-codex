//! fns-agent: single foreground workspace synchronizer binary.
//!
//! Provides `run`, `status --json`, and `diagnose --json` commands.
//! Remains a foreground process: no fork, daemonization, PID killing,
//! service installation, SSH, tmux, Tauri, terminal, or UI code.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod diagnose;
pub mod error;
pub mod obs;
pub mod protocol;
pub mod status;
pub mod supervisor;
pub mod worker;

pub use config::AgentConfig;
pub use error::{AgentError, AgentErrorCode, AgentPhase};
pub use fns_protocol::revision::WorkspaceConflictRevision;
pub use fns_protocol::{ConflictId, WorkspaceConflictChoice};
pub use fns_sync_core::{
    ConflictBlockedReason, ConflictResolutionInput, ConflictResolutionReceipt,
    ConflictResolutionReceiptStatus, ConflictSideView, ConflictView, PendingConflictResolutionView,
};
pub use status::AgentStatus;
pub use supervisor::{AgentCommand, AgentProcess, AgentProcessOptions};

use std::path::Path;

/// Entry point for `status --json` command.
/// Reads runtime status from the config's state directory.
pub fn run_status(config_path: &Path) -> Result<AgentStatus, AgentError> {
    let config = AgentConfig::load_linux(config_path)?;
    let status_path = config.state_dir.join("runtime-status.json");

    let status = AgentStatus::read_or_stored(&status_path, config.workspace_id);

    // If status says running but lock probe says not running, override to stopped.
    if status.running {
        let held = fns_platform::StateDirLease::probe(&config.state_dir)
            .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
        if !held {
            let mut stopped = status;
            stopped.running = false;
            stopped.phase = AgentPhase::Stopped;
            stopped.pid = None;
            return Ok(stopped);
        }
    }

    Ok(status)
}

/// Entry point for `diagnose --json` command.
pub fn run_diagnose(config_path: &Path) -> diagnose::DiagnosticReport {
    diagnose::run_diagnostics(config_path)
}
