//! Deploy sync components to the remote host (not mkdir-only).

use crate::claude_files::expand_local_root;
use crate::claude_ssh::{ResolvedSshTarget, resolve_from_user_config, ssh_invocation_args};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareOutcome {
    pub ok: bool,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ArtifactPaths {
    pub server: PathBuf,
    pub agent: PathBuf,
}

pub fn find_artifacts(resource_dir: Option<&Path>) -> Option<ArtifactPaths> {
    if let Some(explicit) = std::env::var_os("BESTCODEX_CLAUDE_REMOTE_DIR") {
        let root = PathBuf::from(explicit);
        let paths = ArtifactPaths {
            server: root.join("fns-server"),
            agent: root.join("fns-agent"),
        };
        if is_real_artifact(&paths.server) && is_real_artifact(&paths.agent) {
            return Some(paths);
        }
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = resource_dir {
        candidates.push(dir.join("remote").join("linux-x86_64"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("remote").join("linux-x86_64"));
            candidates.push(parent.join("resources").join("remote").join("linux-x86_64"));
        }
    }
    for root in candidates {
        let paths = ArtifactPaths {
            server: root.join("fns-server"),
            agent: root.join("fns-agent"),
        };
        if is_real_artifact(&paths.server) && is_real_artifact(&paths.agent) {
            return Some(paths);
        }
    }
    None
}

fn is_real_artifact(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() > 1024)
        .unwrap_or(false)
}

pub fn human_prepare_detail(code: &str, host: &str, port: u16) -> String {
    match code {
        "DEPLOY_ARTIFACT_MISSING" => "这台电脑还没有同步组件，装不上服务器。".into(),
        "SSH_AUTH_FAILED" => format!("无法登录 {host}。"),
        "SSH_UNREACHABLE" => format!("连不上 {host}:{port}。"),
        "SSH_ALIAS_UNKNOWN" => "本机 SSH 配置里没有这个 Host 别名。".into(),
        "SSH_HOST_REQUIRED" => "先填写公网 IP。".into(),
        _ => "没能在服务器上装好同步组件。".into(),
    }
}

pub fn prepare_components(
    host: &str,
    user: &str,
    port: u16,
    alias: Option<&str>,
    local_root: &str,
    artifacts: Option<&ArtifactPaths>,
) -> PrepareOutcome {
    if let Err(code) = resolve_from_user_config(host, Some(user), port, alias) {
        return PrepareOutcome {
            ok: false,
            error_code: Some(code.into()),
            detail: Some(human_prepare_detail(code, host, port)),
        };
    }
    let local = expand_local_root(local_root);
    if std::fs::create_dir_all(&local).is_err() {
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能创建本机项目目录。".into()),
        };
    }
    if artifacts.is_none() {
        return PrepareOutcome {
            ok: false,
            error_code: Some("DEPLOY_ARTIFACT_MISSING".into()),
            detail: Some(human_prepare_detail("DEPLOY_ARTIFACT_MISSING", host, port)),
        };
    }
    PrepareOutcome {
        ok: true,
        error_code: None,
        detail: None,
    }
}

pub fn deploy_remote(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_root: &str,
    artifacts: &ArtifactPaths,
    run_ssh: impl Fn(&str) -> Result<std::process::Output, &'static str>,
) -> PrepareOutcome {
    let quoted = remote_root.replace('\'', "'\\''");
    let mkdir = format!("mkdir -p '{quoted}' ~/.local/share/bestcodex/bin");
    match run_ssh(&mkdir) {
        Ok(output) if output.status.success() => {}
        Ok(_) | Err(_) => {
            return PrepareOutcome {
                ok: false,
                error_code: Some("SSH_PREPARE_FAILED".into()),
                detail: Some("没能在服务器上建好项目目录。".into()),
            };
        }
    }

    if scp_file(
        target,
        password,
        key_path,
        &artifacts.server,
        "~/.local/share/bestcodex/bin/fns-server",
    )
    .is_err()
        || scp_file(
            target,
            password,
            key_path,
            &artifacts.agent,
            "~/.local/share/bestcodex/bin/fns-agent",
        )
        .is_err()
    {
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能把同步组件传到服务器。".into()),
        };
    }

    let start = format!(
        "chmod 0755 ~/.local/share/bestcodex/bin/fns-server ~/.local/share/bestcodex/bin/fns-agent && mkdir -p '{quoted}'"
    );
    match run_ssh(&start) {
        Ok(output) if output.status.success() => PrepareOutcome {
            ok: true,
            error_code: None,
            detail: None,
        },
        _ => PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能在服务器上装好同步组件。".into()),
        },
    }
}

fn scp_file(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    source: &Path,
    destination: &str,
) -> Result<(), &'static str> {
    let mut args = ssh_invocation_args(target, key_path, None);
    // ssh_invocation_args ends with the destination host; scp wants host:path.
    let host = args.pop().ok_or("SSH_PREPARE_FAILED")?;
    args.push(source.to_string_lossy().into_owned());
    args.push(format!("{host}:{destination}"));
    let mut command = Command::new("scp");
    command.args(&args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let askpass =
        crate::claude_ssh::attach_askpass(&mut command, password, key_path, target.use_config)?;
    let status = command.status().map_err(|_| "SSH_CLIENT_MISSING")?;
    drop(askpass);
    if status.success() {
        Ok(())
    } else {
        Err("SSH_PREPARE_FAILED")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_fails_when_sync_components_are_missing() {
        let local = tempfile::tempdir().unwrap();
        let outcome = prepare_components(
            "43.156.20.8",
            "root",
            22,
            None,
            &local.path().to_string_lossy(),
            None,
        );
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("DEPLOY_ARTIFACT_MISSING")
        );
        assert!(
            outcome
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("同步组件") && !d.contains("agent"))
        );
    }

    #[test]
    fn prepare_with_artifacts_creates_the_local_root() {
        let local = tempfile::tempdir().unwrap();
        let dest = local.path().join("project");
        let artifacts_dir = tempfile::tempdir().unwrap();
        let server = artifacts_dir.path().join("fns-server");
        let agent = artifacts_dir.path().join("fns-agent");
        std::fs::write(&server, b"server").unwrap();
        std::fs::write(&agent, b"agent").unwrap();
        let artifacts = ArtifactPaths { server, agent };
        let outcome = prepare_components(
            "43.156.20.8",
            "root",
            22,
            None,
            &dest.to_string_lossy(),
            Some(&artifacts),
        );
        assert!(outcome.ok);
        assert!(dest.exists());
    }
}
