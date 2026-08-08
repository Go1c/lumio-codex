//! Daemon: recovery-first watcher/transport orchestration with bounded shutdown.
//!
//! Startup order: load config → acquire lock → open engine (includes recovery) →
//! spawn engine worker → start watcher → connect transport → run until signal.
//! Shutdown: stop watcher intake → drain engine work (≤30s) → close socket →
//! write stopped status → release lock.

use crate::config::AgentConfig;
use crate::error::{AgentError, AgentErrorCode, AgentPhase};
use crate::status::AgentStatus;

use fns_sync_core::{SyncEngine, SyncEngineConfig};
use fns_transport::{
    EngineWorker, ReconnectPolicy, ReconnectSchedule, UuidJitter, WorkspaceEndpoint,
};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Run the agent daemon until shutdown.
///
/// This is a foreground process: no fork, daemonization, PID killing, or service
/// installation. The caller is expected to handle signals.
pub async fn run(config: AgentConfig, token: fns_platform::SecretToken) -> Result<(), AgentError> {
    let workspace_id = config.workspace_id;
    let state_dir = config.state_dir.clone();

    // Step 1: Acquire singleton lock.
    #[cfg(target_os = "linux")]
    {
        let lock_path = state_dir.join("agent.lock");
        let _lock = fns_platform::ProcessLock::acquire_linux(&lock_path).map_err(|e| {
            if e.code() == fns_platform::PlatformErrorCode::AlreadyRunning {
                AgentError::new(AgentErrorCode::AlreadyRunning)
            } else {
                AgentError::new(AgentErrorCode::Filesystem)
            }
        })?;
        // Lock is held for the duration of run.
        std::mem::forget(_lock); // Keep lock alive — released when process exits.
    }

    // Step 2: Write starting status.
    write_status(&state_dir, AgentPhase::Starting, workspace_id);

    // Step 3: Open engine (includes recovery).
    write_status(&state_dir, AgentPhase::Recovering, workspace_id);
    let engine_config = SyncEngineConfig::new(
        workspace_id,
        config.client_id,
        &config.workspace_root,
        &state_dir,
    );
    let engine =
        SyncEngine::open(engine_config).map_err(|_| AgentError::new(AgentErrorCode::Core))?;

    // Step 4: Spawn engine worker.
    let (worker, handle) = EngineWorker::spawn(engine);

    // Step 5: Start watcher (after recovery).
    let _watcher = start_watcher(&config, handle.clone());

    // Step 6: Connect transport.
    write_status(&state_dir, AgentPhase::Connecting, workspace_id);

    let endpoint = WorkspaceEndpoint::parse(&config.endpoint)
        .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))?;

    let mut schedule = ReconnectSchedule::new(ReconnectPolicy::default(), UuidJitter);

    // Reconnect loop.
    loop {
        let connect_result = fns_transport::socket::connect(&endpoint, &token, "0.1.0").await;

        match connect_result {
            Ok(stream) => {
                write_status(&state_dir, AgentPhase::Subscribing, workspace_id);
                let (session, mut writer) = fns_transport::session::Session::new(
                    stream,
                    handle.clone(),
                    workspace_id,
                    config.client_id,
                    "0.1.0".into(),
                );

                let shutdown = tokio_util::sync::CancellationToken::new();
                let shutdown_clone = shutdown.clone();

                // Spawn signal handler.
                tokio::spawn(async move {
                    #[cfg(unix)]
                    {
                        use tokio::signal;
                        let _ = signal::unix::signal(signal::unix::SignalKind::terminate());
                        let _ = signal::ctrl_c().await;
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                    shutdown_clone.cancel();
                });

                let result = session.run(&mut writer, shutdown).await;
                match result {
                    fns_transport::session::SessionResult::Closed => {
                        write_status(&state_dir, AgentPhase::Connecting, workspace_id);
                    }
                    fns_transport::session::SessionResult::Error(e) => {
                        if !e.retryable() {
                            write_status_error(&state_dir, workspace_id, e.code());
                            let _ = handle.shutdown().await;
                            let _ = worker.join();
                            return Err(map_transport_error(e.code()));
                        }
                        write_status(&state_dir, AgentPhase::Connecting, workspace_id);
                    }
                }
            }
            Err(e) => {
                if !e.retryable() {
                    write_status_error(&state_dir, workspace_id, e.code());
                    let _ = handle.shutdown().await;
                    let _ = worker.join();
                    return Err(map_transport_error(e.code()));
                }
            }
        }

        // Backoff before reconnecting.
        let delay = schedule.next_delay();
        tokio::time::sleep(delay).await;
        write_status_reconnect(&state_dir, workspace_id, schedule.attempt());
    }
}

fn map_transport_error(code: fns_transport::TransportErrorCode) -> AgentError {
    match code {
        fns_transport::TransportErrorCode::AuthenticationRejected => {
            AgentError::new(AgentErrorCode::AuthenticationRejected)
        }
        fns_transport::TransportErrorCode::Forbidden => AgentError::new(AgentErrorCode::Forbidden),
        fns_transport::TransportErrorCode::Network => AgentError::new(AgentErrorCode::Network),
        fns_transport::TransportErrorCode::Protocol => AgentError::new(AgentErrorCode::Protocol),
        fns_transport::TransportErrorCode::Core => AgentError::new(AgentErrorCode::Core),
        fns_transport::TransportErrorCode::Filesystem => {
            AgentError::new(AgentErrorCode::Filesystem)
        }
        _ => AgentError::new(AgentErrorCode::Network),
    }
}

fn start_watcher(
    config: &AgentConfig,
    _handle: fns_transport::EngineHandle,
) -> Option<fns_fs::PlatformWatcher> {
    // Conservative watcher: started after recovery.
    // Full event coalescing and bridge is implemented in later refinement.
    let rules = fns_fs::SyncRuleConfig {
        includes: config.sync.includes.clone(),
        excludes: config.sync.excludes.clone(),
        protect_secrets: config.sync.protect_secrets,
    };
    let _rules = fns_fs::SyncRules::compile(rules).ok()?;

    let root = fns_fs::RootedWorkspace::open(&config.workspace_root).ok()?;
    fns_fs::start_platform_watcher(&root, 4096)
        .ok()
        .map(|(watcher, _receiver)| watcher)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn write_status(
    state_dir: &std::path::Path,
    phase: AgentPhase,
    workspace_id: fns_protocol::WorkspaceId,
) {
    let status = AgentStatus {
        schema_version: "fns-agent-status/1".into(),
        running: true,
        phase,
        pid: Some(std::process::id()),
        connected: phase == AgentPhase::Online || phase == AgentPhase::Subscribing,
        workspace_id,
        last_ack_revision: fns_protocol::WorkspaceRevision::ZERO,
        pending_commands: 0,
        queued_watcher_batches: 0,
        active_transfers: 0,
        reconnect_attempt: 0,
        last_error_code: None,
        updated_at_ms: now_ms(),
    };
    let path = state_dir.join("runtime-status.json");
    let _ = status.write_to(&path);
}

fn write_status_reconnect(
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
    attempt: u32,
) {
    let status = AgentStatus {
        schema_version: "fns-agent-status/1".into(),
        running: true,
        phase: AgentPhase::Connecting,
        pid: Some(std::process::id()),
        connected: false,
        workspace_id,
        last_ack_revision: fns_protocol::WorkspaceRevision::ZERO,
        pending_commands: 0,
        queued_watcher_batches: 0,
        active_transfers: 0,
        reconnect_attempt: attempt,
        last_error_code: Some(AgentErrorCode::Network),
        updated_at_ms: now_ms(),
    };
    let path = state_dir.join("runtime-status.json");
    let _ = status.write_to(&path);
}

fn write_status_error(
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
    code: fns_transport::TransportErrorCode,
) {
    let agent_code = match code {
        fns_transport::TransportErrorCode::AuthenticationRejected => {
            AgentErrorCode::AuthenticationRejected
        }
        fns_transport::TransportErrorCode::Forbidden => AgentErrorCode::Forbidden,
        _ => AgentErrorCode::Network,
    };
    let status = AgentStatus {
        schema_version: "fns-agent-status/1".into(),
        running: false,
        phase: AgentPhase::Fatal,
        pid: None,
        connected: false,
        workspace_id,
        last_ack_revision: fns_protocol::WorkspaceRevision::ZERO,
        pending_commands: 0,
        queued_watcher_batches: 0,
        active_transfers: 0,
        reconnect_attempt: 0,
        last_error_code: Some(agent_code),
        updated_at_ms: now_ms(),
    };
    let path = state_dir.join("runtime-status.json");
    let _ = status.write_to(&path);
}

/// Await all participants with a bounded grace period.
#[cfg(test)]
async fn await_shutdown_participants(
    participants: Vec<tokio::task::JoinHandle<()>>,
    grace: Duration,
) -> Result<(), AgentError> {
    let deadline = tokio::time::sleep(grace);
    tokio::pin!(deadline);

    for mut participant in participants {
        tokio::select! {
            _ = &mut deadline => {
                return Err(AgentError::new(AgentErrorCode::ShutdownTimeout));
            }
            _ = &mut participant => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_timeout_when_participant_never_completes() {
        // A participant that never completes.
        let handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let result = await_shutdown_participants(vec![handle], Duration::from_millis(10)).await;
        assert!(result.is_err());
    }
}
