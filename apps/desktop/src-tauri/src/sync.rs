//! Sync engine integration: embeds the FNS Agent as an in-process tokio task.
//!
//! When the user opens a project, the App:
//! 1. Creates an SSH tunnel (loopback → remote FNS server port 9000)
//! 2. Spawns `fns_agent::daemon::run_embedded()` on a tokio task
//! 3. The agent connects via the tunnel and syncs files bidirectionally
//!
//! Shutdown is controlled by a CancellationToken — no signals involved.

use crate::project::ProjectConfig;
use crate::ssh_tunnel::{SshTunnel, TunnelState};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Handle to a running sync session.
struct SyncHandle {
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    local_port: u16,
}

/// State manager for sync sessions, keyed by project ID.
pub struct SyncState {
    sessions: Mutex<HashMap<String, SyncHandle>>,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Sync status returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    pub local_port: Option<u16>,
    pub message: String,
}

/// The FNS server token for authentication.
/// MVP: stored in the project config, retrieved during onboarding.
/// In production this will go to macOS Keychain.
const DEFAULT_TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJ1aWQiOjEsIm5pY2tuYW1lIjoiIiwidG9rZW5JZCI6MSwibm9uY2UiOiJvcUhWYW5LM0M4djdvZFFIIiwiaXNzIjoiZmFzdC1ub3RlLXN5bmMtc2VydmljZSIsInN1YiI6InVzZXItdG9rZW4iLCJleHAiOjE4MTc2OTcwMTksIm5iZiI6MTc4NjE2MTAxOSwiaWF0IjoxNzg2MTYxMDE5LCJqdGkiOiIxIn0.XCeHDFAoARApl8DYW7FeuuaXQQEGzZHdZVwW3SmRRj8";

/// Get the per-project state directory.
fn project_state_dir(project_id: &str) -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|b| b.config_dir().join("fns-workspace"))
        .unwrap_or_else(|| PathBuf::from(".config/fns-workspace"));
    base.join(format!("projects-{project_id}")).join("state")
}

/// Start sync for a project.
///
/// Creates an SSH tunnel, builds the AgentConfig, and spawns the embedded
/// FNS Agent daemon as a tokio task.
#[tauri::command]
pub async fn start_sync(
    project_id: String,
    token: Option<String>,
    tunnel_state: tauri::State<'_, TunnelState>,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<SyncStatus, String> {
    // Check if already running.
    {
        let sessions = sync_state.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&project_id) {
            return Ok(SyncStatus {
                running: true,
                local_port: sessions.get(&project_id).map(|h| h.local_port),
                message: "Already running".into(),
            });
        }
    }

    // Load project config.
    let projects = ProjectConfig::list_all().map_err(|e| e.to_string())?;
    let project = projects
        .iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Create or reuse SSH tunnel.
    let local_port = {
        let guard = tunnel_state.tunnel.lock().map_err(|e| e.to_string())?;
        if let Some(ref tunnel) = *guard {
            tunnel.local_port()
        } else {
            drop(guard);
            let tunnel = SshTunnel::create(&project.ssh_host_alias, 9000)?;
            let port = tunnel.local_port();
            let mut guard = tunnel_state.tunnel.lock().map_err(|e| e.to_string())?;
            *guard = Some(tunnel);
            port
        }
    };

    // Ensure local workspace root exists.
    std::fs::create_dir_all(&project.local_root).map_err(|e| e.to_string())?;

    // Ensure state dir exists.
    let state_dir = project_state_dir(&project_id);
    std::fs::create_dir_all(&state_dir).map_err(|e| e.to_string())?;

    // Build the WebSocket endpoint through the tunnel.
    let endpoint = format!("ws://127.0.0.1:{local_port}/api/user/workspace-sync/v2");

    // Parse workspace_id and client_id from project config.
    let workspace_id: fns_protocol::WorkspaceId =
        fns_protocol::WorkspaceId::parse(&project.workspace_id.to_string())
            .map_err(|e| format!("Invalid workspace_id: {e:?}"))?;
    let client_id: fns_protocol::ClientId =
        fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000001")
            .map_err(|e| format!("Invalid client_id: {e:?}"))?;

    // Build AgentConfig.
    let config = fns_agent::AgentConfig {
        schema_version: "fns-agent-config/1".into(),
        endpoint,
        workspace_id,
        client_id,
        workspace_root: PathBuf::from(&project.local_root),
        state_dir: state_dir.clone(),
        token_file: PathBuf::from("/dev/null"), // Not used — token passed directly.
        sync: fns_agent::config::AgentSyncConfig {
            includes: project.sync.includes.clone(),
            excludes: project.sync.excludes.clone(),
            protect_secrets: project.sync.protect_secrets,
        },
        transport: fns_agent::config::AgentTransportConfig {
            max_active_transfers: 2,
        },
    };

    // Create SecretToken from the token string.
    let token_str = token.unwrap_or_else(|| DEFAULT_TOKEN.to_string());
    let secret_token = fns_platform::SecretToken::from_bytes_for_test(token_str.as_bytes());

    // Spawn the embedded agent.
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let task = tokio::spawn(async move {
        eprintln!("[sync] Starting embedded FNS agent");
        if let Err(e) = fns_agent::daemon::run_embedded(config, secret_token, shutdown_clone).await
        {
            eprintln!("[sync] Agent error: {:?}", e);
        }
        eprintln!("[sync] Agent stopped");
    });

    // Register the session.
    {
        let mut sessions = sync_state.sessions.lock().map_err(|e| e.to_string())?;
        sessions.insert(
            project_id.clone(),
            SyncHandle {
                shutdown,
                task,
                local_port,
            },
        );
    }

    Ok(SyncStatus {
        running: true,
        local_port: Some(local_port),
        message: "Sync started".into(),
    })
}

/// Stop sync for a project.
#[tauri::command]
pub async fn stop_sync(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<(), String> {
    let handle = {
        let mut sessions = sync_state.sessions.lock().map_err(|e| e.to_string())?;
        sessions.remove(&project_id)
    };

    if let Some(handle) = handle {
        handle.shutdown.cancel();
        let _ = handle.task.await;
    }

    Ok(())
}

/// Check sync status for a project.
#[tauri::command]
pub fn sync_status(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<SyncStatus, String> {
    let sessions = sync_state.sessions.lock().map_err(|e| e.to_string())?;
    if let Some(handle) = sessions.get(&project_id) {
        Ok(SyncStatus {
            running: true,
            local_port: Some(handle.local_port),
            message: "Running".into(),
        })
    } else {
        Ok(SyncStatus {
            running: false,
            local_port: None,
            message: "Stopped".into(),
        })
    }
}
