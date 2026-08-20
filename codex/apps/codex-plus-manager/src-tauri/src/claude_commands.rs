//! Claude Tab commands: probe, deploy, first-sync, PTY, files, conflicts.
//!
//! No `fns-*` crates. The sidecar is spawned as a binary; passwords never go
//! on argv and are never written to logs.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::claude_conflicts::{self, ConflictStore, Resolution};
use crate::claude_deploy;
use crate::claude_files::{self, expand_local_root};
use crate::claude_remote;
use crate::claude_ssh::{
    ResolvedSshTarget, SshHost, parse_ssh_config, remote_shell_join, remote_shell_path,
    resolve_from_user_config, ssh_invocation_args,
};
use crate::claude_sync::{self, SYNC_PROGRESS_EVENT, SyncEngine, SyncProgress};
use crate::claude_terminal::{TerminalManager, close_remote_session_command};
use crate::claude_tunnel::TunnelManager;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const BANNER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCommandResult<T> {
    pub ok: bool,
    pub error_code: Option<String>,
    pub payload: Option<T>,
}

impl<T> ClaudeCommandResult<T> {
    fn ok(payload: T) -> Self {
        Self {
            ok: true,
            error_code: None,
            payload: Some(payload),
        }
    }

    fn failed(code: &str) -> Self {
        Self {
            ok: false,
            error_code: Some(code.to_string()),
            payload: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeProbePayload {
    pub ok: bool,
    pub reachable: bool,
    pub authenticated: bool,
    pub distro: Option<String>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudePreparePayload {
    pub ok: bool,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeInspectPayload {
    pub ok: bool,
    pub exists: bool,
    pub names: Vec<String>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSyncPayload {
    pub ok: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub error_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRemoteOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

pub(crate) fn ssh_base_args(
    host: &str,
    user: &str,
    port: u16,
    key_path: Option<&str>,
) -> Vec<String> {
    let target = ResolvedSshTarget {
        host: host.to_string(),
        user: user.to_string(),
        port,
        alias: None,
        use_config: false,
        identity_file: None,
    };
    ssh_invocation_args(&target, key_path, None)
}

fn classify_ssh_error(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("auth fail")
        || lower.contains("authentication failed")
    {
        "SSH_AUTH_FAILED"
    } else if lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("no route")
        || lower.contains("network is unreachable")
    {
        "SSH_UNREACHABLE"
    } else if lower.contains("host key") {
        "SSH_HOST_KEY"
    } else if lower.contains("not found") || lower.contains("no such file") {
        "SSH_CLIENT_MISSING"
    } else {
        "SSH_PROBE_FAILED"
    }
}

fn probe_banner(host: &str, port: u16) -> Result<String, &'static str> {
    let address = format!("{host}:{port}");
    let mut addrs = address.to_socket_addrs().map_err(|_| "SSH_UNREACHABLE")?;
    let addr = addrs.next().ok_or("SSH_UNREACHABLE")?;
    let mut stream =
        TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).map_err(|_| "SSH_UNREACHABLE")?;
    let _ = stream.set_read_timeout(Some(BANNER_TIMEOUT));
    let mut buf = [0u8; 128];
    let read = stream.read(&mut buf).map_err(|_| "SSH_NOT_SSH")?;
    if read == 0 {
        return Err("SSH_NOT_SSH");
    }
    let banner = String::from_utf8_lossy(&buf[..read]);
    if banner.starts_with("SSH-") {
        Ok(banner.trim().to_string())
    } else {
        Err("SSH_NOT_SSH")
    }
}

fn run_ssh(
    host: &str,
    user: &str,
    port: u16,
    password: Option<&str>,
    key_path: Option<&str>,
    host_alias: Option<&str>,
    remote: &str,
) -> Result<std::process::Output, &'static str> {
    let target = resolve_from_user_config(host, Some(user), port, host_alias)?;
    run_ssh_target(&target, password, key_path, remote)
}

fn ssh_command_args(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote: &str,
) -> Vec<String> {
    let key = crate::claude_ssh::effective_key_path(key_path, target);
    let plan = crate::claude_ssh::password_auth_plan(password, key, target.use_config);
    let mut args = ssh_invocation_args(target, key, None);
    if plan.batch_mode && !args.iter().any(|arg| arg == "BatchMode=yes") {
        let dest = args.pop().expect("ssh destination");
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args.push(dest);
    }
    args.push(remote.to_string());
    args
}

fn run_ssh_target(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote: &str,
) -> Result<std::process::Output, &'static str> {
    let key = crate::claude_ssh::effective_key_path(key_path, target);
    let args = ssh_command_args(target, password, key_path, remote);
    let mut command = Command::new("ssh");
    command.args(&args);
    command.env("LC_ALL", "C");
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let askpass =
        crate::claude_ssh::attach_askpass(&mut command, password, key, target.use_config)?;
    let output = command.output().map_err(|_| "SSH_CLIENT_MISSING")?;
    drop(askpass);
    Ok(output)
}

fn parse_system_info(stdout: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut distro = None;
    let mut cpu = None;
    let mut memory = None;
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("DISTRO:") {
            distro = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("CPU:") {
            cpu = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("MEM:") {
            memory = Some(value.trim().to_string());
        }
    }
    (distro, cpu, memory)
}

fn human_detail(code: &str, host: &str, port: u16) -> String {
    match code {
        "SSH_AUTH_FAILED" => format!("无法登录 {host}。"),
        "SSH_UNREACHABLE" => format!("连不上 {host}:{port}。"),
        "SSH_NOT_SSH" => format!("{host}:{port} 不是 SSH 服务。"),
        "SSH_CLIENT_MISSING" => "这台电脑还没有 ssh 命令。".into(),
        "SSH_ALIAS_UNKNOWN" => "本机 SSH 配置里没有这个 Host 别名。".into(),
        "DEPLOY_ARTIFACT_MISSING" => {
            return claude_deploy::human_prepare_detail(code, host, port);
        }
        "SSH_PREPARE_FAILED" => "没能在服务器上装好同步组件。".into(),
        "SYNC_COPY_UNCONFIRMED" => "还没把服务器上的文件拉到这台电脑。".into(),
        _ => "连不上这台服务器。".into(),
    }
}

fn conflict_dir() -> std::path::PathBuf {
    let base = directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("BestCodex").join("claude-conflicts"))
        .unwrap_or_else(|| std::path::PathBuf::from(".bestcodex-conflicts"));
    let _ = std::fs::create_dir_all(&base);
    base
}

fn count_remote_project_files(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_root: &str,
) -> Option<u32> {
    let output = run_ssh_target(
        target,
        password,
        key_path,
        &format!(
            "find {} -type f ! -path '*/.bestcodex-sync/*' 2>/dev/null | wc -l",
            remote_shell_path(remote_root)
        ),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
}

fn ingest_detected_conflicts(
    project_id: &str,
    local: &std::path::Path,
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_root: &str,
) {
    let Ok(store) = ConflictStore::new(&conflict_dir(), project_id) else {
        return;
    };
    let state_dir = local.join(".bestcodex-sync");
    let _ = claude_conflicts::ingest_sidecar_conflicts(&store, &state_dir);
    let Ok(output) = run_ssh_target(
        target,
        password,
        key_path,
        &format!(
            "find {} -type f ! -path '*/.bestcodex-sync/*' -printf '%P\\n' 2>/dev/null",
            remote_shell_path(remote_root)
        ),
    ) else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let remote_paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('.'))
        .map(str::to_string)
        .take(40)
        .collect();
    let mut pairs = Vec::new();
    for path in remote_paths {
        let local_file = local.join(&path);
        if !local_file.is_file() {
            continue;
        }
        let Ok(preview) = run_ssh_target(
            target,
            password,
            key_path,
            &format!("head -c 1048576 {}", remote_shell_join(remote_root, &path)),
        ) else {
            continue;
        };
        if !preview.status.success() || preview.stdout.iter().take(512).any(|b| *b == 0) {
            continue;
        }
        pairs.push((path, String::from_utf8_lossy(&preview.stdout).into_owned()));
    }
    let detected = claude_conflicts::detect_content_conflicts(local, &pairs);
    if detected.is_empty() {
        return;
    }
    let _ = claude_conflicts::write_sidecar_conflicts(&state_dir, &detected);
    let _ = claude_conflicts::ingest_engine_conflicts(&store, detected);
}

#[tauri::command]
pub fn lumio_claude_probe_connection(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
) -> ClaudeCommandResult<ClaudeProbePayload> {
    let host = host.trim().to_string();
    let user = if user.trim().is_empty() {
        "root".into()
    } else {
        user.trim().to_string()
    };
    let port = if port == 0 { 22 } else { port };
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());

    let target = match resolve_from_user_config(&host, Some(&user), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCommandResult::ok(ClaudeProbePayload {
                ok: false,
                reachable: false,
                authenticated: false,
                distro: None,
                cpu: None,
                memory: None,
                error_code: Some(code.into()),
                detail: Some(human_detail(code, &host, port)),
            });
        }
    };

    if let Err(code) = probe_banner(&target.host, target.port) {
        return ClaudeCommandResult::ok(ClaudeProbePayload {
            ok: false,
            reachable: false,
            authenticated: false,
            distro: None,
            cpu: None,
            memory: None,
            error_code: Some(code.into()),
            detail: Some(human_detail(code, &target.host, target.port)),
        });
    }

    let remote = ". /etc/os-release 2>/dev/null; echo DISTRO:${PRETTY_NAME:-unknown}; nproc >/tmp/.bc-nproc 2>/dev/null; echo CPU:$(nproc 2>/dev/null || echo ?); echo MEM:$(awk '/MemTotal/ {printf \"%.0f GB\", $2/1024/1024}' /proc/meminfo 2>/dev/null)";
    match run_ssh_target(&target, password.as_deref(), key_path.as_deref(), remote) {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let (distro, cpu, memory) = parse_system_info(&stdout);
            ClaudeCommandResult::ok(ClaudeProbePayload {
                ok: true,
                reachable: true,
                authenticated: true,
                distro,
                cpu,
                memory,
                error_code: None,
                detail: None,
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = classify_ssh_error(&stderr);
            ClaudeCommandResult::ok(ClaudeProbePayload {
                ok: false,
                reachable: true,
                authenticated: false,
                distro: None,
                cpu: None,
                memory: None,
                error_code: Some(code.into()),
                detail: Some(human_detail(code, &target.host, target.port)),
            })
        }
        Err(code) => ClaudeCommandResult::ok(ClaudeProbePayload {
            ok: false,
            reachable: true,
            authenticated: false,
            distro: None,
            cpu: None,
            memory: None,
            error_code: Some(code.into()),
            detail: Some(human_detail(code, &target.host, target.port)),
        }),
    }
}

#[tauri::command]
pub async fn lumio_claude_inspect_remote(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
) -> ClaudeCommandResult<ClaudeInspectPayload> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_remote_inner(
            host,
            user,
            port,
            password,
            key_path,
            host_alias,
            remote_root,
        )
    })
    .await
    .unwrap_or_else(|_| ClaudeCommandResult::failed("SSH_PREPARE_FAILED"))
}

fn inspect_remote_inner(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
) -> ClaudeCommandResult<ClaudeInspectPayload> {
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());
    let target = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCommandResult::ok(ClaudeInspectPayload {
                ok: false,
                exists: false,
                names: Vec::new(),
                error_code: Some(code.into()),
                detail: Some(human_detail(code, host.trim(), port)),
            });
        }
    };
    match run_ssh_target(
        &target,
        password.as_deref(),
        key_path.as_deref(),
        &claude_deploy::inspect_remote_script(&remote_root),
    ) {
        Ok(output) if output.status.success() => {
            let (exists, names) =
                claude_deploy::parse_inspect_output(&String::from_utf8_lossy(&output.stdout));
            ClaudeCommandResult::ok(ClaudeInspectPayload {
                ok: true,
                exists,
                names,
                error_code: None,
                detail: None,
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = classify_ssh_error(&stderr);
            ClaudeCommandResult::ok(ClaudeInspectPayload {
                ok: false,
                exists: false,
                names: Vec::new(),
                error_code: Some(code.into()),
                detail: Some(human_detail(code, &target.host, target.port)),
            })
        }
        Err(code) => ClaudeCommandResult::ok(ClaudeInspectPayload {
            ok: false,
            exists: false,
            names: Vec::new(),
            error_code: Some(code.into()),
            detail: Some(human_detail(code, &target.host, target.port)),
        }),
    }
}

#[tauri::command]
pub async fn lumio_claude_prepare_remote(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    local_root: String,
) -> ClaudeCommandResult<ClaudePreparePayload> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_remote_inner(
            app,
            host,
            user,
            port,
            password,
            key_path,
            host_alias,
            remote_root,
            local_root,
        )
    })
    .await
    .unwrap_or_else(|_| ClaudeCommandResult::failed("SSH_PREPARE_FAILED"))
}

fn prepare_remote_inner(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    local_root: String,
) -> ClaudeCommandResult<ClaudePreparePayload> {
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());
    let target = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCommandResult::ok(ClaudePreparePayload {
                ok: false,
                error_code: Some(code.into()),
                detail: Some(human_detail(code, host.trim(), port)),
            });
        }
    };
    let resource_dir = app.path().resource_dir().ok();
    let artifacts = claude_deploy::find_artifacts(resource_dir.as_deref());
    let planned = claude_deploy::prepare_components(
        &target.host,
        &target.user,
        target.port,
        alias,
        &local_root,
        artifacts.as_ref(),
    );
    if !planned.ok {
        return ClaudeCommandResult::ok(ClaudePreparePayload {
            ok: false,
            error_code: planned.error_code,
            detail: planned.detail,
        });
    }
    let Some(artifacts) = artifacts else {
        return ClaudeCommandResult::ok(ClaudePreparePayload {
            ok: false,
            error_code: Some("DEPLOY_ARTIFACT_MISSING".into()),
            detail: Some(human_detail(
                "DEPLOY_ARTIFACT_MISSING",
                &target.host,
                target.port,
            )),
        });
    };
    let app_progress = app.clone();
    let outcome = claude_deploy::deploy_remote(
        &target,
        password.as_deref(),
        key_path.as_deref(),
        &remote_root,
        &artifacts,
        |remote| run_ssh_target(&target, password.as_deref(), key_path.as_deref(), remote),
        |progress| {
            let _ = app_progress.emit(claude_deploy::PREPARE_PROGRESS_EVENT, progress);
        },
    );
    ClaudeCommandResult::ok(ClaudePreparePayload {
        ok: outcome.ok,
        error_code: outcome.error_code,
        detail: outcome.detail,
    })
}

#[tauri::command]
pub fn lumio_claude_first_sync(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    local_root: String,
    project_id: Option<String>,
) -> ClaudeCommandResult<ClaudeSyncPayload> {
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());
    let target = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCommandResult::ok(ClaudeSyncPayload {
                ok: false,
                files_done: 0,
                files_total: 0,
                error_code: Some(code.into()),
            });
        }
    };
    let key = project_id
        .clone()
        .unwrap_or_else(|| format!("{}@{}", target.user, target.host));
    let app_progress = app.clone();
    let watch_progress = app.clone();
    let engine = app.state::<SyncEngine>();
    let outcome = engine.run_first_sync(&key, &local_root, None, move |progress: SyncProgress| {
        let _ = app_progress.emit(SYNC_PROGRESS_EVENT, progress);
    });
    if outcome.ok {
        return ClaudeCommandResult::ok(ClaudeSyncPayload {
            ok: true,
            files_done: outcome.files_done,
            files_total: outcome.files_total,
            error_code: None,
        });
    }
    if outcome.error_code.as_deref() != Some("SYNC_ENGINE_UNAVAILABLE") {
        return ClaudeCommandResult::ok(ClaudeSyncPayload {
            ok: false,
            files_done: outcome.files_done,
            files_total: outcome.files_total,
            error_code: outcome.error_code,
        });
    }

    if claude_sync::sidecar_command().is_none() {
        return ClaudeCommandResult::ok(ClaudeSyncPayload {
            ok: false,
            files_done: outcome.files_done,
            files_total: outcome.files_total,
            error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
        });
    }

    let local = expand_local_root(&local_root);
    let baseline = claude_sync::count_project_files(&local);
    let remote_total = count_remote_project_files(
        &target,
        password.as_deref(),
        key_path.as_deref(),
        &remote_root,
    );
    let spawned = spawn_official_engine(
        &app,
        &engine,
        &key,
        &target,
        key_path.as_deref(),
        password.as_deref(),
        &local_root,
    )
    .is_ok();

    if spawned {
        engine.watch_local_files(&key, local_root.clone(), move |progress| {
            let _ = watch_progress.emit(SYNC_PROGRESS_EVENT, progress);
        });
        let confirmation = claude_sync::wait_for_confirmed_copy(
            &local,
            baseline,
            remote_total,
            claude_sync::confirm_timeout(),
            std::time::Duration::from_millis(250),
        );
        let finished = claude_sync::first_sync_from_sidecar(true, confirmation);
        ingest_detected_conflicts(
            &key,
            &local,
            &target,
            password.as_deref(),
            key_path.as_deref(),
            &remote_root,
        );
        return ClaudeCommandResult::ok(ClaudeSyncPayload {
            ok: finished.ok,
            files_done: finished.files_done,
            files_total: finished.files_total,
            error_code: finished.error_code,
        });
    }

    ClaudeCommandResult::ok(ClaudeSyncPayload {
        ok: false,
        files_done: outcome.files_done,
        files_total: outcome.files_total,
        error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
    })
}

#[tauri::command]
pub fn lumio_claude_open_system_terminal(
    host: String,
    user: String,
    port: u16,
) -> ClaudeCommandResult<()> {
    let target = format!("{}@{}", user.trim(), host.trim());
    let port = if port == 0 { 22 } else { port };
    let ssh = format!("ssh -p {port} {target}");
    let outcome = open_system_terminal(&ssh);
    match outcome {
        Ok(()) => ClaudeCommandResult::ok(()),
        Err(code) => ClaudeCommandResult::failed(code),
    }
}

fn open_system_terminal(ssh: &str) -> Result<(), &'static str> {
    #[cfg(target_os = "macos")]
    {
        Command::new("osascript")
            .args([
                "-e",
                &format!("tell application \"Terminal\" to do script \"{ssh}\""),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "SSH_CLIENT_MISSING")?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "cmd", "/K", ssh])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "SSH_CLIENT_MISSING")?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for program in [
            "x-terminal-emulator",
            "xdg-terminal",
            "gnome-terminal",
            "konsole",
        ] {
            if Command::new(program)
                .arg("-e")
                .arg(ssh)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err("SSH_CLIENT_MISSING")
    }
}

#[tauri::command]
pub fn lumio_claude_run_remote(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    command: String,
) -> ClaudeCommandResult<ClaudeRemoteOutput> {
    match run_ssh(
        host.trim(),
        user.trim(),
        if port == 0 { 22 } else { port },
        password.as_deref(),
        key_path.as_deref(),
        host_alias.as_deref(),
        command.trim(),
    ) {
        Ok(output) => ClaudeCommandResult::ok(ClaudeRemoteOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(1),
        }),
        Err(code) => ClaudeCommandResult::failed(code),
    }
}

#[tauri::command]
pub fn lumio_claude_list_local_files(
    local_root: String,
) -> ClaudeCommandResult<Vec<claude_files::FileNode>> {
    let root = expand_local_root(&local_root);
    let entries = claude_files::read_tree(&root, "local", 8).unwrap_or_default();
    ClaudeCommandResult::ok(entries)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeFileTrees {
    pub local: Vec<claude_files::FileNode>,
    pub remote: Vec<claude_files::FileNode>,
}

#[tauri::command]
pub fn lumio_claude_list_files(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    local_root: String,
    remote_root: String,
) -> ClaudeCommandResult<ClaudeFileTrees> {
    let mut local =
        claude_files::read_tree(&expand_local_root(&local_root), "local", 8).unwrap_or_default();
    let mut remote = match run_ssh(
        host.trim(),
        user.trim(),
        if port == 0 { 22 } else { port },
        password.as_deref(),
        key_path.as_deref(),
        host_alias.as_deref(),
        &format!(
            "find {} -mindepth 1 \\( -type d -printf '%P/\\n' -o -printf '%P\\n' \\) 2>/dev/null",
            remote_shell_path(&remote_root)
        ),
    ) {
        Ok(output) if output.status.success() => {
            claude_files::parse_remote_listing(&String::from_utf8_lossy(&output.stdout), "remote")
        }
        _ => Vec::new(),
    };
    let root = expand_local_root(&local_root);
    let local_paths = claude_files::flatten_file_paths(&local);
    let remote_paths = claude_files::flatten_file_paths(&remote);
    let overlap: Vec<String> = remote_paths
        .into_iter()
        .filter(|path| local_paths.iter().any(|local_path| local_path == path))
        .take(40)
        .collect();
    if !overlap.is_empty() {
        let mut pairs = Vec::new();
        for path in overlap {
            if let Ok(preview) = run_ssh(
                host.trim(),
                user.trim(),
                if port == 0 { 22 } else { port },
                password.as_deref(),
                key_path.as_deref(),
                host_alias.as_deref(),
                &format!("head -c 1048576 {}", remote_shell_join(&remote_root, &path)),
            ) {
                if preview.status.success() && !preview.stdout.iter().take(512).any(|b| *b == 0) {
                    pairs.push((path, String::from_utf8_lossy(&preview.stdout).into_owned()));
                }
            }
        }
        let mut local_prints = std::collections::HashMap::new();
        let mut remote_prints = std::collections::HashMap::new();
        for (path, remote_content) in &pairs {
            remote_prints.insert(
                path.clone(),
                claude_files::content_fingerprint(remote_content.as_bytes()),
            );
            if let Ok(bytes) = std::fs::read(root.join(path)) {
                local_prints.insert(path.clone(), claude_files::content_fingerprint(&bytes));
            }
        }
        claude_files::apply_fingerprints(&mut local, &local_prints);
        claude_files::apply_fingerprints(&mut remote, &remote_prints);
        let detected = claude_conflicts::detect_content_conflicts(&root, &pairs);
        if !detected.is_empty() {
            let _ =
                claude_conflicts::write_sidecar_conflicts(&root.join(".bestcodex-sync"), &detected);
        }
    }
    ClaudeCommandResult::ok(ClaudeFileTrees { local, remote })
}

#[tauri::command]
pub fn lumio_claude_preview_file(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    local_root: String,
    remote_root: String,
    path: String,
    side: String,
) -> ClaudeCommandResult<claude_files::FilePreview> {
    if side == "remote" {
        match run_ssh(
            host.trim(),
            user.trim(),
            if port == 0 { 22 } else { port },
            password.as_deref(),
            key_path.as_deref(),
            host_alias.as_deref(),
            &format!("head -c 1048576 {}", remote_shell_join(&remote_root, &path)),
        ) {
            Ok(output) => {
                let binary = claude_files::preview_is_binary(&path, &output.stdout);
                ClaudeCommandResult::ok(claude_files::FilePreview {
                    path,
                    side,
                    content: if binary {
                        String::new()
                    } else {
                        String::from_utf8_lossy(&output.stdout).into_owned()
                    },
                    too_large: false,
                    binary,
                })
            }
            Err(code) => ClaudeCommandResult::failed(code),
        }
    } else {
        match claude_files::read_preview(&expand_local_root(&local_root), &path, "local") {
            Ok(preview) => ClaudeCommandResult::ok(preview),
            Err(_) => ClaudeCommandResult::failed("SSH_PREPARE_FAILED"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFsOutcome {
    pub path: String,
}

fn spawn_os_command(cmd: &str, args: &[String]) -> Result<(), String> {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| "FILE_REVEAL_FAILED".to_string())
}

#[tauri::command]
pub fn lumio_claude_local_fs(
    local_root: String,
    action: String,
    path: String,
    is_dir: Option<bool>,
    name: Option<String>,
) -> ClaudeCommandResult<LocalFsOutcome> {
    let root = expand_local_root(&local_root);
    let is_dir = is_dir.unwrap_or(false);
    let result = match action.as_str() {
        "create-file" => {
            let dir = claude_files::target_dir(&path, is_dir);
            let file_name = match name.as_deref() {
                Some(raw) => match claude_files::unique_named_relative(&root, &dir, raw) {
                    Ok(rel) => rel,
                    Err(code) => return ClaudeCommandResult::failed(&code),
                },
                None => match claude_files::unique_relative(&root, &dir, "untitled", "") {
                    Ok(rel) => rel,
                    Err(code) => return ClaudeCommandResult::failed(&code),
                },
            };
            claude_files::create_empty_file(&root, &file_name).map(|()| file_name)
        }
        "create-folder" => {
            let dir = claude_files::target_dir(&path, is_dir);
            let folder_name = match name.as_deref() {
                Some(raw) => match claude_files::unique_named_relative(&root, &dir, raw) {
                    Ok(rel) => rel,
                    Err(code) => return ClaudeCommandResult::failed(&code),
                },
                None => match claude_files::unique_relative(&root, &dir, "untitled-folder", "") {
                    Ok(rel) => rel,
                    Err(code) => return ClaudeCommandResult::failed(&code),
                },
            };
            claude_files::create_folder(&root, &folder_name).map(|()| folder_name)
        }
        "duplicate" => claude_files::duplicate_entry(&root, &path),
        "rename" => match name.as_deref() {
            Some(raw) => claude_files::rename_entry(&root, &path, raw),
            None => Err("FILE_NAME_INVALID".into()),
        },
        "delete" => claude_files::delete_entry(&root, &path).map(|()| path),
        "reveal" => {
            let target = if path.is_empty() {
                root.clone()
            } else {
                match claude_files::resolve_existing(&root, &path) {
                    Ok(p) => p,
                    Err(code) => return ClaudeCommandResult::failed(&code),
                }
            };
            let (cmd, args) = if path.is_empty() {
                claude_files::open_path_invocation(&target)
            } else {
                claude_files::reveal_invocation(&target)
            };
            spawn_os_command(&cmd, &args).map(|()| path)
        }
        "open-folder" => {
            let (cmd, args) = claude_files::open_path_invocation(&root);
            spawn_os_command(&cmd, &args).map(|()| String::new())
        }
        "open-file" => {
            let target = match claude_files::resolve_existing(&root, &path) {
                Ok(p) => p,
                Err(code) => return ClaudeCommandResult::failed(&code),
            };
            let (cmd, args) = claude_files::open_path_invocation(&target);
            spawn_os_command(&cmd, &args).map(|()| path)
        }
        _ => Err("FILE_WRITE_FAILED".into()),
    };
    match result {
        Ok(next) => ClaudeCommandResult::ok(LocalFsOutcome { path: next }),
        Err(code) => ClaudeCommandResult::failed(&code),
    }
}

#[tauri::command]
pub fn lumio_claude_list_conflicts(
    project_id: String,
    local_root: String,
) -> ClaudeCommandResult<Vec<claude_conflicts::ConflictView>> {
    let store = match ConflictStore::new(&conflict_dir(), &project_id) {
        Ok(store) => store,
        Err(_) => return ClaudeCommandResult::ok(Vec::new()),
    };
    let local = expand_local_root(&local_root);
    let _ = claude_conflicts::ingest_sidecar_conflicts(&store, &local.join(".bestcodex-sync"));
    ClaudeCommandResult::ok(
        store
            .list()
            .into_iter()
            .map(|conflict| claude_conflicts::ConflictView {
                id: conflict.id,
                path: conflict.path,
                kind_label: conflict.kind_label,
                local_content: conflict.local.content,
                remote_content: conflict.remote.content,
                can_resolve: conflict.can_resolve,
            })
            .collect(),
    )
}

#[tauri::command]
pub fn lumio_claude_resolve_conflict(
    project_id: String,
    local_root: String,
    conflict_id: String,
    resolution: String,
) -> ClaudeCommandResult<claude_conflicts::ResolutionReceipt> {
    let store = match ConflictStore::new(&conflict_dir(), &project_id) {
        Ok(store) => store,
        Err(_) => {
            return ClaudeCommandResult {
                ok: false,
                error_code: Some("SYNC_FAILED".into()),
                payload: None,
            };
        }
    };
    let parsed = match Resolution::parse(&resolution) {
        Ok(parsed) => parsed,
        Err(_) => return ClaudeCommandResult::failed("SYNC_FAILED"),
    };
    match store.resolve(&expand_local_root(&local_root), &conflict_id, parsed) {
        Ok(receipt) => ClaudeCommandResult::ok(receipt),
        Err(_) => ClaudeCommandResult::failed("SYNC_FAILED"),
    }
}

#[tauri::command]
pub fn lumio_claude_conflict_diff(
    project_id: String,
    local_root: String,
    conflict_id: String,
) -> ClaudeCommandResult<serde_json::Value> {
    let store = match ConflictStore::new(&conflict_dir(), &project_id) {
        Ok(store) => store,
        Err(_) => return ClaudeCommandResult::failed("SYNC_FAILED"),
    };
    let _ = local_root;
    let Some(conflict) = store.list().into_iter().find(|item| item.id == conflict_id) else {
        return ClaudeCommandResult::failed("SYNC_FAILED");
    };
    ClaudeCommandResult::ok(serde_json::json!({
        "path": conflict.path,
        "local": conflict.local.content,
        "remote": conflict.remote.content,
    }))
}

#[tauri::command]
pub fn lumio_claude_list_ssh_hosts() -> ClaudeCommandResult<Vec<SshHost>> {
    ClaudeCommandResult::ok(parse_ssh_config().unwrap_or_default())
}

#[tauri::command]
pub fn lumio_claude_start_terminal(
    app: AppHandle,
    project_id: String,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> ClaudeCommandResult<()> {
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());
    let target = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias) {
        Ok(target) => target,
        Err(code) => return ClaudeCommandResult::failed(code),
    };
    match app.state::<TerminalManager>().start(
        &project_id,
        session_id.as_deref(),
        &target,
        key_path.as_deref(),
        password.as_deref(),
        &remote_root,
        cols.max(20),
        rows.max(8),
        &app,
    ) {
        Ok(()) => ClaudeCommandResult::ok(()),
        Err(_) => ClaudeCommandResult::failed("SSH_CLIENT_MISSING"),
    }
}

fn spawn_official_engine(
    app: &AppHandle,
    engine: &SyncEngine,
    key: &str,
    target: &ResolvedSshTarget,
    key_path: Option<&str>,
    password: Option<&str>,
    local_root: &str,
) -> Result<(), String> {
    let local_port = app
        .state::<TunnelManager>()
        .open(key, target, key_path, password, 9000)?;
    let local = expand_local_root(local_root);
    let _ = std::fs::create_dir_all(&local);
    let state_dir = local.join(".bestcodex-sync");
    let config = claude_sync::write_agent_config(
        &state_dir,
        &local,
        &format!("ws://127.0.0.1:{local_port}/api/user/workspace-sync/v2"),
        key,
        key,
    )?;
    engine.adopt_sidecar(key, &config)
}

fn resume_sync_inner(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    local_root: String,
    project_id: Option<String>,
) -> ClaudeCommandResult<claude_sync::ResumeOutcome> {
    let alias = host_alias.as_deref().filter(|value| !value.is_empty());
    let target = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCommandResult::ok(claude_sync::ResumeOutcome {
                ok: false,
                running: false,
                files_done: 0,
                files_total: 0,
                error_code: Some(code.into()),
            });
        }
    };
    let key = project_id.unwrap_or_else(|| format!("{}@{}", target.user, target.host));
    let engine = app.state::<SyncEngine>();
    let action = claude_sync::plan_resume_sync(
        claude_sync::sidecar_command().is_some(),
        engine.is_running(&key),
    );
    let watch_app = app.clone();
    let watch_root = local_root.clone();
    let watch_key = key.clone();
    let outcome = claude_sync::execute_resume(action, || {
        spawn_official_engine(
            &app,
            &engine,
            &key,
            &target,
            key_path.as_deref(),
            password.as_deref(),
            &local_root,
        )?;
        let emit_app = watch_app.clone();
        engine.watch_local_files(&watch_key, watch_root.clone(), move |progress| {
            let _ = emit_app.emit(SYNC_PROGRESS_EVENT, progress);
        });
        Ok(())
    });
    ClaudeCommandResult::ok(outcome)
}

#[tauri::command]
pub async fn lumio_claude_resume_sync(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    local_root: String,
    project_id: Option<String>,
) -> ClaudeCommandResult<claude_sync::ResumeOutcome> {
    let _ = remote_root;
    tauri::async_runtime::spawn_blocking(move || {
        resume_sync_inner(
            app, host, user, port, password, key_path, host_alias, local_root, project_id,
        )
    })
    .await
    .unwrap_or_else(|_| ClaudeCommandResult::failed("SYNC_ENGINE_UNAVAILABLE"))
}

#[tauri::command]
pub async fn lumio_claude_server_status(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    project_id: String,
    remote_root: String,
) -> ClaudeCommandResult<claude_remote::ServerStatusSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let captured = claude_remote::captured_at_now();
        match run_ssh(
            host.trim(),
            user.trim(),
            if port == 0 { 22 } else { port },
            password.as_deref(),
            key_path.as_deref(),
            host_alias.as_deref(),
            &claude_remote::build_host_probe_script(&remote_root),
        ) {
            Ok(output) if output.status.success() => {
                ClaudeCommandResult::ok(claude_remote::parse_server_status_payload(
                    &String::from_utf8_lossy(&output.stdout),
                    &project_id,
                    &captured,
                ))
            }
            Ok(_) => ClaudeCommandResult::ok(claude_remote::server_status_error(
                &project_id,
                &captured,
                "ssh_failed",
                "没能读取服务器状态。",
            )),
            Err(code) => ClaudeCommandResult::ok(claude_remote::server_status_error(
                &project_id,
                &captured,
                code,
                "没能读取服务器状态。",
            )),
        }
    })
    .await
    .unwrap_or_else(|_| ClaudeCommandResult::failed("SSH_PROBE_FAILED"))
}

#[tauri::command]
pub async fn lumio_claude_list_sessions(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    project_id: String,
) -> ClaudeCommandResult<claude_remote::ConversationSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let captured = claude_remote::captured_at_now();
        match run_ssh(
            host.trim(),
            user.trim(),
            if port == 0 { 22 } else { port },
            password.as_deref(),
            key_path.as_deref(),
            host_alias.as_deref(),
            &claude_remote::conversation_list_command(&project_id),
        ) {
            Ok(output) => ClaudeCommandResult::ok(claude_remote::parse_conversation_windows(
                &String::from_utf8_lossy(&output.stdout),
                &project_id,
                &captured,
            )),
            Err(code) => ClaudeCommandResult::ok(claude_remote::conversations_error(
                &project_id,
                &captured,
                false,
                code,
                "没能读取对话状态。",
            )),
        }
    })
    .await
    .unwrap_or_else(|_| ClaudeCommandResult::failed("SSH_PROBE_FAILED"))
}

#[tauri::command]
pub fn lumio_claude_write_terminal(
    app: AppHandle,
    project_id: String,
    bytes: Vec<u8>,
    session_id: Option<String>,
) -> ClaudeCommandResult<()> {
    match app
        .state::<TerminalManager>()
        .write(&project_id, session_id.as_deref(), &bytes)
    {
        Ok(()) => ClaudeCommandResult::ok(()),
        Err(_) => ClaudeCommandResult::failed("SSH_CLIENT_MISSING"),
    }
}

#[tauri::command]
pub fn lumio_claude_resize_terminal(
    app: AppHandle,
    project_id: String,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> ClaudeCommandResult<()> {
    match app
        .state::<TerminalManager>()
        .resize(&project_id, session_id.as_deref(), cols, rows)
    {
        Ok(()) => ClaudeCommandResult::ok(()),
        Err(_) => ClaudeCommandResult::failed("SSH_CLIENT_MISSING"),
    }
}

#[allow(dead_code)]
#[tauri::command]
pub fn lumio_claude_open_chat(
    app: AppHandle,
    project_id: String,
    session_id: String,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    remote_root: String,
    cols: u16,
    rows: u16,
) -> ClaudeCommandResult<()> {
    if session_id.trim().is_empty() {
        return ClaudeCommandResult::failed("SSH_CLIENT_MISSING");
    }
    lumio_claude_start_terminal(
        app,
        project_id,
        host,
        user,
        port,
        password,
        key_path,
        host_alias,
        remote_root,
        cols,
        rows,
        Some(session_id),
    )
}

#[allow(dead_code)]
#[tauri::command]
pub fn lumio_claude_close_chat(
    app: AppHandle,
    project_id: String,
    session_id: String,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
) -> ClaudeCommandResult<()> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return ClaudeCommandResult::failed("SSH_CLIENT_MISSING");
    }
    let alias = host_alias.as_deref().filter(|v| !v.is_empty());
    let remote_result = match resolve_from_user_config(host.trim(), Some(user.trim()), port, alias)
    {
        Ok(target) => {
            let remote = close_remote_session_command(&project_id, session_id);
            run_ssh_target(&target, password.as_deref(), key_path.as_deref(), &remote)
        }
        Err(code) => {
            let _ = app
                .state::<TerminalManager>()
                .close(&project_id, session_id);
            return ClaudeCommandResult::failed(code);
        }
    };
    let local_result = app
        .state::<TerminalManager>()
        .close(&project_id, session_id);
    match (remote_result, local_result) {
        (Ok(_), Ok(())) => ClaudeCommandResult::ok(()),
        _ => ClaudeCommandResult::failed("SSH_CLIENT_MISSING"),
    }
}

#[allow(dead_code)]
#[tauri::command]
pub fn lumio_claude_list_chats(
    app: AppHandle,
    project_id: String,
) -> ClaudeCommandResult<Vec<String>> {
    match app.state::<TerminalManager>().list_chats(&project_id) {
        Ok(ids) => ClaudeCommandResult::ok(ids),
        Err(_) => ClaudeCommandResult::failed("SSH_CLIENT_MISSING"),
    }
}

#[allow(dead_code)]
fn password_never_on_argv(args: &[String]) -> bool {
    args.iter()
        .all(|arg| !arg.contains("password=") && arg != "-f")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ssh_args_do_not_include_a_password() {
        let args = ssh_base_args("43.156.20.8", "root", 22, None);
        assert!(args.contains(&"root@43.156.20.8".into()));
        assert!(args.contains(&"22".into()));
        assert!(password_never_on_argv(&args));
        assert!(!args.iter().any(|arg| arg.contains("secret")));
    }

    #[test]
    fn classify_permission_denied_as_auth_failure() {
        assert_eq!(
            classify_ssh_error("Permission denied (publickey,password)."),
            "SSH_AUTH_FAILED"
        );
        assert_eq!(
            classify_ssh_error("ssh: connect to host 1.1.1.1 port 22: Connection timed out"),
            "SSH_UNREACHABLE"
        );
        assert_eq!(
            classify_ssh_error("bash: -c: line 1: syntax error near unexpected token `-o'"),
            "SSH_PROBE_FAILED",
            "leftover ssh flags after inspect must not be misread as auth or a missing client"
        );
    }

    #[test]
    fn expand_tilde_local_root_stays_under_bestcodex() {
        let path = expand_local_root("~/BestCodex/my-project");
        assert!(path.ends_with(Path::new("BestCodex/my-project")));
    }

    #[test]
    fn parse_distro_cpu_and_memory_lines() {
        let (distro, cpu, memory) =
            parse_system_info("DISTRO:Ubuntu 22.04.1 LTS\nCPU:4\nMEM:8 GB\n");
        assert_eq!(distro.as_deref(), Some("Ubuntu 22.04.1 LTS"));
        assert_eq!(cpu.as_deref(), Some("4"));
        assert_eq!(memory.as_deref(), Some("8 GB"));
    }

    fn sample_target() -> ResolvedSshTarget {
        ResolvedSshTarget {
            host: "43.156.20.8".into(),
            user: "root".into(),
            port: 22,
            alias: None,
            use_config: false,
            identity_file: None,
        }
    }

    fn assert_options_before_destination(args: &[String], dest: &str, remote: &str) {
        let dest_at = args
            .iter()
            .position(|arg| arg == dest)
            .expect("destination");
        assert_eq!(args.last().map(String::as_str), Some(remote));
        assert!(
            args[dest_at + 1..].iter().all(|arg| arg == remote),
            "ssh options after the destination become the remote command: {args:?}"
        );
        assert!(
            args[..dest_at].windows(2).any(|pair| {
                pair[0] == "-o"
                    && (pair[1] == "BatchMode=yes" || pair[1].starts_with("ConnectTimeout="))
            }),
            "options must stay before {dest}: {args:?}"
        );
    }

    #[test]
    fn inspect_and_key_auth_do_not_append_ssh_flags_after_the_remote_command() {
        let target = sample_target();
        let inspect = claude_deploy::inspect_remote_script("~/bestcodex/my-project");
        assert!(
            !inspect.contains('"'),
            "inspect must not emit double quotes that break sshd -c wrapping: {inspect}"
        );
        let key = ssh_command_args(&target, None, Some("/tmp/id_ed25519"), &inspect);
        assert_options_before_destination(&key, "root@43.156.20.8", &inspect);
        assert!(key.iter().any(|arg| arg == "BatchMode=yes"));
        let agent = ssh_command_args(&target, None, None, &inspect);
        assert_options_before_destination(&agent, "root@43.156.20.8", &inspect);
        assert!(agent.iter().any(|arg| arg == "BatchMode=yes"));
        let password = ssh_command_args(&target, Some("hunter2"), None, &inspect);
        assert_options_before_destination(&password, "root@43.156.20.8", &inspect);
        assert!(!password.iter().any(|arg| arg == "BatchMode=yes"));
    }
}
