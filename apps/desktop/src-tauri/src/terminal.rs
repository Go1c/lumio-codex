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
}

/// Manage active terminal sessions by project ID.
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, TerminalSession>>,
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

        let safe_session = Self::sanitize_session_name(tmux_session);

        // Build SSH command with argv-only (no shell string concatenation).
        let remote_cmd = format!(
            "tmux new-session -A -s {session} -c {root}",
            session = safe_session,
            root = remote_root
        );

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
        cmd.env("TERM", "xterm-256color");

        let _child = pty_pair
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

    /// Close a terminal session.
    pub fn close(&self, project_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        sessions
            .remove(project_id)
            .ok_or("Terminal session not found")?;
        Ok(())
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
            "----rm--rf---"
        );
    }

    #[test]
    fn sanitize_preserves_alphanumeric() {
        let name = "ClaudeCode2026";
        assert_eq!(TerminalManager::sanitize_session_name(name), name);
    }
}
