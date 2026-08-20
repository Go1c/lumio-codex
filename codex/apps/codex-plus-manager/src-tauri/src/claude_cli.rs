//! Install / upgrade the official Claude CLI on the user's Linux server.
//!
//! The constructed installer is executed over SSH on the user's machine, never
//! on this development host. Tests inject [`RemoteShell`].

use crate::claude_ssh::{
    ResolvedSshTarget, posix_single_quote, resolve_from_user_config, ssh_invocation_args,
};
use serde::Serialize;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};

pub const CLI_PROGRESS_EVENT: &str = "lumio://claude-cli-progress";

pub const ERR_NO_NETWORK: &str = "CLAUDE_CLI_NO_NETWORK";
pub const ERR_DNS: &str = "CLAUDE_CLI_DNS";
pub const ERR_NO_CURL: &str = "CLAUDE_CLI_NO_CURL";
pub const ERR_BIN_UNWRITABLE: &str = "CLAUDE_CLI_BIN_UNWRITABLE";
pub const ERR_DOWNLOAD_FAILED: &str = "CLAUDE_CLI_DOWNLOAD_FAILED";
pub const ERR_VERIFY_FAILED: &str = "CLAUDE_CLI_VERIFY_FAILED";
pub const ERR_INSTALL_FAILED: &str = "CLAUDE_CLI_INSTALL_FAILED";

const PATH_EXPORT: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;
const INSTALL_URL: &str = "https://claude.ai/install.sh";
const PROGRESS_TOTAL: u32 = 4;

pub trait RemoteShell {
    fn run(&self, script: &str) -> RemoteOutput;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliProgress {
    pub host: String,
    pub phase: String,
    pub step: u32,
    pub total: u32,
    pub detail: String,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliEnsureOutcome {
    pub ok: bool,
    pub phase: String,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[allow(dead_code)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliCommandResult<T> {
    pub ok: bool,
    pub error_code: Option<String>,
    pub payload: Option<T>,
}

impl<T> ClaudeCliCommandResult<T> {
    #[allow(dead_code)]
    fn ok(payload: T) -> Self {
        Self {
            ok: true,
            error_code: None,
            payload: Some(payload),
        }
    }

    #[allow(dead_code)]
    fn failed(code: &str) -> Self {
        Self {
            ok: false,
            error_code: Some(code.to_string()),
            payload: None,
        }
    }
}

pub fn official_install_command(channel: &str) -> String {
    let channel = channel.trim();
    let channel = if channel.is_empty() {
        "latest"
    } else {
        channel
    };
    format!(
        "{PATH_EXPORT}; curl -fsSL {INSTALL_URL} | bash -s {channel}",
        channel = posix_single_quote(channel)
    )
}

pub fn version_command() -> String {
    format!("{PATH_EXPORT}; claude --version")
}

fn persist_path_command() -> String {
    format!(
        r#"grep -qs '.local/bin' "$HOME/.profile" 2>/dev/null || printf '%s\n' '{PATH_EXPORT}' >> "$HOME/.profile""#
    )
}

pub fn ensure_cli(shell: &impl RemoteShell, channel: &str) -> CliEnsureOutcome {
    ensure_cli_with_progress(shell, channel, "", |_| {})
}

pub fn ensure_cli_with_progress(
    shell: &impl RemoteShell,
    channel: &str,
    host: &str,
    mut on_progress: impl FnMut(CliProgress),
) -> CliEnsureOutcome {
    let emit = |on_progress: &mut dyn FnMut(CliProgress),
                phase: &str,
                step: u32,
                detail: &str,
                version: Option<&str>,
                latest: Option<&str>,
                error_code: Option<&str>| {
        on_progress(CliProgress {
            host: host.to_string(),
            phase: phase.to_string(),
            step,
            total: PROGRESS_TOTAL,
            detail: detail.to_string(),
            version: version.map(str::to_string),
            latest: latest.map(str::to_string),
            error_code: error_code.map(str::to_string),
        });
    };

    emit(
        &mut on_progress,
        "detect",
        1,
        "正在检测已装版本…",
        None,
        None,
        None,
    );
    let detected = shell.run(&version_command());
    let before = (detected.status == 0)
        .then(|| parse_claude_version(&detected.stdout))
        .flatten();

    emit(
        &mut on_progress,
        "install",
        2,
        "正在从官方渠道安装…",
        before.as_deref(),
        None,
        None,
    );
    let install = shell.run(&official_install_command(channel));
    if install.status != 0 {
        let code = classify_failure(&install.stdout, &install.stderr);
        let detail = human_cli_detail(code);
        emit(
            &mut on_progress,
            "fail",
            2,
            &detail,
            before.as_deref(),
            None,
            Some(code),
        );
        return CliEnsureOutcome {
            ok: false,
            phase: "fail".into(),
            version: before,
            latest: None,
            error_code: Some(code.into()),
            detail: Some(detail),
        };
    }

    emit(
        &mut on_progress,
        "install",
        3,
        "正在确认版本…",
        before.as_deref(),
        None,
        None,
    );
    let verified = shell.run(&version_command());
    let after = (verified.status == 0)
        .then(|| parse_claude_version(&verified.stdout))
        .flatten();
    let Some(version) = after else {
        let detail = human_cli_detail(ERR_VERIFY_FAILED);
        emit(
            &mut on_progress,
            "fail",
            3,
            &detail,
            before.as_deref(),
            None,
            Some(ERR_VERIFY_FAILED),
        );
        return CliEnsureOutcome {
            ok: false,
            phase: "fail".into(),
            version: before,
            latest: None,
            error_code: Some(ERR_VERIFY_FAILED.into()),
            detail: Some(detail),
        };
    };

    let phase = match before.as_deref() {
        None => "install",
        Some(previous) if previous == version => "skip",
        Some(_) => "upgrade",
    };
    let detail = match phase {
        "skip" => "已经是最新，跳过",
        "upgrade" => "已升级到最新版本",
        _ => "已装好最新版本",
    };
    emit(
        &mut on_progress,
        phase,
        4,
        detail,
        Some(&version),
        Some(&version),
        None,
    );
    emit(
        &mut on_progress,
        "ok",
        4,
        detail,
        Some(&version),
        Some(&version),
        None,
    );
    let _ = shell.run(&persist_path_command());
    CliEnsureOutcome {
        ok: true,
        phase: phase.into(),
        version: Some(version.clone()),
        latest: Some(version),
        error_code: None,
        detail: Some(detail.into()),
    }
}

fn parse_claude_version(stdout: &str) -> Option<String> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    for raw in line.split_whitespace() {
        let token = raw.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '[' | ']'));
        let token = token.trim_start_matches('v');
        if looks_like_version(token) {
            return Some(token.to_string());
        }
    }
    None
}

fn looks_like_version(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let mut has_digit = false;
    let mut has_dot = false;
    for c in token.chars() {
        match c {
            '0'..='9' => has_digit = true,
            '.' => has_dot = true,
            '-' | '+' | 'a'..='z' | 'A'..='Z' => {}
            _ => return false,
        }
    }
    has_digit && has_dot
}

fn classify_failure(stdout: &str, stderr: &str) -> &'static str {
    let combined = format!("{stdout}\n{stderr}");
    if combined.contains("Could not resolve host") {
        return ERR_DNS;
    }
    if combined.contains("Network is unreachable") || combined.contains("Failed to connect") {
        return ERR_NO_NETWORK;
    }
    if combined.contains("curl: not found")
        || combined.contains("command not found: curl")
        || combined.contains("curl: command not found")
    {
        return ERR_NO_CURL;
    }
    if combined.contains("Permission denied") && combined.contains(".local/bin") {
        return ERR_BIN_UNWRITABLE;
    }
    let lower = combined.to_ascii_lowercase();
    if lower.contains("the requested url returned error")
        || lower.contains("curl: (22)")
        || lower.contains("download failed")
        || lower.contains("failed to download")
    {
        return ERR_DOWNLOAD_FAILED;
    }
    ERR_INSTALL_FAILED
}

fn human_cli_detail(code: &str) -> String {
    match code {
        ERR_NO_NETWORK => "这台服务器现在连不上外网。检查网络后再试。".into(),
        ERR_DNS => "这台服务器解析不了官方下载地址。检查 DNS 后再试。".into(),
        ERR_NO_CURL => "这台服务器没有 curl，装不上官方 Claude。".into(),
        ERR_BIN_UNWRITABLE => "写不进 ~/.local/bin，没法安装 Claude。".into(),
        ERR_DOWNLOAD_FAILED => {
            "服务器连不上官方下载地址。确认这台服务器能访问外网，或稍后再试。".into()
        }
        ERR_VERIFY_FAILED => "装完之后没能读到 Claude 版本，安装可能没有成功。".into(),
        ERR_INSTALL_FAILED => "没能在这台服务器上装好 Claude。".into(),
        "SSH_CLIENT_MISSING" => "这台电脑还没有 ssh 命令。".into(),
        "SSH_ALIAS_UNKNOWN" => "本机 SSH 配置里没有这个 Host 别名。".into(),
        "SSH_HOST_REQUIRED" => "先填写公网 IP。".into(),
        _ => "没能在这台服务器上装好 Claude。".into(),
    }
}

#[allow(dead_code)]
fn ssh_human_detail(code: &str, host: &str, port: u16) -> String {
    match code {
        "SSH_AUTH_FAILED" => format!("无法登录 {host}。"),
        "SSH_UNREACHABLE" => format!("连不上 {host}:{port}。"),
        "SSH_NOT_SSH" => format!("{host}:{port} 不是 SSH 服务。"),
        other => human_cli_detail(other),
    }
}

#[allow(dead_code)]
struct SshRemoteShell {
    target: ResolvedSshTarget,
    password: Option<String>,
    key_path: Option<String>,
}

impl RemoteShell for SshRemoteShell {
    fn run(&self, script: &str) -> RemoteOutput {
        match ssh_run(
            &self.target,
            self.password.as_deref(),
            self.key_path.as_deref(),
            script,
        ) {
            Ok(output) => RemoteOutput {
                status: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(code) => RemoteOutput {
                status: 255,
                stdout: String::new(),
                stderr: code.to_string(),
            },
        }
    }
}

#[allow(dead_code)]
fn ssh_run(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote: &str,
) -> Result<std::process::Output, &'static str> {
    let key = crate::claude_ssh::effective_key_path(key_path, target);
    let plan = crate::claude_ssh::password_auth_plan(password, key, target.use_config);
    let mut args = ssh_invocation_args(target, key, None);
    if plan.batch_mode && !args.iter().any(|arg| arg == "BatchMode=yes") {
        let dest = args.pop().ok_or("SSH_CLIENT_MISSING")?;
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args.push(dest);
    }
    args.push(remote.to_string());
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

#[allow(dead_code)]
fn install_cli_inner(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    channel: Option<String>,
) -> ClaudeCliCommandResult<CliEnsureOutcome> {
    let host = host.trim().to_string();
    let user = if user.trim().is_empty() {
        "root".into()
    } else {
        user.trim().to_string()
    };
    let port = if port == 0 { 22 } else { port };
    let alias = host_alias.as_deref().filter(|value| !value.is_empty());
    let channel = channel
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("latest")
        .to_string();

    let target = match resolve_from_user_config(&host, Some(&user), port, alias) {
        Ok(target) => target,
        Err(code) => {
            return ClaudeCliCommandResult::ok(CliEnsureOutcome {
                ok: false,
                phase: "fail".into(),
                version: None,
                latest: None,
                error_code: Some(code.into()),
                detail: Some(ssh_human_detail(code, host.trim(), port)),
            });
        }
    };

    let shell = SshRemoteShell {
        target: target.clone(),
        password,
        key_path,
    };
    let progress_host = target.host.clone();
    let app_progress = app.clone();
    let outcome = ensure_cli_with_progress(&shell, &channel, &progress_host, |progress| {
        let _ = app_progress.emit(CLI_PROGRESS_EVENT, progress);
    });
    ClaudeCliCommandResult::ok(outcome)
}

#[allow(dead_code)]
#[tauri::command]
pub async fn lumio_claude_install_cli(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    channel: Option<String>,
) -> ClaudeCliCommandResult<CliEnsureOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        install_cli_inner(
            app, host, user, port, password, key_path, host_alias, channel,
        )
    })
    .await
    .unwrap_or_else(|_| ClaudeCliCommandResult::failed(ERR_INSTALL_FAILED))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeShell {
        present: Option<String>,
        after: Option<String>,
        install: RemoteOutput,
        install_ran: Cell<bool>,
        scripts: RefCell<Vec<String>>,
    }

    impl FakeShell {
        fn new(present: Option<&str>, after: Option<&str>, install: RemoteOutput) -> Self {
            Self {
                present: present.map(str::to_string),
                after: after.map(str::to_string),
                install,
                install_ran: Cell::new(false),
                scripts: RefCell::new(Vec::new()),
            }
        }

        fn ok_install() -> RemoteOutput {
            RemoteOutput {
                status: 0,
                stdout: "installed".into(),
                stderr: String::new(),
            }
        }

        fn fail_install(stderr: &str) -> RemoteOutput {
            RemoteOutput {
                status: 1,
                stdout: String::new(),
                stderr: stderr.into(),
            }
        }
    }

    impl RemoteShell for FakeShell {
        fn run(&self, script: &str) -> RemoteOutput {
            self.scripts.borrow_mut().push(script.to_string());
            if script.contains("install.sh") {
                self.install_ran.set(true);
                return self.install.clone();
            }
            if script.contains("claude --version") {
                let version = if self.install_ran.get() {
                    self.after.as_deref()
                } else {
                    self.present.as_deref()
                };
                return match version {
                    Some(text) if !text.trim().is_empty() => RemoteOutput {
                        status: 0,
                        stdout: text.to_string(),
                        stderr: String::new(),
                    },
                    _ => RemoteOutput {
                        status: 127,
                        stdout: String::new(),
                        stderr: "claude: command not found".into(),
                    },
                };
            }
            RemoteOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    }

    fn assert_no_forbidden_copy(text: &str) {
        let lower = text.to_ascii_lowercase();
        assert!(
            !lower.contains("agent"),
            "user-visible copy leaked agent: {text}"
        );
        assert!(
            !lower.contains("tmux"),
            "user-visible copy leaked tmux: {text}"
        );
    }

    #[test]
    fn official_install_command_uses_official_installer_without_sudo() {
        let cmd = official_install_command("latest");
        assert!(
            cmd.contains("https://claude.ai/install.sh"),
            "missing official installer URL: {cmd}"
        );
        assert!(
            cmd.contains("| bash") || cmd.contains("|bash"),
            "must pipe into bash: {cmd}"
        );
        assert!(
            cmd.contains("bash -s"),
            "must pass channel via bash -s: {cmd}"
        );
        assert!(cmd.contains("latest"), "default channel is latest: {cmd}");
        assert!(
            cmd.contains(r#"export PATH="$HOME/.local/bin:$PATH""#),
            "must export ~/.local/bin: {cmd}"
        );
        assert!(!cmd.contains("sudo"), "must not use sudo: {cmd}");
        assert!(
            !cmd.contains("/usr/local/bin"),
            "must not write /usr/local/bin: {cmd}"
        );
    }

    #[test]
    fn official_install_command_shell_quotes_channel() {
        let cmd = official_install_command("2.1.89");
        assert!(
            cmd.contains("bash -s '2.1.89'"),
            "channel must be quoted: {cmd}"
        );
        let weird = official_install_command("it'latest");
        assert!(
            weird.contains("bash -s ") && weird.contains("\\'"),
            "quotes inside channel must be escaped: {weird}"
        );
        assert!(!weird.contains("sudo"));
        assert!(!weird.contains("/usr/local/bin"));
    }

    #[test]
    fn ensure_cli_skips_when_already_latest() {
        let shell = FakeShell::new(
            Some("2.1.89 (Claude Code)"),
            Some("2.1.89 (Claude Code)"),
            FakeShell::ok_install(),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert!(outcome.ok);
        assert_eq!(outcome.phase, "skip");
        assert_eq!(outcome.version.as_deref(), Some("2.1.89"));
        assert_eq!(outcome.latest.as_deref(), Some("2.1.89"));
        assert!(outcome.error_code.is_none());
        assert!(
            shell.install_ran.get(),
            "already-installed still runs official installer"
        );
        let scripts = shell.scripts.borrow();
        assert!(scripts.iter().any(|s| s.contains("claude --version")));
        assert!(scripts.iter().any(|s| s.contains("install.sh")));
    }

    #[test]
    fn ensure_cli_installs_when_missing() {
        let shell = FakeShell::new(None, Some("2.1.89 (Claude Code)"), FakeShell::ok_install());
        let outcome = ensure_cli(&shell, "latest");
        assert!(outcome.ok);
        assert_eq!(outcome.phase, "install");
        assert_eq!(outcome.version.as_deref(), Some("2.1.89"));
        assert_eq!(outcome.latest.as_deref(), Some("2.1.89"));
        assert!(shell.install_ran.get());
    }

    #[test]
    fn ensure_cli_upgrades_old_version() {
        let shell = FakeShell::new(
            Some("1.0.0 (Claude Code)"),
            Some("2.1.89 (Claude Code)"),
            FakeShell::ok_install(),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert!(outcome.ok);
        assert_eq!(outcome.phase, "upgrade");
        assert_eq!(outcome.version.as_deref(), Some("2.1.89"));
        assert_eq!(outcome.latest.as_deref(), Some("2.1.89"));
    }

    #[test]
    fn ensure_cli_maps_dns_failure() {
        let shell = FakeShell::new(
            None,
            None,
            FakeShell::fail_install("curl: (6) Could not resolve host: claude.ai"),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert!(!outcome.ok);
        assert_eq!(outcome.phase, "fail");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_DNS));
        let detail = outcome.detail.as_deref().unwrap_or("");
        assert!(!detail.is_empty());
        assert_no_forbidden_copy(detail);
    }

    #[test]
    fn ensure_cli_maps_network_unreachable() {
        let shell = FakeShell::new(
            None,
            None,
            FakeShell::fail_install("curl: (7) Network is unreachable"),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_NO_NETWORK));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_maps_failed_to_connect() {
        let shell = FakeShell::new(
            None,
            None,
            FakeShell::fail_install("curl: (7) Failed to connect to claude.ai port 443"),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_NO_NETWORK));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_maps_curl_not_found() {
        let shell = FakeShell::new(None, None, FakeShell::fail_install("curl: not found"));
        let outcome = ensure_cli(&shell, "latest");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_NO_CURL));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_maps_command_not_found_curl() {
        let shell = FakeShell::new(
            None,
            None,
            FakeShell::fail_install("bash: command not found: curl"),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_NO_CURL));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_maps_bin_unwritable() {
        let shell = FakeShell::new(
            None,
            None,
            FakeShell::fail_install(
                "cannot create directory '/home/u/.local/bin': Permission denied",
            ),
        );
        let outcome = ensure_cli(&shell, "latest");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_BIN_UNWRITABLE));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_verify_failed_when_version_empty() {
        let shell = FakeShell::new(None, None, FakeShell::ok_install());
        let outcome = ensure_cli(&shell, "latest");
        assert!(!outcome.ok);
        assert_eq!(outcome.phase, "fail");
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_VERIFY_FAILED));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_install_failed_when_unclassified() {
        let shell = FakeShell::new(None, None, FakeShell::fail_install("installer exploded"));
        let outcome = ensure_cli(&shell, "latest");
        assert!(!outcome.ok);
        assert_eq!(outcome.error_code.as_deref(), Some(ERR_INSTALL_FAILED));
        assert_no_forbidden_copy(outcome.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn ensure_cli_scripts_export_local_bin_path() {
        let shell = FakeShell::new(
            Some("2.1.89 (Claude Code)"),
            Some("2.1.89 (Claude Code)"),
            FakeShell::ok_install(),
        );
        let _ = ensure_cli(&shell, "latest");
        let scripts = shell.scripts.borrow();
        assert!(!scripts.is_empty(), "ensure_cli must run remote scripts");
        for script in scripts.iter() {
            if script.contains("claude --version") || script.contains("install.sh") {
                assert!(
                    script.contains(r#"export PATH="$HOME/.local/bin:$PATH""#),
                    "detect/install/verify must export PATH: {script}"
                );
                assert!(!script.contains("sudo"));
                assert!(!script.contains("/usr/local/bin"));
            }
        }
    }

    #[test]
    fn ensure_cli_progress_covers_skip_and_forbids_agent_copy() {
        let shell = FakeShell::new(
            Some("2.1.89 (Claude Code)"),
            Some("2.1.89 (Claude Code)"),
            FakeShell::ok_install(),
        );
        let seen = RefCell::new(Vec::new());
        let outcome = ensure_cli_with_progress(&shell, "latest", "108.80.81.15", |progress| {
            seen.borrow_mut().push(progress);
        });
        assert!(outcome.ok);
        assert_eq!(outcome.phase, "skip");
        let events = seen.borrow();
        let phases: Vec<&str> = events.iter().map(|p| p.phase.as_str()).collect();
        assert!(phases.contains(&"detect"));
        assert!(phases.contains(&"skip"));
        assert!(phases.contains(&"ok"));
        assert_eq!(CLI_PROGRESS_EVENT, "lumio://claude-cli-progress");
        assert_eq!(
            classify_failure("", "curl: (22) The requested URL returned error: 403"),
            ERR_DOWNLOAD_FAILED
        );
        for progress in events.iter() {
            assert_eq!(progress.host, "108.80.81.15");
            assert_eq!(progress.total, 4);
            assert_no_forbidden_copy(&progress.detail);
        }
    }
}
