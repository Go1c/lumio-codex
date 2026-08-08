//! SSH LocalForward tunnel management.
//!
//! Creates an SSH tunnel from a local random port to the remote server's
//! FNS Server (127.0.0.1:9000). Uses system OpenSSH with argv-only (no shell
//! string concatenation). Does NOT copy or host SSH private keys.

use std::process::{Child, Command, Stdio};

/// An active SSH LocalForward tunnel.
pub struct SshTunnel {
    local_port: u16,
    child: Option<Child>,
}

impl SshTunnel {
    /// Create an SSH tunnel from a local random port to the remote FNS Server.
    ///
    /// `ssh_alias` is a host from ~/.ssh/config.
    /// `remote_port` is the port FNS Server listens on (default 9000).
    pub fn create(ssh_alias: &str, remote_port: u16) -> Result<Self, String> {
        // Bind a local port to find an available one.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener); // Release so SSH can bind it.

        let child = Command::new("ssh")
            .arg("-N") // No remote command
            .arg("-L")
            .arg(format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"))
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg(ssh_alias)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start SSH: {e}"))?;

        Ok(Self {
            local_port,
            child: Some(child),
        })
    }

    /// The local port the tunnel is listening on.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// The local WebSocket endpoint URL.
    pub fn local_endpoint(&self) -> String {
        format!(
            "ws://127.0.0.1:{}/api/user/workspace-sync/v2",
            self.local_port
        )
    }

    /// Check if the SSH process is still alive.
    pub fn is_alive(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => true,
                _ => false,
            }
        } else {
            false
        }
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// --- Tauri commands ---

/// State-managed SSH tunnel.
pub struct TunnelState {
    tunnel: std::sync::Mutex<Option<SshTunnel>>,
}

impl TunnelState {
    pub fn new() -> Self {
        Self {
            tunnel: std::sync::Mutex::new(None),
        }
    }
}

impl Default for TunnelState {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub fn create_tunnel(
    ssh_alias: String,
    remote_port: Option<u16>,
    state: tauri::State<'_, TunnelState>,
) -> Result<u16, String> {
    let port = remote_port.unwrap_or(9000);
    let tunnel = SshTunnel::create(&ssh_alias, port)?;
    let local_port = tunnel.local_port();
    *state.tunnel.lock().map_err(|e| e.to_string())? = Some(tunnel);
    Ok(local_port)
}

#[tauri::command]
pub fn tunnel_endpoint(state: tauri::State<'_, TunnelState>) -> Result<String, String> {
    let guard = state.tunnel.lock().map_err(|e| e.to_string())?;
    guard
        .as_ref()
        .map(|t| t.local_endpoint())
        .ok_or_else(|| "No tunnel active".into())
}

#[tauri::command]
pub fn close_tunnel(state: tauri::State<'_, TunnelState>) -> Result<(), String> {
    let mut guard = state.tunnel.lock().map_err(|e| e.to_string())?;
    *guard = None; // Dropping SshTunnel kills the SSH process.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_tunnel_struct_exists() {
        // Type-level check: SshTunnel must implement Drop.
        fn assert_drop<T: Drop>() {}
        assert_drop::<SshTunnel>();
    }
}
