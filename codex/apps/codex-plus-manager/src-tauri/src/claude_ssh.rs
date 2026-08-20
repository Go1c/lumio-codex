//! SSH config / Host-alias resolution and argv construction.
//!
//! Passwords never go on argv. Host aliases are addressed by name so OpenSSH
//! can apply IdentityFile / Port / User from the user's config.

use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSshTarget {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub alias: Option<String>,
    pub use_config: bool,
    pub identity_file: Option<String>,
}

impl ResolvedSshTarget {
    pub fn ssh_destination(&self) -> String {
        if self.use_config {
            if let Some(alias) = &self.alias {
                return alias.clone();
            }
        }
        format!("{}@{}", self.user, self.host)
    }
}

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
                        identity_file: None,
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
            "identityfile" => {
                if let Some(host) = current.as_mut() {
                    host.identity_file = Some(value);
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

pub fn ssh_config_path() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".ssh").join("config"),
        Err(_) => PathBuf::from("~/.ssh/config"),
    }
}

pub fn parse_ssh_config() -> Result<Vec<SshHost>, std::io::Error> {
    let content = match std::fs::read_to_string(ssh_config_path()) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(parse_ssh_config_text(&content))
}

pub fn resolve_ssh_target(
    host: &str,
    user: Option<&str>,
    port: u16,
    alias: Option<&str>,
    config_text: &str,
) -> Result<ResolvedSshTarget, &'static str> {
    let hosts = parse_ssh_config_text(config_text);
    let requested = alias
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let host = host.trim();
            if host.is_empty() {
                None
            } else if hosts.iter().any(|item| item.alias == host) {
                Some(host)
            } else {
                None
            }
        });

    if let Some(alias) = requested {
        let found = hosts
            .iter()
            .find(|item| item.alias == alias)
            .ok_or("SSH_ALIAS_UNKNOWN")?;
        let resolved_host = found.hostname.clone().unwrap_or_else(|| alias.to_string());
        let resolved_user = found
            .user
            .clone()
            .or_else(|| {
                user.map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "root".into());
        let resolved_port = found.port.unwrap_or(if port == 0 { 22 } else { port });
        return Ok(ResolvedSshTarget {
            host: resolved_host,
            user: resolved_user,
            port: resolved_port,
            alias: Some(alias.to_string()),
            use_config: true,
            identity_file: found.identity_file.clone(),
        });
    }

    let host = host.trim();
    if host.is_empty() {
        return Err("SSH_HOST_REQUIRED");
    }
    Ok(ResolvedSshTarget {
        host: host.to_string(),
        user: user
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("root")
            .to_string(),
        port: if port == 0 { 22 } else { port },
        alias: None,
        use_config: false,
        identity_file: None,
    })
}

pub fn resolve_from_user_config(
    host: &str,
    user: Option<&str>,
    port: u16,
    alias: Option<&str>,
) -> Result<ResolvedSshTarget, &'static str> {
    let text = std::fs::read_to_string(ssh_config_path()).unwrap_or_default();
    resolve_ssh_target(host, user, port, alias, &text)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordAuthPlan {
    pub use_askpass: bool,
    pub batch_mode: bool,
}

pub fn effective_key_path<'a>(
    key_path: Option<&'a str>,
    target: &'a ResolvedSshTarget,
) -> Option<&'a str> {
    key_path.filter(|value| !value.is_empty()).or(target
        .identity_file
        .as_deref()
        .filter(|value| !value.is_empty()))
}

pub fn password_auth_plan(
    password: Option<&str>,
    key_path: Option<&str>,
    _use_config: bool,
) -> PasswordAuthPlan {
    if key_path.map(|value| !value.is_empty()).unwrap_or(false) {
        return PasswordAuthPlan {
            use_askpass: false,
            batch_mode: true,
        };
    }
    if password.map(|value| !value.is_empty()).unwrap_or(false) {
        return PasswordAuthPlan {
            use_askpass: true,
            batch_mode: false,
        };
    }
    PasswordAuthPlan {
        use_askpass: false,
        batch_mode: true,
    }
}

pub fn attach_askpass(
    command: &mut Command,
    password: Option<&str>,
    key_path: Option<&str>,
    use_config: bool,
) -> Result<Option<AskpassGuard>, &'static str> {
    let plan = password_auth_plan(password, key_path, use_config);
    if !plan.use_askpass {
        return Ok(None);
    }
    let secret = password.unwrap_or("");
    let guard = AskpassGuard::start(secret)?;
    guard.configure(command, secret);
    Ok(Some(guard))
}

pub fn ssh_invocation_args(
    target: &ResolvedSshTarget,
    key_path: Option<&str>,
    remote_command: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        "ConnectTimeout=8".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "NumberOfPasswordPrompts=1".into(),
    ];
    let effective_key = effective_key_path(key_path, target);
    if let Some(key) = effective_key {
        args.push("-i".into());
        args.push(key.to_string());
        args.push("-o".into());
        args.push("PreferredAuthentications=publickey".into());
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        if !target.use_config {
            args.push("-p".into());
            args.push(target.port.to_string());
        }
    } else if target.use_config {
        // Host alias without IdentityFile: do not force BatchMode so an
        // in-memory password can still be attached via askpass.
    } else {
        args.push("-o".into());
        args.push("PreferredAuthentications=password,keyboard-interactive".into());
        args.push("-o".into());
        args.push("PubkeyAuthentication=no".into());
        args.push("-p".into());
        args.push(target.port.to_string());
    }
    args.push(target.ssh_destination());
    if let Some(command) = remote_command {
        args.push(command.to_string());
    }
    args
}

pub fn posix_single_quote(value: &str) -> String {
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

/// Quote a stored remote project path for the login user's shell.
///
/// Use `~/…` — not `"$HOME"`. sshd often runs `shell -c "command"`, and the
/// extra double quotes close that wrapper, so inspect/mkdir look like a
/// connection failure after probe already succeeded.
/// Absolute paths stay absolute so older projects keep working.
pub fn remote_shell_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("~/{}", posix_single_quote(rest))
    } else if path == "~" {
        "~".into()
    } else {
        posix_single_quote(path)
    }
}

pub fn remote_shell_join(root: &str, rel: &str) -> String {
    format!("{}/{}", remote_shell_path(root), posix_single_quote(rel))
}

pub fn remote_prepare_mkdir(remote_root: &str) -> String {
    format!(
        "mkdir -p {} ~/.local/share/bestcodex/bin",
        remote_shell_path(remote_root)
    )
}

pub struct AskpassGuard {
    pub script: PathBuf,
}

impl AskpassGuard {
    pub fn start(password: &str) -> Result<Self, &'static str> {
        let script = std::env::temp_dir().join(format!(
            "bestcodex-askpass-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        #[cfg(windows)]
        {
            let path = script.with_extension("cmd");
            std::fs::write(
                &path,
                "@echo off\r\n<nul set /p=%BESTCODEX_SSH_ASKPASS%\r\n",
            )
            .map_err(|_| "SSH_PROBE_FAILED")?;
            let _ = password;
            return Ok(Self { script: path });
        }
        #[cfg(not(windows))]
        {
            std::fs::write(&script, "#!/bin/sh\nprintf %s \"$BESTCODEX_SSH_ASKPASS\"\n")
                .map_err(|_| "SSH_PROBE_FAILED")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&script)
                    .map_err(|_| "SSH_PROBE_FAILED")?
                    .permissions();
                perms.set_mode(0o700);
                let _ = std::fs::set_permissions(&script, perms);
            }
            let _ = password;
            Ok(Self { script })
        }
    }

    pub fn configure(&self, command: &mut Command, password: &str) {
        command.env("SSH_ASKPASS", &self.script);
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", ":0");
        command.env("BESTCODEX_SSH_ASKPASS", password);
        command.stdin(Stdio::null());
    }
}

impl Drop for AskpassGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.script);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "Host *\n  ForwardAgent yes\n\nHost prod\n  HostName 10.0.0.1\n  Port 2222\n  User deploy\n";

    #[test]
    fn host_alias_from_temp_config_resolves_for_probe_and_prepare() {
        let resolved = resolve_ssh_target("", None, 22, Some("prod"), CONFIG).expect("alias");
        assert_eq!(resolved.host, "10.0.0.1");
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.user, "deploy");
        assert_eq!(resolved.ssh_destination(), "prod");
        let args = ssh_invocation_args(&resolved, None, None);
        assert_eq!(args.last().map(String::as_str), Some("prod"));
        assert!(
            !args.iter().any(|arg| arg == "2222"),
            "port comes from the config file"
        );
    }

    #[test]
    fn a_plain_host_is_unchanged_when_no_alias_matches() {
        let resolved = resolve_ssh_target("43.156.20.8", Some("root"), 22, None, CONFIG).unwrap();
        assert_eq!(resolved.host, "43.156.20.8");
        assert_eq!(resolved.user, "root");
        assert!(!resolved.use_config);
        let args = ssh_invocation_args(&resolved, None, Some("uptime"));
        assert!(args.contains(&"root@43.156.20.8".into()));
        assert!(args.contains(&"22".into()));
        assert!(args.iter().all(|arg| !arg.contains("password=")));
    }

    #[test]
    fn unknown_alias_is_a_visible_error() {
        assert_eq!(
            resolve_ssh_target("", None, 22, Some("missing"), CONFIG).unwrap_err(),
            "SSH_ALIAS_UNKNOWN"
        );
    }

    #[test]
    fn password_auth_uses_askpass_instead_of_argv() {
        let password = password_auth_plan(Some("hunter2"), None, false);
        assert!(password.use_askpass);
        assert!(!password.batch_mode);
        let key = password_auth_plan(Some("hunter2"), Some("/tmp/id_ed25519"), false);
        assert!(!key.use_askpass);
        assert!(key.batch_mode);
    }

    #[test]
    fn host_alias_without_identity_uses_in_memory_password() {
        let plan = password_auth_plan(Some("hunter2"), None, true);
        assert!(
            plan.use_askpass,
            "alias + typed password must still use askpass when the config has no key"
        );
        assert!(!plan.batch_mode);
        let resolved = resolve_ssh_target("", None, 22, Some("prod"), CONFIG).expect("alias");
        assert!(resolved.identity_file.is_none());
        let args = ssh_invocation_args(&resolved, None, None);
        assert!(
            !args.iter().any(|arg| arg == "BatchMode=yes"),
            "BatchMode would drop the in-memory password"
        );
        assert!(args.iter().all(|arg| !arg.contains("hunter2")));
        assert!(args.iter().all(|arg| !arg.contains("password=")));
    }

    #[test]
    fn remote_shell_path_follows_login_home_instead_of_guessing_absolute() {
        assert_eq!(
            remote_shell_path("~/bestcodex/my-project"),
            "~/'bestcodex/my-project'"
        );
        assert_eq!(
            remote_prepare_mkdir("~/bestcodex/my-project"),
            "mkdir -p ~/'bestcodex/my-project' ~/.local/share/bestcodex/bin"
        );
        assert!(
            !remote_shell_path("~/bestcodex/my-project").contains('"'),
            "double quotes around $HOME break sshd's shell -c wrapper"
        );
        assert!(
            !remote_prepare_mkdir("~/bestcodex/my-project").contains('"'),
            "mkdir must not introduce double quotes either"
        );
        assert_eq!(
            remote_shell_path("/root/bestcodex/legacy"),
            "'/root/bestcodex/legacy'"
        );
    }

    #[test]
    fn host_alias_with_identity_file_stays_batch_mode() {
        let config = "Host bastion\n  HostName 10.0.0.9\n  IdentityFile ~/.ssh/id_ed25519\n";
        let resolved = resolve_ssh_target("", None, 22, Some("bastion"), config).expect("alias");
        assert_eq!(resolved.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
        let plan = password_auth_plan(Some("hunter2"), resolved.identity_file.as_deref(), true);
        assert!(!plan.use_askpass);
        assert!(plan.batch_mode);
    }
}
