//! Per-project SSH local-forward tunnels. Switching tabs does not close them.

use crate::claude_ssh::{ResolvedSshTarget, ssh_invocation_args};
use std::collections::HashMap;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

pub struct TunnelHandle {
    pub local_port: u16,
    child: Child,
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
        remote_port: u16,
    ) -> Result<u16, String> {
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = tunnels.get(project_id) {
            return Ok(existing.local_port);
        }
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|_| "没能打开本机同步通道。".to_string())?;
        let local_port = listener
            .local_addr()
            .map_err(|_| "没能打开本机同步通道。".to_string())?
            .port();
        drop(listener);

        let mut args = ssh_invocation_args(target, key_path, None);
        let dest = args
            .pop()
            .ok_or_else(|| "没能打开本机同步通道。".to_string())?;
        args.insert(0, "-N".into());
        args.insert(1, "-L".into());
        args.insert(2, format!("{local_port}:127.0.0.1:{remote_port}"));
        args.push(dest);
        let child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "这台电脑还没有 ssh 命令。".to_string())?;
        tunnels.insert(project_id.to_string(), TunnelHandle { local_port, child });
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
