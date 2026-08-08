//! SSH config parsing — reads ~/.ssh/config to discover available hosts.
//!
//! Does NOT read or copy SSH private keys. Only parses host aliases and connection
//! parameters from the OpenSSH config file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A parsed SSH host entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
}

/// Parse the user's ~/.ssh/config to discover host aliases.
pub fn parse_ssh_config() -> Result<Vec<SshHost>, std::io::Error> {
    let config_path = ssh_config_path();
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };

    let mut hosts = Vec::new();
    let mut current_host: Option<SshHost> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => continue,
        };

        match key.as_str() {
            "host" => {
                if let Some(h) = current_host.take() {
                    hosts.push(h);
                }
                // Skip wildcard hosts.
                if !value.contains('*') {
                    current_host = Some(SshHost {
                        alias: value,
                        hostname: None,
                        port: None,
                        user: None,
                    });
                }
            }
            "hostname" => {
                if let Some(h) = current_host.as_mut() {
                    h.hostname = Some(value);
                }
            }
            "port" => {
                if let Some(h) = current_host.as_mut() {
                    h.port = value.parse().ok();
                }
            }
            "user" => {
                if let Some(h) = current_host.as_mut() {
                    h.user = Some(value);
                }
            }
            _ => {}
        }
    }

    if let Some(h) = current_host.take() {
        hosts.push(h);
    }

    Ok(hosts)
}

/// Get the path to the SSH config file.
fn ssh_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".ssh").join("config")
    } else {
        PathBuf::from("~/.ssh/config")
    }
}
