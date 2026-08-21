//! First-sync and background continue.
//!
//! Tests use an in-process fixture copy. Production writes an agent config,
//! opens an SSH tunnel, and spawns the bundled sidecar. Jobs are keyed by
//! project so switching Codex / Claude tabs does not stop them.

use crate::claude_files::expand_local_root;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const SYNC_PROGRESS_EVENT: &str = "lumio://claude-sync-progress";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub ok: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

pub fn stopped_progress(project_id: &str, files: u32) -> SyncProgress {
    SyncProgress {
        files_done: files,
        files_total: files.max(1),
        project_id: Some(project_id.to_string()),
        running: Some(false),
        error_code: Some("SYNC_FAILED".into()),
    }
}

pub fn count_files(root: &Path) -> u32 {
    count_files_filtered(root, false)
}

pub fn count_project_files(root: &Path) -> u32 {
    count_files_filtered(root, true)
}

fn count_files_filtered(root: &Path, skip_sync_state: bool) -> u32 {
    fn walk(path: &Path, total: &mut u32, skip_sync_state: bool) {
        let Ok(read) = std::fs::read_dir(path) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            if skip_sync_state && name == ".bestcodex-sync" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, total, skip_sync_state);
            } else {
                *total += 1;
            }
        }
    }
    let mut total = 0;
    walk(root, &mut total, skip_sync_state);
    total
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstSyncConfirmation {
    pub files_done: u32,
    pub files_total: u32,
    pub confirmed: bool,
}

pub fn confirm_copy_from_counts(
    local_after: u32,
    remote_total: Option<u32>,
    local_before: u32,
) -> FirstSyncConfirmation {
    let transferred = local_after.saturating_sub(local_before);
    let files_total = remote_total.unwrap_or(local_after);
    let confirmed = match remote_total {
        Some(0) => true,
        Some(total) => transferred > 0 && local_after >= total,
        None => transferred > 0,
    };
    FirstSyncConfirmation {
        files_done: if confirmed { local_after } else { transferred },
        files_total,
        confirmed,
    }
}

pub fn first_sync_from_sidecar(
    sidecar_available: bool,
    confirmation: FirstSyncConfirmation,
) -> SyncOutcome {
    if !sidecar_available {
        return SyncOutcome {
            ok: false,
            files_done: 0,
            files_total: 0,
            error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
        };
    }
    if confirmation.confirmed {
        return SyncOutcome {
            ok: true,
            files_done: confirmation.files_done,
            files_total: confirmation.files_total.max(confirmation.files_done),
            error_code: None,
        };
    }
    SyncOutcome {
        ok: false,
        files_done: confirmation.files_done,
        files_total: confirmation.files_total,
        error_code: Some("SYNC_COPY_UNCONFIRMED".into()),
    }
}

pub fn confirm_timeout() -> Duration {
    let ms = std::env::var("BESTCODEX_CLAUDE_SYNC_CONFIRM_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60_000);
    Duration::from_millis(ms)
}

pub fn wait_for_confirmed_copy(
    local_root: &Path,
    baseline: u32,
    remote_total: Option<u32>,
    timeout: Duration,
    poll: Duration,
) -> FirstSyncConfirmation {
    if matches!(remote_total, Some(0)) {
        return confirm_copy_from_counts(count_project_files(local_root), remote_total, baseline);
    }
    let started = Instant::now();
    loop {
        let confirmation =
            confirm_copy_from_counts(count_project_files(local_root), remote_total, baseline);
        if confirmation.confirmed || started.elapsed() >= timeout {
            return confirmation;
        }
        thread::sleep(poll);
    }
}

pub fn copy_tree_with_progress(
    src: &Path,
    dest: &Path,
    mut on_progress: impl FnMut(u32, u32),
) -> Result<SyncOutcome, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("没能创建本机项目目录：{e}"))?;
    let total = count_files(src);
    let mut done = 0u32;
    on_progress(0, total);
    copy_walk(src, dest, &mut done, total, &mut on_progress)?;
    on_progress(done, total);
    Ok(SyncOutcome {
        ok: true,
        files_done: done,
        files_total: total,
        error_code: None,
    })
}

fn copy_walk(
    src: &Path,
    dest: &Path,
    done: &mut u32,
    total: u32,
    on_progress: &mut impl FnMut(u32, u32),
) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("没能创建本机项目目录：{e}"))?;
    let read = std::fs::read_dir(src).map_err(|e| format!("读不了同步源：{e}"))?;
    for entry in read.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_walk(&from, &to, done, total, on_progress)?;
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("没能创建本机项目目录：{e}"))?;
            }
            std::fs::copy(&from, &to).map_err(|e| format!("没能拷贝文件：{e}"))?;
            *done += 1;
            on_progress(*done, total);
        }
    }
    Ok(())
}

pub fn fixture_root() -> Option<PathBuf> {
    std::env::var_os("BESTCODEX_CLAUDE_SYNC_FIXTURE").map(PathBuf::from)
}

fn is_real_sidecar(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() > 1024)
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeAction {
    AlreadyRunning,
    StartSidecarAndTunnel,
    EngineUnavailable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOutcome {
    pub ok: bool,
    pub running: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub error_code: Option<String>,
}

pub fn plan_resume_sync(sidecar_available: bool, sidecar_running: bool) -> ResumeAction {
    if sidecar_running {
        ResumeAction::AlreadyRunning
    } else if sidecar_available {
        ResumeAction::StartSidecarAndTunnel
    } else {
        ResumeAction::EngineUnavailable
    }
}

pub fn execute_resume<F>(action: ResumeAction, mut start_engine: F) -> ResumeOutcome
where
    F: FnMut() -> Result<(), String>,
{
    match action {
        ResumeAction::AlreadyRunning => ResumeOutcome {
            ok: true,
            running: true,
            files_done: 0,
            files_total: 0,
            error_code: None,
        },
        ResumeAction::EngineUnavailable => ResumeOutcome {
            ok: false,
            running: false,
            files_done: 0,
            files_total: 0,
            error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
        },
        ResumeAction::StartSidecarAndTunnel => match start_engine() {
            Ok(()) => ResumeOutcome {
                ok: true,
                running: true,
                files_done: 0,
                files_total: 0,
                error_code: None,
            },
            Err(_) => ResumeOutcome {
                ok: false,
                running: false,
                files_done: 0,
                files_total: 0,
                error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
            },
        },
    }
}

#[cfg(test)]
pub fn listed_remote_does_not_confirm_sync(listed_remote: u32) -> FirstSyncConfirmation {
    confirm_copy_from_counts(0, Some(listed_remote), 0)
}

pub fn sidecar_command() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("BESTCODEX_CLAUDE_SIDECAR") {
        let path = PathBuf::from(explicit);
        return is_real_sidecar(&path).then_some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    let name = if cfg!(windows) {
        "fns-agent.exe"
    } else {
        "fns-agent"
    };
    let candidate = parent.join(name);
    is_real_sidecar(&candidate).then_some(candidate)
}

pub fn write_agent_config(
    state_dir: &Path,
    local_root: &Path,
    endpoint: &str,
    workspace_id: &str,
    client_id: &str,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(state_dir).map_err(|e| format!("没能准备同步状态目录：{e}"))?;
    let token_file = state_dir.join("unused-private-pipe-token");
    if !token_file.exists() {
        std::fs::write(&token_file, "bestcodex-local-token\n")
            .map_err(|e| format!("没能准备同步凭据：{e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&token_file)
                .map_err(|e| format!("没能准备同步凭据：{e}"))?
                .permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&token_file, perms);
        }
    }
    let config = serde_json::json!({
        "schemaVersion": "fns-agent-config/1",
        "endpoint": endpoint,
        "workspaceId": workspace_id,
        "clientId": client_id,
        "workspaceRoot": local_root,
        "stateDir": state_dir,
        "tokenFile": token_file,
        "sync": {
            "includes": ["**/*"],
            "excludes": [],
            "protectSecrets": true
        },
        "transport": { "maxActiveTransfers": 2 }
    });
    let path = state_dir.join("agent.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&config).map_err(|e| format!("没能写入同步配置：{e}"))?,
    )
    .map_err(|e| format!("没能写入同步配置：{e}"))?;
    Ok(path)
}

pub fn spawn_sidecar(config_path: &Path) -> Result<std::process::Child, String> {
    let binary = sidecar_command().ok_or("SYNC_ENGINE_UNAVAILABLE")?;
    Command::new(binary)
        .arg("run")
        .arg("--config")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "SYNC_ENGINE_UNAVAILABLE".to_string())
}

#[derive(Default)]
struct JobSlot {
    handle: Option<JoinHandle<()>>,
}

pub struct SyncEngine {
    jobs: Mutex<HashMap<String, Arc<Mutex<JobSlot>>>>,
    sidecars: Arc<Mutex<HashMap<String, std::process::Child>>>,
}

impl SyncEngine {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            sidecars: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn is_running(&self, key: &str) -> bool {
        Self::sidecar_running(&self.sidecars, key)
    }

    pub fn adopt_sidecar(&self, key: &str, config_path: &Path) -> Result<(), String> {
        let child = spawn_sidecar(config_path)?;
        let mut sidecars = self.sidecars.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut previous) = sidecars.insert(key.to_string(), child) {
            let _ = previous.kill();
        }
        Ok(())
    }

    fn sidecar_running(sidecars: &Mutex<HashMap<String, std::process::Child>>, key: &str) -> bool {
        let mut guard = sidecars.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(key) {
            Some(child) => child.try_wait().ok().flatten().is_none(),
            None => false,
        }
    }

    pub fn watch_local_files(
        &self,
        key: &str,
        local_root: String,
        on_progress: impl Fn(SyncProgress) + Send + 'static,
    ) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.contains_key(key) {
            return;
        }
        let slot = Arc::new(Mutex::new(JobSlot::default()));
        let key_owned = key.to_string();
        let sidecars = Arc::clone(&self.sidecars);
        let handle = thread::spawn(move || {
            let root = expand_local_root(&local_root);
            let mut last = 0u32;
            loop {
                let files = count_project_files(&root);
                if files != last {
                    last = files;
                    on_progress(SyncProgress {
                        files_done: files,
                        files_total: files.max(1),
                        project_id: Some(key_owned.clone()),
                        ..SyncProgress::default()
                    });
                }
                if !Self::sidecar_running(&sidecars, &key_owned) {
                    on_progress(stopped_progress(&key_owned, last));
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
        if let Ok(mut guard) = slot.lock() {
            guard.handle = Some(handle);
        }
        jobs.insert(key.to_string(), slot);
    }

    pub fn run_first_sync(
        &self,
        key: &str,
        local_root: &str,
        remote_fixture: Option<&Path>,
        on_progress: impl Fn(SyncProgress) + Send + 'static,
    ) -> SyncOutcome {
        let local = expand_local_root(local_root);
        let fixture = remote_fixture.map(Path::to_path_buf).or_else(fixture_root);
        if let Some(src) = fixture {
            let key = key.to_string();
            return match copy_tree_with_progress(&src, &local, |done, total| {
                on_progress(SyncProgress {
                    files_done: done,
                    files_total: total,
                    project_id: Some(key.clone()),
                    ..SyncProgress::default()
                });
            }) {
                Ok(outcome) => outcome,
                Err(_) => SyncOutcome {
                    ok: false,
                    files_done: 0,
                    files_total: 0,
                    error_code: Some("SYNC_FAILED".into()),
                },
            };
        }

        if sidecar_command().is_none() {
            return SyncOutcome {
                ok: false,
                files_done: 0,
                files_total: 0,
                error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
            };
        }

        let _ = std::fs::create_dir_all(&local);
        SyncOutcome {
            ok: false,
            files_done: 0,
            files_total: 0,
            error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
        }
    }
}

impl Default for SyncEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sync_copies_fixture_files_and_reports_progress() {
        let remote = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(remote.path().join("src")).unwrap();
        std::fs::write(remote.path().join("hello.txt"), "hi\n").unwrap();
        std::fs::write(remote.path().join("src/lib.rs"), "ok\n").unwrap();
        let local = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let result = copy_tree_with_progress(remote.path(), local.path(), |done, total| {
            progress.push((done, total));
        })
        .expect("copy");
        assert!(result.ok);
        assert!(result.files_done >= 1);
        assert_eq!(
            std::fs::read_to_string(local.path().join("hello.txt")).unwrap(),
            "hi\n"
        );
        assert!(local.path().join("src/lib.rs").exists());
        assert!(progress.iter().any(|(done, total)| *done > 0 && *total > 0));
        assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[test]
    fn first_sync_engine_copies_fixture_into_local_root() {
        let remote = tempfile::tempdir().unwrap();
        std::fs::write(remote.path().join("hello.txt"), "hi\n").unwrap();
        let local = tempfile::tempdir().unwrap();
        let engine = SyncEngine::new();
        let progress = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_cb = progress.clone();
        let outcome = engine.run_first_sync(
            "p-docs",
            &local.path().to_string_lossy(),
            Some(remote.path()),
            move |item| {
                progress_cb
                    .lock()
                    .unwrap()
                    .push((item.files_done, item.files_total));
            },
        );
        assert!(outcome.ok);
        assert!(outcome.files_done >= 1);
        assert_eq!(
            std::fs::read_to_string(local.path().join("hello.txt")).unwrap(),
            "hi\n"
        );
        let seen = progress.lock().unwrap().clone();
        assert!(seen.iter().any(|(done, total)| *done > 0 && *total > 0));
    }

    #[test]
    fn missing_sidecar_is_not_a_successful_sync() {
        let engine = SyncEngine::new();
        let local = tempfile::tempdir().unwrap();
        let outcome = engine.run_first_sync("p1", &local.path().to_string_lossy(), None, |_| {});
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("SYNC_ENGINE_UNAVAILABLE")
        );
        assert_eq!(outcome.files_done, 0);
    }

    #[test]
    fn sidecar_spawn_without_confirmed_copy_is_not_success() {
        let outcome = first_sync_from_sidecar(
            true,
            FirstSyncConfirmation {
                files_done: 0,
                files_total: 0,
                confirmed: false,
            },
        );
        assert!(!outcome.ok);
        assert_ne!(
            outcome.error_code.as_deref(),
            Some("SYNC_ENGINE_UNAVAILABLE")
        );
        assert_eq!(outcome.files_done, 0);
    }

    #[test]
    fn existing_local_files_are_not_a_confirmed_remote_copy() {
        let confirmation = confirm_copy_from_counts(3, None, 3);
        assert!(!confirmation.confirmed);
        let outcome = first_sync_from_sidecar(true, confirmation);
        assert!(!outcome.ok);
    }

    #[test]
    fn confirmed_remote_to_local_copy_is_success() {
        let confirmation = confirm_copy_from_counts(2, Some(2), 0);
        assert!(confirmation.confirmed);
        assert_eq!(confirmation.files_done, 2);
        let outcome = first_sync_from_sidecar(true, confirmation);
        assert!(outcome.ok);
        assert_eq!(outcome.files_done, 2);
        assert!(outcome.error_code.is_none());
    }

    #[test]
    fn missing_sidecar_stays_engine_unavailable() {
        let outcome = first_sync_from_sidecar(
            false,
            FirstSyncConfirmation {
                files_done: 0,
                files_total: 0,
                confirmed: false,
            },
        );
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("SYNC_ENGINE_UNAVAILABLE")
        );
    }

    #[test]
    fn project_file_count_ignores_sync_state_dir() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".bestcodex-sync")).unwrap();
        std::fs::write(root.path().join(".bestcodex-sync/agent.json"), "{}\n").unwrap();
        std::fs::write(root.path().join("readme.md"), "hi\n").unwrap();
        assert_eq!(count_project_files(root.path()), 1);
    }

    #[test]
    fn listed_remote_files_are_not_a_confirmed_sync() {
        let confirmation = listed_remote_does_not_confirm_sync(12);
        assert!(!confirmation.confirmed);
        let outcome = first_sync_from_sidecar(true, confirmation);
        assert!(!outcome.ok);
        assert_eq!(outcome.error_code.as_deref(), Some("SYNC_COPY_UNCONFIRMED"));
    }

    #[test]
    fn resume_of_a_stopped_engine_starts_sidecar_and_tunnel() {
        let mut started = false;
        let action = plan_resume_sync(true, false);
        assert_eq!(action, ResumeAction::StartSidecarAndTunnel);
        let outcome = execute_resume(action, || {
            started = true;
            Ok(())
        });
        assert!(started, "resume must start the sidecar/tunnel, not no-op");
        assert!(outcome.ok);
        assert!(outcome.running);
    }

    #[test]
    fn resume_without_sidecar_is_engine_unavailable_not_success() {
        let mut started = false;
        let outcome = execute_resume(plan_resume_sync(false, false), || {
            started = true;
            Ok(())
        });
        assert!(!started);
        assert!(!outcome.ok);
        assert!(!outcome.running);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("SYNC_ENGINE_UNAVAILABLE")
        );
    }

    #[test]
    fn stopped_progress_is_not_a_running_engine() {
        let progress = stopped_progress("p-docs", 3);
        assert_eq!(progress.running, Some(false));
        assert_eq!(progress.error_code.as_deref(), Some("SYNC_FAILED"));
        assert_eq!(progress.project_id.as_deref(), Some("p-docs"));
    }

    #[test]
    fn resume_already_running_reports_running() {
        let mut started = false;
        let outcome = execute_resume(plan_resume_sync(true, true), || {
            started = true;
            Ok(())
        });
        assert!(!started);
        assert!(outcome.ok);
        assert!(outcome.running);
    }
}
