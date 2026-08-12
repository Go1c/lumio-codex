//! CC避风港（CCHaven）desktop backend — Tauri 2.
//!
//! Command surface for the macOS app: browser-based account access, project
//! configuration and deployment, the local sync folder explorer, conflict
//! resolution, and the embedded terminal.

pub mod askpass;
pub mod auth;
pub mod conflicts;
pub mod control;
pub mod deploy;
pub mod files;
pub mod project;
pub mod ssh;
pub mod sync;
pub mod terminal;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use conflicts::{Conflict, ConflictStore, Resolution, ResolutionReceipt};
use files::{EntryKind, FileNode, FilePreview, TrashTicket};
use project::ProjectConfig;
use sync::{SyncManager, SyncStatus};

/// Startup facts the frontend needs before it renders anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    /// True when the control plane is mocked (see `README.md`).
    pub mock_control: bool,
    pub links: ExternalLinks,
}

/// The four ↗ destinations of the account menu (5.6). All open in the browser.
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
    sync: tauri::State<'_, SyncManager>,
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
    // Best-effort: local engine + watcher come up even when no agent endpoint yet.
    let _ = sync.ensure_open(&config).await;
    Ok(config)
}

/// Remove a project from the app. Nothing on disk or on the server is deleted.
#[tauri::command]
async fn delete_project(
    project_id: String,
    auth: tauri::State<'_, auth::AuthState>,
    sync: tauri::State<'_, SyncManager>,
) -> Result<(), String> {
    let _ = sync.close(&project_id).await;
    let _ = auth.secrets().clear_ssh_password(&project_id);
    let _ = auth.secrets().clear_sync_agent_token(&project_id);
    ProjectConfig::delete(&project_id).map_err(|e| format!("无法删除项目：{e}"))
}

#[tauri::command]
fn project_presets(name: String, user: String) -> ProjectPresets {
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~"));
    ProjectPresets {
        remote_root: project::default_remote_root(&user, &name),
        local_root: project::default_local_root(&home, &name),
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

// --- Deployment ---

#[tauri::command]
async fn deploy_project(
    app: tauri::AppHandle,
    project_id: String,
    from_stage: Option<usize>,
    auth: tauri::State<'_, auth::AuthState>,
) -> Result<(), deploy::DeployError> {
    let config = ProjectConfig::get(&project_id)
        .map_err(|e| deploy::DeployError {
            stage: deploy::Stage::Connect,
            message: format!("无法读取项目配置：{e}"),
        })?
        .ok_or(deploy::DeployError {
            stage: deploy::Stage::Connect,
            message: "项目不存在。".into(),
        })?;
    let password = auth.secrets().ssh_password(&project_id).ok().flatten();

    deploy::run(
        &app,
        &config,
        password.as_deref(),
        deploy::Stage::from_index(from_stage.unwrap_or(0)),
    )
    .await
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

#[tauri::command]
async fn create_entry(
    project_id: String,
    parent: String,
    name: String,
    kind: EntryKind,
    sync: tauri::State<'_, SyncManager>,
) -> Result<String, String> {
    let path = files::create_entry(&local_root_of(&project_id)?, &parent, &name, kind)?;
    if let Ok(changes) = sync::changes_for_create(&path) {
        let _ = sync.record_paths(&project_id, changes).await;
    }
    Ok(path)
}

#[tauri::command]
async fn rename_entry(
    project_id: String,
    path: String,
    new_name: String,
    sync: tauri::State<'_, SyncManager>,
) -> Result<String, String> {
    let new_path = files::rename_entry(&local_root_of(&project_id)?, &path, &new_name)?;
    if let Ok(changes) = sync::changes_for_rename(&path, &new_path) {
        let _ = sync.record_paths(&project_id, changes).await;
    }
    Ok(new_path)
}

#[tauri::command]
async fn delete_entry(
    project_id: String,
    path: String,
    sync: tauri::State<'_, SyncManager>,
) -> Result<TrashTicket, String> {
    let ticket = files::delete_entry(&local_root_of(&project_id)?, &path, &staging_dir()?)?;
    if let Ok(changes) = sync::changes_for_delete(&path) {
        let _ = sync.record_paths(&project_id, changes).await;
    }
    Ok(ticket)
}

#[tauri::command]
async fn undo_delete(
    project_id: String,
    token: String,
    sync: tauri::State<'_, SyncManager>,
) -> Result<String, String> {
    let path = files::restore_entry(&local_root_of(&project_id)?, &staging_dir()?, &token)?;
    if let Ok(changes) = sync::changes_for_create(&path) {
        let _ = sync.record_paths(&project_id, changes).await;
    }
    Ok(path)
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

// --- Conflicts ---

fn conflict_store(project_id: &str) -> Result<ConflictStore, String> {
    ConflictStore::new(&conflicts_dir()?, project_id)
}

#[tauri::command]
async fn list_conflicts(
    project_id: String,
    auth: tauri::State<'_, auth::AuthState>,
    sync: tauri::State<'_, SyncManager>,
) -> Result<Vec<Conflict>, String> {
    let store = conflict_store(&project_id)?;
    // Engine rows win when present. An empty engine list falls through so mock
    // mode can still seed UI samples before any real conflict exists.
    if let Some(engine_conflicts) = sync.list_engine_conflicts(&project_id).await?
        && !engine_conflicts.is_empty()
    {
        store.replace(engine_conflicts.clone())?;
        return Ok(engine_conflicts);
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

fn seeded_marker(project_id: &str) -> Result<PathBuf, String> {
    Ok(conflicts_dir()?.join(format!("{project_id}.seeded")))
}

#[tauri::command]
async fn resolve_conflict(
    project_id: String,
    conflict_id: String,
    resolution: Resolution,
    sync: tauri::State<'_, SyncManager>,
) -> Result<ResolutionReceipt, String> {
    let root = local_root_of(&project_id)?;
    let receipt = conflict_store(&project_id)?.resolve(&root, &conflict_id, resolution)?;
    let _ = sync
        .resolve_engine_conflict(&project_id, &conflict_id, resolution)
        .await;
    Ok(receipt)
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

// --- Sync status ---

/// Derive the 6.3 status for a project from the in-process sync session.
#[tauri::command]
async fn sync_status(
    project_id: String,
    sync: tauri::State<'_, SyncManager>,
) -> Result<SyncStatus, String> {
    sync.status(&project_id).await
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

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
#[cfg_attr(target_os = "android", tauri::mobile_entry_point)]
pub fn run() {
    let credential_state = credentials::CredentialState::production();
    let sync_state = sync::SyncState::with_credentials(Arc::new(credential_state.clone()));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            app.manage(auth::AuthState::from_env());
            app.manage(terminal::TerminalManager::new());
            app.manage(SyncManager::from_env().map_err(std::io::Error::other)?);
            Ok(())
        })
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
            deploy_project,
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
            // conflicts
            list_conflicts,
            resolve_conflict,
            undo_conflict,
            forget_conflict_undo,
            conflict_diff,
            sync_status,
            // terminal
            terminal::start_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::close_terminal,
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
