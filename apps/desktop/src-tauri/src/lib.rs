//! FNS Workspace Desktop — Tauri 2 backend.
//!
//! Provides project configuration, SSH host parsing, and deployment orchestration
//! commands for the macOS desktop application.

mod credentials;
mod deploy;
mod diagnostics;
mod files;
mod project;
mod ssh;
mod ssh_tunnel;
mod sync;
mod terminal;

use project::ProjectConfig;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;

const FINAL_EXIT_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(120);

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

/// Learn Tauri command — returns a greeting (placeholder for onboarding).
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! Welcome to FNS Workspace.")
}

/// Save a project configuration.
#[tauri::command]
fn save_project(config: ProjectConfig) -> Result<String, String> {
    let id = config.id.to_string();
    config
        .save_to_default()
        .map_err(|e| format!("Failed to save project: {e}"))?;
    Ok(id)
}

/// List all saved projects.
#[tauri::command]
fn list_projects() -> Result<Vec<ProjectConfig>, String> {
    ProjectConfig::list_all().map_err(|e| format!("Failed to list projects: {e}"))
}

/// Delete a project by ID.
#[tauri::command]
fn delete_project(id: String) -> Result<(), String> {
    ProjectConfig::delete(&id).map_err(|e| format!("Failed to delete project: {e}"))
}

/// Parse SSH config to discover available hosts.
#[tauri::command]
fn parse_ssh_hosts() -> Result<Vec<ssh::SshHost>, String> {
    ssh::parse_ssh_config().map_err(|e| format!("Failed to parse SSH config: {e}"))
}

/// App configuration payload from the onboarding wizard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRequest {
    pub project_name: String,
    pub ssh_host_alias: String,
    pub remote_root: String,
    pub local_root: String,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub protect_secrets: bool,
}

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
#[cfg_attr(target_os = "android", tauri::mobile_entry_point)]
pub fn run() {
    let credential_state = credentials::CredentialState::production();
    let sync_state = sync::SyncState::with_credentials(Arc::new(credential_state.clone()));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(terminal::TerminalManager::new())
        .manage(ssh_tunnel::TunnelState::new())
        .manage(deploy::DeployState::production())
        .manage(credential_state)
        .manage(sync_state)
        .manage(diagnostics::DiagnosticsState::default())
        .invoke_handler(tauri::generate_handler![
            greet,
            save_project,
            list_projects,
            delete_project,
            parse_ssh_hosts,
            diagnostics::diagnostics_list_events,
            diagnostics::diagnostics_get_health,
            diagnostics::diagnostics_preview_support_bundle,
            diagnostics::diagnostics_export_support_bundle,
            diagnostics::diagnostics_run_self_test,
            diagnostics::diagnostics_cancel_self_test,
            terminal::start_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::close_terminal,
            terminal::new_claude_session,
            terminal::close_tmux_window,
            terminal::list_tmux_windows,
            terminal::kill_all_sessions,
            files::browse_files,
            files::read_file,
            files::compute_diff,
            files::open_in_finder,
            ssh_tunnel::create_tunnel,
            ssh_tunnel::tunnel_endpoint,
            ssh_tunnel::close_tunnel,
            credentials::provision_workspace_credential,
            credentials::reprovision_workspace_credential,
            credentials::workspace_credential_status,
            credentials::probe_workspace_access,
            credentials::delete_workspace_credential,
            credentials::cancel_workspace_provisioning,
            credentials::retry_workspace_credential_cleanup,
            credentials::workspace_credential_cleanup_status,
            deploy::preview_remote_deployment,
            deploy::execute_remote_deployment,
            deploy::cancel_remote_deployment,
            sync::start_sync,
            sync::stop_sync,
            sync::sync_status,
            sync::list_sync_conflicts,
            sync::resolve_sync_conflict,
            sync::cancel_sync_conflict_request,
            sync::cancel_sync_conflict_generation,
            sync::list_sync_conflict_operations,
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
