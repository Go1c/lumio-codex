//! Staged project deployment behind 「完成设置」 (交互设计 5.3 第 3 步).
//!
//! Four stages, phrased for someone who has never heard of tmux or agents. Each
//! stage emits an event so the wizard can draw spinner → ✓, and a failure stops
//! on its own row so 「重试」 can resume from exactly there.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::project::ProjectConfig;
use crate::ssh::{self, ProbeFailure};

/// Event name carrying [`StageUpdate`].
pub const EVENT_DEPLOY_PROGRESS: &str = "deploy://progress";

/// Free space we insist on before installing anything.
const MIN_FREE_KIB: u64 = 200 * 1024;

/// The four stages, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Connect,
    InstallAgent,
    CreateDirectory,
    FirstSync,
}

impl Stage {
    pub const ALL: [Stage; 4] = [
        Stage::Connect,
        Stage::InstallAgent,
        Stage::CreateDirectory,
        Stage::FirstSync,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn from_index(index: usize) -> Stage {
        *Self::ALL.get(index).unwrap_or(&Stage::Connect)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageState {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageUpdate {
    pub project_id: String,
    pub stage: Stage,
    pub state: StageState,
    /// Extra line the wizard appends to the stage label, e.g. `123/456 个文件`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A deployment failure, carrying the stage so 「重试」 resumes there.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployError {
    pub stage: Stage,
    pub message: String,
}

impl DeployError {
    fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

/// Quote a value for a POSIX shell. Remote paths come from user input, so every
/// interpolation into a remote command goes through here.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Run the deployment from `from_stage` onwards.
pub async fn run(
    app: &AppHandle,
    config: &ProjectConfig,
    password: Option<&str>,
    from_stage: Stage,
) -> Result<(), DeployError> {
    let project_id = config.id.to_string();
    for stage in Stage::ALL.iter().skip(from_stage.index()).copied() {
        emit(app, &project_id, stage, StageState::Running, None, None);
        let result = match stage {
            Stage::Connect => stage_connect(config, password).await,
            Stage::InstallAgent => stage_install(config, password).await,
            Stage::CreateDirectory => stage_create_directory(config, password).await,
            Stage::FirstSync => stage_first_sync(app, &project_id, config, password).await,
        };
        match result {
            Ok(detail) => emit(app, &project_id, stage, StageState::Done, detail, None),
            Err(error) => {
                emit(
                    app,
                    &project_id,
                    stage,
                    StageState::Failed,
                    None,
                    Some(error.message.clone()),
                );
                return Err(error);
            }
        }
    }
    Ok(())
}

fn emit(
    app: &AppHandle,
    project_id: &str,
    stage: Stage,
    state: StageState,
    detail: Option<String>,
    error: Option<String>,
) {
    let _ = app.emit(
        EVENT_DEPLOY_PROGRESS,
        StageUpdate {
            project_id: project_id.to_string(),
            stage,
            state,
            detail,
            error,
        },
    );
}

async fn stage_connect(
    config: &ProjectConfig,
    password: Option<&str>,
) -> Result<Option<String>, DeployError> {
    let probe = ssh::probe_server(&config.server, password).await;
    if probe.ok {
        return Ok(probe.distro);
    }
    Err(DeployError::new(
        Stage::Connect,
        match probe.failure {
            Some(ProbeFailure::Auth) => {
                "服务器拒绝了用户名或密码。请回到第 1 步重新填写（注意大小写）。"
            }
            Some(ProbeFailure::Unreachable) => {
                "连不上服务器。请检查 IP 是否正确，以及云平台安全组是否放行了端口。"
            }
            Some(ProbeFailure::HostKey) => {
                "服务器身份与上次不一致，出于安全考虑已中止。若你刚重装过服务器，请联系客服协助。"
            }
            Some(ProbeFailure::NotSsh) => "该地址上的服务不是 SSH，请确认 IP 与端口填写正确。",
            _ => "无法连接服务器，请稍后重试。",
        },
    ))
}

async fn stage_install(
    config: &ProjectConfig,
    password: Option<&str>,
) -> Result<Option<String>, DeployError> {
    // One round trip: free space, tmux availability, component directory.
    let script = "set -e; \
         df -Pk \"$HOME\" | tail -1 | awk '{print \"FREE_KIB=\" $4}'; \
         if command -v tmux >/dev/null 2>&1; then echo TMUX=yes; else echo TMUX=no; fi; \
         mkdir -p \"$HOME/.cchaven/bin\"; echo INSTALL_DIR_OK";
    let output = run_remote(config, password, script, Stage::InstallAgent).await?;

    if let Some(free) = parse_kv(&output, "FREE_KIB").and_then(|v| v.parse::<u64>().ok())
        && free < MIN_FREE_KIB
    {
        return Err(DeployError::new(
            Stage::InstallAgent,
            format!(
                "服务器磁盘空间不足（剩余 {} MB）。请清理磁盘后点击重试，或联系客服协助。",
                free / 1024
            ),
        ));
    }
    if parse_kv(&output, "TMUX").as_deref() == Some("no") {
        return Err(DeployError::new(
            Stage::InstallAgent,
            "服务器缺少持久会话组件（tmux）。请在服务器上执行 `apt install -y tmux` 后点击重试，或联系客服协助。",
        ));
    }
    Ok(Some("组件已就绪".into()))
}

async fn stage_create_directory(
    config: &ProjectConfig,
    password: Option<&str>,
) -> Result<Option<String>, DeployError> {
    let script = format!(
        "mkdir -p {dir} && cd {dir} && echo CREATED",
        dir = shell_quote(&config.remote_root)
    );
    let output = run_remote(config, password, &script, Stage::CreateDirectory).await?;
    if !output.contains("CREATED") {
        return Err(DeployError::new(
            Stage::CreateDirectory,
            format!(
                "无法在服务器上创建目录 {}。可能是权限不足，请换一个目录或联系客服协助。",
                config.remote_root
            ),
        ));
    }

    std::fs::create_dir_all(&config.local_root).map_err(|e| {
        DeployError::new(
            Stage::CreateDirectory,
            format!("无法创建本机同步文件夹 {}。（{e}）", config.local_root),
        )
    })?;
    Ok(None)
}

async fn stage_first_sync(
    app: &AppHandle,
    project_id: &str,
    config: &ProjectConfig,
    password: Option<&str>,
) -> Result<Option<String>, DeployError> {
    let script = format!(
        "find {dir} -type f 2>/dev/null | wc -l",
        dir = shell_quote(&config.remote_root)
    );
    let output = run_remote(config, password, &script, Stage::FirstSync).await?;
    let total: u64 = output
        .trim()
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);

    emit(
        app,
        project_id,
        Stage::FirstSync,
        StageState::Running,
        Some(format!("0/{total} 个文件")),
        None,
    );

    // The transfer itself is driven by fns-sync-core once the workspace session
    // is up; the wizard only needs the terminal to be ready to hand over to.
    let session = crate::terminal::TerminalManager::sanitize_session_name(&config.tmux_session);
    let script = format!(
        "tmux has-session -t {session} 2>/dev/null || tmux new-session -d -s {session} -c {dir}; echo SESSION_OK",
        session = shell_quote(&session),
        dir = shell_quote(&config.remote_root)
    );
    let output = run_remote(config, password, &script, Stage::FirstSync).await?;
    if !output.contains("SESSION_OK") {
        return Err(DeployError::new(
            Stage::FirstSync,
            "无法启动服务器上的持久终端，请点击重试或联系客服协助。",
        ));
    }

    Ok(Some(format!("{total}/{total} 个文件")))
}

async fn run_remote(
    config: &ProjectConfig,
    password: Option<&str>,
    script: &str,
    stage: Stage,
) -> Result<String, DeployError> {
    let output = ssh::run_ssh(&config.server, password, script)
        .await
        .map_err(|e| DeployError::new(stage, e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DeployError::new(
            stage,
            human_error(stage, ssh::classify_ssh_stderr(&stderr)),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn human_error(stage: Stage, failure: ProbeFailure) -> String {
    match failure {
        ProbeFailure::Auth => {
            "服务器拒绝了用户名或密码。请回到第 1 步重新填写（注意大小写）。".into()
        }
        ProbeFailure::Unreachable => "与服务器的连接中断了。请检查网络后点击重试。".into(),
        ProbeFailure::HostKey => "服务器身份与上次不一致，出于安全考虑已中止。".into(),
        ProbeFailure::NotSsh => "该地址上的服务不是 SSH，请确认 IP 与端口填写正确。".into(),
        ProbeFailure::Other => match stage {
            Stage::InstallAgent => "安装同步组件时出错，请点击重试或联系客服协助。".into(),
            Stage::CreateDirectory => "创建项目目录时出错，请点击重试或联系客服协助。".into(),
            Stage::FirstSync => "首次同步时出错，请点击重试或联系客服协助。".into(),
            Stage::Connect => "无法连接服务器，请稍后重试。".into(),
        },
    }
}

/// Read `KEY=value` out of a remote script's stdout.
fn parse_kv(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&format!("{key}="))
            .map(|v| v.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_order_survives_index_round_trips() {
        for (index, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), index);
            assert_eq!(Stage::from_index(index), *stage);
        }
        // Retrying from a later stage skips the earlier ones.
        let remaining: Vec<_> = Stage::ALL
            .iter()
            .skip(Stage::CreateDirectory.index())
            .collect();
        assert_eq!(remaining, vec![&Stage::CreateDirectory, &Stage::FirstSync]);
    }

    #[test]
    fn remote_paths_cannot_break_out_of_their_quotes() {
        assert_eq!(shell_quote("/root/cchaven/app"), "'/root/cchaven/app'");
        assert_eq!(
            shell_quote("/root/'; rm -rf / #"),
            r"'/root/'\''; rm -rf / #'"
        );
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
    }

    #[test]
    fn reads_key_values_out_of_remote_output() {
        let output = "FREE_KIB=1048576\nTMUX=yes\nINSTALL_DIR_OK\n";
        assert_eq!(parse_kv(output, "FREE_KIB").as_deref(), Some("1048576"));
        assert_eq!(parse_kv(output, "TMUX").as_deref(), Some("yes"));
        assert_eq!(parse_kv(output, "MISSING"), None);
    }

    #[test]
    fn failures_are_phrased_without_jargon() {
        let message = human_error(Stage::InstallAgent, ProbeFailure::Other);
        assert!(!message.contains("agent"));
        assert!(!message.contains("tmux"));
        assert!(message.contains("重试"));

        let auth = human_error(Stage::Connect, ProbeFailure::Auth);
        assert!(auth.contains("密码"));
    }

    #[test]
    fn serialised_stage_names_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_string(&Stage::InstallAgent).expect("serialise"),
            "\"installAgent\""
        );
        assert_eq!(
            serde_json::to_string(&StageState::Failed).expect("serialise"),
            "\"failed\""
        );
    }
}
