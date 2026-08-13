//! Embedded terminal: a PTY running the system `ssh` client against the
//! project's tmux session.
//!
//! The command is built as argv (never a shell string) and the password, when
//! one is needed, is delivered through the askpass socket rather than the
//! command line or the environment. On top of plain attach/detach the manager
//! drives the remote tmux session: open a new Claude window, close the current
//! one, list windows, and tear the whole session down.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::askpass::AskpassServer;
use crate::project::{AuthMethod, ProjectConfig, ServerConfig};
use crate::ssh;

/// Terminal session state.
pub struct TerminalSession {
    pub writer: Box<dyn Write + Send>,
    pub master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Sanitised tmux session name, needed to tear the remote session down.
    pub tmux_session: String,
    /// Kept alive for the session: OpenSSH may re-prompt after a reconnect.
    askpass: Option<AskpassServer>,
}

/// Manage active terminal sessions by project ID.
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, TerminalSession>>,
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

    /// The remote command that attaches to (or creates) the persistent session.
    ///
    /// `TERM` is exported inside the remote shell as well as on the local PTY:
    /// tmux only propagates terminal modes (cursor keys in particular) to its
    /// panes when the outer terminal already advertises `xterm-256color`.
    pub fn remote_command(tmux_session: &str, remote_root: &str) -> String {
        format!(
            "env TERM=xterm-256color tmux new-session -A -s {session} -c {root}",
            session = Self::posix_shell_single_quote(&Self::sanitize_session_name(tmux_session)),
            root = Self::posix_shell_single_quote(remote_root)
        )
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
            tmux_session: Self::sanitize_session_name(tmux_session),
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

    /// The sanitised tmux session name of a running terminal.
    pub fn tmux_session_of(&self, project_id: &str) -> Result<String, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        Ok(sessions
            .get(project_id)
            .ok_or("终端未运行。")?
            .tmux_session
            .clone())
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

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

/// The remote command that tears down one project's tmux session.
///
/// Scoped to this project's session on purpose: a global `pkill claude` would
/// take out other projects sharing the same host.
pub fn kill_session_command(tmux_session: &str) -> String {
    let quoted = TerminalManager::posix_shell_single_quote(
        &TerminalManager::sanitize_session_name(tmux_session),
    );
    format!("tmux kill-session -t {quoted} 2>/dev/null; echo done")
}

// --- Tauri commands ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTerminalRequest {
    pub project_id: String,
    pub cols: u16,
    pub rows: u16,
}

/// Load the project and its stored password — every remote action needs both.
async fn project_and_password(
    project_id: &str,
    auth: &crate::auth::AuthState,
) -> Result<(ProjectConfig, Option<String>), String> {
    let project = ProjectConfig::get(project_id)
        .map_err(|e| format!("无法读取项目配置：{e}"))?
        .ok_or("项目不存在。")?;
    let password = auth
        .secrets()
        .ssh_password(project_id)
        .map_err(|e| e.to_string())?;
    Ok((project, password))
}

#[tauri::command]
pub async fn start_terminal(
    request: StartTerminalRequest,
    app: AppHandle,
    state: tauri::State<'_, TerminalManager>,
    auth: tauri::State<'_, crate::auth::AuthState>,
) -> Result<(), String> {
    let (project, password) = project_and_password(&request.project_id, &auth).await?;

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

/// Open a new tmux window and start Claude Code in it.
#[tauri::command]
pub fn new_claude_session(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix Ctrl-B, then `c` for a new window, then run `claude`.
    state.write(&project_id, b"\x02cclaude\r")
}

/// Close the current tmux window (kills the pane) once a Claude session is done.
#[tauri::command]
pub fn close_tmux_window(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix Ctrl-B, then `&` to kill the window, then `y` to confirm.
    state.write(&project_id, b"\x02&y")
}

/// Show tmux's own window list inside the terminal.
#[tauri::command]
pub fn list_tmux_windows(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    // tmux prefix Ctrl-B, then `w`.
    state.write(&project_id, b"\x02w")
}

/// Kill this project's whole tmux session on the server (and with it every
/// Claude process inside), then drop the local PTY so the UI resets.
#[tauri::command]
pub async fn kill_all_sessions(
    project_id: String,
    app: AppHandle,
    state: tauri::State<'_, TerminalManager>,
    auth: tauri::State<'_, crate::auth::AuthState>,
) -> Result<(), String> {
    let tmux_session = state.tmux_session_of(&project_id)?;
    let (project, password) = project_and_password(&project_id, &auth).await?;

    // Best effort: even if the host is unreachable the local terminal must go
    // away, otherwise the UI is stuck on a dead session.
    let _ = ssh::run_ssh(
        &project.server,
        password.as_deref(),
        &kill_session_command(&tmux_session),
    )
    .await;

    state.close(&project_id).await?;
    let _ = app.emit(&format!("terminal-closed-{project_id}"), ());
    Ok(())
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
    fn posix_shell_single_quote_escapes_embedded_quotes() {
        assert_eq!(
            TerminalManager::posix_shell_single_quote("it's"),
            "'it'\\''s'"
        );
        assert_eq!(
            TerminalManager::posix_shell_single_quote("plain"),
            "'plain'"
        );
        assert_eq!(TerminalManager::posix_shell_single_quote("a;b"), "'a;b'");
    }

    #[test]
    fn the_remote_command_quotes_both_interpolations() {
        let command = TerminalManager::remote_command("cchaven-app", "/root/cchaven/app");
        assert_eq!(
            command,
            "env TERM=xterm-256color tmux new-session -A -s 'cchaven-app' -c '/root/cchaven/app'"
        );

        let hostile = TerminalManager::remote_command("s", "/root/'; id #");
        assert!(hostile.ends_with(r"-c '/root/'\''; id #'"));
    }

    #[test]
    fn terminal_remote_root_is_shell_escaped() {
        let injection = "/tmp/proj; touch /tmp/pwned; #";
        let cmd = TerminalManager::remote_command("my-session", injection);

        let expected_root = TerminalManager::posix_shell_single_quote(injection);
        assert!(
            cmd.contains(&format!("-c {expected_root}")),
            "root must be a single quoted -c argument: {cmd}"
        );
        assert!(
            !cmd.contains("-c /tmp/proj;"),
            "unescaped remote_root must not appear: {cmd}"
        );
        assert!(cmd.contains("tmux new-session"));
    }

    #[test]
    fn session_name_is_sanitized_and_quoted_in_remote_cmd() {
        let session = "evil;rm -rf /";
        let sanitized = TerminalManager::sanitize_session_name(session);
        let cmd = TerminalManager::remote_command(session, "/home/u/p");
        let quoted = TerminalManager::posix_shell_single_quote(&sanitized);
        assert!(
            cmd.contains(&quoted),
            "expected quoted sanitized session {quoted:?} in {cmd}"
        );
        assert!(!cmd.contains("evil;rm"));
        assert_eq!(sanitized, "evil-rm--rf--");
    }

    #[test]
    fn kill_targets_only_this_projects_session() {
        let command = kill_session_command("cchaven-app");
        assert_eq!(
            command,
            "tmux kill-session -t 'cchaven-app' 2>/dev/null; echo done"
        );
        // Never a host-wide sweep: other projects share the machine.
        assert!(!command.contains("pkill"));
        assert!(!command.contains("kill-server"));

        let hostile = kill_session_command("a'; rm -rf /; #");
        assert!(!hostile.contains("rm -rf /"), "{hostile}");
    }
}
