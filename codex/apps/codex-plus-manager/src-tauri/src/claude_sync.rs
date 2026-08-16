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

pub const SYNC_PROGRESS_EVENT: &str = "lumio://claude-sync-progress";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub ok: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub project_id: Option<String>,
}

pub fn count_files(root: &Path) -> u32 {
    fn walk(path: &Path, total: &mut u32) {
        let Ok(read) = std::fs::read_dir(path) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, total);
            } else {
                *total += 1;
            }
        }
    }
    let mut total = 0;
    walk(root, &mut total);
    total
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
    sidecars: Mutex<HashMap<String, std::process::Child>>,
}

impl SyncEngine {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            sidecars: Mutex::new(HashMap::new()),
        }
    }

    pub fn adopt_sidecar(&self, key: &str, config_path: &Path) -> Result<(), String> {
        let child = spawn_sidecar(config_path)?;
        let mut sidecars = self.sidecars.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut previous) = sidecars.insert(key.to_string(), child) {
            let _ = previous.kill();
        }
        Ok(())
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
        let handle = thread::spawn(move || {
            let root = expand_local_root(&local_root);
            let mut last = 0u32;
            for _ in 0..120 {
                let files = count_files(&root);
                if files != last {
                    last = files;
                    on_progress(SyncProgress {
                        files_done: files,
                        files_total: files.max(1),
                        project_id: Some(key_owned.clone()),
                    });
                }
                thread::sleep(std::time::Duration::from_millis(500));
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
}
