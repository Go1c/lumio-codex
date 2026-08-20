//! Anthropic login bridge on the remote official Claude CLI.
//!
//! Official commands (Claude Code CLI reference, 2026-08):
//! - start: `claude auth login` — interactive OAuth; prints a browser URL, then
//!   waits at “Paste code here”.
//! - status: `claude auth status` — JSON by default; exit 0 if logged in, 1 if not.
//!
//! The login process runs in a controlled `script` PTY on the server so it does
//! not occupy the user's conversation session. Authorization codes never go into
//! events, `detail`, or logger output. Login URLs may.

use serde::Serialize;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};

use crate::claude_ssh::{
    ResolvedSshTarget, attach_askpass, effective_key_path, posix_single_quote,
    resolve_from_user_config, ssh_invocation_args,
};

pub const LOGIN_PROGRESS_EVENT: &str = "lumio://claude-login-progress";

const PATH_EXPORT: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;
const LOGIN_DIR: &str = r#"$HOME/.local/share/bestcodex/claude-login"#;

const ERR_NO_CLI: &str = "CLAUDE_LOGIN_NO_CLI";
const ERR_NO_URL: &str = "CLAUDE_LOGIN_NO_URL";
const ERR_CODE_REJECTED: &str = "CLAUDE_LOGIN_CODE_REJECTED";
const ERR_EXPIRED: &str = "CLAUDE_LOGIN_EXPIRED";
const ERR_FAILED: &str = "CLAUDE_LOGIN_FAILED";

pub struct RemoteOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait RemoteShell {
    fn run(&self, script: &str) -> RemoteOutput;
}

pub trait LoginLogger {
    fn log(&self, message: &str);
}

#[allow(dead_code)]
pub struct SilentLogger;

impl LoginLogger for SilentLogger {
    fn log(&self, _message: &str) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeLoginPhase {
    Unknown,
    LoggedOut,
    LoggingIn,
    LoggedIn,
    Expired,
}

impl ClaudeLoginPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::LoggedOut => "logged-out",
            Self::LoggingIn => "logging-in",
            Self::LoggedIn => "logged-in",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProgress {
    pub host: String,
    pub phase: String,
    pub login_url: Option<String>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStartPayload {
    pub ok: bool,
    pub login_url: Option<String>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSubmitPayload {
    pub ok: bool,
    pub phase: String,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStatusPayload {
    pub phase: String,
    pub error_code: Option<String>,
}

pub fn login_start_script() -> String {
    format!(
        r#"{PATH_EXPORT}
if ! command -v claude >/dev/null 2>&1; then
  echo CLAUDE_LOGIN_NO_CLI
  exit 127
fi
dir="{LOGIN_DIR}"
mkdir -p "$dir"
rm -f "$dir/in"
mkfifo "$dir/in"
: > "$dir/out"
# Hold the fifo open so the login process does not see EOF before the code arrives.
tail -f /dev/null > "$dir/in" &
echo $! > "$dir/keeper"
# `script` allocates a PTY without occupying the conversation session.
setsid script -q -f -c "claude auth login" "$dir/out" < "$dir/in" >/dev/null 2>&1 &
echo $! > "$dir/pid"
i=0
while [ "$i" -lt 40 ]; do
  if grep -q "https://" "$dir/out" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.25
done
cat "$dir/out" 2>/dev/null || true
"#
    )
}

pub fn login_submit_script(code: &str) -> String {
    let quoted = posix_single_quote(code);
    format!(
        r#"{PATH_EXPORT}
dir="{LOGIN_DIR}"
printf '%s\n' {quoted} > "$dir/in"
i=0
while [ "$i" -lt 40 ]; do
  if [ ! -f "$dir/pid" ]; then
    break
  fi
  if ! kill -0 "$(cat "$dir/pid" 2>/dev/null)" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.25
done
cat "$dir/out" 2>/dev/null || true
if [ -f "$dir/keeper" ]; then
  kill "$(cat "$dir/keeper")" 2>/dev/null || true
fi
claude auth status 2>/dev/null || true
"#
    )
}

pub fn login_status_script() -> String {
    format!(
        r#"{PATH_EXPORT}
if ! command -v claude >/dev/null 2>&1; then
  echo CLAUDE_LOGIN_NO_CLI
  exit 127
fi
claude auth status
"#
    )
}

pub fn extract_login_url(output: &str) -> Option<String> {
    stitch_wrapped_https_urls(output)
        .into_iter()
        .find(|url| is_claude_login_url(url))
}

pub fn parse_login_status(output: &str) -> ClaudeLoginPhase {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return ClaudeLoginPhase::Unknown;
    }
    if let Some(value) = parse_json_blob(trimmed) {
        if json_is_expired(&value) {
            return ClaudeLoginPhase::Expired;
        }
        if let Some(logged_in) = json_logged_in(&value) {
            return if logged_in {
                ClaudeLoginPhase::LoggedIn
            } else {
                ClaudeLoginPhase::LoggedOut
            };
        }
        if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
            let phase = parse_status_word(status);
            if phase != ClaudeLoginPhase::Unknown {
                return phase;
            }
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("expired") {
        return ClaudeLoginPhase::Expired;
    }
    if lower.contains("logged in") && !lower.contains("not logged") {
        return ClaudeLoginPhase::LoggedIn;
    }
    if lower.contains("not logged") || lower.contains("logged out") {
        return ClaudeLoginPhase::LoggedOut;
    }
    ClaudeLoginPhase::Unknown
}

pub fn start_login(
    shell: &impl RemoteShell,
    host: &str,
    logger: &impl LoginLogger,
    mut on_progress: impl FnMut(LoginProgress),
) -> LoginStartPayload {
    logger.log("starting Anthropic login");
    on_progress(progress(
        host,
        "logging-in",
        None,
        None,
        Some("正在打开 Anthropic 登录…"),
    ));
    let output = shell.run(&login_start_script());
    if looks_like_no_cli(&output) {
        return fail_start(host, ERR_NO_CLI, on_progress);
    }
    let combined = join_output(&output);
    if let Some(url) = extract_login_url(&combined) {
        on_progress(progress(
            host,
            "logging-in",
            Some(url.as_str()),
            None,
            Some("请在浏览器中完成授权。"),
        ));
        return LoginStartPayload {
            ok: true,
            login_url: Some(url),
            error_code: None,
            detail: None,
        };
    }
    fail_start(host, ERR_NO_URL, on_progress)
}

pub fn submit_login(
    shell: &impl RemoteShell,
    host: &str,
    code: &str,
    logger: &impl LoginLogger,
    mut on_progress: impl FnMut(LoginProgress),
) -> LoginSubmitPayload {
    logger.log("submitting Anthropic authorization code");
    on_progress(progress(
        host,
        "logging-in",
        None,
        None,
        Some("正在提交授权码…"),
    ));
    let output = shell.run(&login_submit_script(code));
    let combined = redact_secret(&join_output(&output), code);
    if looks_like_no_cli(&output) {
        return fail_submit(host, ERR_NO_CLI, on_progress);
    }
    let phase = parse_login_status(&combined);
    if phase == ClaudeLoginPhase::LoggedIn {
        on_progress(progress(host, "logged-in", None, None, None));
        return LoginSubmitPayload {
            ok: true,
            phase: "logged-in".into(),
            error_code: None,
            detail: None,
        };
    }
    if looks_like_code_rejected(&combined) {
        return fail_submit(host, ERR_CODE_REJECTED, on_progress);
    }
    if phase == ClaudeLoginPhase::Expired {
        return fail_submit(host, ERR_EXPIRED, on_progress);
    }
    fail_submit(host, ERR_FAILED, on_progress)
}

pub fn login_status(shell: &impl RemoteShell, host: &str) -> LoginStatusPayload {
    let _ = host;
    let output = shell.run(&login_status_script());
    if looks_like_no_cli(&output) {
        return LoginStatusPayload {
            phase: ClaudeLoginPhase::Unknown.as_str().into(),
            error_code: Some(ERR_NO_CLI.into()),
        };
    }
    let combined = join_output(&output);
    let mut phase = parse_login_status(&combined);
    if phase == ClaudeLoginPhase::Unknown {
        phase = match output.status {
            0 => ClaudeLoginPhase::LoggedIn,
            1 => ClaudeLoginPhase::LoggedOut,
            _ => ClaudeLoginPhase::Unknown,
        };
    }
    LoginStatusPayload {
        phase: phase.as_str().into(),
        error_code: match phase {
            ClaudeLoginPhase::Expired => Some(ERR_EXPIRED.into()),
            _ => None,
        },
    }
}

fn fail_start(
    host: &str,
    code: &str,
    mut on_progress: impl FnMut(LoginProgress),
) -> LoginStartPayload {
    let detail = human_detail(code);
    on_progress(progress(
        host,
        "logged-out",
        None,
        Some(code),
        Some(detail.as_str()),
    ));
    LoginStartPayload {
        ok: false,
        login_url: None,
        error_code: Some(code.into()),
        detail: Some(detail),
    }
}

fn fail_submit(
    host: &str,
    code: &str,
    mut on_progress: impl FnMut(LoginProgress),
) -> LoginSubmitPayload {
    let detail = human_detail(code);
    on_progress(progress(
        host,
        "fail",
        None,
        Some(code),
        Some(detail.as_str()),
    ));
    LoginSubmitPayload {
        ok: false,
        phase: "fail".into(),
        error_code: Some(code.into()),
        detail: Some(detail),
    }
}

fn progress(
    host: &str,
    phase: &str,
    login_url: Option<&str>,
    error_code: Option<&str>,
    detail: Option<&str>,
) -> LoginProgress {
    LoginProgress {
        host: host.to_string(),
        phase: phase.to_string(),
        login_url: login_url.map(str::to_string),
        error_code: error_code.map(str::to_string),
        detail: detail.map(str::to_string),
    }
}

fn human_detail(code: &str) -> String {
    match code {
        ERR_NO_CLI => "服务器上还没有官方 Claude 命令。".into(),
        ERR_NO_URL => "没能拿到登录链接。".into(),
        ERR_CODE_REJECTED => "授权码未被接受。".into(),
        ERR_EXPIRED => "登录已过期。".into(),
        ERR_FAILED => "没能完成 Anthropic 登录。".into(),
        "SSH_AUTH_FAILED" => "无法登录这台服务器。".into(),
        "SSH_UNREACHABLE" => "连不上这台服务器。".into(),
        _ => "没能完成 Anthropic 登录。".into(),
    }
}

fn join_output(output: &RemoteOutput) -> String {
    if output.stderr.is_empty() {
        output.stdout.clone()
    } else {
        format!("{}\n{}", output.stdout, output.stderr)
    }
}

fn looks_like_no_cli(output: &RemoteOutput) -> bool {
    if output.status == 127 {
        return true;
    }
    let blob = join_output(output).to_ascii_lowercase();
    blob.contains("claude_login_no_cli")
        || blob.contains("claude: not found")
        || blob.contains("command not found: claude")
        || (blob.contains("command not found") && blob.contains("claude"))
}

fn looks_like_code_rejected(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("invalid")
        || lower.contains("rejected")
        || lower.contains("incorrect")
        || lower.contains("not accepted")
        || lower.contains("unauthorized")
        || lower.contains("paste code here")
}

fn redact_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "••••")
}

fn parse_json_blob(output: &str) -> Option<serde_json::Value> {
    let trimmed = output.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&trimmed[start..=end]).ok()
}

fn json_logged_in(value: &serde_json::Value) -> Option<bool> {
    value
        .get("loggedIn")
        .or_else(|| value.get("logged_in"))
        .or_else(|| value.get("authenticated"))
        .and_then(|v| v.as_bool())
}

fn json_is_expired(value: &serde_json::Value) -> bool {
    if value.get("expired").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    value
        .get("status")
        .and_then(|v| v.as_str())
        .is_some_and(|status| status.to_ascii_lowercase().contains("expired"))
}

fn parse_status_word(status: &str) -> ClaudeLoginPhase {
    let lower = status.to_ascii_lowercase().replace('_', "-");
    if lower.contains("expired") {
        ClaudeLoginPhase::Expired
    } else if lower.contains("logged-in") || lower == "ok" {
        ClaudeLoginPhase::LoggedIn
    } else if lower.contains("logged-out") || lower.contains("logged-off") {
        ClaudeLoginPhase::LoggedOut
    } else {
        ClaudeLoginPhase::Unknown
    }
}

fn is_claude_login_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "claude.ai"
        || host == "claude.com"
        || host == "platform.claude.com"
        || host.ends_with(".claude.ai")
        || host.ends_with(".claude.com")
}

fn stitch_wrapped_https_urls(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let needle = b"https://";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let consumed = consume_https_url(text, i);
            if consumed.url.len() > "https://".len() {
                let url = trim_trailing_url_junk(&consumed.url);
                if !found.iter().any(|existing| existing == &url) {
                    found.push(url);
                }
                i = consumed.end;
                continue;
            }
        }
        i += 1;
    }
    found
}

struct ConsumedUrl {
    url: String,
    end: usize,
}

fn consume_https_url(text: &str, start: usize) -> ConsumedUrl {
    let mut url = String::new();
    let mut i = start;
    let chars: Vec<(usize, char)> = text[start..]
        .char_indices()
        .map(|(offset, ch)| (start + offset, ch))
        .collect();
    let mut idx = 0;
    while idx < chars.len() {
        let (pos, ch) = chars[idx];
        if ch == '\n' || ch == '\r' {
            let next_idx = skip_wrap_idx(&chars, idx);
            if next_idx == idx {
                break;
            }
            let next_pos = chars[next_idx].0;
            let rest = &text[next_pos..];
            if !can_continue_url(&url, rest) {
                break;
            }
            idx = next_idx;
            i = next_pos;
            continue;
        }
        if ch == ' ' {
            break;
        }
        if !is_url_char(ch) {
            break;
        }
        url.push(ch);
        i = pos + ch.len_utf8();
        idx += 1;
    }
    ConsumedUrl { url, end: i }
}

fn skip_wrap_idx(chars: &[(usize, char)], index: usize) -> usize {
    let mut i = index;
    while i < chars.len() {
        let ch = chars[i].1;
        if ch == '\n' || ch == '\r' || ch == ' ' {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn can_continue_url(url_so_far: &str, remaining: &str) -> bool {
    if remaining.is_empty() || sentence_after_url(remaining) {
        return false;
    }
    let first = remaining.chars().next().unwrap_or('\0');
    if matches!(first, '?' | '&' | '#' | '/' | '=' | '%') {
        return true;
    }
    if url_so_far.ends_with(['?', '&', '=', '/', '%']) {
        return true;
    }
    is_url_char(first) && url_so_far.len() > 20 && !starts_like_sentence(remaining)
}

fn sentence_after_url(remaining: &str) -> bool {
    let lower = remaining.to_ascii_lowercase();
    lower.starts_with("browser")
        || lower.starts_with("paste")
        || lower.starts_with("login")
        || lower.starts_with("welcome")
        || lower.starts_with("tips")
        || lower.starts_with("esc")
        || lower.starts_with("use ")
        || lower.starts_with("and ")
        || lower.starts_with("or ")
        || lower.starts_with("see ")
}

fn starts_like_sentence(remaining: &str) -> bool {
    let mut chars = remaining.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let mut lower = 0;
    for ch in chars {
        if ch.is_ascii_lowercase() {
            lower += 1;
        } else {
            break;
        }
    }
    lower >= 2
}

fn is_url_char(ch: char) -> bool {
    matches!(
        ch,
        'A'..='Z'
            | 'a'..='z'
            | '0'..='9'
            | '-'
            | '.'
            | '_'
            | '~'
            | ':'
            | '/'
            | '?'
            | '#'
            | '['
            | ']'
            | '@'
            | '!'
            | '$'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | ';'
            | '='
            | '%'
    )
}

fn trim_trailing_url_junk(url: &str) -> String {
    url.trim_end_matches([')', ',', '.', ';']).to_string()
}

struct CommandLogger;

impl LoginLogger for CommandLogger {
    fn log(&self, _message: &str) {}
}

#[allow(dead_code)]
pub struct SshRemoteShell {
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
            Ok(output) => output,
            Err(_) => RemoteOutput {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        }
    }
}

fn ssh_run(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote: &str,
) -> Result<RemoteOutput, &'static str> {
    let key = effective_key_path(key_path, target);
    let mut args = ssh_invocation_args(target, key, None);
    let dest = args.pop().ok_or("SSH_PROBE_FAILED")?;
    let plan = crate::claude_ssh::password_auth_plan(password, key, target.use_config);
    if plan.batch_mode && !args.iter().any(|arg| arg == "BatchMode=yes") {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    args.push(dest);
    args.push(remote.to_string());
    let mut command = Command::new("ssh");
    command.args(&args);
    command.env("LC_ALL", "C");
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let askpass = attach_askpass(&mut command, password, key, target.use_config)?;
    let output = command.output().map_err(|_| "SSH_CLIENT_MISSING")?;
    drop(askpass);
    Ok(RemoteOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn make_shell(
    host: &str,
    user: &str,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
) -> Result<SshRemoteShell, &'static str> {
    let alias = host_alias.as_deref().filter(|value| !value.is_empty());
    let target = resolve_from_user_config(host.trim(), Some(user.trim()), port, alias)?;
    Ok(SshRemoteShell {
        target,
        password,
        key_path,
    })
}

fn emit_progress(app: &AppHandle, event: LoginProgress) {
    let _ = app.emit(LOGIN_PROGRESS_EVENT, event);
}

#[tauri::command]
#[allow(dead_code)]
pub async fn lumio_claude_login_start(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
) -> LoginStartPayload {
    tauri::async_runtime::spawn_blocking(move || {
        let host_key = host.trim().to_string();
        match make_shell(&host, &user, port, password, key_path, host_alias) {
            Ok(shell) => {
                let app_for_events = app.clone();
                start_login(&shell, &host_key, &CommandLogger, |event| {
                    emit_progress(&app_for_events, event);
                })
            }
            Err(code) => LoginStartPayload {
                ok: false,
                login_url: None,
                error_code: Some(code.into()),
                detail: Some(human_detail(code)),
            },
        }
    })
    .await
    .unwrap_or_else(|_| LoginStartPayload {
        ok: false,
        login_url: None,
        error_code: Some(ERR_FAILED.into()),
        detail: Some(human_detail(ERR_FAILED)),
    })
}

#[tauri::command]
#[allow(dead_code)]
pub async fn lumio_claude_login_submit(
    app: AppHandle,
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
    code: String,
) -> LoginSubmitPayload {
    tauri::async_runtime::spawn_blocking(move || {
        let host_key = host.trim().to_string();
        match make_shell(&host, &user, port, password, key_path, host_alias) {
            Ok(shell) => {
                let app_for_events = app.clone();
                submit_login(&shell, &host_key, &code, &CommandLogger, |event| {
                    emit_progress(&app_for_events, event);
                })
            }
            Err(code) => LoginSubmitPayload {
                ok: false,
                phase: "fail".into(),
                error_code: Some(code.into()),
                detail: Some(human_detail(code)),
            },
        }
    })
    .await
    .unwrap_or_else(|_| LoginSubmitPayload {
        ok: false,
        phase: "fail".into(),
        error_code: Some(ERR_FAILED.into()),
        detail: Some(human_detail(ERR_FAILED)),
    })
}

#[tauri::command]
#[allow(dead_code)]
pub async fn lumio_claude_login_status(
    host: String,
    user: String,
    port: u16,
    password: Option<String>,
    key_path: Option<String>,
    host_alias: Option<String>,
) -> LoginStatusPayload {
    tauri::async_runtime::spawn_blocking(move || {
        let host_key = host.trim().to_string();
        match make_shell(&host, &user, port, password, key_path, host_alias) {
            Ok(shell) => login_status(&shell, &host_key),
            Err(_) => LoginStatusPayload {
                phase: ClaudeLoginPhase::Unknown.as_str().into(),
                error_code: Some(ERR_FAILED.into()),
            },
        }
    })
    .await
    .unwrap_or_else(|_| LoginStatusPayload {
        phase: ClaudeLoginPhase::Unknown.as_str().into(),
        error_code: Some(ERR_FAILED.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    const FAKE_CODE: &str = "AUTH-CODE-9f3c-DO-NOT-LEAK";
    const HOST: &str = "108.80.81.15";

    struct FakeShell {
        calls: RefCell<Vec<String>>,
        outputs: RefCell<VecDeque<RemoteOutput>>,
    }

    impl FakeShell {
        fn new(outputs: Vec<RemoteOutput>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                outputs: RefCell::new(VecDeque::from(outputs)),
            }
        }
    }

    impl RemoteShell for FakeShell {
        fn run(&self, script: &str) -> RemoteOutput {
            self.calls.borrow_mut().push(script.to_string());
            self.outputs
                .borrow_mut()
                .pop_front()
                .unwrap_or(RemoteOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                })
        }
    }

    struct RecordingLogger {
        lines: RefCell<Vec<String>>,
    }

    impl RecordingLogger {
        fn new() -> Self {
            Self {
                lines: RefCell::new(Vec::new()),
            }
        }
    }

    impl LoginLogger for RecordingLogger {
        fn log(&self, message: &str) {
            self.lines.borrow_mut().push(message.to_string());
        }
    }

    fn out(status: i32, stdout: &str, stderr: &str) -> RemoteOutput {
        RemoteOutput {
            status,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn assert_user_copy_clean(text: &str) {
        let lower = text.to_ascii_lowercase();
        assert!(
            !lower.contains("agent"),
            "user-visible copy leaked agent: {text}"
        );
        assert!(
            !lower.contains("tmux"),
            "user-visible copy leaked tmux: {text}"
        );
        assert!(
            !text.contains(FAKE_CODE),
            "user-visible copy leaked code: {text}"
        );
    }

    #[test]
    fn extract_login_url_from_claude_ai_https() {
        let output = "Open this page:\nhttps://claude.ai/oauth/authorize?code=true&client_id=abc\nPaste code here";
        assert_eq!(
            extract_login_url(output).as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&client_id=abc")
        );
    }

    #[test]
    fn extract_login_url_from_platform_claude_com() {
        let output = "Visit https://platform.claude.com/login/oauth?code=true&x=1 to continue";
        assert_eq!(
            extract_login_url(output).as_deref(),
            Some("https://platform.claude.com/login/oauth?code=true&x=1")
        );
    }

    #[test]
    fn extract_login_url_from_claude_com_oauth() {
        let output = [
            "Browser didn't open? Use the url below to sign in (c to copy)",
            "https://claude.com/cai/oauth/authorize?code=true&client_id=abc",
            "def&state=xyz&redirect_uri=https://claude.com/done",
            "Paste code here if prompted >",
        ]
        .join("\n");
        assert_eq!(
            extract_login_url(&output).as_deref(),
            Some(
                "https://claude.com/cai/oauth/authorize?code=true&client_id=abcdef&state=xyz&redirect_uri=https://claude.com/done"
            )
        );
    }

    #[test]
    fn extract_login_url_ignores_http() {
        let output = "http://claude.ai/oauth/authorize?code=true\nsee also ftp://claude.ai/x";
        assert_eq!(extract_login_url(output), None);
    }

    #[test]
    fn extract_login_url_does_not_glue_the_paste_prompt() {
        let output =
            "https://claude.ai/oauth/authorize?code=true&x=1\nPaste code here if prompted >";
        assert_eq!(
            extract_login_url(output).as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&x=1")
        );
    }

    #[test]
    fn parse_login_status_logged_in() {
        assert_eq!(
            parse_login_status(r#"{"loggedIn":true,"authMethod":"claude.ai"}"#),
            ClaudeLoginPhase::LoggedIn
        );
        assert_eq!(
            parse_login_status("Logged in as user@example.com"),
            ClaudeLoginPhase::LoggedIn
        );
    }

    #[test]
    fn parse_login_status_logged_out() {
        assert_eq!(
            parse_login_status(r#"{"loggedIn":false}"#),
            ClaudeLoginPhase::LoggedOut
        );
        assert_eq!(
            parse_login_status("Not logged in"),
            ClaudeLoginPhase::LoggedOut
        );
    }

    #[test]
    fn parse_login_status_expired() {
        assert_eq!(
            parse_login_status("Login expired · Please run /login"),
            ClaudeLoginPhase::Expired
        );
        assert_eq!(
            parse_login_status(
                "Failed to authenticate: OAuth session expired and could not be refreshed"
            ),
            ClaudeLoginPhase::Expired
        );
        assert_eq!(
            parse_login_status(r#"{"loggedIn":false,"expired":true}"#),
            ClaudeLoginPhase::Expired
        );
    }

    #[test]
    fn parse_login_status_unknown() {
        assert_eq!(parse_login_status(""), ClaudeLoginPhase::Unknown);
        assert_eq!(parse_login_status("???"), ClaudeLoginPhase::Unknown);
    }

    #[test]
    fn start_without_claude_returns_no_cli() {
        let shell = FakeShell::new(vec![out(
            127,
            "CLAUDE_LOGIN_NO_CLI\n",
            "bash: claude: command not found\n",
        )]);
        let logger = RecordingLogger::new();
        let events = RefCell::new(Vec::new());
        let result = start_login(&shell, HOST, &logger, |event| {
            events.borrow_mut().push(event);
        });
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("CLAUDE_LOGIN_NO_CLI"));
        assert!(result.login_url.is_none());
        let detail = result.detail.as_deref().unwrap_or("");
        assert!(detail.contains("Claude"));
        assert_user_copy_clean(detail);
        let script = &shell.calls.borrow()[0];
        assert!(script.contains(r#"export PATH="$HOME/.local/bin:$PATH""#));
        assert!(script.contains("command -v claude"));
        assert!(script.contains("claude auth login"));
        assert!(!script.contains("sudo"));
        for event in events.borrow().iter() {
            if let Some(detail) = &event.detail {
                assert_user_copy_clean(detail);
            }
        }
    }

    #[test]
    fn start_returns_stitched_login_url() {
        let wrapped = [
            "Browser didn't open? Use the url below to sign in (c to copy)",
            "https://claude.ai/oauth/authorize?code=true&client_id=abc",
            "def&state=xyz",
            "Paste code here if prompted >",
        ]
        .join("\n");
        let shell = FakeShell::new(vec![out(0, &wrapped, "")]);
        let logger = RecordingLogger::new();
        let events = RefCell::new(Vec::new());
        let result = start_login(&shell, HOST, &logger, |event| {
            events.borrow_mut().push(event);
        });
        assert!(result.ok);
        assert_eq!(
            result.login_url.as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true&client_id=abcdef&state=xyz")
        );
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| event.login_url.as_deref() == result.login_url.as_deref())
        );
    }

    #[test]
    fn start_without_https_url_returns_no_url() {
        let shell = FakeShell::new(vec![out(0, "waiting for a browser that never opens\n", "")]);
        let logger = RecordingLogger::new();
        let result = start_login(&shell, HOST, &logger, |_| {});
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("CLAUDE_LOGIN_NO_URL"));
        assert_user_copy_clean(result.detail.as_deref().unwrap_or(""));
    }

    #[test]
    fn submit_sends_code_but_never_leaks_it() {
        let shell = FakeShell::new(vec![out(0, r#"{"loggedIn":true}"#, "")]);
        let logger = RecordingLogger::new();
        let events = RefCell::new(Vec::new());
        let result = submit_login(&shell, HOST, FAKE_CODE, &logger, |event| {
            events.borrow_mut().push(event);
        });
        assert!(result.ok);
        assert_eq!(result.phase, "logged-in");
        let script = shell.calls.borrow().join("\n");
        assert!(
            script.contains(FAKE_CODE),
            "remote script must carry the authorization code: {script}"
        );
        let detail = result.detail.clone().unwrap_or_default();
        assert_user_copy_clean(&detail);
        assert_user_copy_clean(&result.phase);
        for event in events.borrow().iter() {
            let blob = format!(
                "{}{}{}{}",
                event.host,
                event.phase,
                event.login_url.clone().unwrap_or_default(),
                event.detail.clone().unwrap_or_default()
            );
            assert_user_copy_clean(&blob);
            if let Some(url) = &event.login_url {
                assert!(url.starts_with("https://") || url.is_empty());
            }
        }
        for line in logger.lines.borrow().iter() {
            assert_user_copy_clean(line);
        }
    }

    #[test]
    fn login_status_reads_server_level_phase() {
        let logged_in = FakeShell::new(vec![out(0, r#"{"loggedIn":true}"#, "")]);
        let status = login_status(&logged_in, HOST);
        assert_eq!(status.phase, "logged-in");
        assert!(logged_in.calls.borrow()[0].contains("claude auth status"));
        assert!(logged_in.calls.borrow()[0].contains(r#"export PATH="$HOME/.local/bin:$PATH""#));

        let logged_out = FakeShell::new(vec![out(1, r#"{"loggedIn":false}"#, "")]);
        assert_eq!(login_status(&logged_out, HOST).phase, "logged-out");

        let expired = FakeShell::new(vec![out(1, "Login expired · Please run /login", "")]);
        let status = login_status(&expired, HOST);
        assert_eq!(status.phase, "expired");
        assert_eq!(status.error_code.as_deref(), Some("CLAUDE_LOGIN_EXPIRED"));
    }
}
