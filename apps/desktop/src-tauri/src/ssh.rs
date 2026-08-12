//! SSH connectivity: `~/.ssh/config` discovery, pasted-command parsing, and the
//! connection probe behind 「连接并继续」.
//!
//! Private key material is never read by this app; OpenSSH does that itself.
//! Passwords are handed to `ssh` through the askpass socket (see `askpass.rs`),
//! so they never appear in argv, in the environment, or on disk.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::askpass::AskpassServer;
use crate::project::{AuthMethod, ServerConfig};

/// Timeout for the step-1 probe. Long enough for a slow overseas host, short
/// enough that the wizard does not feel stuck.
const PROBE_TIMEOUT_SECS: u64 = 12;

/// A parsed SSH host entry from `~/.ssh/config`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
}

/// Result of pasting a connection string into the IP field (5.3 粘贴识别).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Parse text pasted into the 服务器 IP 地址 field.
///
/// Accepts what a cloud console hands the user: a full `ssh` command (with the
/// port before or after the target), a bare `user@host`, or a plain address.
/// Returns `None` when nothing host-shaped can be recognised, so the field just
/// takes the raw text.
pub fn parse_ssh_target(text: &str) -> Option<SshTarget> {
    let text = text.trim();
    if text.is_empty() || text.lines().count() > 1 {
        return None;
    }

    let mut tokens = text.split_whitespace().peekable();
    let mut target: Option<&str> = None;
    let mut port: Option<u16> = None;

    if tokens.peek().is_some_and(|t| t.eq_ignore_ascii_case("ssh")) {
        tokens.next();
        while let Some(token) = tokens.next() {
            match token {
                "-p" => port = tokens.next().and_then(|v| v.parse().ok()),
                _ if token.starts_with("-p") && token.len() > 2 => {
                    port = token[2..].parse().ok();
                }
                // Flags that take a value we do not care about.
                "-i" | "-o" | "-l" | "-J" | "-F" => {
                    let value = tokens.next();
                    if token == "-l" {
                        // `ssh -l root host` still tells us the user.
                        if let (Some(user), Some(host)) = (value, tokens.peek()) {
                            let host = *host;
                            let mut parsed = split_user_host(host);
                            parsed.user.get_or_insert(user.to_string());
                            parsed.port = parsed.port.or(port);
                            return valid(parsed);
                        }
                    }
                }
                _ if token.starts_with('-') => {}
                _ if target.is_none() => target = Some(token),
                // Everything after the target is the remote command.
                _ => break,
            }
        }
    } else {
        target = Some(text);
    }

    let mut parsed = split_user_host(target?);
    parsed.port = parsed.port.or(port);
    valid(parsed)
}

fn split_user_host(token: &str) -> SshTarget {
    let (user, rest) = match token.split_once('@') {
        Some((user, rest)) if !user.is_empty() => (Some(user.to_string()), rest),
        _ => (None, token),
    };
    // `host:port` is not SSH syntax, but users paste it anyway.
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            (host, port.parse().ok())
        }
        _ => (rest, None),
    };
    SshTarget {
        host: host.to_string(),
        user,
        port,
    }
}

fn valid(target: SshTarget) -> Option<SshTarget> {
    let host = target.host.trim();
    let plausible = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        && host.chars().any(|c| c.is_ascii_alphanumeric());
    plausible.then_some(target)
}

/// Outcome of the step-1 connection probe.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub ok: bool,
    /// e.g. `Ubuntu 24.04 LTS`, shown in the green success banner.
    pub distro: Option<String>,
    /// Machine-readable reason so the UI can order its troubleshooting list.
    pub failure: Option<ProbeFailure>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFailure {
    /// Nothing listening / filtered: wrong IP or a closed security group.
    Unreachable,
    /// TCP is open but the peer is not an SSH server.
    NotSsh,
    /// Credentials rejected.
    Auth,
    /// The host key changed since last time.
    HostKey,
    Other,
}

/// Build the argv for an `ssh` invocation. Kept separate from execution so the
/// option set can be asserted in tests.
pub fn ssh_args(server: &ServerConfig, remote_command: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        format!("ConnectTimeout={PROBE_TIMEOUT_SECS}"),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "NumberOfPasswordPrompts=1".into(),
    ];

    match server.auth {
        AuthMethod::Password => {
            args.push("-o".into());
            args.push("PreferredAuthentications=password,keyboard-interactive".into());
            args.push("-o".into());
            args.push("PubkeyAuthentication=no".into());
        }
        AuthMethod::Key => {
            args.push("-o".into());
            args.push("BatchMode=yes".into());
            args.push("-o".into());
            args.push("PreferredAuthentications=publickey".into());
            if let Some(key) = &server.key_path {
                args.push("-i".into());
                args.push(key.clone());
            }
        }
        AuthMethod::SshConfig => {
            args.push("-o".into());
            args.push("BatchMode=yes".into());
        }
    }

    if server.auth != AuthMethod::SshConfig {
        args.push("-p".into());
        args.push(server.port.to_string());
    }
    args.push(server.ssh_target());
    if let Some(command) = remote_command {
        args.push(command.to_string());
    }
    args
}

/// Run a remote command, feeding the password through the askpass socket when
/// password auth is in use.
pub async fn run_ssh(
    server: &ServerConfig,
    password: Option<&str>,
    remote_command: &str,
) -> Result<std::process::Output, String> {
    let args = ssh_args(server, Some(remote_command));
    let askpass = match (server.auth, password) {
        (AuthMethod::Password, Some(password)) => Some(
            AskpassServer::start(password)
                .await
                .map_err(|e| e.to_string())?,
        ),
        (AuthMethod::Password, None) => {
            return Err("缺少服务器密码，请在项目设置中重新填写。".into());
        }
        _ => None,
    };

    let mut command = Command::new("ssh");
    command.args(&args);
    command.env("LC_ALL", "C");
    if let Some(askpass) = &askpass {
        askpass.configure(&mut command)?;
    }

    let output = tokio::task::spawn_blocking(move || command.output())
        .await
        .map_err(|e| format!("无法执行 ssh：{e}"))?
        .map_err(|e| format!("无法执行 ssh：{e}"))?;
    if let Some(askpass) = askpass {
        askpass.shutdown().await;
    }
    Ok(output)
}

/// TCP + banner reachability check, used to tell 「IP/端口不通」 apart from
/// 「密码不对」 before any credential is sent.
pub async fn probe_reachable(host: &str, port: u16) -> Result<String, ProbeFailure> {
    use tokio::io::AsyncReadExt;

    let address = format!("{host}:{port}");
    let stream = tokio::time::timeout(
        Duration::from_secs(PROBE_TIMEOUT_SECS),
        tokio::net::TcpStream::connect(&address),
    )
    .await
    .map_err(|_| ProbeFailure::Unreachable)?
    .map_err(|_| ProbeFailure::Unreachable)?;

    let mut stream = stream;
    let mut buf = [0u8; 128];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .map_err(|_| ProbeFailure::NotSsh)?
        .map_err(|_| ProbeFailure::NotSsh)?;
    let banner = String::from_utf8_lossy(&buf[..read]).trim().to_string();
    if banner.starts_with("SSH-") {
        Ok(banner)
    } else {
        Err(ProbeFailure::NotSsh)
    }
}

/// Full step-1 probe: reachability, then authentication, then distro detection.
pub async fn probe_server(server: &ServerConfig, password: Option<&str>) -> ProbeResult {
    if server.auth != AuthMethod::SshConfig
        && let Err(failure) = probe_reachable(&server.host, server.port).await
    {
        return ProbeResult {
            ok: false,
            distro: None,
            failure: Some(failure),
            detail: None,
        };
    }

    match run_ssh(
        server,
        password,
        "cat /etc/os-release 2>/dev/null || uname -sr",
    )
    .await
    {
        Ok(output) if output.status.success() => ProbeResult {
            ok: true,
            distro: Some(parse_distro(&String::from_utf8_lossy(&output.stdout))),
            failure: None,
            detail: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            ProbeResult {
                ok: false,
                distro: None,
                failure: Some(classify_ssh_stderr(&stderr)),
                detail: Some(redact(&stderr)),
            }
        }
        Err(detail) => ProbeResult {
            ok: false,
            distro: None,
            failure: Some(ProbeFailure::Other),
            detail: Some(detail),
        },
    }
}

/// Map `ssh` stderr onto the troubleshooting list ordering of 5.3.
pub fn classify_ssh_stderr(stderr: &str) -> ProbeFailure {
    let lower = stderr.to_lowercase();
    if lower.contains("permission denied") || lower.contains("authentication failed") {
        ProbeFailure::Auth
    } else if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
    {
        ProbeFailure::HostKey
    } else if lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("no route to host")
        || lower.contains("could not resolve hostname")
        || lower.contains("network is unreachable")
    {
        ProbeFailure::Unreachable
    } else {
        ProbeFailure::Other
    }
}

/// Pull `PRETTY_NAME` out of `/etc/os-release`, falling back to `uname` output.
pub fn parse_distro(text: &str) -> String {
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return value.trim_matches('"').to_string();
        }
    }
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Linux")
        .trim()
        .to_string()
}

/// Strip anything that could carry a credential out of diagnostics we surface.
fn redact(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            !lower.contains("password") && !lower.contains("passphrase")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Parse the user's `~/.ssh/config` to discover host aliases.
pub fn parse_ssh_config() -> Result<Vec<SshHost>, std::io::Error> {
    let content = match std::fs::read_to_string(ssh_config_path()) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(parse_ssh_config_text(&content))
}

/// Config parsing split out so it can be exercised without a real home dir.
pub fn parse_ssh_config_text(content: &str) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    let mut current: Option<SshHost> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let (key, value) = (key.to_lowercase(), value.trim().to_string());

        match key.as_str() {
            "host" => {
                if let Some(host) = current.take() {
                    hosts.push(host);
                }
                if !value.contains('*') {
                    current = Some(SshHost {
                        alias: value,
                        hostname: None,
                        port: None,
                        user: None,
                    });
                }
            }
            "hostname" => {
                if let Some(host) = current.as_mut() {
                    host.hostname = Some(value);
                }
            }
            "port" => {
                if let Some(host) = current.as_mut() {
                    host.port = value.parse().ok();
                }
            }
            "user" => {
                if let Some(host) = current.as_mut() {
                    host.user = Some(value);
                }
            }
            _ => {}
        }
    }
    if let Some(host) = current.take() {
        hosts.push(host);
    }
    hosts
}

fn ssh_config_path() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".ssh").join("config"),
        Err(_) => PathBuf::from("~/.ssh/config"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str, user: Option<&str>, port: Option<u16>) -> Option<SshTarget> {
        Some(SshTarget {
            host: host.into(),
            user: user.map(Into::into),
            port,
        })
    }

    #[test]
    fn parses_a_full_ssh_command() {
        assert_eq!(
            parse_ssh_target("ssh root@1.2.3.4 -p 2222"),
            target("1.2.3.4", Some("root"), Some(2222))
        );
    }

    #[test]
    fn parses_the_port_flag_before_the_target() {
        assert_eq!(
            parse_ssh_target("ssh -p 2222 ubuntu@example.com"),
            target("example.com", Some("ubuntu"), Some(2222))
        );
        assert_eq!(
            parse_ssh_target("ssh -p2222 ubuntu@example.com"),
            target("example.com", Some("ubuntu"), Some(2222))
        );
    }

    #[test]
    fn parses_user_at_host_without_the_command() {
        assert_eq!(
            parse_ssh_target("root@43.156.20.8"),
            target("43.156.20.8", Some("root"), None)
        );
    }

    #[test]
    fn parses_a_bare_address() {
        assert_eq!(
            parse_ssh_target("43.156.20.8"),
            target("43.156.20.8", None, None)
        );
        assert_eq!(
            parse_ssh_target("  my-host.example.com  "),
            target("my-host.example.com", None, None)
        );
    }

    #[test]
    fn accepts_host_colon_port_even_though_ssh_does_not() {
        assert_eq!(
            parse_ssh_target("root@1.2.3.4:2222"),
            target("1.2.3.4", Some("root"), Some(2222))
        );
    }

    #[test]
    fn ignores_flags_with_values_and_trailing_remote_commands() {
        assert_eq!(
            parse_ssh_target(
                "ssh -i ~/.ssh/id_ed25519 -o StrictHostKeyChecking=no root@1.2.3.4 uptime"
            ),
            target("1.2.3.4", Some("root"), None)
        );
    }

    #[test]
    fn understands_the_separate_login_name_flag() {
        assert_eq!(
            parse_ssh_target("ssh -l ubuntu 1.2.3.4"),
            target("1.2.3.4", Some("ubuntu"), None)
        );
    }

    #[test]
    fn rejects_text_that_is_not_a_host() {
        for text in [
            "",
            "   ",
            "ssh",
            "hello world here",
            "line1\nline2",
            "-p 22",
        ] {
            assert_eq!(
                parse_ssh_target(text),
                None,
                "expected {text:?} to be rejected"
            );
        }
    }

    #[test]
    fn password_auth_never_puts_the_secret_in_argv() {
        let server = ServerConfig {
            host: "1.2.3.4".into(),
            user: "root".into(),
            port: 2222,
            auth: AuthMethod::Password,
            key_path: None,
            config_alias: None,
        };
        let args = ssh_args(&server, Some("uptime"));
        assert!(args.iter().all(|arg| !arg.contains("hunter2")));
        assert!(args.contains(&"root@1.2.3.4".to_string()));
        assert!(args.contains(&"2222".to_string()));
        assert!(args.contains(&"PubkeyAuthentication=no".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("uptime"));
    }

    #[test]
    fn key_auth_runs_in_batch_mode_with_the_selected_identity() {
        let server = ServerConfig {
            host: "example.com".into(),
            user: "ubuntu".into(),
            port: 22,
            auth: AuthMethod::Key,
            key_path: Some("/Users/mary/.ssh/id_ed25519".into()),
            config_alias: None,
        };
        let args = ssh_args(&server, None);
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"/Users/mary/.ssh/id_ed25519".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("ubuntu@example.com"));
    }

    #[test]
    fn ssh_config_hosts_are_addressed_by_alias_only() {
        let server = ServerConfig {
            host: "ignored".into(),
            user: "ignored".into(),
            port: 2200,
            auth: AuthMethod::SshConfig,
            key_path: None,
            config_alias: Some("prod".into()),
        };
        let args = ssh_args(&server, None);
        assert_eq!(args.last().map(String::as_str), Some("prod"));
        assert!(
            !args.contains(&"2200".to_string()),
            "port comes from the config file"
        );
    }

    #[test]
    fn classifies_ssh_failures_for_the_troubleshooting_list() {
        assert_eq!(
            classify_ssh_stderr("root@1.2.3.4: Permission denied (publickey,password)."),
            ProbeFailure::Auth
        );
        assert_eq!(
            classify_ssh_stderr("ssh: connect to host 1.2.3.4 port 22: Connection refused"),
            ProbeFailure::Unreachable
        );
        assert_eq!(
            classify_ssh_stderr("Host key verification failed."),
            ProbeFailure::HostKey
        );
        assert_eq!(
            classify_ssh_stderr("something else entirely"),
            ProbeFailure::Other
        );
    }

    #[test]
    fn reads_the_distro_name_from_os_release() {
        let os_release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(parse_distro(os_release), "Ubuntu 24.04.1 LTS");
        assert_eq!(
            parse_distro("Linux 6.8.0-41-generic"),
            "Linux 6.8.0-41-generic"
        );
        assert_eq!(parse_distro(""), "Linux");
    }

    #[test]
    fn diagnostics_drop_password_prompts() {
        let stderr = "root@host's password: \nPermission denied, please try again.";
        let redacted = redact(stderr);
        assert!(!redacted.contains("password"));
        assert!(redacted.contains("Permission denied"));
    }

    #[test]
    fn ssh_config_parsing_skips_wildcards_and_keeps_fields() {
        let hosts = parse_ssh_config_text(
            "Host *\n  ForwardAgent yes\n\nHost prod\n  HostName 10.0.0.1\n  Port 2222\n  User deploy\n",
        );
        assert_eq!(
            hosts,
            vec![SshHost {
                alias: "prod".into(),
                hostname: Some("10.0.0.1".into()),
                port: Some(2222),
                user: Some("deploy".into()),
            }]
        );
    }
}
