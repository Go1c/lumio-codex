//! Embedded PTY: system `ssh` into a persistent remote session.
//!
//! Switching Codex / Claude tabs, or selecting another project, must not kill
//! other projects' sessions. User-visible errors never mention the session tool.

use crate::claude_ssh::{AskpassGuard, ResolvedSshTarget, remote_shell_path, ssh_invocation_args};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub const DEFAULT_SESSION_ID: &str = "default";

pub fn terminal_key(project_id: &str, session_id: &str) -> String {
    format!("{project_id}::{session_id}")
}

pub fn effective_session_id(session_id: Option<&str>) -> &str {
    match session_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => id,
        None => DEFAULT_SESSION_ID,
    }
}

pub fn remote_session_name(project_id: &str, session_id: &str) -> String {
    format!(
        "bestcodex-{project}-{session}",
        project = TerminalManager::sanitize_session_name(project_id),
        session = TerminalManager::sanitize_session_name(session_id),
    )
}

pub fn terminal_output_event(project_id: &str, session_id: &str) -> String {
    format!("lumio://claude-terminal-output-{project_id}-{session_id}")
}

pub fn terminal_closed_event(project_id: &str, session_id: &str) -> String {
    format!("lumio://claude-terminal-closed-{project_id}-{session_id}")
}

pub fn legacy_terminal_output_event(project_id: &str) -> String {
    format!("lumio://claude-terminal-output-{project_id}")
}

pub fn legacy_terminal_closed_event(project_id: &str) -> String {
    format!("lumio://claude-terminal-closed-{project_id}")
}

fn with_legacy_events(session_id: &str, primary: String, legacy: String) -> Vec<String> {
    if session_id == DEFAULT_SESSION_ID {
        vec![primary, legacy]
    } else {
        vec![primary]
    }
}

pub fn terminal_output_events(project_id: &str, session_id: &str) -> Vec<String> {
    with_legacy_events(
        session_id,
        terminal_output_event(project_id, session_id),
        legacy_terminal_output_event(project_id),
    )
}

pub fn terminal_closed_events(project_id: &str, session_id: &str) -> Vec<String> {
    with_legacy_events(
        session_id,
        terminal_closed_event(project_id, session_id),
        legacy_terminal_closed_event(project_id),
    )
}

pub fn open_remote_session_command(
    project_id: &str,
    session_id: &str,
    remote_root: &str,
) -> String {
    TerminalManager::remote_command(&remote_session_name(project_id, session_id), remote_root)
}

pub fn close_remote_session_command(_project_id: &str, _session_id: &str) -> String {
    // Local PTY kill tears down the SSH child; do not wrap Claude in a remote
    // session manager (its status bar is user-visible).
    "echo done".into()
}

pub fn chat_ids_for_project<V>(sessions: &HashMap<String, V>, project_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = sessions
        .keys()
        .filter_map(|key| {
            let (proj, session) = key.split_once("::")?;
            (proj == project_id).then(|| session.to_string())
        })
        .collect();
    ids.sort();
    ids
}

pub fn remove_chat<V>(
    sessions: &mut HashMap<String, V>,
    project_id: &str,
    session_id: &str,
) -> Option<V> {
    sessions.remove(&terminal_key(project_id, session_id))
}

pub struct TerminalSession {
    writer: Box<dyn Write + Send>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
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

    pub fn remote_command(_session: &str, remote_root: &str) -> String {
        format!(
            "cd {root} && PATH=$HOME/.local/bin:$PATH env TERM=xterm-256color exec $HOME/.local/bin/claude",
            root = remote_shell_path(remote_root)
        )
    }

    pub fn start(
        &self,
        project_id: &str,
        session_id: Option<&str>,
        target: &ResolvedSshTarget,
        key_path: Option<&str>,
        password: Option<&str>,
        remote_root: &str,
        cols: u16,
        rows: u16,
        app: &AppHandle,
    ) -> Result<(), String> {
        let session_id = effective_session_id(session_id);
        let map_key = terminal_key(project_id, session_id);
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&map_key) {
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
            Some(&open_remote_session_command(
                project_id,
                session_id,
                remote_root,
            )),
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

        let child = pty_pair
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

        let output_events = terminal_output_events(project_id, session_id);
        let closed_events = terminal_closed_events(project_id, session_id);
        let app_clone = app.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for name in &output_events {
                            let _ = app_clone.emit(name, buf[..n].to_vec());
                        }
                    }
                    Err(_) => break,
                }
            }
            for name in &closed_events {
                let _ = app_clone.emit(name, ());
            }
        });

        sessions.insert(
            map_key,
            TerminalSession {
                writer,
                master: Arc::new(Mutex::new(pty_pair.master)),
                child,
                _askpass: askpass,
            },
        );
        Ok(())
    }

    pub fn write(
        &self,
        project_id: &str,
        session_id: Option<&str>,
        bytes: &[u8],
    ) -> Result<(), String> {
        let map_key = terminal_key(project_id, effective_session_id(session_id));
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get_mut(&map_key)
            .ok_or_else(|| "终端还没打开。".to_string())?;
        session
            .writer
            .write_all(bytes)
            .map_err(|_| "无法写入终端。".to_string())
    }

    pub fn resize(
        &self,
        project_id: &str,
        session_id: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let map_key = terminal_key(project_id, effective_session_id(session_id));
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(&map_key)
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

    pub fn close(&self, project_id: &str, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(mut session) = remove_chat(&mut sessions, project_id, session_id) else {
            return Ok(());
        };
        drop(sessions);
        let _ = session.child.kill();
        Ok(())
    }

    pub fn list_chats(&self, project_id: &str) -> Result<Vec<String>, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        Ok(chat_ids_for_project(&sessions, project_id))
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
        assert!(command.contains("~/'bestcodex/my-project'"));
        assert!(
            !command.contains('"'),
            "tilde path must not add double quotes"
        );
        assert!(command.contains(".local/bin/claude"));
    }

    #[test]
    fn terminal_key_distinguishes_sessions_and_projects() {
        assert_ne!(
            terminal_key("alpha", "chat-1"),
            terminal_key("alpha", "chat-2")
        );
        assert_ne!(
            terminal_key("alpha", "chat-1"),
            terminal_key("beta", "chat-1")
        );
        assert_eq!(terminal_key("alpha", "chat-1"), "alpha::chat-1");
    }

    #[test]
    fn open_session_command_includes_quoted_root() {
        let command = open_remote_session_command("my project", "chat 1", "~/bestcodex/my-project");
        assert!(
            command.contains("~/'bestcodex/my-project'"),
            "open command must include the quoted project root"
        );
        assert!(
            !command.contains("sudo"),
            "open command must not use elevated execution"
        );
    }

    #[test]
    fn new_conversation_starts_official_claude_in_the_project() {
        let command = open_remote_session_command("p-docs", "s-new", "~/bestcodex/docs");
        assert!(
            command.contains(".local/bin/claude"),
            "new conversation must run official Claude in the project, not a bare shell: {command}"
        );
        assert!(command.contains("~/'bestcodex/docs'"));
        assert!(
            !command.contains('"'),
            "double quotes break sshd's shell -c wrapper: {command}"
        );
        assert!(!command.contains("sudo"));
    }

    #[test]
    fn new_conversation_command_does_not_surface_session_manager() {
        let command = open_remote_session_command("p-docs", "s-new", "~/bestcodex/docs");
        let lower = command.to_ascii_lowercase();
        assert!(
            !lower.contains("tmux"),
            "wrapping Claude in a session manager paints a status bar into the user's terminal: {command}"
        );
        assert!(
            command.contains(".local/bin/claude"),
            "must still start official Claude: {command}"
        );
    }

    #[test]
    fn close_session_command_does_not_use_a_session_manager() {
        let close = close_remote_session_command("my project", "chat 1");
        assert!(
            !close.to_ascii_lowercase().contains("tmux"),
            "close command must not mention a session manager: {close}"
        );
        assert!(
            !close.contains("sudo"),
            "close command must not use elevated execution"
        );
    }

    #[test]
    fn event_names_include_project_and_session() {
        assert_eq!(
            terminal_output_event("proj", "chat-2"),
            "lumio://claude-terminal-output-proj-chat-2"
        );
        assert_eq!(
            terminal_closed_event("proj", "chat-2"),
            "lumio://claude-terminal-closed-proj-chat-2"
        );
    }

    #[test]
    fn default_session_also_uses_legacy_event_names() {
        let output = terminal_output_events("proj", DEFAULT_SESSION_ID);
        assert!(output.contains(&"lumio://claude-terminal-output-proj-default".to_string()));
        assert!(output.contains(&legacy_terminal_output_event("proj")));
        assert_eq!(
            legacy_terminal_output_event("proj"),
            "lumio://claude-terminal-output-proj"
        );
        let closed = terminal_closed_events("proj", DEFAULT_SESSION_ID);
        assert!(closed.contains(&"lumio://claude-terminal-closed-proj-default".to_string()));
        assert!(closed.contains(&legacy_terminal_closed_event("proj")));
        let other = terminal_output_events("proj", "chat-2");
        assert!(!other.contains(&legacy_terminal_output_event("proj")));
        assert!(
            !terminal_closed_events("proj", "chat-2")
                .contains(&legacy_terminal_closed_event("proj"))
        );
    }

    #[test]
    fn closing_one_chat_leaves_the_other_keys() {
        let mut sessions = HashMap::new();
        sessions.insert(terminal_key("alpha", "a"), ());
        sessions.insert(terminal_key("alpha", "b"), ());
        sessions.insert(terminal_key("beta", "a"), ());
        assert!(remove_chat(&mut sessions, "alpha", "a").is_some());
        assert_eq!(
            chat_ids_for_project(&sessions, "alpha"),
            vec!["b".to_string()]
        );
        assert_eq!(
            chat_ids_for_project(&sessions, "beta"),
            vec!["a".to_string()]
        );
        assert!(remove_chat(&mut sessions, "alpha", "a").is_none());
    }

    #[test]
    fn list_and_close_without_a_pty_are_safe() {
        let manager = TerminalManager::new();
        assert_eq!(manager.list_chats("alpha").unwrap(), Vec::<String>::new());
        assert!(manager.close("alpha", "missing").is_ok());
    }
}
