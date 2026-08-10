//! Bounded, two-phase deployment of the Linux server and remote sync agent.

use crate::credentials::CredentialState;
use crate::sync::CredentialProvider;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tauri::Manager as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const SSH_TIMEOUT: Duration = Duration::from_secs(20);
const AGENT_SYSTEMD_STOP_TIMEOUT_SECONDS: u64 = 35;
const SERVER_SYSTEMD_STOP_TIMEOUT_SECONDS: u64 = 35;
const SYSTEMD_LIFECYCLE_TIMEOUT_MARGIN_SECONDS: u64 = 30;
const SYSTEMD_MAX_STOP_TIMEOUT_SECONDS: u64 =
    if AGENT_SYSTEMD_STOP_TIMEOUT_SECONDS > SERVER_SYSTEMD_STOP_TIMEOUT_SECONDS {
        AGENT_SYSTEMD_STOP_TIMEOUT_SECONDS
    } else {
        SERVER_SYSTEMD_STOP_TIMEOUT_SECONDS
    };
const SYSTEMD_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(
    SYSTEMD_MAX_STOP_TIMEOUT_SECONDS + SYSTEMD_LIFECYCLE_TIMEOUT_MARGIN_SECONDS,
);
const SCP_TIMEOUT: Duration = Duration::from_secs(180);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
const PREVIEW_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_PROCESS_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const SERVER_PORT: u16 = 9000;
const DEPLOY_PROGRESS_EVENT: &str = "deploy://progress";

const REMOTE_PROBE_SCRIPT: &str = r#"set -u
encode() { printf '%s' "$1" | base64 | tr -d '\n'; }
home=${HOME:-}
if [ -z "$home" ] || [ ! -d "$home" ]; then exit 40; fi
system=$(uname -s 2>/dev/null || true)
arch=$(uname -m 2>/dev/null || true)
uid=$(id -u 2>/dev/null || true)
system_manager=0
user_manager=0
if command -v systemctl >/dev/null 2>&1; then
  if [ "$uid" = "0" ] && systemctl --version >/dev/null 2>&1; then system_manager=1; fi
  if systemctl --user show-environment >/dev/null 2>&1 && \
     command -v loginctl >/dev/null 2>&1 && \
     [ "$(loginctl show-user "$uid" -p Linger --value 2>/dev/null || true)" = "yes" ]
  then
    user_manager=1
  fi
fi
agent_load_state=unavailable
agent_unit_file_state=unavailable
agent_active_state=unavailable
if [ "$system_manager" = "1" ]; then
  agent_load_state=$(systemctl show --property LoadState --value "$agent_unit" 2>/dev/null || true)
  case "$agent_load_state" in
    loaded)
      agent_unit_file_state=$(systemctl show --property UnitFileState --value "$agent_unit" 2>/dev/null || true)
      agent_active_state=$(systemctl show --property ActiveState --value "$agent_unit" 2>/dev/null || true)
      ;;
    not-found)
      agent_unit_file_state=not-found
      agent_active_state=not-found
      ;;
  esac
elif [ "$user_manager" = "1" ]; then
  agent_load_state=$(systemctl --user show --property LoadState --value "$agent_unit" 2>/dev/null || true)
  case "$agent_load_state" in
    loaded)
      agent_unit_file_state=$(systemctl --user show --property UnitFileState --value "$agent_unit" 2>/dev/null || true)
      agent_active_state=$(systemctl --user show --property ActiveState --value "$agent_unit" 2>/dev/null || true)
      ;;
    not-found)
      agent_unit_file_state=not-found
      agent_active_state=not-found
      ;;
  esac
fi
config=""
workdir=""
if [ -r "$home/.config/fns-workspace/server-source-path" ]; then
  IFS= read -r config < "$home/.config/fns-workspace/server-source-path" || true
fi
if [ -r "$home/.config/fns-workspace/server-working-directory" ]; then
  IFS= read -r workdir < "$home/.config/fns-workspace/server-working-directory" || true
fi
if [ -z "$config" ] || [ ! -f "$config" ]; then
  for candidate in \
    "$home/.config/fns-workspace/server.yaml" \
    "$home/fns-deploy/fns-server-patched/config/config.yaml" \
    "$home"/fns-deploy/*/config/config.yaml \
    "$home/fns-server/config/config.yaml" \
    "$home/fast-note-sync-service/config/config.yaml"
  do
    if [ -f "$candidate" ]; then config=$candidate; break; fi
  done
fi
if [ -n "$config" ] && [ -z "$workdir" ]; then
  workdir=$(dirname "$(dirname "$config")")
fi
current=""
if [ -L "$home/.local/share/fns-workspace/current" ]; then
  current=$(readlink "$home/.local/share/fns-workspace/current" 2>/dev/null || true)
fi
pid=""
if command -v fuser >/dev/null 2>&1; then
  for candidate in $(fuser -n tcp 9000 2>/dev/null || true); do
    case "$candidate" in (*[!0-9]*) ;; (*) pid=$candidate; break ;; esac
  done
fi
if [ -z "$pid" ] && command -v ss >/dev/null 2>&1; then
  pid=$(ss -ltnp 2>/dev/null | sed -n '/:9000[[:space:]]/s/.*pid=\([0-9][0-9]*\).*/\1/p' | head -n 1 || true)
fi
exe=""
cwd=""
if [ -n "$pid" ] && [ -e "/proc/$pid/exe" ]; then
  exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
  cwd=$(readlink -f "/proc/$pid/cwd" 2>/dev/null || true)
fi
available_kb=$(df -Pk "$home" 2>/dev/null | awk 'NR==2 { print $4 }' || true)
printf 'system=%s\n' "$system"
printf 'arch=%s\n' "$arch"
printf 'uid=%s\n' "$uid"
printf 'system_manager=%s\n' "$system_manager"
printf 'user_manager=%s\n' "$user_manager"
printf 'agent_load_state=%s\n' "$agent_load_state"
printf 'agent_unit_file_state=%s\n' "$agent_unit_file_state"
printf 'agent_active_state=%s\n' "$agent_active_state"
printf 'available_kb=%s\n' "$available_kb"
printf 'home_b64=%s\n' "$(encode "$home")"
printf 'config_b64=%s\n' "$(encode "$config")"
printf 'workdir_b64=%s\n' "$(encode "$workdir")"
printf 'current_b64=%s\n' "$(encode "$current")"
printf 'pid=%s\n' "$pid"
printf 'exe_b64=%s\n' "$(encode "$exe")"
printf 'cwd_b64=%s\n' "$(encode "$cwd")"
"#;

#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeploymentRequest {
    pub(crate) project_id: String,
    pub(crate) ssh_host_alias: String,
    pub(crate) workspace_id: String,
    pub(crate) remote_root: String,
    pub(crate) includes: Vec<String>,
    pub(crate) excludes: Vec<String>,
    pub(crate) protect_secrets: bool,
}

impl fmt::Debug for DeploymentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentRequest")
            .field("project_id", &self.project_id)
            .field("workspace_id", &self.workspace_id)
            .field("ssh_host_alias", &"[REDACTED]")
            .field("remote_root", &"[REDACTED]")
            .field("include_count", &self.includes.len())
            .field("exclude_count", &self.excludes.len())
            .field("protect_secrets", &self.protect_secrets)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceManager {
    System,
    User,
}

impl ServiceManager {
    const fn command(self) -> &'static str {
        match self {
            Self::System => "systemctl",
            Self::User => "systemctl --user",
        }
    }

    fn unit_directory(self, home: &str) -> String {
        match self {
            Self::System => "/etc/systemd/system".to_owned(),
            Self::User => format!("{home}/.config/systemd/user"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeployStep {
    ValidateRemote,
    EnsureDirectories,
    UploadServer,
    UploadAgent,
    VerifyArtifacts,
    PrepareConfiguration,
    SwitchVersion,
    InstallServices,
    StartServices,
    VerifyHealth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeployStepStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeployProgress {
    pub(crate) project_id: String,
    pub(crate) step: DeployStep,
    pub(crate) status: DeployStepStatus,
    pub(crate) error_code: Option<DeployErrorCode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeployErrorCode {
    InvalidRequest,
    ArtifactMissing,
    ArtifactInvalid,
    ArtifactChanged,
    ArtifactChecksumMismatch,
    UnsupportedRemote,
    InsufficientDisk,
    SystemdUnavailable,
    ServerConfigMissing,
    ServerConfigInvalid,
    CredentialMissing,
    CredentialInvalid,
    PreviewRequired,
    PreviewExpired,
    AlreadyRunning,
    SshSpawnFailed,
    SshFailed,
    ResponseTooLarge,
    MalformedResponse,
    RemoteFilesystem,
    ServiceUnhealthy,
    Timeout,
    Cancelled,
    ProgressUnavailable,
    RollbackFailed,
}

impl DeployErrorCode {
    const fn stable(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::ArtifactMissing => "artifact_missing",
            Self::ArtifactInvalid => "artifact_invalid",
            Self::ArtifactChanged => "artifact_changed",
            Self::ArtifactChecksumMismatch => "artifact_checksum_mismatch",
            Self::UnsupportedRemote => "unsupported_remote",
            Self::InsufficientDisk => "insufficient_disk",
            Self::SystemdUnavailable => "systemd_unavailable",
            Self::ServerConfigMissing => "server_config_missing",
            Self::ServerConfigInvalid => "server_config_invalid",
            Self::CredentialMissing => "credential_missing",
            Self::CredentialInvalid => "credential_invalid",
            Self::PreviewRequired => "preview_required",
            Self::PreviewExpired => "preview_expired",
            Self::AlreadyRunning => "already_running",
            Self::SshSpawnFailed => "ssh_spawn_failed",
            Self::SshFailed => "ssh_failed",
            Self::ResponseTooLarge => "response_too_large",
            Self::MalformedResponse => "malformed_response",
            Self::RemoteFilesystem => "remote_filesystem",
            Self::ServiceUnhealthy => "service_unhealthy",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::ProgressUnavailable => "progress_unavailable",
            Self::RollbackFailed => "rollback_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeployFailure {
    pub(crate) primary: DeployErrorCode,
    pub(crate) step: Option<DeployStep>,
    pub(crate) cleanup: Vec<DeployErrorCode>,
}

impl DeployFailure {
    fn new(primary: DeployErrorCode) -> Self {
        Self {
            primary,
            step: None,
            cleanup: Vec::new(),
        }
    }

    fn with_cleanup(mut self, cleanup: DeployErrorCode) -> Self {
        self.cleanup.push(cleanup);
        self
    }
}

impl From<DeployErrorCode> for DeployFailure {
    fn from(value: DeployErrorCode) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for DeployFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.primary.stable())?;
        if let Some(step) = self.step {
            write!(formatter, ";step={step:?}")?;
        }
        for cleanup in &self.cleanup {
            write!(formatter, ";cleanup={}", cleanup.stable())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactSummary {
    pub(crate) kind: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeploymentPreview {
    pub(crate) preview_id: String,
    pub(crate) target: String,
    pub(crate) version: String,
    pub(crate) service_manager: Option<ServiceManager>,
    pub(crate) existing_version: Option<String>,
    pub(crate) artifacts: Vec<ArtifactSummary>,
    pub(crate) steps: Vec<DeployStep>,
    pub(crate) warnings: Vec<DeployErrorCode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeploymentOutcome {
    pub(crate) project_id: String,
    pub(crate) version: String,
    pub(crate) service_manager: ServiceManager,
    pub(crate) server_active: bool,
    pub(crate) agent_online: bool,
    pub(crate) rolled_back: bool,
}

#[derive(Clone, Debug)]
struct ArtifactInfo {
    path: PathBuf,
    sha256: String,
    bytes: u64,
}

#[derive(Clone)]
struct ArtifactPaths {
    server: PathBuf,
    agent: PathBuf,
}

impl ArtifactInfo {
    fn summary(&self, kind: &str) -> ArtifactSummary {
        ArtifactSummary {
            kind: kind.to_owned(),
            sha256: self.sha256.clone(),
            bytes: self.bytes,
        }
    }
}

#[derive(Clone, Debug)]
struct RemoteProbe {
    home: String,
    service_manager: Option<ServiceManager>,
    available_bytes: u64,
    server_config: Option<String>,
    server_workdir: Option<String>,
    current_target: Option<String>,
    existing_pid: Option<u32>,
    existing_executable: Option<String>,
    agent_unit_state: Option<RemoteAgentUnitState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteAgentUnitState {
    Absent,
    Present { enabled: bool, active: bool },
}

#[derive(Clone)]
struct StoredPreview {
    request: DeploymentRequest,
    created_at: Instant,
    server: ArtifactInfo,
    agent: ArtifactInfo,
    remote: RemoteProbe,
    version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessKind {
    Ssh,
    Scp,
}

#[derive(Clone)]
struct ProcessSpec {
    kind: ProcessKind,
    program: &'static str,
    args: Vec<OsString>,
}

impl fmt::Debug for ProcessSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessSpec")
            .field("kind", &self.kind)
            .field("program", &self.program)
            .field("argument_count", &self.args.len())
            .finish()
    }
}

struct ProcessInput(Zeroizing<Vec<u8>>);

impl ProcessInput {
    fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl fmt::Debug for ProcessInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessInput([REDACTED])")
    }
}

#[derive(Clone, Debug)]
struct ProcessOutput {
    success: bool,
    stdout: Vec<u8>,
}

type ProcessFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProcessOutput, DeployFailure>> + Send + 'a>>;

trait ProcessRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        spec: ProcessSpec,
        input: Option<ProcessInput>,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> ProcessFuture<'a>;
}

#[derive(Default)]
struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run<'a>(
        &'a self,
        spec: ProcessSpec,
        input: Option<ProcessInput>,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> ProcessFuture<'a> {
        Box::pin(async move { run_system_process(spec, input, timeout, cancellation).await })
    }
}

pub(crate) struct DeployState {
    runner: Arc<dyn ProcessRunner>,
    previews: StdMutex<HashMap<String, StoredPreview>>,
    operations: StdMutex<HashMap<String, CancellationToken>>,
}

impl DeployState {
    pub(crate) fn production() -> Self {
        Self::with_runner(Arc::new(SystemProcessRunner))
    }

    fn with_runner(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            previews: StdMutex::new(HashMap::new()),
            operations: StdMutex::new(HashMap::new()),
        }
    }

    fn store_preview(&self, preview: StoredPreview) -> String {
        let preview_id = uuid::Uuid::new_v4().to_string();
        let mut previews = self
            .previews
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        previews.retain(|_, value| value.created_at.elapsed() <= PREVIEW_LIFETIME);
        if previews.len() >= 32
            && let Some(oldest) = previews
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(key, _)| key.clone())
        {
            previews.remove(&oldest);
        }
        previews.insert(preview_id.clone(), preview);
        preview_id
    }

    fn load_preview(
        &self,
        preview_id: &str,
        request: &DeploymentRequest,
    ) -> Result<StoredPreview, DeployFailure> {
        let previews = self
            .previews
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let preview = previews
            .get(preview_id)
            .ok_or(DeployErrorCode::PreviewRequired)?;
        if preview.created_at.elapsed() > PREVIEW_LIFETIME {
            return Err(DeployErrorCode::PreviewExpired.into());
        }
        if &preview.request != request {
            return Err(DeployErrorCode::PreviewRequired.into());
        }
        Ok(preview.clone())
    }

    fn begin_operation(&self, project_id: &str) -> Result<OperationGuard<'_>, DeployFailure> {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if operations.contains_key(project_id) {
            return Err(DeployErrorCode::AlreadyRunning.into());
        }
        let cancellation = CancellationToken::new();
        operations.insert(project_id.to_owned(), cancellation.clone());
        Ok(OperationGuard {
            state: self,
            project_id: project_id.to_owned(),
            cancellation,
        })
    }

    fn cancel(&self, project_id: &str) -> bool {
        self.operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .is_some_and(|token| {
                token.cancel();
                true
            })
    }
}

impl Drop for DeployState {
    fn drop(&mut self) {
        for cancellation in self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            cancellation.cancel();
        }
    }
}

struct OperationGuard<'a> {
    state: &'a DeployState,
    project_id: String,
    cancellation: CancellationToken,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.state
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.project_id);
    }
}

async fn run_system_process(
    spec: ProcessSpec,
    input: Option<ProcessInput>,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<ProcessOutput, DeployFailure> {
    let mut command = Command::new(spec.program);
    command
        .args(&spec.args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| DeployFailure::new(DeployErrorCode::SshSpawnFailed))?;
    let stdout = child.stdout.take().ok_or(DeployErrorCode::SshSpawnFailed)?;
    let stderr = child.stderr.take().ok_or(DeployErrorCode::SshSpawnFailed)?;
    let mut stdin = child.stdin.take();

    let interaction = async {
        let write = async move {
            if let Some(input) = input {
                let writer = stdin.as_mut().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin unavailable")
                })?;
                writer.write_all(&input.0).await?;
                writer.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        };
        let (stdout, _stderr, (), status) = tokio::try_join!(
            read_bounded(stdout),
            read_bounded(stderr),
            write,
            child.wait()
        )?;
        Ok::<_, std::io::Error>((stdout, status.success()))
    };

    let result = tokio::select! {
        _ = cancellation.cancelled() => Err(DeployErrorCode::Cancelled),
        result = tokio::time::timeout(timeout, interaction) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::FileTooLarge => {
                Err(DeployErrorCode::ResponseTooLarge)
            }
            Ok(Err(_)) => Err(DeployErrorCode::SshFailed),
            Err(_) => Err(DeployErrorCode::Timeout),
        },
    };
    match result {
        Ok((stdout, success)) => Ok(ProcessOutput { success, stdout }),
        Err(code) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(code.into())
        }
    }
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, std::io::Error> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > MAX_PROCESS_OUTPUT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "process output exceeded limit",
            ));
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

fn ssh_spec(alias: &str, remote_command: impl Into<OsString>) -> ProcessSpec {
    ProcessSpec {
        kind: ProcessKind::Ssh,
        program: "ssh",
        args: vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-o".into(),
            "ServerAliveInterval=5".into(),
            "-o".into(),
            "ServerAliveCountMax=2".into(),
            alias.into(),
            remote_command.into(),
        ],
    }
}

fn scp_spec(alias: &str, source: &Path, destination: &str) -> ProcessSpec {
    ProcessSpec {
        kind: ProcessKind::Scp,
        program: "scp",
        args: vec![
            "-q".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            source.as_os_str().to_owned(),
            format!("{alias}:{destination}").into(),
        ],
    }
}

async fn run_checked(
    runner: &dyn ProcessRunner,
    spec: ProcessSpec,
    input: Option<ProcessInput>,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, DeployFailure> {
    let output = runner.run(spec, input, timeout, cancellation).await?;
    if !output.success {
        return Err(DeployErrorCode::SshFailed.into());
    }
    Ok(output.stdout)
}

fn validate_request(request: &DeploymentRequest) -> Result<(), DeployFailure> {
    let project_id =
        uuid::Uuid::parse_str(&request.project_id).map_err(|_| DeployErrorCode::InvalidRequest)?;
    let workspace_id = uuid::Uuid::parse_str(&request.workspace_id)
        .map_err(|_| DeployErrorCode::InvalidRequest)?;
    if project_id.is_nil()
        || workspace_id.is_nil()
        || project_id.to_string() != request.project_id
        || workspace_id.to_string() != request.workspace_id
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    if request.ssh_host_alias.is_empty()
        || request.ssh_host_alias.starts_with('-')
        || !request.ssh_host_alias.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@' | b':')
        })
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    validate_remote_root(&request.remote_root)?;
    if request.includes.len() > 256
        || request.excludes.len() > 256
        || request
            .includes
            .iter()
            .chain(&request.excludes)
            .any(|rule| rule.is_empty() || rule.len() > 4096 || rule.contains('\0'))
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    Ok(())
}

fn validate_remote_root(root: &str) -> Result<(), DeployFailure> {
    if root.is_empty()
        || root.len() > 4096
        || root
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    let path = Path::new(root);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || matches!(root, "/" | "/root" | "/home")
        || (root.starts_with("/home/") && root[6..].split('/').count() == 1)
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    Ok(())
}

async fn inspect_artifact(path: &Path) -> Result<ArtifactInfo, DeployFailure> {
    let path = path.to_owned();
    if !path.is_absolute() {
        return Err(DeployErrorCode::ArtifactMissing.into());
    }
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| DeployErrorCode::ArtifactMissing)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES
    {
        return Err(DeployErrorCode::ArtifactInvalid.into());
    }
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| DeployErrorCode::ArtifactMissing)?;
    let mut header = [0_u8; 20];
    file.read_exact(&mut header)
        .await
        .map_err(|_| DeployErrorCode::ArtifactInvalid)?;
    if header[..4] != *b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || u16::from_le_bytes([header[18], header[19]]) != 62
    {
        return Err(DeployErrorCode::ArtifactInvalid.into());
    }
    let mut digest = Sha256::new();
    digest.update(header);
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| DeployErrorCode::ArtifactInvalid)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let sha256 = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ArtifactInfo {
        path,
        sha256,
        bytes: metadata.len(),
    })
}

async fn inspect_artifact_bounded(
    path: &Path,
    cancellation: CancellationToken,
) -> Result<ArtifactInfo, DeployFailure> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(DeployErrorCode::Cancelled.into()),
        result = tokio::time::timeout(Duration::from_secs(60), inspect_artifact(path)) => {
            result.map_err(|_| DeployFailure::from(DeployErrorCode::Timeout))?
        }
    }
}

fn remote_probe_script(project_id: &str) -> String {
    let agent_unit = format!("fns-workspace-agent-{project_id}.service");
    format!(
        "agent_unit={}\n{REMOTE_PROBE_SCRIPT}",
        shell_quote(&agent_unit)
    )
}

async fn probe_remote(
    runner: &dyn ProcessRunner,
    request: &DeploymentRequest,
    cancellation: CancellationToken,
) -> Result<RemoteProbe, DeployFailure> {
    let probe_script = remote_probe_script(&request.project_id);
    let stdout = run_checked(
        runner,
        ssh_spec(&request.ssh_host_alias, probe_script),
        None,
        SSH_TIMEOUT,
        cancellation.clone(),
    )
    .await?;
    let fields = parse_probe_fields(&stdout)?;
    if fields.get("system").map(String::as_str) != Some("Linux")
        || fields.get("arch").map(String::as_str) != Some("x86_64")
    {
        return Err(DeployErrorCode::UnsupportedRemote.into());
    }
    let home = decode_probe_path(&fields, "home_b64")?;
    validate_generated_remote_path(&home)?;
    if !home
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        return Err(DeployErrorCode::MalformedResponse.into());
    }
    let service_manager = if fields.get("system_manager").map(String::as_str) == Some("1") {
        Some(ServiceManager::System)
    } else if fields.get("user_manager").map(String::as_str) == Some("1") {
        Some(ServiceManager::User)
    } else {
        None
    };
    let available_bytes = fields
        .get("available_kb")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| value.checked_mul(1024))
        .ok_or(DeployErrorCode::MalformedResponse)?;
    let decode_optional = |name| -> Result<Option<String>, DeployFailure> {
        let value = decode_probe_path(&fields, name)?;
        if value.is_empty() {
            Ok(None)
        } else {
            validate_generated_remote_path(&value)?;
            Ok(Some(value))
        }
    };
    let current_target = decode_probe_path(&fields, "current_b64")?;
    let current_target = if current_target.is_empty() {
        None
    } else {
        let path = Path::new(&current_target);
        if path.is_absolute() {
            validate_generated_remote_path(&current_target)?;
        } else if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
            || current_target
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(DeployErrorCode::MalformedResponse.into());
        }
        Some(current_target)
    };
    let existing_pid = match fields.get("pid").map(String::as_str) {
        None | Some("") => None,
        Some(value) => Some(
            value
                .parse::<u32>()
                .map_err(|_| DeployErrorCode::MalformedResponse)?,
        ),
    };
    let agent_unit_state = match service_manager {
        None => {
            if fields.get("agent_load_state").map(String::as_str) != Some("unavailable")
                || fields.get("agent_unit_file_state").map(String::as_str) != Some("unavailable")
                || fields.get("agent_active_state").map(String::as_str) != Some("unavailable")
            {
                return Err(DeployErrorCode::MalformedResponse.into());
            }
            None
        }
        Some(_) => match (
            fields.get("agent_load_state").map(String::as_str),
            fields.get("agent_unit_file_state").map(String::as_str),
            fields.get("agent_active_state").map(String::as_str),
        ) {
            (Some("not-found"), Some("not-found"), Some("not-found")) => {
                Some(RemoteAgentUnitState::Absent)
            }
            (Some("loaded"), Some(unit_file_state), Some(active_state)) => {
                let enabled = match unit_file_state {
                    "enabled" => true,
                    "disabled" => false,
                    _ => return Err(DeployErrorCode::MalformedResponse.into()),
                };
                let active = match active_state {
                    "active" => true,
                    "inactive" => false,
                    _ => return Err(DeployErrorCode::MalformedResponse.into()),
                };
                Some(RemoteAgentUnitState::Present { enabled, active })
            }
            _ => return Err(DeployErrorCode::MalformedResponse.into()),
        },
    };

    let root_command = format!(
        "if [ -e {root} ]; then [ -d {root} ] && readlink -f -- {root}; else printf '%s\\n' '__FNS_MISSING__'; fi",
        root = shell_quote(&request.remote_root)
    );
    let root_output = run_checked(
        runner,
        ssh_spec(&request.ssh_host_alias, root_command),
        None,
        SSH_TIMEOUT,
        cancellation,
    )
    .await?;
    let resolved = std::str::from_utf8(&root_output)
        .map_err(|_| DeployErrorCode::MalformedResponse)?
        .trim_end_matches(['\r', '\n']);
    if resolved != "__FNS_MISSING__" && resolved != request.remote_root {
        return Err(DeployErrorCode::InvalidRequest.into());
    }

    Ok(RemoteProbe {
        home,
        service_manager,
        available_bytes,
        server_config: decode_optional("config_b64")?,
        server_workdir: decode_optional("workdir_b64")?,
        current_target,
        existing_pid,
        existing_executable: decode_optional("exe_b64")?,
        agent_unit_state,
    })
}

fn parse_probe_fields(stdout: &[u8]) -> Result<HashMap<String, String>, DeployFailure> {
    let text = std::str::from_utf8(stdout).map_err(|_| DeployErrorCode::MalformedResponse)?;
    let mut fields = HashMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or(DeployErrorCode::MalformedResponse)?;
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(DeployErrorCode::MalformedResponse.into());
        }
    }
    Ok(fields)
}

fn decode_probe_path(
    fields: &HashMap<String, String>,
    name: &str,
) -> Result<String, DeployFailure> {
    let encoded = fields.get(name).ok_or(DeployErrorCode::MalformedResponse)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| DeployErrorCode::MalformedResponse)?;
    String::from_utf8(decoded).map_err(|_| DeployErrorCode::MalformedResponse.into())
}

fn validate_generated_remote_path(path: &str) -> Result<(), DeployFailure> {
    if path.len() > 4096
        || !Path::new(path).is_absolute()
        || path
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(DeployErrorCode::MalformedResponse.into());
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn release_version(server: &ArtifactInfo, agent: &ArtifactInfo) -> String {
    format!("0.1.0-{}-{}", &server.sha256[..12], &agent.sha256[..12])
}

fn planned_steps() -> Vec<DeployStep> {
    vec![
        DeployStep::ValidateRemote,
        DeployStep::EnsureDirectories,
        DeployStep::UploadServer,
        DeployStep::UploadAgent,
        DeployStep::VerifyArtifacts,
        DeployStep::PrepareConfiguration,
        DeployStep::SwitchVersion,
        DeployStep::InstallServices,
        DeployStep::StartServices,
        DeployStep::VerifyHealth,
    ]
}

async fn build_preview(
    state: &DeployState,
    request: DeploymentRequest,
    artifacts: ArtifactPaths,
) -> Result<DeploymentPreview, DeployFailure> {
    validate_request(&request)?;
    let cancellation = CancellationToken::new();
    let (server, agent, remote) = tokio::try_join!(
        inspect_artifact_bounded(&artifacts.server, cancellation.clone()),
        inspect_artifact_bounded(&artifacts.agent, cancellation.clone()),
        probe_remote(state.runner.as_ref(), &request, cancellation)
    )?;
    let required = server
        .bytes
        .saturating_add(agent.bytes)
        .saturating_add(256 * 1024 * 1024);
    let mut warnings = Vec::new();
    if remote.service_manager.is_none() {
        warnings.push(DeployErrorCode::SystemdUnavailable);
    }
    if remote.server_config.is_none() || remote.server_workdir.is_none() {
        warnings.push(DeployErrorCode::ServerConfigMissing);
    }
    if remote.available_bytes < required {
        warnings.push(DeployErrorCode::InsufficientDisk);
    }
    let version = release_version(&server, &agent);
    let stored = StoredPreview {
        request,
        created_at: Instant::now(),
        server: server.clone(),
        agent: agent.clone(),
        remote: remote.clone(),
        version: version.clone(),
    };
    let preview_id = state.store_preview(stored);
    Ok(DeploymentPreview {
        preview_id,
        target: "linux-x86_64".to_owned(),
        version,
        service_manager: remote.service_manager,
        existing_version: remote
            .current_target
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .map(str::to_owned),
        artifacts: vec![server.summary("server"), agent.summary("agent")],
        steps: planned_steps(),
        warnings,
    })
}

#[derive(Clone)]
struct RemotePaths {
    share: String,
    release: String,
    release_target: String,
    current: String,
    previous: String,
    config_dir: String,
    state_dir: String,
    server_config: String,
    agent_config: String,
    token: String,
    server_source_marker: String,
    server_workdir_marker: String,
    unit_dir: String,
    server_unit: String,
    agent_unit: String,
}

fn remote_paths(
    request: &DeploymentRequest,
    remote: &RemoteProbe,
    version: &str,
    manager: ServiceManager,
) -> RemotePaths {
    let share = format!("{}/.local/share/fns-workspace", remote.home);
    let release_target = format!("releases/{version}");
    let config_dir = format!("{}/.config/fns-workspace", remote.home);
    let unit_dir = manager.unit_directory(&remote.home);
    RemotePaths {
        share: share.clone(),
        release: format!("{share}/{release_target}"),
        release_target,
        current: format!("{share}/current"),
        previous: format!("{share}/previous"),
        config_dir: config_dir.clone(),
        state_dir: format!(
            "{}/.local/state/fns-workspace/{}",
            remote.home, request.project_id
        ),
        server_config: format!("{config_dir}/server.yaml"),
        agent_config: format!("{config_dir}/agent-{}.json", request.project_id),
        token: format!("{config_dir}/agent-{}.token", request.project_id),
        server_source_marker: format!("{config_dir}/server-source-path"),
        server_workdir_marker: format!("{config_dir}/server-working-directory"),
        unit_dir: unit_dir.clone(),
        server_unit: format!("{unit_dir}/fns-workspace-server.service"),
        agent_unit: format!(
            "{unit_dir}/fns-workspace-agent-{}.service",
            request.project_id
        ),
    }
}

fn derived_remote_client_id(project_id: &str) -> Result<String, DeployFailure> {
    let parsed = uuid::Uuid::parse_str(project_id).map_err(|_| DeployErrorCode::InvalidRequest)?;
    let mut bytes = *parsed.as_bytes();
    for (byte, mask) in bytes.iter_mut().zip(*b"FNSREMOTECLIENT!") {
        *byte ^= mask;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

fn legacy_release_target(executable: &str) -> String {
    let digest = Sha256::digest(executable.as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("releases/legacy-{suffix}")
}

fn token_user_id(token: &fns_platform::SecretToken) -> Result<i64, DeployFailure> {
    token.with_exposed(|bytes| {
        let token = std::str::from_utf8(bytes).map_err(|_| DeployErrorCode::CredentialInvalid)?;
        let mut segments = token.split('.');
        let _header = segments.next().ok_or(DeployErrorCode::CredentialInvalid)?;
        let payload = segments.next().ok_or(DeployErrorCode::CredentialInvalid)?;
        let _signature = segments.next().ok_or(DeployErrorCode::CredentialInvalid)?;
        if segments.next().is_some() {
            return Err(DeployErrorCode::CredentialInvalid.into());
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| DeployErrorCode::CredentialInvalid)?;
        #[derive(Deserialize)]
        struct Claims {
            uid: i64,
        }
        let claims: Claims =
            serde_json::from_slice(&decoded).map_err(|_| DeployErrorCode::CredentialInvalid)?;
        if claims.uid <= 0 {
            return Err(DeployErrorCode::CredentialInvalid.into());
        }
        Ok(claims.uid)
    })
}

fn patch_server_config(
    bytes: &[u8],
    user_id: i64,
    workspace_id: &str,
    remote_root: &str,
) -> Result<Vec<u8>, DeployFailure> {
    let mut value: serde_yaml::Value =
        serde_yaml::from_slice(bytes).map_err(|_| DeployErrorCode::ServerConfigInvalid)?;
    let document = value
        .as_mapping_mut()
        .ok_or(DeployErrorCode::ServerConfigInvalid)?;
    let server = mapping_mut(document, "server")?;
    set_mapping_string(server, "http-port", "127.0.0.1:9000");
    set_mapping_string(server, "private-http-listen", "");
    set_mapping_string(server, "webgui-port", "");
    set_mapping_string(server, "share-port", "");

    let workspace = mapping_mut(document, "workspace")?;
    let roots_key = serde_yaml::Value::String("roots".to_owned());
    let roots = workspace
        .entry(roots_key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or(DeployErrorCode::ServerConfigInvalid)?;
    roots.retain(|entry| {
        let Some(mapping) = entry.as_mapping() else {
            return true;
        };
        let uid_matches = mapping
            .get(serde_yaml::Value::String("uid".to_owned()))
            .and_then(serde_yaml::Value::as_i64)
            == Some(user_id);
        let workspace_matches = mapping
            .get(serde_yaml::Value::String("workspace-id".to_owned()))
            .and_then(serde_yaml::Value::as_str)
            == Some(workspace_id);
        !(uid_matches && workspace_matches)
    });
    let mut root = serde_yaml::Mapping::new();
    root.insert(
        serde_yaml::Value::String("uid".to_owned()),
        serde_yaml::Value::Number(user_id.into()),
    );
    root.insert(
        serde_yaml::Value::String("workspace-id".to_owned()),
        serde_yaml::Value::String(workspace_id.to_owned()),
    );
    root.insert(
        serde_yaml::Value::String("root".to_owned()),
        serde_yaml::Value::String(remote_root.to_owned()),
    );
    roots.push(serde_yaml::Value::Mapping(root));
    serde_yaml::to_string(&value)
        .map(String::into_bytes)
        .map_err(|_| DeployErrorCode::ServerConfigInvalid.into())
}

fn mapping_mut<'a>(
    document: &'a mut serde_yaml::Mapping,
    name: &str,
) -> Result<&'a mut serde_yaml::Mapping, DeployFailure> {
    document
        .get_mut(serde_yaml::Value::String(name.to_owned()))
        .and_then(serde_yaml::Value::as_mapping_mut)
        .ok_or_else(|| DeployErrorCode::ServerConfigInvalid.into())
}

fn set_mapping_string(mapping: &mut serde_yaml::Mapping, name: &str, value: &str) {
    mapping.insert(
        serde_yaml::Value::String(name.to_owned()),
        serde_yaml::Value::String(value.to_owned()),
    );
}

fn build_agent_config(
    request: &DeploymentRequest,
    paths: &RemotePaths,
) -> Result<Vec<u8>, DeployFailure> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": "fns-agent-config/1",
        "endpoint": format!("ws://127.0.0.1:{SERVER_PORT}/api/user/workspace-sync/v2"),
        "workspaceId": request.workspace_id,
        "clientId": derived_remote_client_id(&request.project_id)?,
        "workspaceRoot": request.remote_root,
        "stateDir": paths.state_dir,
        "tokenFile": paths.token,
        "sync": {
            "includes": request.includes,
            "excludes": request.excludes,
            "protectSecrets": request.protect_secrets,
        },
        "transport": { "maxActiveTransfers": 4 },
    }))
    .map_err(|_| DeployErrorCode::InvalidRequest.into())
}

fn systemd_quote(value: &str) -> Result<String, DeployFailure> {
    if value
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn server_unit(
    paths: &RemotePaths,
    workdir: &str,
    manager: ServiceManager,
) -> Result<Vec<u8>, DeployFailure> {
    let target = match manager {
        ServiceManager::System => "multi-user.target",
        ServiceManager::User => "default.target",
    };
    Ok(format!(
        "[Unit]\nDescription=FNS Workspace Server\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nTimeoutStopSec={}s\nWorkingDirectory={}\nExecStart={} run --config {}\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=true\n\n[Install]\nWantedBy={target}\n",
        SERVER_SYSTEMD_STOP_TIMEOUT_SECONDS,
        systemd_quote(workdir)?,
        systemd_quote(&format!("{}/fns-server", paths.current))?,
        systemd_quote(&paths.server_config)?,
    )
    .into_bytes())
}

fn agent_unit_for_manager(
    request: &DeploymentRequest,
    paths: &RemotePaths,
    manager: ServiceManager,
) -> Result<Vec<u8>, DeployFailure> {
    let target = match manager {
        ServiceManager::System => "multi-user.target",
        ServiceManager::User => "default.target",
    };
    Ok(format!(
        "[Unit]\nDescription=FNS Workspace Remote Agent ({})\nAfter=fns-workspace-server.service\nRequires=fns-workspace-server.service\n\n[Service]\nType=simple\nKillMode=mixed\nTimeoutStopSec={}s\nWorkingDirectory={}\nExecStart={} run --config {}\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=true\n\n[Install]\nWantedBy={target}\n",
        request.project_id,
        AGENT_SYSTEMD_STOP_TIMEOUT_SECONDS,
        systemd_quote(&request.remote_root)?,
        systemd_quote(&format!("{}/fns-agent", paths.current))?,
        systemd_quote(&paths.agent_config)?,
    )
    .into_bytes())
}

async fn run_systemd_lifecycle_command(
    runner: &dyn ProcessRunner,
    alias: &str,
    command: String,
    cancellation: CancellationToken,
) -> Result<(), DeployFailure> {
    run_checked(
        runner,
        ssh_spec(alias, command),
        None,
        SYSTEMD_LIFECYCLE_TIMEOUT,
        cancellation,
    )
    .await
    .map(|_| ())
}

async fn write_remote_file(
    runner: &dyn ProcessRunner,
    alias: &str,
    destination: &str,
    mode: &str,
    bytes: Vec<u8>,
    cancellation: CancellationToken,
) -> Result<(), DeployFailure> {
    let temporary = format!("{destination}.tmp-{}", uuid::Uuid::new_v4());
    let command = format!(
        "set -eu; umask 077; cat > {temporary}; chmod {mode} {temporary}; mv -f {temporary} {destination}",
        temporary = shell_quote(&temporary),
        destination = shell_quote(destination),
    );
    run_checked(
        runner,
        ssh_spec(alias, command),
        Some(ProcessInput::new(bytes)),
        SSH_TIMEOUT,
        cancellation,
    )
    .await
    .map(|_| ())
}

async fn emit_step<T, F, Fut>(
    request: &DeploymentRequest,
    step: DeployStep,
    progress: &mut T,
    operation: F,
) -> Result<(), DeployFailure>
where
    T: FnMut(DeployProgress),
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), DeployFailure>>,
{
    progress(DeployProgress {
        project_id: request.project_id.clone(),
        step,
        status: DeployStepStatus::Running,
        error_code: None,
    });
    match operation().await {
        Ok(()) => {
            progress(DeployProgress {
                project_id: request.project_id.clone(),
                step,
                status: DeployStepStatus::Succeeded,
                error_code: None,
            });
            Ok(())
        }
        Err(mut failure) => {
            failure.step = Some(step);
            progress(DeployProgress {
                project_id: request.project_id.clone(),
                step,
                status: DeployStepStatus::Failed,
                error_code: Some(failure.primary),
            });
            Err(failure)
        }
    }
}

async fn execute_plan<T: FnMut(DeployProgress)>(
    runner: &dyn ProcessRunner,
    request: &DeploymentRequest,
    preview: &StoredPreview,
    token: &fns_platform::SecretToken,
    cancellation: CancellationToken,
    mut progress: T,
) -> Result<DeploymentOutcome, DeployFailure> {
    let manager = preview
        .remote
        .service_manager
        .ok_or(DeployErrorCode::SystemdUnavailable)?;
    let source_config = preview
        .remote
        .server_config
        .as_deref()
        .ok_or(DeployErrorCode::ServerConfigMissing)?;
    let server_workdir = preview
        .remote
        .server_workdir
        .as_deref()
        .ok_or(DeployErrorCode::ServerConfigMissing)?;
    let required = preview
        .server
        .bytes
        .saturating_add(preview.agent.bytes)
        .saturating_add(256 * 1024 * 1024);
    if preview.remote.available_bytes < required {
        return Err(DeployErrorCode::InsufficientDisk.into());
    }
    let paths = remote_paths(request, &preview.remote, &preview.version, manager);
    let mut switched = false;

    let result = async {
        emit_step(
            request,
            DeployStep::ValidateRemote,
            &mut progress,
            || async {
                let (server, agent, remote) = tokio::try_join!(
                    inspect_artifact_bounded(&preview.server.path, cancellation.clone()),
                    inspect_artifact_bounded(&preview.agent.path, cancellation.clone()),
                    probe_remote(runner, request, cancellation.clone())
                )?;
                if server.sha256 != preview.server.sha256
                    || agent.sha256 != preview.agent.sha256
                    || remote.service_manager != preview.remote.service_manager
                    || remote.server_config != preview.remote.server_config
                    || remote.agent_unit_state != preview.remote.agent_unit_state
                {
                    return Err(DeployErrorCode::ArtifactChanged.into());
                }
                Ok(())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::EnsureDirectories,
            &mut progress,
            || async {
                let command = format!(
                    "set -eu; install -d -m 700 {share} {releases} {release} {config} {state}; if [ -e {root} ]; then [ -d {root} ]; else install -d -m 700 {root}; fi",
                    share = shell_quote(&paths.share),
                    releases = shell_quote(&format!("{}/releases", paths.share)),
                    release = shell_quote(&paths.release),
                    config = shell_quote(&paths.config_dir),
                    state = shell_quote(&paths.state_dir),
                    root = shell_quote(&request.remote_root),
                );
                run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await
                .map(|_| ())
            },
        )
        .await?;

        let server_upload = format!("{}/.fns-server.upload", paths.release);
        emit_step(
            request,
            DeployStep::UploadServer,
            &mut progress,
            || async {
                run_checked(
                    runner,
                    scp_spec(
                        &request.ssh_host_alias,
                        &preview.server.path,
                        &server_upload,
                    ),
                    None,
                    SCP_TIMEOUT,
                    cancellation.clone(),
                )
                .await?;
                let command = format!(
                    "set -eu; chmod 0755 {upload}; mv -f {upload} {destination}",
                    upload = shell_quote(&server_upload),
                    destination = shell_quote(&format!("{}/fns-server", paths.release)),
                );
                run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await
                .map(|_| ())
            },
        )
        .await?;

        let agent_upload = format!("{}/.fns-agent.upload", paths.release);
        emit_step(
            request,
            DeployStep::UploadAgent,
            &mut progress,
            || async {
                run_checked(
                    runner,
                    scp_spec(
                        &request.ssh_host_alias,
                        &preview.agent.path,
                        &agent_upload,
                    ),
                    None,
                    SCP_TIMEOUT,
                    cancellation.clone(),
                )
                .await?;
                let command = format!(
                    "set -eu; chmod 0755 {upload}; mv -f {upload} {destination}",
                    upload = shell_quote(&agent_upload),
                    destination = shell_quote(&format!("{}/fns-agent", paths.release)),
                );
                run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await
                .map(|_| ())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::VerifyArtifacts,
            &mut progress,
            || async {
                let command = format!(
                    "sha256sum -- {server} {agent}",
                    server = shell_quote(&format!("{}/fns-server", paths.release)),
                    agent = shell_quote(&format!("{}/fns-agent", paths.release)),
                );
                let output = run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await?;
                let text = std::str::from_utf8(&output)
                    .map_err(|_| DeployErrorCode::MalformedResponse)?;
                let hashes = text
                    .lines()
                    .filter_map(|line| line.split_whitespace().next())
                    .collect::<Vec<_>>();
                if hashes != [preview.server.sha256.as_str(), preview.agent.sha256.as_str()] {
                    return Err(DeployErrorCode::ArtifactChecksumMismatch.into());
                }
                Ok(())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::PrepareConfiguration,
            &mut progress,
            || async {
                let source = run_checked(
                    runner,
                    ssh_spec(
                        &request.ssh_host_alias,
                        format!("cat -- {}", shell_quote(source_config)),
                    ),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await?;
                let server_config = patch_server_config(
                    &source,
                    token_user_id(token)?,
                    &request.workspace_id,
                    &request.remote_root,
                )?;
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.server_config,
                    "0600",
                    server_config,
                    cancellation.clone(),
                )
                .await?;
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.agent_config,
                    "0600",
                    build_agent_config(request, &paths)?,
                    cancellation.clone(),
                )
                .await?;
                let token_bytes = token.with_exposed(|bytes| bytes.to_vec());
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.token,
                    "0600",
                    token_bytes,
                    cancellation.clone(),
                )
                .await?;
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.server_source_marker,
                    "0600",
                    source_config.as_bytes().to_vec(),
                    cancellation.clone(),
                )
                .await?;
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.server_workdir_marker,
                    "0600",
                    server_workdir.as_bytes().to_vec(),
                    cancellation.clone(),
                )
                .await
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::SwitchVersion,
            &mut progress,
            || async {
                let temporary = format!("{}.tmp-{}", paths.current, request.project_id);
                let previous = preview.remote.current_target.as_deref().unwrap_or("");
                let legacy = preview
                    .remote
                    .existing_executable
                    .as_deref()
                    .map(legacy_release_target)
                    .unwrap_or_else(|| "releases/legacy-unknown".to_owned());
                let legacy_absolute = format!("{}/{}", paths.share, legacy);
                let legacy_command = match preview.remote.existing_executable.as_deref() {
                    Some(executable) if previous.is_empty() => format!(
                        "install -d -m 700 {legacy}; cp -- {executable} {legacy}/fns-server; chmod 0755 {legacy}/fns-server; old={old}",
                        legacy = shell_quote(&legacy_absolute),
                        executable = shell_quote(executable),
                        old = shell_quote(&legacy),
                    ),
                    _ => format!("old={}", shell_quote(previous)),
                };
                let command = format!(
                    "set -eu; {legacy_command}; if [ -n \"$old\" ]; then ln -sfn -- \"$old\" {previous}; fi; ln -sfn -- {target} {temporary}; mv -Tf -- {temporary} {current}",
                    previous = shell_quote(&paths.previous),
                    target = shell_quote(&paths.release_target),
                    temporary = shell_quote(&temporary),
                    current = shell_quote(&paths.current),
                );
                run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await?;
                switched = true;
                Ok(())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::InstallServices,
            &mut progress,
            || async {
                if manager == ServiceManager::User {
                    run_checked(
                        runner,
                        ssh_spec(
                            &request.ssh_host_alias,
                            format!("install -d -m 700 {}", shell_quote(&paths.unit_dir)),
                        ),
                        None,
                        SSH_TIMEOUT,
                        cancellation.clone(),
                    )
                    .await?;
                }
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.server_unit,
                    "0644",
                    server_unit(&paths, server_workdir, manager)?,
                    cancellation.clone(),
                )
                .await?;
                write_remote_file(
                    runner,
                    &request.ssh_host_alias,
                    &paths.agent_unit,
                    "0644",
                    agent_unit_for_manager(request, &paths, manager)?,
                    cancellation.clone(),
                )
                .await?;
                let command = format!("{} daemon-reload", manager.command());
                run_checked(
                    runner,
                    ssh_spec(&request.ssh_host_alias, command),
                    None,
                    SSH_TIMEOUT,
                    cancellation.clone(),
                )
                .await
                .map(|_| ())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::StartServices,
            &mut progress,
            || async {
                for legacy_unit in ["fns-server.service", "fast-note-sync-service.service"] {
                    let command = format!(
                        "{} stop {} >/dev/null 2>&1 || true",
                        manager.command(),
                        legacy_unit
                    );
                    run_systemd_lifecycle_command(
                        runner,
                        &request.ssh_host_alias,
                        command,
                        cancellation.clone(),
                    )
                    .await?;
                }
                if let (Some(pid), Some(executable)) = (
                    preview.remote.existing_pid,
                    preview.remote.existing_executable.as_deref(),
                ) {
                    let stop = format!(
                        "set -eu; if [ -e /proc/{pid}/exe ] && [ \"$(readlink -f /proc/{pid}/exe)\" = {executable} ]; then kill -TERM {pid}; i=0; while kill -0 {pid} 2>/dev/null && [ $i -lt 50 ]; do sleep 0.2; i=$((i+1)); done; ! kill -0 {pid} 2>/dev/null; fi",
                        executable = shell_quote(executable),
                    );
                    run_checked(
                        runner,
                        ssh_spec(&request.ssh_host_alias, stop),
                        None,
                        SSH_TIMEOUT,
                        cancellation.clone(),
                    )
                    .await?;
                }
                let agent_name = format!("fns-workspace-agent-{}.service", request.project_id);
                for command in [
                    format!("{} enable fns-workspace-server.service", manager.command()),
                    format!("{} enable {}", manager.command(), agent_name),
                ] {
                    run_checked(
                        runner,
                        ssh_spec(&request.ssh_host_alias, command),
                        None,
                        SSH_TIMEOUT,
                        cancellation.clone(),
                    )
                    .await?;
                }
                for command in [
                    format!("{} restart fns-workspace-server.service", manager.command()),
                    format!("{} restart {}", manager.command(), agent_name),
                ] {
                    run_systemd_lifecycle_command(
                        runner,
                        &request.ssh_host_alias,
                        command,
                        cancellation.clone(),
                    )
                    .await?;
                }
                Ok(())
            },
        )
        .await?;

        emit_step(
            request,
            DeployStep::VerifyHealth,
            &mut progress,
            || async {
                wait_for_health(
                    runner,
                    request,
                    &paths,
                    manager,
                    cancellation.clone(),
                )
                .await
            },
        )
        .await?;

        Ok(DeploymentOutcome {
            project_id: request.project_id.clone(),
            version: preview.version.clone(),
            service_manager: manager,
            server_active: true,
            agent_online: true,
            rolled_back: false,
        })
    }
    .await;

    match result {
        Ok(outcome) => Ok(outcome),
        Err(failure) if switched => {
            let rollback = rollback_remote(
                runner,
                request,
                &paths,
                manager,
                &preview.remote,
                CancellationToken::new(),
            )
            .await;
            match rollback {
                Ok(()) => Err(failure),
                Err(_) => Err(failure.with_cleanup(DeployErrorCode::RollbackFailed)),
            }
        }
        Err(failure) => Err(failure),
    }
}

async fn wait_for_health(
    runner: &dyn ProcessRunner,
    request: &DeploymentRequest,
    paths: &RemotePaths,
    manager: ServiceManager,
    cancellation: CancellationToken,
) -> Result<(), DeployFailure> {
    let expected_workspace = fns_protocol::WorkspaceId::parse(&request.workspace_id)
        .map_err(|_| DeployErrorCode::InvalidRequest)?;
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return Err(DeployErrorCode::Cancelled.into());
        }
        if started.elapsed() >= HEALTH_TIMEOUT {
            return Err(DeployErrorCode::ServiceUnhealthy.into());
        }
        let agent_name = format!("fns-workspace-agent-{}.service", request.project_id);
        let expected_agent_executable = format!("{}/fns-agent", paths.release);
        let command = format!(
            "if {manager} is-active --quiet fns-workspace-server.service && {manager} is-active --quiet {agent}; then encode() {{ printf '%s' \"$1\" | base64 | tr -d '\n'; }}; main_pid=$({manager} show --property MainPID --value {agent}); case \"$main_pid\" in ''|*[!0-9]*|0) exit 0 ;; esac; main_exe=$(readlink -f /proc/\"$main_pid\"/exe 2>/dev/null || true); printf 'main_pid=%s\nmain_exe_b64=%s\nchild_processes=' \"$main_pid\" \"$(encode \"$main_exe\")\"; for child_file in /proc/\"$main_pid\"/task/*/children; do if [ -r \"$child_file\" ]; then for child_pid in $(cat \"$child_file\"); do case \"$child_pid\" in ''|*[!0-9]*|0) continue ;; esac; child_exe=$(readlink -f /proc/\"$child_pid\"/exe 2>/dev/null || true); printf '%s:%s ' \"$child_pid\" \"$(encode \"$child_exe\")\"; done; fi; done; printf '\n'; {binary} status --config {config} --json || true; fi",
            manager = manager.command(),
            agent = agent_name,
            binary = shell_quote(&format!("{}/fns-agent", paths.current)),
            config = shell_quote(&paths.agent_config),
        );
        let output = run_checked(
            runner,
            ssh_spec(&request.ssh_host_alias, command),
            None,
            SSH_TIMEOUT,
            cancellation.clone(),
        )
        .await?;
        if remote_agent_is_healthy(&output, expected_workspace, &expected_agent_executable) {
            return Ok(());
        }
        tokio::select! {
            _ = cancellation.cancelled() => return Err(DeployErrorCode::Cancelled.into()),
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
}

fn remote_agent_is_healthy(
    output: &[u8],
    expected_workspace: fns_protocol::WorkspaceId,
    expected_agent_executable: &str,
) -> bool {
    let mut sections = output.splitn(4, |byte| *byte == b'\n');
    let (Some(main_pid_line), Some(main_exe_line), Some(child_processes_line), Some(status_json)) = (
        sections.next(),
        sections.next(),
        sections.next(),
        sections.next(),
    ) else {
        return false;
    };
    let Ok(main_pid_line) = std::str::from_utf8(main_pid_line) else {
        return false;
    };
    let Some(main_pid_text) = main_pid_line.strip_prefix("main_pid=") else {
        return false;
    };
    let Ok(main_pid) = main_pid_text.parse::<u32>() else {
        return false;
    };
    if main_pid == 0 {
        return false;
    }
    let Ok(main_exe_line) = std::str::from_utf8(main_exe_line) else {
        return false;
    };
    let Some(main_exe_b64) = main_exe_line.strip_prefix("main_exe_b64=") else {
        return false;
    };
    let Ok(main_exe) = base64::engine::general_purpose::STANDARD.decode(main_exe_b64) else {
        return false;
    };
    let Ok(main_exe) = std::str::from_utf8(&main_exe) else {
        return false;
    };
    if main_exe != expected_agent_executable {
        return false;
    }
    let Ok(child_processes_line) = std::str::from_utf8(child_processes_line) else {
        return false;
    };
    let Some(child_processes) = child_processes_line.strip_prefix("child_processes=") else {
        return false;
    };
    let Ok(status) = serde_json::from_slice::<fns_agent::AgentStatus>(status_json) else {
        return false;
    };
    let Some(worker_pid) = status.pid else {
        return false;
    };
    if worker_pid == 0 || worker_pid == main_pid {
        return false;
    }
    let mut worker_executable = None;
    for child_process in child_processes.split_ascii_whitespace() {
        let Some((child_pid, child_exe_b64)) = child_process.split_once(':') else {
            return false;
        };
        let Ok(child_pid) = child_pid.parse::<u32>() else {
            return false;
        };
        if child_pid == 0 {
            return false;
        }
        let Ok(child_exe) = base64::engine::general_purpose::STANDARD.decode(child_exe_b64) else {
            return false;
        };
        let Ok(child_exe) = String::from_utf8(child_exe) else {
            return false;
        };
        if child_pid == worker_pid && worker_executable.replace(child_exe).is_some() {
            return false;
        }
    }
    status.schema_version == "fns-agent-status/1"
        && status.workspace_id == expected_workspace
        && status.running
        && status.phase == fns_agent::AgentPhase::Online
        && status.connected
        && worker_executable.as_deref() == Some(expected_agent_executable)
        && status.pending_commands == 0
        && status.queued_watcher_batches == 0
        && status.active_transfers == 0
        && status.reconnect_attempt == 0
        && status.last_error_code.is_none()
        && status.updated_at_ms > 0
}

async fn rollback_remote(
    runner: &dyn ProcessRunner,
    request: &DeploymentRequest,
    paths: &RemotePaths,
    manager: ServiceManager,
    remote: &RemoteProbe,
    cancellation: CancellationToken,
) -> Result<(), DeployFailure> {
    let temporary = format!("{}.rollback-{}", paths.current, request.project_id);
    let agent_name = format!("fns-workspace-agent-{}.service", request.project_id);
    let systemd_action_if_loaded = |action: &str, unit: &str| {
        format!(
            "set -eu; load_state=$({manager} show --property LoadState --value {unit}); case \"$load_state\" in loaded) {manager} {action} {unit} ;; not-found) ;; *) exit 42 ;; esac",
            manager = manager.command(),
        )
    };
    let stop_agent = systemd_action_if_loaded("stop", &agent_name);
    let existing_alive = remote
        .existing_pid
        .zip(remote.existing_executable.as_deref());
    let server_recovery = existing_alive.map_or_else(
        || format!("{} restart fns-workspace-server.service", manager.command()),
        |(pid, executable)| {
            format!(
                "if ! [ -e /proc/{pid}/exe ] || [ \"$(readlink -f /proc/{pid}/exe)\" != {executable} ]; then {} restart fns-workspace-server.service; fi",
                manager.command(),
                executable = shell_quote(executable),
            )
        },
    );
    let previous_target = remote.current_target.clone().or_else(|| {
        remote
            .existing_executable
            .as_deref()
            .map(legacy_release_target)
    });
    let prior_agent_state = remote
        .agent_unit_state
        .ok_or(DeployErrorCode::RollbackFailed)?;

    let result: Result<(), DeployFailure> = async {
        run_systemd_lifecycle_command(
            runner,
            &request.ssh_host_alias,
            stop_agent,
            cancellation.clone(),
        )
        .await?;
        if let Some(previous_target) = previous_target {
            let switch_previous = format!(
                "set -eu; [ -L {previous} ]; target=$(readlink {previous}); [ \"$target\" = {expected} ]; ln -sfn -- \"$target\" {temporary}; mv -Tf -- {temporary} {current}; [ -L {current} ]; [ \"$(readlink {current})\" = \"$target\" ]",
                previous = shell_quote(&paths.previous),
                expected = shell_quote(&previous_target),
                temporary = shell_quote(&temporary),
                current = shell_quote(&paths.current),
            );
            run_checked(
                runner,
                ssh_spec(&request.ssh_host_alias, switch_previous),
                None,
                SSH_TIMEOUT,
                cancellation.clone(),
            )
            .await?;
            run_systemd_lifecycle_command(
                runner,
                &request.ssh_host_alias,
                server_recovery,
                cancellation.clone(),
            )
            .await?;
            let restore_agent_commands = match prior_agent_state {
                RemoteAgentUnitState::Absent => {
                    vec![systemd_action_if_loaded("disable", &agent_name)]
                }
                RemoteAgentUnitState::Present { enabled, active } => vec![
                    format!(
                        "{} {} {agent_name}",
                        manager.command(),
                        if enabled { "enable" } else { "disable" }
                    ),
                    format!(
                        "{} {} {agent_name}",
                        manager.command(),
                        match (enabled, active) {
                            (true, true) => "restart",
                            (false, true) => "start",
                            (_, false) => "stop",
                        }
                    ),
                ],
            };
            for command in restore_agent_commands {
                run_systemd_lifecycle_command(
                    runner,
                    &request.ssh_host_alias,
                    command,
                    cancellation.clone(),
                )
                .await?;
            }
            Ok(())
        } else {
            for command in [
                systemd_action_if_loaded("disable", &agent_name),
                systemd_action_if_loaded("stop", "fns-workspace-server.service"),
                systemd_action_if_loaded("disable", "fns-workspace-server.service"),
            ] {
                run_systemd_lifecycle_command(
                    runner,
                    &request.ssh_host_alias,
                    command,
                    cancellation.clone(),
                )
                .await?;
            }
            let remove_current = format!(
                "set -eu; if [ -L {current} ]; then target=$(readlink {current}); [ \"$target\" = {expected} ]; rm -f -- {current}; elif [ -e {current} ]; then exit 42; fi; [ ! -e {current} ]; [ ! -L {current} ]",
                current = shell_quote(&paths.current),
                expected = shell_quote(&paths.release_target),
            );
            run_checked(
                runner,
                ssh_spec(&request.ssh_host_alias, remove_current),
                None,
                SSH_TIMEOUT,
                cancellation,
            )
            .await
            .map(|_| ())
        }
    }
    .await;
    result.map_err(|_| DeployErrorCode::RollbackFailed.into())
}

#[tauri::command]
pub(crate) async fn preview_remote_deployment(
    request: DeploymentRequest,
    app: tauri::AppHandle,
    deploy_state: tauri::State<'_, DeployState>,
) -> Result<DeploymentPreview, DeployFailure> {
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| DeployErrorCode::ArtifactMissing)?;
    let root = resources.join("remote").join("linux-x86_64");
    build_preview(
        deploy_state.inner(),
        request,
        ArtifactPaths {
            server: root.join("fns-server"),
            agent: root.join("fns-agent"),
        },
    )
    .await
}

#[tauri::command]
pub(crate) async fn execute_remote_deployment(
    preview_id: String,
    request: DeploymentRequest,
    app: tauri::AppHandle,
    deploy_state: tauri::State<'_, DeployState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<DeploymentOutcome, DeployFailure> {
    validate_request(&request)?;
    let preview = deploy_state.load_preview(&preview_id, &request)?;
    let operation = deploy_state.begin_operation(&request.project_id)?;
    let token = credential_state
        .token_for_project(&request.project_id)
        .map_err(|_| DeployErrorCode::CredentialMissing)?;
    let outcome = execute_plan(
        deploy_state.runner.as_ref(),
        &request,
        &preview,
        &token,
        operation.cancellation.clone(),
        |progress| {
            if app.emit(DEPLOY_PROGRESS_EVENT, progress).is_err() {
                eprintln!("fns_deploy_progress_emit_failed");
            }
        },
    )
    .await?;
    deploy_state
        .previews
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&preview_id);
    Ok(outcome)
}

#[tauri::command]
pub(crate) fn cancel_remote_deployment(
    project_id: String,
    deploy_state: tauri::State<'_, DeployState>,
) -> Result<bool, DeployFailure> {
    let parsed = uuid::Uuid::parse_str(&project_id).map_err(|_| DeployErrorCode::InvalidRequest)?;
    if parsed.is_nil() || parsed.to_string() != project_id {
        return Err(DeployErrorCode::InvalidRequest.into());
    }
    Ok(deploy_state.cancel(&project_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const PROJECT_ID: &str = "8a5af216-515c-4b34-bd88-f9f9332ce333";
    const WORKSPACE_ID: &str = "4a762b52-edab-42d6-93b1-481359f83164";
    const TOKEN_SENTINEL: &str = "e30.eyJ1aWQiOjQxfQ.signature-sentinel";
    const HEALTH_AGENT_EXE: &str = "/opt/fns/releases/current/fns-agent";

    #[derive(Clone, Debug)]
    struct RecordedCall {
        kind: ProcessKind,
        args: Vec<String>,
        input: Option<Vec<u8>>,
        timeout: Duration,
    }

    struct FakeRunner {
        calls: StdMutex<Vec<RecordedCall>>,
        server_hash: String,
        agent_hash: String,
        checksum_mismatch: bool,
        service_manager: bool,
        fail_command: Option<String>,
    }

    impl FakeRunner {
        fn new(server_hash: String, agent_hash: String) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                server_hash,
                agent_hash,
                checksum_mismatch: false,
                service_manager: true,
                fail_command: None,
            }
        }

        fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().unwrap().clone()
        }

        fn response(&self, spec: &ProcessSpec) -> ProcessOutput {
            if spec.kind == ProcessKind::Scp {
                return ProcessOutput {
                    success: true,
                    stdout: Vec::new(),
                };
            }
            let command = spec
                .args
                .last()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let stdout = if command.ends_with(REMOTE_PROBE_SCRIPT) {
                probe_output(self.service_manager)
            } else if command.contains("__FNS_MISSING__") {
                b"/srv/work\n".to_vec()
            } else if command.starts_with("sha256sum --") {
                let server = if self.checksum_mismatch {
                    "0000000000000000000000000000000000000000000000000000000000000000"
                } else {
                    &self.server_hash
                };
                format!("{server}  fns-server\n{}  fns-agent\n", self.agent_hash).into_bytes()
            } else if command.starts_with("cat --") {
                b"server:\n  http-port: ':9000'\n  private-http-listen: ''\n  webgui-port: ''\n  share-port: ''\nworkspace:\n  roots: []\n"
                    .to_vec()
            } else if command.contains(" status --config ") {
                let executable = format!(
                    "/home/fns/.local/share/fns-workspace/releases/0.1.0-{}-{}/fns-agent",
                    &self.server_hash[..12],
                    &self.agent_hash[..12],
                );
                healthy_status_output(4_242, 4_243, &executable, WORKSPACE_ID)
            } else {
                Vec::new()
            };
            let success = self
                .fail_command
                .as_ref()
                .map(|needle| !command.contains(needle.as_str()))
                .unwrap_or(true);
            ProcessOutput { success, stdout }
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run<'a>(
            &'a self,
            spec: ProcessSpec,
            input: Option<ProcessInput>,
            timeout: Duration,
            cancellation: CancellationToken,
        ) -> ProcessFuture<'a> {
            let input = input.map(|value| value.0.to_vec());
            self.calls.lock().unwrap().push(RecordedCall {
                kind: spec.kind,
                args: spec
                    .args
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
                input,
                timeout,
            });
            let output = self.response(&spec);
            Box::pin(async move {
                if cancellation.is_cancelled() {
                    Err(DeployErrorCode::Cancelled.into())
                } else {
                    Ok(output)
                }
            })
        }
    }

    fn request() -> DeploymentRequest {
        DeploymentRequest {
            project_id: PROJECT_ID.to_owned(),
            ssh_host_alias: "fixture-host".to_owned(),
            workspace_id: WORKSPACE_ID.to_owned(),
            remote_root: "/srv/work".to_owned(),
            includes: vec!["**".to_owned()],
            excludes: vec![".git/**".to_owned()],
            protect_secrets: true,
        }
    }

    #[test]
    fn agent_units_allow_the_supervisor_to_shut_down_its_worker() {
        let request = request();
        let remote = RemoteProbe {
            home: "/home/fns".to_owned(),
            service_manager: Some(ServiceManager::System),
            available_bytes: 1024 * 1024 * 1024,
            server_config: Some("/opt/fns/config/config.yaml".to_owned()),
            server_workdir: Some("/opt/fns".to_owned()),
            current_target: None,
            existing_pid: None,
            existing_executable: None,
            agent_unit_state: Some(RemoteAgentUnitState::Absent),
        };

        for manager in [ServiceManager::System, ServiceManager::User] {
            let paths = remote_paths(&request, &remote, "test-version", manager);
            let unit = String::from_utf8(
                agent_unit_for_manager(&request, &paths, manager).expect("agent unit"),
            )
            .expect("utf-8 unit");

            for directive in [
                "KillMode=mixed",
                "TimeoutStopSec=35s",
                "Restart=on-failure",
                "RestartSec=2",
            ] {
                assert!(
                    unit.lines().any(|line| line == directive),
                    "missing {directive} in {manager:?} agent unit:\n{unit}"
                );
            }

            let stop_seconds = unit
                .lines()
                .find_map(|line| line.strip_prefix("TimeoutStopSec="))
                .and_then(|value| value.strip_suffix('s'))
                .and_then(|value| value.parse::<u64>().ok())
                .expect("numeric TimeoutStopSec");
            assert_eq!(stop_seconds, AGENT_SYSTEMD_STOP_TIMEOUT_SECONDS);
            assert!(
                SYSTEMD_LIFECYCLE_TIMEOUT
                    >= Duration::from_secs(stop_seconds + SYSTEMD_LIFECYCLE_TIMEOUT_MARGIN_SECONDS),
                "systemd lifecycle timeout must exceed TimeoutStopSec with cancellation margin"
            );
        }
    }

    #[test]
    fn server_units_bound_systemd_shutdown() {
        let request = request();
        let remote = RemoteProbe {
            home: "/home/fns".to_owned(),
            service_manager: Some(ServiceManager::System),
            available_bytes: 1024 * 1024 * 1024,
            server_config: Some("/opt/fns/config/config.yaml".to_owned()),
            server_workdir: Some("/opt/fns".to_owned()),
            current_target: None,
            existing_pid: None,
            existing_executable: None,
            agent_unit_state: Some(RemoteAgentUnitState::Absent),
        };

        for manager in [ServiceManager::System, ServiceManager::User] {
            let paths = remote_paths(&request, &remote, "test-version", manager);
            let unit =
                String::from_utf8(server_unit(&paths, "/opt/fns", manager).expect("server unit"))
                    .expect("utf-8 unit");
            assert!(
                unit.lines().any(|line| line == "TimeoutStopSec=35s"),
                "missing bounded TimeoutStopSec in {manager:?} server unit:\n{unit}"
            );
            assert!(
                SYSTEMD_LIFECYCLE_TIMEOUT
                    >= Duration::from_secs(
                        SERVER_SYSTEMD_STOP_TIMEOUT_SECONDS
                            + SYSTEMD_LIFECYCLE_TIMEOUT_MARGIN_SECONDS
                    ),
                "systemd lifecycle timeout must exceed the server stop timeout"
            );
        }
    }

    fn rollback_remote_probe(current_target: Option<&str>) -> RemoteProbe {
        RemoteProbe {
            home: "/home/fns".to_owned(),
            service_manager: Some(ServiceManager::System),
            available_bytes: 1024 * 1024 * 1024,
            server_config: Some("/opt/fns/config/config.yaml".to_owned()),
            server_workdir: Some("/opt/fns".to_owned()),
            current_target: current_target.map(str::to_owned),
            existing_pid: current_target.is_none().then_some(4_200),
            existing_executable: current_target
                .is_none()
                .then(|| "/opt/legacy/fns-server".to_owned()),
            agent_unit_state: Some(if current_target.is_some() {
                RemoteAgentUnitState::Present {
                    enabled: true,
                    active: true,
                }
            } else {
                RemoteAgentUnitState::Absent
            }),
        }
    }

    fn fresh_rollback_remote_probe() -> RemoteProbe {
        let mut remote = rollback_remote_probe(None);
        remote.existing_pid = None;
        remote.existing_executable = None;
        remote
    }

    async fn run_rollback_fixture(
        runner: &FakeRunner,
        remote: &RemoteProbe,
    ) -> Result<(), DeployFailure> {
        let request = request();
        let paths = remote_paths(&request, remote, "failed-version", ServiceManager::System);
        rollback_remote(
            runner,
            &request,
            &paths,
            ServiceManager::System,
            remote,
            CancellationToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn managed_rollback_restores_server_and_agent_in_independent_steps() {
        let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        let remote = rollback_remote_probe(Some("releases/previous"));

        run_rollback_fixture(&runner, &remote).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 5);
        let commands = calls
            .iter()
            .map(|call| call.args.last().expect("remote command").as_str())
            .collect::<Vec<_>>();
        assert!(commands[0].contains("show --property LoadState --value"));
        assert!(commands[0].contains(" stop fns-workspace-agent-"));
        assert!(commands[0].contains("not-found)"));
        assert!(commands[0].contains("*) exit 42"));
        assert!(!commands[0].contains("|| true"));
        assert!(commands[1].contains(".rollback-"));
        assert!(commands[1].contains("[ -L"));
        assert!(commands[1].contains("releases/previous"));
        assert!(commands[2].contains("restart fns-workspace-server.service"));
        assert!(commands[3].contains("enable fns-workspace-agent-"));
        assert!(commands[4].contains("restart fns-workspace-agent-"));
        assert_eq!(calls[0].timeout, SYSTEMD_LIFECYCLE_TIMEOUT);
        assert_eq!(calls[1].timeout, SSH_TIMEOUT);
        assert_eq!(calls[2].timeout, SYSTEMD_LIFECYCLE_TIMEOUT);
        assert_eq!(calls[3].timeout, SYSTEMD_LIFECYCLE_TIMEOUT);
        assert_eq!(calls[4].timeout, SYSTEMD_LIFECYCLE_TIMEOUT);
    }

    #[tokio::test]
    async fn managed_first_project_rollback_keeps_absent_agent_stopped_and_disabled() {
        let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        let mut remote = rollback_remote_probe(Some("releases/previous"));
        remote.agent_unit_state = Some(RemoteAgentUnitState::Absent);

        run_rollback_fixture(&runner, &remote).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        let commands = calls
            .iter()
            .map(|call| call.args.last().expect("remote command").as_str())
            .collect::<Vec<_>>();
        assert!(commands[0].contains(" stop fns-workspace-agent-"));
        assert!(commands[3].contains(" disable fns-workspace-agent-"));
        assert!(commands.iter().all(|command| {
            !command.contains(" restart fns-workspace-agent-")
                && !command.contains(" start fns-workspace-agent-")
        }));
    }

    #[tokio::test]
    async fn managed_rollback_restores_disabled_inactive_agent() {
        let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        let mut remote = rollback_remote_probe(Some("releases/previous"));
        remote.agent_unit_state = Some(RemoteAgentUnitState::Present {
            enabled: false,
            active: false,
        });

        run_rollback_fixture(&runner, &remote).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 5);
        let enablement = calls[3].args.last().expect("agent enablement");
        let lifecycle = calls[4].args.last().expect("agent lifecycle");
        assert!(enablement.contains("disable fns-workspace-agent-"));
        assert!(lifecycle.contains("stop fns-workspace-agent-"));
        assert!(!lifecycle.contains("restart fns-workspace-agent-"));
    }

    #[tokio::test]
    async fn managed_rollback_restores_mixed_agent_enablement_and_activity() {
        for (enabled, active, expected_enablement, expected_lifecycle) in [
            (true, false, "enable", "stop"),
            (false, true, "disable", "start"),
        ] {
            let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
            let mut remote = rollback_remote_probe(Some("releases/previous"));
            remote.agent_unit_state = Some(RemoteAgentUnitState::Present { enabled, active });

            run_rollback_fixture(&runner, &remote).await.unwrap();

            let calls = runner.calls();
            assert_eq!(calls.len(), 5);
            assert!(
                calls[3]
                    .args
                    .last()
                    .expect("agent enablement")
                    .contains(&format!("{expected_enablement} fns-workspace-agent-"))
            );
            assert!(
                calls[4]
                    .args
                    .last()
                    .expect("agent lifecycle")
                    .contains(&format!("{expected_lifecycle} fns-workspace-agent-"))
            );
            assert!(
                !calls[4]
                    .args
                    .last()
                    .expect("agent lifecycle")
                    .contains("restart fns-workspace-agent-")
            );
        }
    }

    #[tokio::test]
    async fn legacy_rollback_disables_new_agent_instead_of_restarting_it() {
        let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        let remote = rollback_remote_probe(None);

        run_rollback_fixture(&runner, &remote).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        let server_recovery = calls[2].args.last().expect("server recovery command");
        let agent_restore = calls[3].args.last().expect("agent restore command");
        let switch_previous = calls[1].args.last().expect("switch previous command");
        assert!(switch_previous.contains(&legacy_release_target(
            remote.existing_executable.as_deref().unwrap()
        )));
        assert!(server_recovery.contains("/proc/4200/exe"));
        assert!(server_recovery.contains("restart fns-workspace-server.service"));
        assert!(agent_restore.contains("show --property LoadState --value"));
        assert!(agent_restore.contains("disable fns-workspace-agent-"));
        assert!(!agent_restore.contains("restart fns-workspace-agent-"));
        assert!(!agent_restore.contains("|| true"));
    }

    #[tokio::test]
    async fn fresh_install_rollback_disables_services_and_removes_current() {
        let runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        let remote = fresh_rollback_remote_probe();

        run_rollback_fixture(&runner, &remote).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 5);
        let commands = calls
            .iter()
            .map(|call| call.args.last().expect("remote command").as_str())
            .collect::<Vec<_>>();
        assert!(commands[0].contains(" stop fns-workspace-agent-"));
        assert!(commands[1].contains(" disable fns-workspace-agent-"));
        assert!(commands[2].contains(" stop fns-workspace-server.service"));
        assert!(commands[3].contains(" disable fns-workspace-server.service"));
        assert!(commands[4].contains("releases/failed-version"));
        assert!(commands[4].contains("rm -f --"));
        assert!(
            commands
                .iter()
                .all(|command| !command.contains(" restart "))
        );
        assert!(
            calls[..4]
                .iter()
                .all(|call| call.timeout == SYSTEMD_LIFECYCLE_TIMEOUT)
        );
        assert_eq!(calls[4].timeout, SSH_TIMEOUT);
    }

    #[tokio::test]
    async fn missing_or_changed_previous_is_an_observable_rollback_failure() {
        let mut runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        runner.fail_command = Some("[ -L".to_owned());
        let remote = rollback_remote_probe(Some("releases/previous"));

        let failure = run_rollback_fixture(&runner, &remote).await.unwrap_err();

        assert_eq!(failure.primary, DeployErrorCode::RollbackFailed);
        assert_eq!(runner.calls().len(), 2);
    }

    #[tokio::test]
    async fn rollback_stop_failure_is_observable_and_blocks_later_steps() {
        let mut runner = FakeRunner::new("a".repeat(64), "b".repeat(64));
        runner.fail_command = Some(" stop fns-workspace-agent-".to_owned());
        let remote = rollback_remote_probe(Some("releases/previous"));

        let failure = run_rollback_fixture(&runner, &remote).await.unwrap_err();

        assert_eq!(failure.primary, DeployErrorCode::RollbackFailed);
        assert_eq!(runner.calls().len(), 1);
    }

    fn write_elf(path: &Path, marker: u8) {
        let mut bytes = vec![0_u8; 4096];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[100] = marker;
        std::fs::write(path, bytes).unwrap();
    }

    fn artifact_paths(root: &Path) -> ArtifactPaths {
        let server = root.join("fns-server");
        let agent = root.join("fns-agent");
        write_elf(&server, 1);
        write_elf(&agent, 2);
        ArtifactPaths { server, agent }
    }

    fn encode_path(path: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(path)
    }

    fn probe_output(service_manager: bool) -> Vec<u8> {
        let (agent_load_state, agent_unit_file_state, agent_active_state) = if service_manager {
            ("not-found", "not-found", "not-found")
        } else {
            ("unavailable", "unavailable", "unavailable")
        };
        format!(
            "system=Linux\narch=x86_64\nuid=0\nsystem_manager={}\nuser_manager=0\nagent_load_state={agent_load_state}\nagent_unit_file_state={agent_unit_file_state}\nagent_active_state={agent_active_state}\navailable_kb=1048576\nhome_b64={}\nconfig_b64={}\nworkdir_b64={}\ncurrent_b64={}\npid=\nexe_b64=\ncwd_b64=\n",
            u8::from(service_manager),
            encode_path("/home/fns"),
            encode_path("/opt/fns/config/config.yaml"),
            encode_path("/opt/fns"),
            encode_path("releases/old"),
        )
        .into_bytes()
    }

    fn online_status(pid: u32, workspace_id: &str) -> fns_agent::AgentStatus {
        let workspace_id = fns_protocol::WorkspaceId::parse(workspace_id).unwrap();
        let mut status = fns_agent::AgentStatus::stopped(workspace_id);
        status.running = true;
        status.phase = fns_agent::AgentPhase::Online;
        status.pid = Some(pid);
        status.connected = true;
        status.updated_at_ms = 1;
        status
    }

    fn status_output(
        main_pid: u32,
        main_executable: &str,
        child_processes: &[(u32, &str)],
        status: &fns_agent::AgentStatus,
    ) -> Vec<u8> {
        let child_processes = child_processes
            .iter()
            .map(|(pid, executable)| {
                format!(
                    "{pid}:{}",
                    base64::engine::general_purpose::STANDARD.encode(executable)
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let main_executable = base64::engine::general_purpose::STANDARD.encode(main_executable);
        let mut output = format!(
            "main_pid={main_pid}\nmain_exe_b64={main_executable}\nchild_processes={child_processes}\n"
        )
        .into_bytes();
        output.extend(serde_json::to_vec(status).unwrap());
        output
    }

    fn healthy_status_output(
        main_pid: u32,
        worker_pid: u32,
        executable: &str,
        workspace_id: &str,
    ) -> Vec<u8> {
        status_output(
            main_pid,
            executable,
            &[(worker_pid, executable)],
            &online_status(worker_pid, workspace_id),
        )
    }

    #[test]
    fn remote_health_requires_online_quiescent_direct_worker_child() {
        let expected = fns_protocol::WorkspaceId::parse(WORKSPACE_ID).unwrap();
        let healthy = healthy_status_output(91, 92, HEALTH_AGENT_EXE, WORKSPACE_ID);
        assert!(remote_agent_is_healthy(
            &healthy,
            expected,
            HEALTH_AGENT_EXE
        ));

        let multiple_children = status_output(
            91,
            HEALTH_AGENT_EXE,
            &[
                (90, "/usr/bin/helper"),
                (92, HEALTH_AGENT_EXE),
                (93, "/usr/bin/other"),
            ],
            &online_status(92, WORKSPACE_ID),
        );
        assert!(remote_agent_is_healthy(
            &multiple_children,
            expected,
            HEALTH_AGENT_EXE
        ));

        let non_child = status_output(
            91,
            HEALTH_AGENT_EXE,
            &[(92, HEALTH_AGENT_EXE)],
            &online_status(93, WORKSPACE_ID),
        );
        assert!(!remote_agent_is_healthy(
            &non_child,
            expected,
            HEALTH_AGENT_EXE
        ));

        let supervisor_as_worker = status_output(
            91,
            HEALTH_AGENT_EXE,
            &[(91, HEALTH_AGENT_EXE)],
            &online_status(91, WORKSPACE_ID),
        );
        assert!(!remote_agent_is_healthy(
            &supervisor_as_worker,
            expected,
            HEALTH_AGENT_EXE
        ));

        let no_children =
            status_output(91, HEALTH_AGENT_EXE, &[], &online_status(92, WORKSPACE_ID));
        assert!(!remote_agent_is_healthy(
            &no_children,
            expected,
            HEALTH_AGENT_EXE
        ));

        let wrong_supervisor_executable = status_output(
            91,
            "/tmp/stale/fns-agent",
            &[(92, HEALTH_AGENT_EXE)],
            &online_status(92, WORKSPACE_ID),
        );
        assert!(!remote_agent_is_healthy(
            &wrong_supervisor_executable,
            expected,
            HEALTH_AGENT_EXE
        ));

        let wrong_worker_executable = status_output(
            91,
            HEALTH_AGENT_EXE,
            &[(92, "/tmp/stale/fns-agent")],
            &online_status(92, WORKSPACE_ID),
        );
        assert!(!remote_agent_is_healthy(
            &wrong_worker_executable,
            expected,
            HEALTH_AGENT_EXE
        ));

        let mut status = online_status(92, WORKSPACE_ID);
        status.connected = false;
        let disconnected = status_output(91, HEALTH_AGENT_EXE, &[(92, HEALTH_AGENT_EXE)], &status);
        assert!(!remote_agent_is_healthy(
            &disconnected,
            expected,
            HEALTH_AGENT_EXE
        ));

        status.connected = true;
        status.pending_commands = 1;
        let pending = status_output(91, HEALTH_AGENT_EXE, &[(92, HEALTH_AGENT_EXE)], &status);
        assert!(!remote_agent_is_healthy(
            &pending,
            expected,
            HEALTH_AGENT_EXE
        ));

        let malformed_children = {
            let encoded = base64::engine::general_purpose::STANDARD.encode(HEALTH_AGENT_EXE);
            let mut output = format!(
                "main_pid=91\nmain_exe_b64={encoded}\nchild_processes=92:{encoded} invalid\n"
            )
            .into_bytes();
            output.extend(serde_json::to_vec(&online_status(92, WORKSPACE_ID)).unwrap());
            output
        };
        assert!(!remote_agent_is_healthy(
            &malformed_children,
            expected,
            HEALTH_AGENT_EXE
        ));
    }

    async fn preview_fixture(
        runner: Arc<FakeRunner>,
        artifacts: ArtifactPaths,
    ) -> (DeployState, DeploymentPreview) {
        let state = DeployState::with_runner(runner);
        let preview = build_preview(&state, request(), artifacts).await.unwrap();
        (state, preview)
    }

    #[tokio::test]
    async fn preview_is_read_only_and_reports_managed_upgrade_plan() {
        let temporary = tempfile::tempdir().unwrap();
        let artifacts = artifact_paths(temporary.path());
        let server = inspect_artifact(&artifacts.server).await.unwrap();
        let agent = inspect_artifact(&artifacts.agent).await.unwrap();
        let runner = Arc::new(FakeRunner::new(server.sha256, agent.sha256));
        let (state, preview) = preview_fixture(Arc::clone(&runner), artifacts).await;

        assert_eq!(preview.target, "linux-x86_64");
        assert_eq!(preview.service_manager, Some(ServiceManager::System));
        assert!(preview.warnings.is_empty());
        assert_eq!(preview.steps, planned_steps());
        assert!(state.load_preview(&preview.preview_id, &request()).is_ok());
        let calls = runner.calls();
        assert!(calls.iter().all(|call| call.kind == ProcessKind::Ssh));
        let argv = calls
            .iter()
            .flat_map(|call| &call.args)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        for forbidden in ["mkdir", "cat >", "systemctl enable", "nohup", "disown"] {
            assert!(!argv.contains(forbidden), "preview invoked {forbidden}");
        }
    }

    #[tokio::test]
    async fn full_deploy_keeps_token_in_stdin_and_emits_the_exact_step_trace() {
        let temporary = tempfile::tempdir().unwrap();
        let artifacts = artifact_paths(temporary.path());
        let server = inspect_artifact(&artifacts.server).await.unwrap();
        let agent = inspect_artifact(&artifacts.agent).await.unwrap();
        let runner = Arc::new(FakeRunner::new(server.sha256, agent.sha256));
        let (state, preview_result) = preview_fixture(Arc::clone(&runner), artifacts).await;
        let preview = state
            .load_preview(&preview_result.preview_id, &request())
            .unwrap();
        let token = fns_platform::SecretToken::from_bytes_for_test(TOKEN_SENTINEL.as_bytes());
        let mut progress = Vec::new();

        let outcome = execute_plan(
            runner.as_ref(),
            &request(),
            &preview,
            &token,
            CancellationToken::new(),
            |event| progress.push(event),
        )
        .await
        .unwrap();

        assert!(outcome.server_active && outcome.agent_online && !outcome.rolled_back);
        let succeeded = progress
            .iter()
            .filter(|event| event.status == DeployStepStatus::Succeeded)
            .map(|event| event.step)
            .collect::<Vec<_>>();
        assert_eq!(succeeded, planned_steps());
        let calls = runner.calls();
        let argv = calls
            .iter()
            .flat_map(|call| &call.args)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!argv.contains(TOKEN_SENTINEL));
        for forbidden in ["nohup", "screen", "disown"] {
            assert!(!argv.contains(forbidden));
        }
        let health_command = calls
            .iter()
            .filter(|call| call.kind == ProcessKind::Ssh)
            .filter_map(|call| call.args.last())
            .find(|command| command.contains(" status --config "))
            .expect("health command");
        assert!(health_command.contains("/task/*/children"));
        assert!(health_command.contains("/proc/\"$main_pid\"/exe"));
        assert!(health_command.contains("/proc/\"$child_pid\"/exe"));
        assert!(health_command.contains("main_pid=%s"));
        assert!(health_command.contains("main_exe_b64=%s"));
        assert!(health_command.contains("child_processes="));
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.input.as_deref() == Some(TOKEN_SENTINEL.as_bytes()))
                .count(),
            1
        );
        let lifecycle_calls = calls
            .iter()
            .filter(|call| {
                call.kind == ProcessKind::Ssh
                    && call.args.last().is_some_and(|command| {
                        command.contains(" stop ") || command.contains(" restart ")
                    })
            })
            .collect::<Vec<_>>();
        assert!(!lifecycle_calls.is_empty());
        assert!(
            lifecycle_calls
                .iter()
                .all(|call| call.timeout == SYSTEMD_LIFECYCLE_TIMEOUT)
        );
    }

    #[tokio::test]
    async fn checksum_mismatch_stops_before_version_switch() {
        let temporary = tempfile::tempdir().unwrap();
        let artifacts = artifact_paths(temporary.path());
        let server = inspect_artifact(&artifacts.server).await.unwrap();
        let agent = inspect_artifact(&artifacts.agent).await.unwrap();
        let mut fake = FakeRunner::new(server.sha256, agent.sha256);
        fake.checksum_mismatch = true;
        let runner = Arc::new(fake);
        let (state, preview_result) = preview_fixture(Arc::clone(&runner), artifacts).await;
        let preview = state
            .load_preview(&preview_result.preview_id, &request())
            .unwrap();
        let token = fns_platform::SecretToken::from_bytes_for_test(TOKEN_SENTINEL.as_bytes());

        let failure = execute_plan(
            runner.as_ref(),
            &request(),
            &preview,
            &token,
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(failure.primary, DeployErrorCode::ArtifactChecksumMismatch);
        assert_eq!(failure.step, Some(DeployStep::VerifyArtifacts));
        let argv = runner
            .calls()
            .into_iter()
            .flat_map(|call| call.args)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!argv.contains("ln -sfn"));
    }

    #[tokio::test]
    async fn unsupported_systemd_is_visible_in_preview_and_execute_is_gated() {
        let temporary = tempfile::tempdir().unwrap();
        let artifacts = artifact_paths(temporary.path());
        let server = inspect_artifact(&artifacts.server).await.unwrap();
        let agent = inspect_artifact(&artifacts.agent).await.unwrap();
        let mut fake = FakeRunner::new(server.sha256, agent.sha256);
        fake.service_manager = false;
        let runner = Arc::new(fake);
        let (state, preview_result) = preview_fixture(Arc::clone(&runner), artifacts).await;
        assert!(
            preview_result
                .warnings
                .contains(&DeployErrorCode::SystemdUnavailable)
        );
        let preview = state
            .load_preview(&preview_result.preview_id, &request())
            .unwrap();
        let token = fns_platform::SecretToken::from_bytes_for_test(TOKEN_SENTINEL.as_bytes());
        let failure = execute_plan(
            runner.as_ref(),
            &request(),
            &preview,
            &token,
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(failure.primary, DeployErrorCode::SystemdUnavailable);
    }

    #[tokio::test]
    async fn artifact_validation_rejects_symlinks_and_non_linux_binaries() {
        let temporary = tempfile::tempdir().unwrap();
        let valid = temporary.path().join("valid");
        write_elf(&valid, 1);
        let link = temporary.path().join("link");
        symlink(&valid, &link).unwrap();
        assert_eq!(
            inspect_artifact(&link).await.unwrap_err().primary,
            DeployErrorCode::ArtifactInvalid
        );
        let invalid = temporary.path().join("invalid");
        std::fs::write(&invalid, vec![7_u8; 4096]).unwrap();
        assert_eq!(
            inspect_artifact(&invalid).await.unwrap_err().primary,
            DeployErrorCode::ArtifactInvalid
        );
    }

    #[test]
    fn server_config_patch_is_structured_loopback_only_and_preserves_other_data() {
        let source = br#"
server:
  http-port: ":9000"
  private-http-listen: ":9001"
  webgui-port: ":9002"
  share-port: ":9003"
security:
  auth-token-key: keep-this-value
workspace:
  roots:
    - uid: 7
      workspace-id: 100e2fa9-b501-467d-9231-72aabeec5a73
      root: /srv/other
"#;
        let patched = patch_server_config(source, 41, WORKSPACE_ID, "/srv/work").unwrap();
        let value: serde_yaml::Value = serde_yaml::from_slice(&patched).unwrap();
        assert_eq!(value["server"]["http-port"], "127.0.0.1:9000");
        assert_eq!(value["server"]["private-http-listen"], "");
        assert_eq!(value["server"]["webgui-port"], "");
        assert_eq!(value["server"]["share-port"], "");
        assert_eq!(value["security"]["auth-token-key"], "keep-this-value");
        assert_eq!(value["workspace"]["roots"].as_sequence().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cancellation_kills_and_reaps_a_blocked_child() {
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let operation = tokio::spawn(async move {
            run_system_process(
                ProcessSpec {
                    kind: ProcessKind::Ssh,
                    program: "/bin/sh",
                    args: vec!["-c".into(), "sleep 30".into()],
                },
                None,
                Duration::from_secs(30),
                cancellation,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let failure = tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .expect("cancelled child was not reaped")
            .unwrap()
            .unwrap_err();
        assert_eq!(failure.primary, DeployErrorCode::Cancelled);
    }
}
