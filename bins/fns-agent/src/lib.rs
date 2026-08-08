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
pub mod status;

pub use config::AgentConfig;
pub use error::{AgentError, AgentErrorCode, AgentPhase};
pub use status::AgentStatus;

use std::path::Path;

/// Entry point for `status --json` command.
/// Reads runtime status from the config's state directory.
pub fn run_status(config_path: &Path) -> Result<AgentStatus, AgentError> {
    let config = AgentConfig::load_linux(config_path)?;
    let status_path = config.state_dir.join("runtime-status.json");

    // Probe the lock before trusting running:true.
    #[cfg(target_os = "linux")]
    {
        let lock_path = config.state_dir.join("agent.lock");
        if let Ok(Some(_)) = fns_platform::ProcessLock::probe_linux(&lock_path) {
            // Another agent is running — read the status file.
        }
    }

    let status = AgentStatus::read_or_stored(&status_path, config.workspace_id);

    // If status says running but lock probe says not running, override to stopped.
    #[cfg(target_os = "linux")]
    {
        let lock_path = config.state_dir.join("agent.lock");
        if status.running {
            if let Ok(None) = fns_platform::ProcessLock::probe_linux(&lock_path) {
                let mut stopped = status;
                stopped.running = false;
                stopped.phase = AgentPhase::Stopped;
                stopped.pid = None;
                return Ok(stopped);
            }
        }
    }

    Ok(status)
}

/// Entry point for `diagnose --json` command.
pub fn run_diagnose(config_path: &Path) -> diagnose::DiagnosticReport {
    diagnose::run_diagnostics(config_path)
}
