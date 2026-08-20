//! Embedded PTY: system `ssh` into a persistent remote session.
//!
//! Switching Codex / Claude tabs, or selecting another project, must not kill
//! other projects' sessions. User-visible errors never mention the session tool.

use crate::claude_ssh::{remote_shell_path, ssh_invocation_args, AskpassGuard, ResolvedSshTarget};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub fn terminal_output_event(project_id: &str) -> String {
    format!("lumio://claude-terminal-output-{project_id}")
}

pub fn terminal_closed_event(project_id: &str) -> String {
    format!("lumio://claude-terminal-closed-{project_id}")
}

pub struct TerminalSession {
    writer: Box<dyn Write + Send>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    _askpass: Option<AskpassGuard>,
}

pub struct TerminalManager {
    sessions: Mutex<HashMap<String, TerminalSession>>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

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
            "bestcodex".into()
        } else {
            sanitized
        }
    }

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

    pub fn remote_command(session: &str, remote_root: &str) -> String {
        format!(
            "env TERM=xterm-256color tmux new-session -A -s {session} -c {root}",
            session = Self::posix_shell_single_quote(&Self::sanitize_session_name(session)),
            root = remote_shell_path(remote_root)
        )
    }

    pub fn start(
        &self,
        project_id: &str,
        target: &ResolvedSshTarget,
        key_path: Option<&str>,
        password: Option<&str>,
        remote_root: &str,
        cols: u16,
        rows: u16,
        app: &AppHandle,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(project_id) {
            return Ok(());
        }

        let key = crate::claude_ssh::effective_key_path(key_path, target);
        let plan = crate::claude_ssh::password_auth_plan(password, key, target.use_config);
        let askpass = if plan.use_askpass {
            let secret = password.ok_or_else(|| "缺少服务器密码，请重新填写。".to_string())?;
            Some(AskpassGuard::start(secret).map_err(|_| "无法准备安全凭据通道。".to_string())?)
        } else if key.is_some() || target.use_config {
            None
        } else {
            return Err("缺少服务器密码，请重新填写。".into());
        };

        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| "无法创建终端。".to_string())?;

        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg("-tt");
        cmd.arg("-o");
        cmd.arg("ServerAliveInterval=30");
        for arg in ssh_invocation_args(
            target,
            key,
            Some(&Self::remote_command(project_id, remote_root)),
        ) {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        if let (Some(askpass), Some(password)) = (&askpass, password) {
            cmd.env("SSH_ASKPASS", askpass.script.as_os_str());
            cmd.env("SSH_ASKPASS_REQUIRE", "force");
            cmd.env("DISPLAY", ":0");
            cmd.env("BESTCODEX_SSH_ASKPASS", password);
        }

        let _child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|_| "无法启动远程会话。".to_string())?;
        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|_| "无法写入终端。".to_string())?;
        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|_| "无法读取终端输出。".to_string())?;

        let event_name = terminal_output_event(project_id);
        let closed_event = terminal_closed_event(project_id);
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

        sessions.insert(
            project_id.to_string(),
            TerminalSession {
                writer,
                master: Arc::new(Mutex::new(pty_pair.master)),
                _askpass: askpass,
            },
        );
        Ok(())
    }

    pub fn write(&self, project_id: &str, bytes: &[u8]) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get_mut(project_id)
            .ok_or_else(|| "终端还没打开。".to_string())?;
        session
            .writer
            .write_all(bytes)
            .map_err(|_| "无法写入终端。".to_string())
    }

    pub fn resize(&self, project_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(project_id)
            .ok_or_else(|| "终端还没打开。".to_string())?;
        let master = session.master.lock().map_err(|e| e.to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| "无法调整终端大小。".to_string())
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_command_quotes_the_project_root() {
        let command = TerminalManager::remote_command("my project", "~/bestcodex/my-project");
        assert!(command.contains("\"$HOME\"/'bestcodex/my-project'"));
        assert!(command.contains("'my-project'"));
    }
}
