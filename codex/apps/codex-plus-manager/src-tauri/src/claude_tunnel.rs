//! Per-project SSH local-forward tunnels. Switching tabs does not close them.

use crate::claude_ssh::{AskpassGuard, ResolvedSshTarget, attach_askpass, ssh_invocation_args};
use std::collections::HashMap;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

pub struct TunnelHandle {
    pub local_port: u16,
    child: Child,
    _askpass: Option<AskpassGuard>,
}

pub struct TunnelManager {
    tunnels: Mutex<HashMap<String, TunnelHandle>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: Mutex::new(HashMap::new()),
        }
    }

    pub fn open(
        &self,
        project_id: &str,
        target: &ResolvedSshTarget,
        key_path: Option<&str>,
        password: Option<&str>,
        remote_port: u16,
    ) -> Result<u16, String> {
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = tunnels.get_mut(project_id)
            && tunnel_child_alive(&mut existing.child)
        {
            return Ok(existing.local_port);
        }
        tunnels.remove(project_id);
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|_| "没能打开本机同步通道。".to_string())?;
        let local_port = listener
            .local_addr()
            .map_err(|_| "没能打开本机同步通道。".to_string())?
            .port();
        drop(listener);

        let key = crate::claude_ssh::effective_key_path(key_path, target);
        let args = sync_tunnel_ssh_args(
            ssh_invocation_args(target, key, None),
            local_port,
            remote_port,
        )?;
        let mut command = Command::new("ssh");
        command.args(&args);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        let askpass = attach_askpass(&mut command, password, key, target.use_config)
            .map_err(|_| "无法准备安全凭据通道。".to_string())?;
        let child = command
            .spawn()
            .map_err(|_| "这台电脑还没有 ssh 命令。".to_string())?;
        tunnels.insert(
            project_id.to_string(),
            TunnelHandle {
                local_port,
                child,
                _askpass: askpass,
            },
        );
        Ok(local_port)
    }
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn sync_tunnel_ssh_args(
    invocation: Vec<String>,
    local_port: u16,
    remote_port: u16,
) -> Result<Vec<String>, String> {
    let mut args = invocation;
    let dest = args
        .pop()
        .ok_or_else(|| "没能打开本机同步通道。".to_string())?;
    args.insert(0, "-N".into());
    args.insert(1, "-L".into());
    args.insert(2, format!("{local_port}:127.0.0.1:{remote_port}"));
    args.extend([
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
    ]);
    args.push(dest);
    Ok(args)
}

pub fn tunnel_child_alive(child: &mut Child) -> bool {
    child.try_wait().ok().flatten().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_tunnel_keeps_the_ssh_session_alive() {
        let args = sync_tunnel_ssh_args(
            vec![
                "-o".into(),
                "BatchMode=yes".into(),
                "root@108.80.81.15".into(),
            ],
            43123,
            9000,
        )
        .expect("args");
        let joined = args.join(" ");
        assert!(
            joined.contains("ServerAliveInterval=30"),
            "idle NAT drops kill sync after ~1–2 min without keepalive: {joined}"
        );
        assert!(
            joined.contains("ServerAliveCountMax=3"),
            "missing ServerAliveCountMax: {joined}"
        );
        assert!(
            joined.contains("ExitOnForwardFailure=yes"),
            "forward failure must not look like a live tunnel: {joined}"
        );
        assert!(joined.contains("-N"));
        assert!(joined.contains("43123:127.0.0.1:9000"));
        assert_eq!(args.last().map(String::as_str), Some("root@108.80.81.15"));
        assert!(!joined.to_ascii_lowercase().contains("tmux"));
        assert!(!joined.to_ascii_lowercase().contains("agent"));
    }

    #[test]
    fn exited_tunnel_process_is_not_treated_as_live() {
        let mut child = Command::new("true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn true");
        let _ = child.wait();
        assert!(
            !tunnel_child_alive(&mut child),
            "a finished ssh child must not be reused as a live tunnel"
        );
    }
}
