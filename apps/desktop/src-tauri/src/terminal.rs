//! Embedded terminal: PTY-based SSH connection to remote tmux sessions.
//!
//! Uses portable-pty to spawn a system SSH process (no shell string concatenation),
//! connecting to the project's tmux session. xterm.js renders the terminal in the
//! frontend; Tauri events bridge PTY output to the frontend.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Terminal session state.
pub struct TerminalSession {
    pub writer: Box<dyn Write + Send>,
    pub master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub child: Box<dyn portable_pty::Child + Send>,
    pub ssh_alias: String,
    pub tmux_session: String,
}

/// Manage active terminal sessions by project ID.
pub struct TerminalManager {
    pub sessions: Mutex<HashMap<String, TerminalSession>>,
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
        name.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
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
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        project_id: &str,
        ssh_alias: &str,
        remote_root: &str,
        tmux_session: &str,
        cols: u16,
        rows: u16,
        app: &AppHandle,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(project_id) {
            return Err("Terminal session already exists".into());
        }

        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {e}"))?;

        let cmd = Self::build_ssh_cmd(ssh_alias, remote_root, tmux_session);

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn SSH: {e}"))?;

        // Get writer and reader.
        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {e}"))?;

        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to clone PTY reader: {e}"))?;

        // Spawn a thread to read PTY output and emit Tauri events.
        let event_name = format!("terminal-output-{project_id}");
        let closed_event = format!("terminal-closed-{project_id}");
        let app_clone = app.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        let _ = app_clone.emit(&event_name, data);
                    }
                    Err(_) => break,
                }
            }
            let _ = app_clone.emit(&closed_event, ());
        });

        let session = TerminalSession {
            writer,
            master: Arc::new(Mutex::new(pty_pair.master)),
            child,
            ssh_alias: ssh_alias.to_string(),
            tmux_session: Self::sanitize_session_name(tmux_session),
        };
        sessions.insert(project_id.to_string(), session);

        Ok(())
    }

    /// Write input to a terminal session.
    pub fn write(&self, project_id: &str, data: &[u8]) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get_mut(project_id)
            .ok_or("Terminal session not found")?;
        session
            .writer
            .write_all(data)
            .map_err(|e| format!("Failed to write to PTY: {e}"))?;
        session
            .writer
            .flush()
            .map_err(|e| format!("Failed to flush PTY: {e}"))?;
        Ok(())
    }

    /// Resize a terminal session.
    pub fn resize(&self, project_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(project_id)
            .ok_or("Terminal session not found")?;
        let master = session.master.lock().map_err(|e| e.to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to resize PTY: {e}"))
    }

    /// Close a terminal session: kill the SSH child process, then drop everything.
    pub fn close(&self, project_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(mut session) = sessions.remove(project_id) {
            // Kill the SSH child process first so the reader thread unblocks.
            let _ = session.child.kill();
            let _ = session.child.wait();
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
    pub ssh_host_alias: String,
    pub remote_root: String,
    pub tmux_session: String,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn start_terminal(
    request: StartTerminalRequest,
    state: tauri::State<'_, TerminalManager>,
    app: AppHandle,
) -> Result<(), String> {
    state.start(
        &request.project_id,
        &request.ssh_host_alias,
        &request.remote_root,
        &request.tmux_session,
        request.cols,
        request.rows,
        &app,
    )
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
pub fn close_terminal(
    project_id: String,
    state: tauri::State<'_, TerminalManager>,
) -> Result<(), String> {
    state.close(&project_id)
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

    // Spawn a fresh SSH process to kill the specific tmux session.
    // This works even if the main PTY is stuck.
    let kill_cmd = format!(
        "tmux kill-session -t {tmux_session} 2>/dev/null; pkill -f claude 2>/dev/null; echo done"
    );
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
    }

    #[test]
    fn sanitize_preserves_alphanumeric() {
        let name = "ClaudeCode2026";
        assert_eq!(TerminalManager::sanitize_session_name(name), name);
    }

    #[test]
    fn terminal_remote_root_is_shell_escaped() {
        let injection = "/tmp/proj; touch /tmp/pwned; #";
        let cmd = TerminalManager::build_remote_tmux_cmd(injection, "my-session");

        // POSIX single-quoted form wraps the entire root so `;` is data, not syntax.
        let expected_root = TerminalManager::posix_shell_single_quote(injection);
        assert!(
            cmd.contains(&expected_root),
            "escaped root missing from cmd: {cmd}"
        );
        assert!(
            cmd.contains(&format!("-c {expected_root}")),
            "root must be a single quoted -c argument: {cmd}"
        );
        // Unescaped form would put shell metacharacters outside quotes.
        assert!(
            !cmd.contains("-c /tmp/proj;"),
            "unescaped remote_root must not appear: {cmd}"
        );
        assert!(cmd.contains("tmux new-session"));
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
    fn session_name_is_sanitized_and_quoted_in_remote_cmd() {
        let session = "evil;rm -rf /";
        let sanitized = TerminalManager::sanitize_session_name(session);
        let cmd = TerminalManager::build_remote_tmux_cmd("/home/u/p", session);
        let quoted = TerminalManager::posix_shell_single_quote(&sanitized);
        // Session chars are sanitized before quoting.
        assert!(
            cmd.contains(&quoted),
            "expected quoted sanitized session {quoted:?} in {cmd}"
        );
        assert!(!cmd.contains("evil;rm"));
        assert_eq!(sanitized, "evil-rm--rf--");
    }
}
