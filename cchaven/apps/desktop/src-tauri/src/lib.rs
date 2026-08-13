//! CC避风港（CCHaven）desktop backend — Tauri 2.
//!
//! One command surface covering both halves of the product:
//!
//! * the consumer shell — browser account access, the three-step project
//!   wizard, the local sync folder with undo, and the conflict page;
//! * the engineering surface — remote deployment, Claude session control,
//!   server monitoring, diagnostics, SSH tunnels and the sync engine's own
//!   conflict state machine.
//!
//! The sync engine runs as a local `fns-agent` sidecar reached over an SSH
//! tunnel; `sync.rs` owns its lifecycle and `conflict_bridge.rs` translates its
//! conflict control surface into the one the conflict page renders.

pub mod askpass;
pub mod auth;
pub mod conflict_bridge;
pub mod conflicts;
pub mod control;
pub mod files;
pub mod project;
pub mod ssh;
pub mod terminal;

// The engineering surface stays crate-private: its error types are `pub(crate)`
// and nothing outside this crate drives deployment, tunnels or the agent.
mod credentials;
mod deploy;
mod diagnostics;
mod remote_monitor;
mod ssh_tunnel;
mod sync;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use conflicts::{Conflict, ConflictStore, Resolution, ResolutionReceipt};
use files::{EntryKind, FileNode, FilePreview, TrashTicket};
use project::ProjectConfig;

const FINAL_EXIT_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(120);

// --- App info ---

/// Startup facts the frontend needs before it renders anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    /// True when the control plane is mocked (see `README.md`).
    pub mock_control: bool,
    pub links: ExternalLinks,
}

/// The ↗ destinations of the account menu (5.6). All open in the browser.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLinks {
    pub account: String,
    pub invite: String,
    pub docs: String,
    pub support: String,
    pub server_guide: String,
    pub troubleshooting: String,
}

/// Directory presets derived for wizard step 2.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPresets {
    pub remote_root: String,
    pub local_root: String,
    pub tmux_session: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProjectRequest {
    pub config: ProjectConfig,
    /// Only present when the wizard captured a new password; goes straight to
    /// the keychain and is never written to `projects.json`.
    #[serde(default)]
    pub password: Option<String>,
}

#[tauri::command]
fn app_info(auth: tauri::State<'_, auth::AuthState>) -> AppInfo {
    let web = auth.control().config().web_base.clone();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        mock_control: auth.control().is_mock(),
        links: ExternalLinks {
            account: format!("{web}/account"),
            invite: format!("{web}/account#invite"),
            docs: format!("{web}/docs"),
            support: format!("{web}/support"),
            server_guide: format!("{web}/docs/buy-a-server"),
            troubleshooting: format!("{web}/docs/connection-troubleshooting"),
        },
    }
}

// --- Projects ---

#[tauri::command]
fn list_projects() -> Result<Vec<ProjectConfig>, String> {
    ProjectConfig::list_all().map_err(|e| format!("无法读取项目列表：{e}"))
}

#[tauri::command]
fn get_project(project_id: String) -> Result<Option<ProjectConfig>, String> {
    ProjectConfig::get(&project_id).map_err(|e| format!("无法读取项目：{e}"))
}

#[tauri::command]
async fn save_project(
    request: SaveProjectRequest,
    auth: tauri::State<'_, auth::AuthState>,
) -> Result<ProjectConfig, String> {
    let mut config = request.config;
    config.sync = project::normalise_sync(config.sync);
    if config.tmux_session.trim().is_empty() {
        config.tmux_session = project::default_tmux_session(&config.name);
    }
    if config.created_at.is_empty() {
        config.created_at = format!("{}", files::now_ms());
    }

    if let Some(password) = request.password.filter(|p| !p.is_empty()) {
        auth.secrets()
            .store_ssh_password(&config.id.to_string(), &password)
            .map_err(|e| e.to_string())?;
    }
    config
        .save_to_default()
        .map_err(|e| format!("无法保存项目：{e}"))?;
    Ok(config)
}

/// Remove a project from the app. Nothing on disk or on the server is deleted.
#[tauri::command]
async fn delete_project(
    project_id: String,
    auth: tauri::State<'_, auth::AuthState>,
    sync_state: tauri::State<'_, sync::SyncState>,
) -> Result<(), String> {
    let _ = sync_state.stop(&project_id).await;
    let _ = auth.secrets().clear_ssh_password(&project_id);
    let _ = auth.secrets().clear_sync_agent_token(&project_id);
    ProjectConfig::delete(&project_id).map_err(|e| format!("无法删除项目：{e}"))
}

#[tauri::command]
fn project_presets(name: String, user: String) -> ProjectPresets {
    ProjectPresets {
        remote_root: project::default_remote_root(&user, &name),
        local_root: project::default_local_root(&home_dir(), &name),
        tmux_session: project::default_tmux_session(&name),
    }
}

// --- SSH ---

#[tauri::command]
fn parse_ssh_hosts() -> Result<Vec<ssh::SshHost>, String> {
    ssh::parse_ssh_config().map_err(|e| format!("无法读取 ~/.ssh/config：{e}"))
}

/// Paste recognition for the 服务器 IP 地址 field (5.3).
#[tauri::command]
fn parse_pasted_target(text: String) -> Option<ssh::SshTarget> {
    ssh::parse_ssh_target(&text)
}

#[tauri::command]
async fn test_connection(
    server: project::ServerConfig,
    password: Option<String>,
) -> ssh::ProbeResult {
    ssh::probe_server(&server, password.as_deref()).await
}

// --- Files ---

fn local_root_of(project_id: &str) -> Result<PathBuf, String> {
    let config = ProjectConfig::get(project_id)
        .map_err(|e| format!("无法读取项目配置：{e}"))?
        .ok_or("项目不存在。")?;
    Ok(PathBuf::from(config.local_root))
}

fn staging_dir() -> Result<PathBuf, String> {
    let dir = ProjectConfig::config_dir()
        .map_err(|e| format!("无法准备回收暂存目录：{e}"))?
        .join("trash");
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法准备回收暂存目录：{e}"))?;
    Ok(dir)
}

fn conflicts_dir() -> Result<PathBuf, String> {
    Ok(ProjectConfig::config_dir()
        .map_err(|e| format!("无法准备冲突记录目录：{e}"))?
        .join("conflicts"))
}

#[tauri::command]
fn list_files(project_id: String) -> Result<Vec<FileNode>, String> {
    let root = local_root_of(&project_id)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    files::read_tree(&root, 8).map_err(|e| format!("无法读取本机同步文件夹：{e}"))
}

#[tauri::command]
fn recent_files(project_id: String, limit: Option<usize>) -> Result<Vec<FileNode>, String> {
    let root = local_root_of(&project_id)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    files::recent_files(&root, limit.unwrap_or(6))
        .map_err(|e| format!("无法读取本机同步文件夹：{e}"))
}

#[tauri::command]
fn read_file(project_id: String, path: String) -> Result<FilePreview, String> {
    files::read_preview(&local_root_of(&project_id)?, &path)
}

// The agent watches the project's local root, so a write made here is picked up
// by the same watcher that sees the user's editor. Nothing has to be recorded.

#[tauri::command]
fn create_entry(
    project_id: String,
    parent: String,
    name: String,
    kind: EntryKind,
) -> Result<String, String> {
    files::create_entry(&local_root_of(&project_id)?, &parent, &name, kind)
}

#[tauri::command]
fn rename_entry(project_id: String, path: String, new_name: String) -> Result<String, String> {
    files::rename_entry(&local_root_of(&project_id)?, &path, &new_name)
}

#[tauri::command]
fn delete_entry(project_id: String, path: String) -> Result<TrashTicket, String> {
    files::delete_entry(&local_root_of(&project_id)?, &path, &staging_dir()?)
}

#[tauri::command]
fn undo_delete(project_id: String, token: String) -> Result<String, String> {
    files::restore_entry(&local_root_of(&project_id)?, &staging_dir()?, &token)
}

#[tauri::command]
fn purge_delete(token: String) -> Result<(), String> {
    files::purge_staged(&staging_dir()?, &token);
    Ok(())
}

#[tauri::command]
fn reveal_entry(project_id: String, path: Option<String>) -> Result<(), String> {
    let root = local_root_of(&project_id)?;
    let target = match path {
        Some(path) if !path.is_empty() => files::resolve_within(&root, &path)?,
        _ => root,
    };
    files::reveal(&target)
}

#[tauri::command]
fn open_entry(project_id: String, path: String) -> Result<(), String> {
    let target = files::resolve_within(&local_root_of(&project_id)?, &path)?;
    files::open_default(&target)
}

/// 「打开本地文件夹」 in the workspace top bar.
#[tauri::command]
fn open_local_folder(project_id: String) -> Result<(), String> {
    let root = local_root_of(&project_id)?;
    if !root.exists() {
        std::fs::create_dir_all(&root).map_err(|e| format!("无法创建本机同步文件夹：{e}"))?;
    }
    files::open_default(&root)
}

// --- Conflicts (product level) ---

fn conflict_store(project_id: &str) -> Result<ConflictStore, String> {
    ConflictStore::new(&conflicts_dir()?, project_id)
}

fn seeded_marker(project_id: &str) -> Result<PathBuf, String> {
    Ok(conflicts_dir()?.join(format!("{project_id}.seeded")))
}

#[tauri::command]
async fn list_conflicts(
    project_id: String,
    auth: tauri::State<'_, auth::AuthState>,
    sync_state: tauri::State<'_, sync::SyncState>,
) -> Result<Vec<Conflict>, String> {
    let store = conflict_store(&project_id)?;
    // Engine rows win when a session is up. Without one — offline, or before
    // the first deployment — the last projection is what the page shows.
    if let Ok(views) = sync_state.list_conflicts(&project_id).await
        && !views.is_empty()
    {
        let state_dir = sync::project_state_dir(&project_id);
        let conflicts: Vec<_> = views
            .iter()
            .map(|view| conflict_bridge::conflict_from_view(&state_dir, view))
            .collect();
        store.replace(conflicts.clone())?;
        return Ok(conflicts);
    }
    let conflicts = store.list();
    // Mock mode seeds a sample pair once so the page is reachable without a
    // running sync engine session.
    if conflicts.is_empty() && auth.control().is_mock() && !seeded_marker(&project_id)?.exists() {
        let seeded = conflicts::sample_conflicts(files::now_ms());
        store.replace(seeded.clone())?;
        std::fs::write(seeded_marker(&project_id)?, "1").ok();
        return Ok(seeded);
    }
    Ok(conflicts)
}

#[tauri::command]
async fn resolve_conflict(
    project_id: String,
    conflict_id: String,
    resolution: Resolution,
    identity: Option<sync::ConflictControlIdentity>,
    sync_state: tauri::State<'_, sync::SyncState>,
) -> Result<ResolutionReceipt, String> {
    let root = local_root_of(&project_id)?;

    // Ask the engine first when a session is up: if it refuses (stale revision,
    // a competing request) the local folder must not be touched at all.
    if let Some(identity) = identity
        && let Ok(views) = sync_state.list_conflicts(&project_id).await
        && let Some(view) = conflict_bridge::find_view(&views, &conflict_id)
    {
        let input = fns_agent::ConflictResolutionInput {
            conflict_id: view.conflict_id,
            conflict_revision: view.conflict_revision,
            choice: conflict_bridge::engine_choice(resolution, view.incoming.tombstone),
        };
        sync_state
            .resolve_conflict(&project_id, identity, input)
            .await
            .map_err(|failure| sync::stable_error_code(&failure))?;
    }

    conflict_store(&project_id)?.resolve(&root, &conflict_id, resolution)
}

#[tauri::command]
fn undo_conflict(project_id: String, conflict_id: String) -> Result<Conflict, String> {
    let root = local_root_of(&project_id)?;
    conflict_store(&project_id)?.undo(&root, &conflict_id)
}

#[tauri::command]
fn forget_conflict_undo(project_id: String, conflict_id: String) -> Result<(), String> {
    conflict_store(&project_id)?.forget_undo(&conflict_id);
    Ok(())
}

#[tauri::command]
fn conflict_diff(project_id: String, conflict_id: String) -> Result<files::DiffResult, String> {
    let conflict = conflict_store(&project_id)?
        .list()
        .into_iter()
        .find(|c| c.id == conflict_id)
        .ok_or("该冲突已被处理。")?;
    Ok(files::compute_text_diff(
        &conflict.local.content,
        &conflict.remote.content,
    ))
}

// --- Aggregate sync status (交互设计 6.3) ---

/// 6.3 全局唯一语义. Nothing in the app may invent a fifth state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStateLabel {
    Synced,
    Syncing,
    Conflicts,
    Offline,
}

/// The shape the frontend consumes (`SyncStatus` in `lib/types.ts`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncStateLabel,
    pub conflicts: usize,
    pub pending: usize,
    /// Why the session is down, for the activity panel and for support.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Reduce what the session registry and the agent's own status file know onto
/// the four states, worst case first.
pub fn reduce_sync_status(
    connected: bool,
    conflicts: usize,
    pending: usize,
    detail: Option<String>,
) -> SyncStatus {
    let state = if conflicts > 0 {
        SyncStateLabel::Conflicts
    } else if !connected {
        SyncStateLabel::Offline
    } else if pending > 0 {
        SyncStateLabel::Syncing
    } else {
        SyncStateLabel::Synced
    };
    SyncStatus {
        state,
        conflicts,
        pending,
        detail: (!connected).then_some(detail).flatten(),
    }
}

/// Derive the 6.3 status for a project.
///
/// The counts come from the agent's `runtime-status.json`: it is the only place
/// that knows how much work is still queued, and the agent rewrites it
/// atomically on every state change.
#[tauri::command]
async fn sync_status(
    project_id: String,
    sync_state: tauri::State<'_, sync::SyncState>,
) -> Result<SyncStatus, String> {
    let session = sync_state.status(&project_id).await;
    let agent = sync::agent_runtime_status(&project_id);

    let conflicts = match sync_state.list_conflicts(&project_id).await {
        Ok(views) => views.len(),
        Err(_) => conflict_store(&project_id)
            .map(|store| store.list().len())
            .unwrap_or(0),
    };
    let (connected, pending) = match agent {
        Some(agent) => (
            session.running && agent.connected,
            usize::try_from(agent.pending_commands).unwrap_or(usize::MAX)
                + agent.queued_watcher_batches
                + agent.active_transfers,
        ),
        None => (false, 0),
    };
    let detail = session
        .error
        .as_ref()
        .map(sync::stable_error_code)
        .or_else(|| (!session.message.is_empty()).then(|| session.message.clone()));

    Ok(reduce_sync_status(connected, conflicts, pending, detail))
}

/// Directory presets need a home dir; exposed for tests of the command layer.
pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~"))
}

/// True when a path is inside the user's home directory.
pub fn is_under_home(path: &Path) -> bool {
    path.starts_with(home_dir())
}

// --- Exit lifecycle ---

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitDecision {
    StartCleanup(i32),
    Prevent,
    Allow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitPhase {
    Idle,
    Cleaning(i32),
    Failed,
    Authorized(i32),
    Exiting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitCleanupOutcome {
    Skipped,
    Succeeded,
    Failed,
    TimedOut,
}

struct ExitLifecycle {
    phase: Mutex<ExitPhase>,
    tasks: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
}

impl ExitLifecycle {
    fn request(&self, code: Option<i32>) -> ExitDecision {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *phase {
            ExitPhase::Idle | ExitPhase::Failed => {
                let code = code.unwrap_or(0);
                *phase = ExitPhase::Cleaning(code);
                ExitDecision::StartCleanup(code)
            }
            ExitPhase::Cleaning(_) => ExitDecision::Prevent,
            ExitPhase::Authorized(expected) if code == Some(expected) => {
                *phase = ExitPhase::Exiting;
                ExitDecision::Allow
            }
            ExitPhase::Authorized(_) => ExitDecision::Prevent,
            ExitPhase::Exiting => ExitDecision::Allow,
        }
    }

    fn finish_cleanup(&self, succeeded: bool) -> Option<i32> {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ExitPhase::Cleaning(code) = *phase else {
            return None;
        };
        if succeeded {
            *phase = ExitPhase::Authorized(code);
            Some(code)
        } else {
            *phase = ExitPhase::Failed;
            None
        }
    }

    fn begin_final_cleanup(&self) -> bool {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *phase {
            ExitPhase::Authorized(_) | ExitPhase::Exiting => false,
            ExitPhase::Idle | ExitPhase::Cleaning(_) | ExitPhase::Failed => {
                *phase = ExitPhase::Exiting;
                true
            }
        }
    }

    fn own(&self, task: tauri::async_runtime::JoinHandle<()>) {
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(task);
    }
}

impl Default for ExitLifecycle {
    fn default() -> Self {
        Self {
            phase: Mutex::new(ExitPhase::Idle),
            tasks: Mutex::new(Vec::new()),
        }
    }
}

async fn shutdown_graceful_for_exit(
    credential_state: &credentials::CredentialState,
    sync_state: &sync::SyncState,
    tunnel_state: &ssh_tunnel::TunnelState,
) -> bool {
    let (credential_result, sync_result) = tokio::join!(
        credential_state.shutdown_all(tunnel_state.clone()),
        sync_state.shutdown_all(),
    );
    if let Err(failure) = credential_result.as_ref() {
        eprintln!("fns_credential_shutdown_failed:{failure}");
    }
    if let Err(failure) = sync_result.as_ref() {
        eprintln!(
            "fns_sync_shutdown_failed:{}",
            sync::stable_error_code(failure)
        );
    }
    credential_result.is_ok() && sync_result.is_ok()
}

async fn shutdown_for_exit(
    credential_state: &credentials::CredentialState,
    sync_state: &sync::SyncState,
    tunnel_state: &ssh_tunnel::TunnelState,
) -> bool {
    let graceful_succeeded =
        shutdown_graceful_for_exit(credential_state, sync_state, tunnel_state).await;
    let tunnel_result = if credential_state.has_active_operations() {
        None
    } else {
        Some(tunnel_state.close_all().await)
    };
    if let Some(Err(failure)) = tunnel_result.as_ref() {
        eprintln!("fns_ssh_shutdown_failed:{failure}");
    }
    graceful_succeeded && tunnel_result.is_some_and(|result| result.is_ok())
}

async fn cleanup_final_resources<F>(
    tunnel_state: &ssh_tunnel::TunnelState,
    graceful_timeout: Duration,
    graceful_shutdown: F,
) -> ExitCleanupOutcome
where
    F: Future<Output = bool>,
{
    let graceful_outcome = match tokio::time::timeout(graceful_timeout, graceful_shutdown).await {
        Ok(true) => {
            eprintln!("fns_final_exit_graceful_complete");
            ExitCleanupOutcome::Succeeded
        }
        Ok(false) => {
            eprintln!("fns_final_exit_graceful_failed");
            ExitCleanupOutcome::Failed
        }
        Err(_) => {
            eprintln!("fns_final_exit_graceful_timeout");
            ExitCleanupOutcome::TimedOut
        }
    };

    match tunnel_state.close_all().await {
        Ok(()) => {
            eprintln!("fns_final_exit_tunnel_complete");
            graceful_outcome
        }
        Err(failure) => {
            eprintln!("fns_final_exit_tunnel_failed:{failure}");
            ExitCleanupOutcome::Failed
        }
    }
}

fn cleanup_after_final_event(
    lifecycle: &ExitLifecycle,
    credential_state: &credentials::CredentialState,
    sync_state: &sync::SyncState,
    tunnel_state: &ssh_tunnel::TunnelState,
    graceful_timeout: Duration,
) -> ExitCleanupOutcome {
    if !lifecycle.begin_final_cleanup() {
        return ExitCleanupOutcome::Skipped;
    }

    match tauri::async_runtime::block_on(async {
        cleanup_final_resources(
            tunnel_state,
            graceful_timeout,
            shutdown_graceful_for_exit(credential_state, sync_state, tunnel_state),
        )
        .await
    }) {
        ExitCleanupOutcome::Succeeded => {
            eprintln!("fns_final_exit_cleanup_complete");
            ExitCleanupOutcome::Succeeded
        }
        ExitCleanupOutcome::Failed => {
            eprintln!("fns_final_exit_cleanup_failed");
            ExitCleanupOutcome::Failed
        }
        ExitCleanupOutcome::TimedOut => {
            eprintln!("fns_final_exit_cleanup_timeout");
            ExitCleanupOutcome::TimedOut
        }
        ExitCleanupOutcome::Skipped => ExitCleanupOutcome::Skipped,
    }
}

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
#[cfg_attr(target_os = "android", tauri::mobile_entry_point)]
pub fn run() {
    let credential_state = credentials::CredentialState::production();
    let sync_state = sync::SyncState::with_credentials(Arc::new(credential_state.clone()));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(auth::AuthState::from_env())
        .manage(terminal::TerminalManager::new())
        .manage(ssh_tunnel::TunnelState::new())
        .manage(deploy::DeployState::production())
        .manage(credential_state)
        .manage(sync_state)
        .manage(diagnostics::DiagnosticsState::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            // account
            auth::auth_begin_login,
            auth::auth_reopen_browser,
            auth::auth_cancel_login,
            auth::auth_submit_manual_code,
            auth::auth_restore_session,
            auth::auth_logout,
            auth::auth_heartbeat,
            auth::auth_session,
            auth::open_external,
            // projects
            list_projects,
            get_project,
            save_project,
            delete_project,
            project_presets,
            parse_ssh_hosts,
            parse_pasted_target,
            test_connection,
            // deployment
            deploy::preview_remote_deployment,
            deploy::execute_remote_deployment,
            deploy::cancel_remote_deployment,
            // files
            list_files,
            recent_files,
            read_file,
            create_entry,
            rename_entry,
            delete_entry,
            undo_delete,
            purge_delete,
            reveal_entry,
            open_entry,
            open_local_folder,
            // conflicts (product level)
            list_conflicts,
            resolve_conflict,
            undo_conflict,
            forget_conflict_undo,
            conflict_diff,
            sync_status,
            // sync engine
            sync::start_sync,
            sync::stop_sync,
            sync::sync_engine_status,
            sync::list_sync_conflicts,
            sync::resolve_sync_conflict,
            sync::cancel_sync_conflict_request,
            sync::cancel_sync_conflict_generation,
            sync::list_sync_conflict_operations,
            // terminal
            terminal::start_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::close_terminal,
            terminal::new_claude_session,
            terminal::close_tmux_window,
            terminal::list_tmux_windows,
            terminal::kill_all_sessions,
            // remote monitoring
            remote_monitor::get_server_status,
            remote_monitor::list_claude_sessions,
            remote_monitor::switch_claude_session,
            remote_monitor::kill_claude_session,
            // diagnostics
            diagnostics::diagnostics_list_events,
            diagnostics::diagnostics_get_health,
            diagnostics::diagnostics_preview_support_bundle,
            diagnostics::diagnostics_export_support_bundle,
            diagnostics::diagnostics_run_self_test,
            diagnostics::diagnostics_cancel_self_test,
            // ssh tunnels
            ssh_tunnel::create_tunnel,
            ssh_tunnel::tunnel_endpoint,
            ssh_tunnel::close_tunnel,
            // workspace credentials
            credentials::provision_workspace_credential,
            credentials::reprovision_workspace_credential,
            credentials::workspace_credential_status,
            credentials::probe_workspace_access,
            credentials::delete_workspace_credential,
            credentials::cancel_workspace_provisioning,
            credentials::retry_workspace_credential_cleanup,
            credentials::workspace_credential_cleanup_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    let exit_lifecycle = Arc::new(ExitLifecycle::default());
    app.run(move |handle, event| match event {
        tauri::RunEvent::ExitRequested { code, api, .. } => match exit_lifecycle.request(code) {
            ExitDecision::Allow => {}
            ExitDecision::Prevent => api.prevent_exit(),
            ExitDecision::StartCleanup(exit_code) => {
                api.prevent_exit();
                let handle = handle.clone();
                let lifecycle = Arc::clone(&exit_lifecycle);
                let task = tauri::async_runtime::spawn(async move {
                    let cleanup_handle = handle.clone();
                    match tauri::async_runtime::spawn(async move {
                        let sync_state = cleanup_handle.state::<sync::SyncState>();
                        let credential_state =
                            cleanup_handle.state::<credentials::CredentialState>();
                        let tunnel_state = cleanup_handle.state::<ssh_tunnel::TunnelState>();
                        shutdown_for_exit(
                            credential_state.inner(),
                            sync_state.inner(),
                            tunnel_state.inner(),
                        )
                        .await
                    })
                    .await
                    {
                        Ok(succeeded) => {
                            if lifecycle.finish_cleanup(succeeded) == Some(exit_code) {
                                eprintln!("fns_exit_requested_cleanup_complete");
                                handle.exit(exit_code);
                            } else if !succeeded {
                                eprintln!("fns_exit_requested_cleanup_failed");
                            }
                        }
                        Err(_) => {
                            eprintln!("fns_sync_shutdown_failed:abnormal_exit");
                            eprintln!("fns_exit_requested_cleanup_failed:abnormal_exit");
                            let _ = lifecycle.finish_cleanup(false);
                        }
                    }
                });
                exit_lifecycle.own(task);
            }
        },
        tauri::RunEvent::Exit => {
            let sync_state = handle.state::<sync::SyncState>();
            let credential_state = handle.state::<credentials::CredentialState>();
            let tunnel_state = handle.state::<ssh_tunnel::TunnelState>();
            let _ = cleanup_after_final_event(
                exit_lifecycle.as_ref(),
                credential_state.inner(),
                sync_state.inner(),
                tunnel_state.inner(),
                FINAL_EXIT_GRACEFUL_TIMEOUT,
            );
        }
        _ => {}
    });
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn a_connected_idle_session_is_fully_synced() {
        let status = reduce_sync_status(true, 0, 0, None);
        assert_eq!(status.state, SyncStateLabel::Synced);
        assert_eq!(status.detail, None);
    }

    #[test]
    fn outstanding_work_counts_what_the_agent_still_owes() {
        let status = reduce_sync_status(true, 0, 3, None);
        assert_eq!(status.state, SyncStateLabel::Syncing);
        assert_eq!(status.pending, 3);
    }

    #[test]
    fn conflicts_outrank_everything_including_being_offline() {
        let status = reduce_sync_status(false, 2, 5, Some("transport".into()));
        assert_eq!(status.state, SyncStateLabel::Conflicts);
        assert_eq!(status.conflicts, 2);
    }

    #[test]
    fn being_offline_outranks_having_transfers_queued() {
        let status = reduce_sync_status(false, 0, 4, Some("transport".into()));
        assert_eq!(status.state, SyncStateLabel::Offline);
        // The queue is still reported: it is what will move once we reconnect.
        assert_eq!(status.pending, 4);
        assert_eq!(status.detail.as_deref(), Some("transport"));
    }

    #[test]
    fn a_connected_session_stops_advertising_a_failure_reason() {
        let status = reduce_sync_status(true, 0, 0, Some("stale".into()));
        assert_eq!(status.state, SyncStateLabel::Synced);
        assert_eq!(status.detail, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::time::Duration;

    struct ExitCredentialBackend;

    impl credentials::CredentialBackend for ExitCredentialBackend {
        fn store(
            &self,
            _project_id: &str,
            _token: &fns_platform::SecretToken,
        ) -> Result<(), credentials::CredentialBackendFailure> {
            Ok(())
        }

        fn load(
            &self,
            _project_id: &str,
        ) -> Result<Option<fns_platform::SecretToken>, credentials::CredentialBackendFailure>
        {
            Ok(None)
        }

        fn delete(&self, _project_id: &str) -> Result<(), credentials::CredentialBackendFailure> {
            Ok(())
        }
    }

    struct ExitTunnelControl {
        creates: AtomicUsize,
        close_failures: AtomicUsize,
        close_attempts: Mutex<Vec<u64>>,
        successful_closes: AtomicUsize,
        dropped_unclosed: AtomicUsize,
    }

    struct ExitTunnelFactory {
        control: Arc<ExitTunnelControl>,
    }

    impl ssh_tunnel::TunnelFactory for ExitTunnelFactory {
        fn create(
            &self,
            _tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn ssh_tunnel::TunnelResource>, ssh_tunnel::TunnelCreateFailure> {
            self.control.creates.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ExitTunnelResource {
                identity: 73,
                closed: false,
                control: Arc::clone(&self.control),
            }))
        }
    }

    struct ExitTunnelResource {
        identity: u64,
        closed: bool,
        control: Arc<ExitTunnelControl>,
    }

    impl ssh_tunnel::TunnelResource for ExitTunnelResource {
        fn local_port(&self) -> u16 {
            19050
        }

        fn is_alive(&mut self) -> Result<bool, ssh_tunnel::TunnelFailure> {
            Ok(!self.closed)
        }

        fn close(&mut self) -> Result<(), ssh_tunnel::TunnelFailure> {
            self.control
                .close_attempts
                .lock()
                .unwrap()
                .push(self.identity);
            if self
                .control
                .close_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(ssh_tunnel::TunnelErrorCode::WaitTimeout.into());
            }
            if !self.closed {
                self.closed = true;
                self.control
                    .successful_closes
                    .fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    impl Drop for ExitTunnelResource {
        fn drop(&mut self) {
            if !self.closed {
                self.control.dropped_unclosed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[cfg(unix)]
    struct ExitProcessTunnelControl {
        pid: AtomicU32,
        closed: AtomicBool,
    }

    #[cfg(unix)]
    struct ExitProcessTunnelFactory {
        control: Arc<ExitProcessTunnelControl>,
    }

    #[cfg(unix)]
    impl ssh_tunnel::TunnelFactory for ExitProcessTunnelFactory {
        fn create(
            &self,
            _tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn ssh_tunnel::TunnelResource>, ssh_tunnel::TunnelCreateFailure> {
            let child = Command::new("/bin/sleep")
                .arg("300")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| ssh_tunnel::TunnelErrorCode::SpawnFailed)?;
            self.control.pid.store(child.id(), Ordering::SeqCst);
            Ok(Box::new(ExitProcessTunnelResource {
                child: Some(child),
                control: Arc::clone(&self.control),
            }))
        }
    }

    #[cfg(unix)]
    struct ExitProcessTunnelResource {
        child: Option<Child>,
        control: Arc<ExitProcessTunnelControl>,
    }

    #[cfg(unix)]
    impl ssh_tunnel::TunnelResource for ExitProcessTunnelResource {
        fn local_port(&self) -> u16 {
            19051
        }

        fn is_alive(&mut self) -> Result<bool, ssh_tunnel::TunnelFailure> {
            self.child
                .as_mut()
                .expect("fixture child missing")
                .try_wait()
                .map(|status| status.is_none())
                .map_err(|_| ssh_tunnel::TunnelErrorCode::WaitFailed.into())
        }

        fn close(&mut self) -> Result<(), ssh_tunnel::TunnelFailure> {
            let Some(mut child) = self.child.take() else {
                return Ok(());
            };
            if child
                .try_wait()
                .map_err(|_| ssh_tunnel::TunnelErrorCode::WaitFailed)?
                .is_none()
            {
                child
                    .kill()
                    .map_err(|_| ssh_tunnel::TunnelErrorCode::KillFailed)?;
                child
                    .wait()
                    .map_err(|_| ssh_tunnel::TunnelErrorCode::WaitFailed)?;
            }
            self.control.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[cfg(unix)]
    impl Drop for ExitProcessTunnelResource {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn final_exit_cleanup_reaps_real_tunnel_child_after_direct_macos_exit() {
        let control = Arc::new(ExitProcessTunnelControl {
            pid: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });
        let tunnels = ssh_tunnel::TunnelState::with_factory(Arc::new(ExitProcessTunnelFactory {
            control: Arc::clone(&control),
        }));
        tunnels
            .get_or_create("direct-exit", "fixture-host", 9000)
            .unwrap();
        let sync = sync::SyncState::new();
        let credentials = credentials::CredentialState::with_backend_and_deadlines(
            Arc::new(ExitCredentialBackend),
            credentials::ProvisionDeadlines::default(),
        );
        let lifecycle = ExitLifecycle::default();
        let pid = control.pid.load(Ordering::SeqCst);
        assert!(
            Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "fixture process was not alive before final exit cleanup"
        );

        let outcome = cleanup_after_final_event(
            &lifecycle,
            &credentials,
            &sync,
            &tunnels,
            Duration::from_secs(5),
        );

        assert_eq!(outcome, ExitCleanupOutcome::Succeeded);
        assert!(control.closed.load(Ordering::SeqCst));
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "final exit cleanup left the real tunnel process alive or unreaped"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_exit_graceful_timeout_still_reaps_real_tunnel_child() {
        let control = Arc::new(ExitProcessTunnelControl {
            pid: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });
        let tunnels = ssh_tunnel::TunnelState::with_factory(Arc::new(ExitProcessTunnelFactory {
            control: Arc::clone(&control),
        }));
        tunnels
            .get_or_create("timed-out-final-exit", "fixture-host", 9000)
            .unwrap();
        let pid = control.pid.load(Ordering::SeqCst);
        assert!(
            Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "fixture process was not alive before timed final cleanup"
        );

        let outcome = cleanup_final_resources(
            &tunnels,
            Duration::from_millis(10),
            std::future::pending::<bool>(),
        )
        .await;

        assert_eq!(outcome, ExitCleanupOutcome::TimedOut);
        assert!(control.closed.load(Ordering::SeqCst));
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "graceful timeout skipped the real tunnel process cleanup"
        );
    }

    #[test]
    fn exit_lifecycle_prevents_repeated_exit_and_reissues_once_after_retry() {
        let lifecycle = ExitLifecycle::default();

        assert_eq!(lifecycle.request(None), ExitDecision::StartCleanup(0));
        assert_eq!(lifecycle.request(None), ExitDecision::Prevent);
        assert_eq!(lifecycle.finish_cleanup(false), None);
        assert_eq!(lifecycle.request(None), ExitDecision::StartCleanup(0));
        assert_eq!(lifecycle.finish_cleanup(true), Some(0));
        assert_eq!(
            lifecycle.finish_cleanup(true),
            None,
            "cleanup completion reissued exit more than once"
        );
        assert_eq!(lifecycle.request(Some(0)), ExitDecision::Allow);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_cleanup_retains_failed_tunnel_and_reissues_only_after_retry_reaps_it() {
        let control = Arc::new(ExitTunnelControl {
            creates: AtomicUsize::new(0),
            close_failures: AtomicUsize::new(1),
            close_attempts: Mutex::new(Vec::new()),
            successful_closes: AtomicUsize::new(0),
            dropped_unclosed: AtomicUsize::new(0),
        });
        let tunnels = ssh_tunnel::TunnelState::with_factory(Arc::new(ExitTunnelFactory {
            control: Arc::clone(&control),
        }));
        tunnels
            .get_or_create("onboarding:fixture-host", "fixture-host", 9000)
            .unwrap();
        let sync = sync::SyncState::new();
        let credentials = credentials::CredentialState::with_backend_and_deadlines(
            Arc::new(ExitCredentialBackend),
            credentials::ProvisionDeadlines::default(),
        );
        let lifecycle = ExitLifecycle::default();

        assert_eq!(lifecycle.request(None), ExitDecision::StartCleanup(0));
        assert!(!shutdown_for_exit(&credentials, &sync, &tunnels).await);
        assert_eq!(lifecycle.finish_cleanup(false), None);
        assert_eq!(control.creates.load(Ordering::SeqCst), 1);
        assert_eq!(control.successful_closes.load(Ordering::SeqCst), 0);
        assert_eq!(control.dropped_unclosed.load(Ordering::SeqCst), 0);
        assert_eq!(*control.close_attempts.lock().unwrap(), vec![73]);

        assert_eq!(lifecycle.request(None), ExitDecision::StartCleanup(0));
        assert!(shutdown_for_exit(&credentials, &sync, &tunnels).await);
        assert_eq!(lifecycle.finish_cleanup(true), Some(0));
        assert_eq!(control.creates.load(Ordering::SeqCst), 1);
        assert_eq!(control.successful_closes.load(Ordering::SeqCst), 1);
        assert_eq!(control.dropped_unclosed.load(Ordering::SeqCst), 0);
        assert_eq!(*control.close_attempts.lock().unwrap(), vec![73, 73]);
        assert_eq!(lifecycle.request(Some(0)), ExitDecision::Allow);
        assert_eq!(
            cleanup_after_final_event(
                &lifecycle,
                &credentials,
                &sync,
                &tunnels,
                Duration::from_millis(100),
            ),
            ExitCleanupOutcome::Skipped
        );
        assert_eq!(
            *control.close_attempts.lock().unwrap(),
            vec![73, 73],
            "final event fallback repeated an already successful cleanup"
        );
    }
}
