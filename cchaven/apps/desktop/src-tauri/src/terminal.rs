//! Embedded terminal: a PTY running the system `ssh` client against the
//! project's tmux session.
//!
//! The command is built as argv (never a shell string) and the password, when
//! one is needed, is delivered through the askpass socket rather than the
//! command line or the environment.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::askpass::AskpassServer;
use crate::project::{AuthMethod, ServerConfig};
use crate::ssh;

/// Terminal session state.
pub struct TerminalSession {
    pub writer: Box<dyn Write + Send>,
    pub master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Kept alive for the session: OpenSSH may re-prompt after a reconnect.
    askpass: Option<AskpassServer>,
}

/// Manage active terminal sessions by project ID.
pub struct TerminalManager {
    pub sessions: Mutex<HashMap<String, TerminalSession>>,
}

/// Everything needed to attach one project's terminal.
pub struct StartParams<'a> {
    pub project_id: &'a str,
    pub server: &'a ServerConfig,
    pub password: Option<&'a str>,
    pub remote_root: &'a str,
    pub tmux_session: &'a str,
    pub cols: u16,
    pub rows: u16,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Sanitize a tmux session name to prevent shell injection.
    /// Only alphanumeric, dash, and underscore are allowed.
    pub fn sanitize_session_name(name: &str) -> String {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if sanitized.is_empty() {
            "cchaven".into()
        } else {
            sanitized
        }
    }

    /// The remote command that attaches to (or creates) the persistent session.
    pub fn remote_command(tmux_session: &str, remote_root: &str) -> String {
        format!(
            "tmux new-session -A -s {session} -c {root}",
            session = crate::deploy::shell_quote(&Self::sanitize_session_name(tmux_session)),
            root = crate::deploy::shell_quote(remote_root)
        )
    }

    /// Escape a string for safe inclusion in a single-quoted POSIX shell string.
    /// Result is wrapped in single quotes; internal `'` become `'\''`.
    pub fn posix_shell_single_quote(value: &str) -> String {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('\'');
        for ch in value.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }

    /// Build the remote command string (single ssh remote argument).
    /// `remote_root` and session name are shell-escaped / sanitized.
    pub fn build_remote_tmux_cmd(remote_root: &str, tmux_session: &str) -> String {
        let safe_session = Self::sanitize_session_name(tmux_session);
        let safe_root = Self::posix_shell_single_quote(remote_root);
        // Session is already restricted to [A-Za-z0-9_-]; still quote for defense in depth.
        let safe_session_quoted = Self::posix_shell_single_quote(&safe_session);
        // Wrap tmux with env to ensure TERM is properly inherited inside tmux.
        // We export TERM=xterm-256color so the outer SSH pty matches xterm.js,
        // then tmux propagates terminal modes (including cursor key mode) from
        // the outer terminal to its panes.
        format!(
            "env TERM=xterm-256color tmux new-session -A -s {session} -c {root}",
            session = safe_session_quoted,
            root = safe_root
        )
    }

    /// Build the SSH command argv for connecting to a tmux session.
    fn build_ssh_cmd(ssh_alias: &str, remote_root: &str, tmux_session: &str) -> CommandBuilder {
        let remote_cmd = Self::build_remote_tmux_cmd(remote_root, tmux_session);

        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg("-tt");
        cmd.arg("-o");
        cmd.arg("BatchMode=yes");
        cmd.arg("-o");
        cmd.arg("ServerAliveInterval=30");
        cmd.arg("-o");
        cmd.arg("ServerAliveCountMax=3");
        cmd.arg(ssh_alias);
        cmd.arg(remote_cmd);
        // xterm.js emulates xterm-256color; SSH PTY must match.
        cmd.env("TERM", "xterm-256color");
        cmd
    }

    /// Start a new terminal session for a project.
    pub async fn start(&self, params: StartParams<'_>, app: &AppHandle) -> Result<(), String> {
        let StartParams {
            project_id,
            server,
            password,
            remote_root,
            tmux_session,
            cols,
            rows,
        } = params;

        if self
            .sessions
            .lock()
            .map_err(|e| e.to_string())?
            .contains_key(project_id)
        {
            return Err("终端已在运行。".into());
        }

        let askpass = match (server.auth, password) {
            (AuthMethod::Password, Some(password)) => Some(
                AskpassServer::start(password)
                    .await
                    .map_err(|e| format!("无法准备安全凭据通道：{e}"))?,
            ),
            (AuthMethod::Password, None) => {
                return Err("缺少服务器密码，请在项目设置中重新填写。".into());
            }
            _ => None,
        };

        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("无法创建终端：{e}"))?;

        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg("-tt");
        cmd.arg("-o");
        cmd.arg("ServerAliveInterval=30");
        cmd.arg("-o");
        cmd.arg("ServerAliveCountMax=3");
        for arg in ssh::ssh_args(
            server,
            Some(&Self::remote_command(tmux_session, remote_root)),
        ) {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        if let Some(askpass) = &askpass {
            let exe = std::env::current_exe().map_err(|e| format!("无法定位程序路径：{e}"))?;
            cmd.env("SSH_ASKPASS", exe);
            cmd.env("SSH_ASKPASS_REQUIRE", "force");
            cmd.env("DISPLAY", ":0");
            cmd.env(crate::askpass::SOCKET_ENV, askpass.socket_path());
        }

        let _child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("无法启动 SSH：{e}"))?;

        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| format!("无法写入终端：{e}"))?;
        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("无法读取终端输出：{e}"))?;

        let event_name = format!("terminal-output-{project_id}");
        let closed_event = format!("terminal-closed-{project_id}");
        let app_clone = app.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = app_clone.emit(&event_name, buf[..n].to_vec());
                    }
                    Err(_) => break,
                }
            }
            let _ = app_clone.emit(&closed_event, ());
        });

        let session = TerminalSession {
            writer,
            master: Arc::new(Mutex::new(pty_pair.master)),
            askpass,
        };
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .insert(project_id.to_string(), session);

        Ok(())
    }

    /// Write input to a terminal session.
    pub fn write(&self, project_id: &str, data: &[u8]) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get_mut(project_id).ok_or("终端未运行。")?;
        session
            .writer
            .write_all(data)
            .map_err(|e| format!("无法写入终端：{e}"))?;
        session
            .writer
            .flush()
            .map_err(|e| format!("无法写入终端：{e}"))?;
        Ok(())
    }

    /// Resize a terminal session.
    pub fn resize(&self, project_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get(project_id).ok_or("终端未运行。")?;
        let master = session.master.lock().map_err(|e| e.to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("无法调整终端大小：{e}"))
    }

    /// Close a terminal session. Missing sessions are not an error: the UI calls
    /// this on unmount and on every reconnect.
    pub async fn close(&self, project_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|e| e.to_string())?
            .remove(project_id);
        if let Some(session) = session
            && let Some(askpass) = session.askpass
        {
            askpass.shutdown().await;
        }
        Ok(())
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        // On app shutdown, kill all terminal sessions.
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in sessions.drain() {
                let _ = session.child.kill();
                let _ = session.child.wait();
            }
        }
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

// --- Tauri commands ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTerminalRequest {
    pub project_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub async fn start_terminal(
    request: StartTerminalRequest,
    app: AppHandle,
    state: tauri::State<'_, TerminalManager>,
    auth: tauri::State<'_, crate::auth::AuthState>,
) -> Result<(), String> {
    let project = crate::project::ProjectConfig::get(&request.project_id)
        .map_err(|e| format!("无法读取项目配置：{e}"))?
        .ok_or("项目不存在。")?;
    let password = auth
        .secrets()
        .ssh_password(&request.project_id)
        .map_err(|e| e.to_string())?;

    state
        .start(
            StartParams {
                project_id: &request.project_id,
                server: &project.server,
                password: password.as_deref(),
                remote_root: &project.remote_root,
                tmux_session: &project.tmux_session,
                cols: request.cols,
                rows: request.rows,
            },
            &app,
        )
        .await
}

#[tauri::command]
pub fn write_terminal(
    project_id: String,
    data: Vec<u8>,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    state.write(&project_id, &data)
}

#[tauri::command]
pub fn resize_terminal(
    project_id: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    state.resize(&project_id, cols, rows)
}

#[tauri::command]
pub async fn close_terminal(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    state.close(&project_id).await
}

/// Create a new tmux window running `claude` in the session's tmux group.
/// This sends the tmux key sequence to create a new window and launch Claude Code.
#[tauri::command]
pub fn new_claude_session(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix: Ctrl-B, then 'c' creates a new window.
    // After the new window opens, type 'claude' + Enter to start Claude Code.
    let sequence = b"\x02cclaude\r";
    state.write(&project_id, sequence)
}

/// Close the current tmux window (kills the pane). Use when a Claude session is done.
#[tauri::command]
pub fn close_tmux_window(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix: Ctrl-B, then '&' to kill window, then 'y' to confirm.
    let sequence = b"\x02&y";
    state.write(&project_id, sequence)
}

/// Kill all sessions: spawn a fresh SSH process to kill the entire tmux
/// session (and all Claude processes within it) on the remote host.
/// Then close the local PTY so the UI resets.
#[tauri::command]
pub fn kill_all_sessions(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
    app: AppHandle,
) -> Result<(), String> {
    // Extract ssh_alias and tmux_session from the session, then close it.
    let (ssh_alias, tmux_session) = {
        let sessions = state.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(&project_id)
            .ok_or("Terminal session not found")?;
        (session.ssh_alias.clone(), session.tmux_session.clone())
    };

    // Kill only this project's tmux session — never global pkill claude
    // (other projects on the same host must keep their Claude windows).
    let safe_session = TerminalManager::sanitize_session_name(&tmux_session);
    let quoted = TerminalManager::posix_shell_single_quote(&safe_session);
    let kill_cmd = format!("tmux kill-session -t {quoted} 2>/dev/null; echo done");
    let _ = std::process::Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(&ssh_alias)
        .arg(&kill_cmd)
        .output();

    // Now close the local terminal session (kills SSH child + PTY).
    state.close(&project_id)?;

    // Emit closed event so the frontend shows a clean state.
    let closed_event = format!("terminal-closed-{project_id}");
    let _ = app.emit(&closed_event, ());

    Ok(())
}

/// List tmux windows in the current session (sends the tmux window list shortcut).
#[tauri::command]
pub fn list_tmux_windows(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix: Ctrl-B, then 'w' to show window list.
    let sequence = b"\x02w";
    state.write(&project_id, sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(
            TerminalManager::sanitize_session_name("my-project"),
            "my-project"
        );
        assert_eq!(
            TerminalManager::sanitize_session_name("proj_123"),
            "proj_123"
        );
        assert_eq!(TerminalManager::sanitize_session_name("a;b&c"), "a-b-c");
        assert_eq!(
            TerminalManager::sanitize_session_name("$(rm -rf /)"),
            "--rm--rf---"
        );
        assert_eq!(TerminalManager::sanitize_session_name(""), "cchaven");
    }

    #[test]
    fn sanitize_preserves_alphanumeric() {
        let name = "ClaudeCode2026";
        assert_eq!(TerminalManager::sanitize_session_name(name), name);
    }

    #[test]
    fn the_remote_command_quotes_both_interpolations() {
        let command = TerminalManager::remote_command("cchaven-app", "/root/cchaven/app");
        assert_eq!(
            command,
            "tmux new-session -A -s 'cchaven-app' -c '/root/cchaven/app'"
        );

        let hostile = TerminalManager::remote_command("s", "/root/'; id #");
        assert!(hostile.ends_with(r"-c '/root/'\''; id #'"));
    }
}
