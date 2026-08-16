//! Thin Claude/SSH helpers for the launcher tab.
//!
//! No `fns-*` crates and no new Cargo deps: TCP + `std::process` `ssh`.
//! Passwords never go on argv and are never written to logs.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeFileEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
}

pub(crate) fn ssh_base_args(
    host: &str,
    user: &str,
    port: u16,
    key_path: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        format!("ConnectTimeout={}", PROBE_TIMEOUT.as_secs()),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "NumberOfPasswordPrompts=1".into(),
        "-p".into(),
        port.to_string(),
    ];
    if let Some(key) = key_path.filter(|value| !value.is_empty()) {
        args.push("-i".into());
        args.push(key.to_string());
        args.push("-o".into());
        args.push("PreferredAuthentications=publickey".into());
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    } else {
        args.push("-o".into());
        args.push("PreferredAuthentications=password,keyboard-interactive".into());
        args.push("-o".into());
        args.push("PubkeyAuthentication=no".into());
    }
    args.push(format!("{user}@{host}"));
    args
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

struct AskpassGuard {
    script: PathBuf,
}

impl AskpassGuard {
    fn start(password: &str) -> Result<Self, &'static str> {
        let script = std::env::temp_dir().join(format!(
            "bestcodex-askpass-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        #[cfg(windows)]
        {
            let path = script.with_extension("cmd");
            std::fs::write(
                &path,
                "@echo off\r\n<nul set /p=%BESTCODEX_SSH_ASKPASS%\r\n",
            )
            .map_err(|_| "SSH_PROBE_FAILED")?;
            let _ = password;
            return Ok(Self { script: path });
        }
        #[cfg(not(windows))]
        {
            std::fs::write(&script, "#!/bin/sh\nprintf %s \"$BESTCODEX_SSH_ASKPASS\"\n")
                .map_err(|_| "SSH_PROBE_FAILED")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&script)
                    .map_err(|_| "SSH_PROBE_FAILED")?
                    .permissions();
                perms.set_mode(0o700);
                let _ = std::fs::set_permissions(&script, perms);
            }
            let _ = password;
            Ok(Self { script })
        }
    }

    fn configure(&self, command: &mut Command, password: &str) {
        command.env("SSH_ASKPASS", &self.script);
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", ":0");
        command.env("BESTCODEX_SSH_ASKPASS", password);
        command.stdin(Stdio::null());
    }
}

impl Drop for AskpassGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.script);
    }
}

fn run_ssh(
    host: &str,
    user: &str,
    port: u16,
    password: Option<&str>,
    key_path: Option<&str>,
    remote: &str,
) -> Result<std::process::Output, &'static str> {
    let mut args = ssh_base_args(host, user, port, key_path);
    args.push(remote.to_string());
    let mut command = Command::new("ssh");
    command.args(&args);
    command.env("LC_ALL", "C");
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let askpass = if key_path.map(|v| !v.is_empty()).unwrap_or(false) {
        None
    } else if let Some(password) = password {
        let guard = AskpassGuard::start(password)?;
        guard.configure(&mut command, password);
        Some(guard)
    } else {
        command.arg("-o").arg("BatchMode=yes");
        None
    };
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

fn expand_local_root(local_root: &str) -> PathBuf {
    if let Some(rest) = local_root.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(rest)
    } else {
        PathBuf::from(local_root)
    }
}

fn human_detail(code: &str, host: &str, port: u16) -> String {
    match code {
        "SSH_AUTH_FAILED" => format!("无法登录 {host}。"),
        "SSH_UNREACHABLE" => format!("连不上 {host}:{port}。"),
        "SSH_NOT_SSH" => format!("{host}:{port} 不是 SSH 服务。"),
        "SSH_CLIENT_MISSING" => "这台电脑还没有 ssh 命令。".into(),
        _ => "连不上这台服务器。".into(),
    }
}

#[tauri::command]
pub fn lumio_claude_probe_connection(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
) -> ClaudeCommandResult<ClaudeProbePayload> {
    let host = host.trim().to_string();
    let user = if user.trim().is_empty() {
        "root".into()
    } else {
        user.trim().to_string()
    };
    let port = if port == 0 { 22 } else { port };

    if host.is_empty() {
        return ClaudeCommandResult::ok(ClaudeProbePayload {
            ok: false,
            reachable: false,
            authenticated: false,
            distro: None,
            cpu: None,
            memory: None,
            error_code: Some("SSH_HOST_REQUIRED".into()),
            detail: Some("先填写公网 IP。".into()),
        });
    }

    if let Err(code) = probe_banner(&host, port) {
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

    let remote = ". /etc/os-release 2>/dev/null; echo DISTRO:${PRETTY_NAME:-unknown}; nproc >/tmp/.bc-nproc 2>/dev/null; echo CPU:$(nproc 2>/dev/null || echo ?); echo MEM:$(awk '/MemTotal/ {printf \"%.0f GB\", $2/1024/1024}' /proc/meminfo 2>/dev/null)";
    match run_ssh(
        &host,
        &user,
        port,
        password.as_deref(),
        key_path.as_deref(),
        remote,
    ) {
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
                detail: Some(human_detail(code, &host, port)),
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
            detail: Some(human_detail(code, &host, port)),
        }),
    }
}

#[tauri::command]
pub fn lumio_claude_prepare_remote(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    remote_root: String,
    local_root: String,
) -> ClaudeCommandResult<ClaudePreparePayload> {
    let local = expand_local_root(&local_root);
    if std::fs::create_dir_all(&local).is_err() {
        return ClaudeCommandResult::ok(ClaudePreparePayload {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能创建本机项目目录。".into()),
        });
    }

    let quoted = remote_root.replace('\'', "'\\''");
    match run_ssh(
        host.trim(),
        user.trim(),
        if port == 0 { 22 } else { port },
        password.as_deref(),
        key_path.as_deref(),
        &format!("mkdir -p '{quoted}'"),
    ) {
        Ok(output) if output.status.success() => ClaudeCommandResult::ok(ClaudePreparePayload {
            ok: true,
            error_code: None,
            detail: None,
        }),
        Ok(_) | Err(_) => ClaudeCommandResult::ok(ClaudePreparePayload {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能在服务器上建好项目目录。".into()),
        }),
    }
}

#[tauri::command]
pub fn lumio_claude_first_sync(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    remote_root: String,
    local_root: String,
) -> ClaudeCommandResult<ClaudeSyncPayload> {
    let local = expand_local_root(&local_root);
    let _ = std::fs::create_dir_all(&local);
    let quoted = remote_root.replace('\'', "'\\''");
    let remote_cmd = format!("mkdir -p '{quoted}' && find '{quoted}' -type f 2>/dev/null | wc -l");
    match run_ssh(
        host.trim(),
        user.trim(),
        if port == 0 { 22 } else { port },
        password.as_deref(),
        key_path.as_deref(),
        &remote_cmd,
    ) {
        Ok(output) if output.status.success() => {
            let total = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            ClaudeCommandResult::ok(ClaudeSyncPayload {
                ok: false,
                files_done: 0,
                files_total: total,
                error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
            })
        }
        _ => ClaudeCommandResult::ok(ClaudeSyncPayload {
            ok: false,
            files_done: 0,
            files_total: 0,
            error_code: Some("SYNC_ENGINE_UNAVAILABLE".into()),
        }),
    }
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
    command: String,
) -> ClaudeCommandResult<ClaudeRemoteOutput> {
    match run_ssh(
        host.trim(),
        user.trim(),
        if port == 0 { 22 } else { port },
        password.as_deref(),
        key_path.as_deref(),
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
) -> ClaudeCommandResult<Vec<ClaudeFileEntry>> {
    let root = expand_local_root(&local_root);
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(&root) {
        for entry in read.flatten().take(200) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "directory"
            } else {
                "file"
            };
            entries.push(ClaudeFileEntry {
                path: entry.path().to_string_lossy().into_owned(),
                name,
                kind: kind.into(),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    ClaudeCommandResult::ok(entries)
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
}
