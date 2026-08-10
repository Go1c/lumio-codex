use crate::ssh_tunnel::{TunnelFailure, TunnelState};
use crate::sync::CredentialProvider;
use fns_agent::error::AgentErrorCode;
use fns_platform::{CredentialStore, PlatformErrorCode, SecretToken};
use fns_protocol::{
    ClientId, DecodedEnvelope, DecodedFrame, MessageBody, RequestId, WorkspaceAction,
    WorkspaceHelloRequest, WorkspaceId, WorkspaceRevision, WorkspaceSubscribeRequest,
    WorkspaceV2ErrorCode, decode_server_text_frame, encode_request,
};
use fns_transport::socket::{InboundMessage, SocketReader, SocketWriter};
use fns_transport::{TransportErrorCode, WorkspaceEndpoint};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use zeroize::Zeroizing;

const REMOTE_SERVER_PORT: u16 = 9000;
const AGENT_TOKEN_EXPIRY_DAYS: u16 = 30;
const MAX_HTTP_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const PRODUCTION_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PRODUCTION_IO_TIMEOUT: Duration = Duration::from_secs(10);
const PRODUCTION_OPERATION_TIMEOUT: Duration = Duration::from_secs(35);
const PRODUCTION_CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
const TOKEN_EXPIRY_TRANSPORT_SKEW: chrono::Duration = chrono::Duration::minutes(2);
const PENDING_AGENT_DELETION_FORMAT_VERSION: u8 = 1;
const MAX_PENDING_AGENT_DELETION_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialBackendFailure {
    Access,
    Integrity,
}

pub(crate) trait CredentialBackend: Send + Sync + 'static {
    fn store(&self, project_id: &str, token: &SecretToken) -> Result<(), CredentialBackendFailure>;
    fn load(&self, project_id: &str) -> Result<Option<SecretToken>, CredentialBackendFailure>;
    fn delete(&self, project_id: &str) -> Result<(), CredentialBackendFailure>;

    fn load_pending_agent_deletions(&self) -> Result<Option<Vec<u8>>, CredentialBackendFailure> {
        Ok(None)
    }

    fn replace_pending_agent_deletions(
        &self,
        _encoded: &[u8],
    ) -> Result<(), CredentialBackendFailure> {
        Ok(())
    }
}

struct PlatformCredentialBackend {
    store: Result<CredentialStore, CredentialBackendFailure>,
    pending_agent_deletions_path: PathBuf,
}

impl PlatformCredentialBackend {
    fn open() -> Self {
        let directory = credential_directory();
        Self {
            store: CredentialStore::open(&directory).map_err(map_platform_failure),
            pending_agent_deletions_path: directory.join(".pending-agent-deletions-v1.json"),
        }
    }

    fn store(&self) -> Result<&CredentialStore, CredentialBackendFailure> {
        self.store.as_ref().map_err(|failure| *failure)
    }
}

impl CredentialBackend for PlatformCredentialBackend {
    fn store(&self, project_id: &str, token: &SecretToken) -> Result<(), CredentialBackendFailure> {
        self.store()?
            .store(project_id, token)
            .map_err(|error| map_platform_code(error.code()))
    }

    fn load(&self, project_id: &str) -> Result<Option<SecretToken>, CredentialBackendFailure> {
        self.store()?
            .load(project_id)
            .map_err(|error| map_platform_code(error.code()))
    }

    fn delete(&self, project_id: &str) -> Result<(), CredentialBackendFailure> {
        self.store()?
            .delete(project_id)
            .map_err(|error| map_platform_code(error.code()))
    }

    fn load_pending_agent_deletions(&self) -> Result<Option<Vec<u8>>, CredentialBackendFailure> {
        let metadata = match std::fs::symlink_metadata(&self.pending_agent_deletions_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(CredentialBackendFailure::Access),
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_PENDING_AGENT_DELETION_BYTES {
            return Err(CredentialBackendFailure::Integrity);
        }
        std::fs::read(&self.pending_agent_deletions_path)
            .map(Some)
            .map_err(|_| CredentialBackendFailure::Access)
    }

    fn replace_pending_agent_deletions(
        &self,
        encoded: &[u8],
    ) -> Result<(), CredentialBackendFailure> {
        if encoded.len() as u64 > MAX_PENDING_AGENT_DELETION_BYTES {
            return Err(CredentialBackendFailure::Integrity);
        }
        let parent = self
            .pending_agent_deletions_path
            .parent()
            .ok_or(CredentialBackendFailure::Integrity)?;
        std::fs::create_dir_all(parent).map_err(|_| CredentialBackendFailure::Access)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| CredentialBackendFailure::Access)?;
        temporary
            .write_all(encoded)
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| CredentialBackendFailure::Access)?;
        temporary
            .persist(&self.pending_agent_deletions_path)
            .map_err(|_| CredentialBackendFailure::Access)?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| CredentialBackendFailure::Access)?;
        Ok(())
    }
}

fn credential_directory() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".config/fns-workspace/credentials"),
        |base| base.config_dir().join("fns-workspace/credentials"),
    )
}

fn map_platform_failure(error: fns_platform::PlatformError) -> CredentialBackendFailure {
    map_platform_code(error.code())
}

fn map_platform_code(code: PlatformErrorCode) -> CredentialBackendFailure {
    match code {
        PlatformErrorCode::InvalidProjectId
        | PlatformErrorCode::InvalidFileType
        | PlatformErrorCode::InsecurePermissions
        | PlatformErrorCode::WrongOwner
        | PlatformErrorCode::InvalidSecret => CredentialBackendFailure::Integrity,
        PlatformErrorCode::UnsupportedPlatform
        | PlatformErrorCode::InvalidCredentialPath
        | PlatformErrorCode::CredentialAccess
        | PlatformErrorCode::CredentialInteractionNotAllowed
        | PlatformErrorCode::AlreadyRunning
        | PlatformErrorCode::CorruptLock
        | PlatformErrorCode::Io => CredentialBackendFailure::Access,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvisionErrorCode {
    InvalidProjectId,
    InvalidRequest,
    AlreadyRunning,
    TunnelAcquireFailed,
    TunnelCleanupFailed,
    Network,
    Timeout,
    Cancelled,
    AuthenticationRejected,
    Forbidden,
    ServerRejected,
    MalformedResponse,
    ResponseTooLarge,
    ScopeMismatch,
    ClientTypeMismatch,
    CredentialMissing,
    CredentialAccess,
    CredentialIntegrity,
    CredentialDeletionPending,
    RevocationFailed,
    WorkspaceAccessRejected,
    WorkspaceIdentityMismatch,
    WorkspaceProbeFailed,
    WorkspaceProbeCleanupFailed,
    AbnormalExit,
}

impl ProvisionErrorCode {
    const fn stable(self) -> &'static str {
        match self {
            Self::InvalidProjectId => "invalid_project_id",
            Self::InvalidRequest => "invalid_request",
            Self::AlreadyRunning => "already_running",
            Self::TunnelAcquireFailed => "tunnel_acquire_failed",
            Self::TunnelCleanupFailed => "tunnel_cleanup_failed",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::AuthenticationRejected => "authentication_rejected",
            Self::Forbidden => "forbidden",
            Self::ServerRejected => "server_rejected",
            Self::MalformedResponse => "malformed_response",
            Self::ResponseTooLarge => "response_too_large",
            Self::ScopeMismatch => "scope_mismatch",
            Self::ClientTypeMismatch => "client_type_mismatch",
            Self::CredentialMissing => "credential_missing",
            Self::CredentialAccess => "credential_access",
            Self::CredentialIntegrity => "credential_integrity",
            Self::CredentialDeletionPending => "credential_deletion_pending",
            Self::RevocationFailed => "revocation_failed",
            Self::WorkspaceAccessRejected => "workspace_access_rejected",
            Self::WorkspaceIdentityMismatch => "workspace_identity_mismatch",
            Self::WorkspaceProbeFailed => "workspace_probe_failed",
            Self::WorkspaceProbeCleanupFailed => "workspace_probe_cleanup_failed",
            Self::AbnormalExit => "abnormal_exit",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProvisionFailure {
    pub(crate) primary: ProvisionErrorCode,
    pub(crate) cleanup: Vec<ProvisionErrorCode>,
}

impl ProvisionFailure {
    fn primary(primary: ProvisionErrorCode) -> Self {
        Self {
            primary,
            cleanup: Vec::new(),
        }
    }

    fn with_cleanup(mut self, cleanup: Self) -> Self {
        self.cleanup.push(cleanup.primary);
        self.cleanup.extend(cleanup.cleanup);
        self
    }

    fn has_cleanup_failure(&self) -> bool {
        let is_cleanup = |code| {
            matches!(
                code,
                ProvisionErrorCode::RevocationFailed
                    | ProvisionErrorCode::TunnelCleanupFailed
                    | ProvisionErrorCode::WorkspaceProbeCleanupFailed
            )
        };
        is_cleanup(self.primary) || self.cleanup.iter().copied().any(is_cleanup)
    }
}

impl From<ProvisionErrorCode> for ProvisionFailure {
    fn from(code: ProvisionErrorCode) -> Self {
        Self::primary(code)
    }
}

impl fmt::Display for ProvisionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.primary.stable())?;
        for cleanup in &self.cleanup {
            formatter.write_str(";cleanup=")?;
            formatter.write_str(cleanup.stable())?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProvisionRequest {
    pub(crate) project_id: String,
    pub(crate) ssh_host_alias: String,
    pub(crate) username: Zeroizing<String>,
    pub(crate) password: Zeroizing<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkspaceProbeRequest {
    pub(crate) project_id: String,
    pub(crate) ssh_host_alias: String,
    pub(crate) workspace_id: String,
}

impl fmt::Debug for ProvisionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionRequest")
            .field("project_id", &"[REDACTED]")
            .field("ssh_host_alias", &"[REDACTED]")
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProvisionStatus {
    pub(crate) provisioned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceCredentialStatus {
    pub(crate) available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteCredentialStatus {
    pub(crate) deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CredentialCleanupStatus {
    pub(crate) active: bool,
    pub(crate) pending_agent_deletion: bool,
    pub(crate) pending_revocation: bool,
    pub(crate) pending_tunnel_cleanup: bool,
    pub(crate) last_error: Option<ProvisionErrorCode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CredentialRollbackStatus {
    pub(crate) credential_deleted: bool,
    pub(crate) active: bool,
    pub(crate) pending_agent_deletion: bool,
    pub(crate) pending_revocation: bool,
    pub(crate) pending_tunnel_cleanup: bool,
    pub(crate) last_error: Option<ProvisionErrorCode>,
}

impl CredentialRollbackStatus {
    fn deleted(cleanup: CredentialCleanupStatus) -> Self {
        Self {
            credential_deleted: true,
            // The reservation is still held while this response is assembled,
            // but no rollback work remains active when the command returns.
            active: false,
            pending_agent_deletion: cleanup.pending_agent_deletion,
            pending_revocation: cleanup.pending_revocation,
            pending_tunnel_cleanup: cleanup.pending_tunnel_cleanup,
            last_error: cleanup.last_error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceProbeStatus {
    pub(crate) accepted: bool,
    pub(crate) workspace_id: String,
}

#[derive(Clone, Copy)]
pub(crate) struct ProvisionDeadlines {
    pub(crate) connect: Duration,
    pub(crate) io: Duration,
    pub(crate) operation: Duration,
    pub(crate) cleanup: Duration,
}

impl Default for ProvisionDeadlines {
    fn default() -> Self {
        Self {
            connect: PRODUCTION_CONNECT_TIMEOUT,
            io: PRODUCTION_IO_TIMEOUT,
            operation: PRODUCTION_OPERATION_TIMEOUT,
            cleanup: PRODUCTION_CLEANUP_TIMEOUT,
        }
    }
}

#[derive(Clone)]
struct PendingRevocation {
    generation: uuid::Uuid,
    ssh_host_alias: String,
    login_token: Option<Arc<Zeroizing<String>>>,
    last_error: Option<ProvisionErrorCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingAgentDeletion {
    project_id: String,
    generation: uuid::Uuid,
    last_error: ProvisionErrorCode,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingAgentDeletionFile {
    version: u8,
    deletions: Vec<PendingAgentDeletion>,
}

struct ActiveCredentialOperation {
    generation: uuid::Uuid,
    control: Option<Arc<OperationControl>>,
}

struct CredentialStateInner {
    backend: Arc<dyn CredentialBackend>,
    deadlines: ProvisionDeadlines,
    tasks: TaskTracker,
    active: Mutex<HashMap<String, ActiveCredentialOperation>>,
    rollback_reservations: Mutex<HashSet<String>>,
    pending_agent_deletions: Mutex<HashMap<String, PendingAgentDeletion>>,
    pending_deletion_load_error: Option<ProvisionErrorCode>,
    pending_deletion_write_error: Mutex<Option<ProvisionErrorCode>>,
    pending_revocations: Mutex<HashMap<String, PendingRevocation>>,
    activity: Notify,
    shutting_down: AtomicBool,
    shutdown_failures: Mutex<Vec<ProvisionFailure>>,
}

struct CancelOnDrop {
    control: Arc<OperationControl>,
}

const OPERATION_RUNNING: u8 = 0;
const OPERATION_CANCELLED: u8 = 1;
const OPERATION_HANDOFF: u8 = 2;
const OPERATION_COMPLETED: u8 = 3;

struct OperationControl {
    cancellation: CancellationToken,
    lifecycle: AtomicU8,
    activity: Notify,
}

struct CleanupReservationGuard {
    state: CredentialState,
    project_id: String,
}

struct ActiveOperationGuard {
    state: CredentialState,
    project_id: String,
    generation: uuid::Uuid,
}

impl Drop for ActiveOperationGuard {
    fn drop(&mut self) {
        self.state
            .finish_operation(&self.project_id, self.generation);
    }
}

impl OperationControl {
    fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            lifecycle: AtomicU8::new(OPERATION_RUNNING),
            activity: Notify::new(),
        }
    }

    fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    fn cancel(&self) -> bool {
        loop {
            let lifecycle = self.lifecycle.load(Ordering::Acquire);
            if !matches!(lifecycle, OPERATION_RUNNING | OPERATION_HANDOFF) {
                return false;
            }
            if self
                .lifecycle
                .compare_exchange(
                    lifecycle,
                    OPERATION_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                self.cancellation.cancel();
                self.activity.notify_waiters();
                return true;
            }
        }
    }

    fn publish(&self) -> bool {
        let published = self
            .lifecycle
            .compare_exchange(
                OPERATION_RUNNING,
                OPERATION_HANDOFF,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if published {
            self.activity.notify_waiters();
        }
        published
    }

    fn complete(&self) -> bool {
        let completed = self
            .lifecycle
            .compare_exchange(
                OPERATION_HANDOFF,
                OPERATION_COMPLETED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if completed {
            self.activity.notify_waiters();
        }
        completed
    }

    async fn public_result_was_accepted(&self) -> bool {
        loop {
            let notified = self.activity.notified();
            match self.lifecycle.load(Ordering::Acquire) {
                OPERATION_COMPLETED => return true,
                OPERATION_CANCELLED => return false,
                _ => notified.await,
            }
        }
    }
}

impl CancelOnDrop {
    fn new(control: Arc<OperationControl>) -> Self {
        Self { control }
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

impl Drop for CleanupReservationGuard {
    fn drop(&mut self) {
        self.state
            .inner
            .rollback_reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.project_id);
        self.state.inner.activity.notify_waiters();
    }
}

fn decode_pending_agent_deletions(
    encoded: Option<Vec<u8>>,
) -> Result<HashMap<String, PendingAgentDeletion>, ProvisionErrorCode> {
    let Some(encoded) = encoded else {
        return Ok(HashMap::new());
    };
    let file: PendingAgentDeletionFile =
        serde_json::from_slice(&encoded).map_err(|_| ProvisionErrorCode::CredentialIntegrity)?;
    if file.version != PENDING_AGENT_DELETION_FORMAT_VERSION {
        return Err(ProvisionErrorCode::CredentialIntegrity);
    }
    let mut deletions = HashMap::new();
    for deletion in file.deletions {
        if deletion.generation.is_nil()
            || canonical_project_id(&deletion.project_id).is_err()
            || deletions
                .insert(deletion.project_id.clone(), deletion)
                .is_some()
        {
            return Err(ProvisionErrorCode::CredentialIntegrity);
        }
    }
    Ok(deletions)
}

fn encode_pending_agent_deletions(
    deletions: &HashMap<String, PendingAgentDeletion>,
) -> Result<Vec<u8>, ProvisionFailure> {
    let mut deletions = deletions.values().cloned().collect::<Vec<_>>();
    deletions.sort_by(|left, right| left.project_id.cmp(&right.project_id));
    serde_json::to_vec(&PendingAgentDeletionFile {
        version: PENDING_AGENT_DELETION_FORMAT_VERSION,
        deletions,
    })
    .map_err(|_| ProvisionErrorCode::CredentialIntegrity.into())
}

#[derive(Clone)]
pub(crate) struct CredentialState {
    inner: Arc<CredentialStateInner>,
}

impl CredentialState {
    pub(crate) fn production() -> Self {
        let state = Self::with_backend_and_deadlines(
            Arc::new(PlatformCredentialBackend::open()),
            ProvisionDeadlines::default(),
        );
        state.retry_pending_agent_deletions_blocking();
        state
    }

    pub(crate) fn with_backend_and_deadlines(
        backend: Arc<dyn CredentialBackend>,
        deadlines: ProvisionDeadlines,
    ) -> Self {
        let (pending_agent_deletions, pending_deletion_load_error) =
            match backend.load_pending_agent_deletions() {
                Ok(encoded) => match decode_pending_agent_deletions(encoded) {
                    Ok(deletions) => (deletions, None),
                    Err(error) => (HashMap::new(), Some(error)),
                },
                Err(failure) => (HashMap::new(), Some(backend_failure(failure).primary)),
            };
        Self {
            inner: Arc::new(CredentialStateInner {
                backend,
                deadlines,
                tasks: TaskTracker::new(),
                active: Mutex::new(HashMap::new()),
                rollback_reservations: Mutex::new(HashSet::new()),
                pending_agent_deletions: Mutex::new(pending_agent_deletions),
                pending_deletion_load_error,
                pending_deletion_write_error: Mutex::new(None),
                pending_revocations: Mutex::new(HashMap::new()),
                activity: Notify::new(),
                shutting_down: AtomicBool::new(false),
                shutdown_failures: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) async fn provision(
        &self,
        request: ProvisionRequest,
        tunnels: TunnelState,
    ) -> Result<ProvisionStatus, ProvisionFailure> {
        let validated = ValidatedProvisionRequest::try_from(request)?;
        let project_id = validated.project_id.clone();
        let control = Arc::new(OperationControl::new());
        let generation = self.begin_operation(&project_id, Some(Arc::clone(&control)))?;
        let _cancel_on_drop = CancelOnDrop::new(Arc::clone(&control));
        let task_cancellation = control.cancellation();
        let task_control = Arc::clone(&control);
        let task_project_id = project_id.clone();
        let state = self.clone();
        let (result_tx, mut result_rx) = oneshot::channel();
        let (settled_tx, settled_rx) = oneshot::channel();
        self.inner.tasks.spawn(async move {
            let operation = ActiveOperationGuard {
                state: state.clone(),
                project_id: task_project_id.clone(),
                generation,
            };
            let mut result = state
                .run_provision(validated, tunnels, task_cancellation)
                .await;
            if task_control.publish() {
                if result_tx.send(result.clone()).is_err() {
                    task_control.cancel();
                }
                if !task_control.public_result_was_accepted().await {
                    result = state
                        .rollback_cancelled_provision(&task_project_id, generation, result)
                        .await;
                }
            } else {
                result = state
                    .rollback_cancelled_provision(&task_project_id, generation, result)
                    .await;
                let _ = result_tx.send(result.clone());
            }
            if result
                .as_ref()
                .is_err_and(ProvisionFailure::has_cleanup_failure)
            {
                state
                    .inner
                    .shutdown_failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(result.as_ref().unwrap_err().clone());
            }
            drop(operation);
            let _ = settled_tx.send(result);
        });
        match tokio::time::timeout(self.inner.deadlines.operation, &mut result_rx).await {
            Ok(Ok(result)) if control.complete() => {
                self.finish_operation(&project_id, generation);
                result
            }
            Ok(Ok(_)) => settled_rx
                .await
                .unwrap_or_else(|_| Err(ProvisionErrorCode::AbnormalExit.into())),
            Ok(Err(_)) => Err(ProvisionErrorCode::AbnormalExit.into()),
            Err(_) => {
                control.cancel();
                let _ = tokio::time::timeout(self.inner.deadlines.cleanup, settled_rx).await;
                Err(ProvisionErrorCode::Timeout.into())
            }
        }
    }

    pub(crate) async fn reprovision(
        &self,
        request: ProvisionRequest,
        tunnels: TunnelState,
    ) -> Result<ProvisionStatus, ProvisionFailure> {
        self.provision(request, tunnels).await
    }

    pub(crate) async fn status(
        &self,
        project_id: &str,
    ) -> Result<WorkspaceCredentialStatus, ProvisionFailure> {
        let project_id = canonical_project_id(project_id)?;
        let backend = Arc::clone(&self.inner.backend);
        let token = tokio::task::spawn_blocking(move || backend.load(&project_id))
            .await
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?
            .map_err(backend_failure)?;
        Ok(WorkspaceCredentialStatus {
            available: token.is_some(),
        })
    }

    pub(crate) async fn probe_workspace(
        &self,
        request: WorkspaceProbeRequest,
        tunnels: TunnelState,
    ) -> Result<WorkspaceProbeStatus, ProvisionFailure> {
        let request = ValidatedWorkspaceProbeRequest::try_from(request)?;
        let project_id = request.project_id.clone();
        let control = Arc::new(OperationControl::new());
        let generation = self.begin_operation(&project_id, Some(Arc::clone(&control)))?;
        let _cancel_on_drop = CancelOnDrop::new(Arc::clone(&control));
        let task_cancellation = control.cancellation();
        let task_control = Arc::clone(&control);
        let task_project_id = project_id.clone();
        let state = self.clone();
        let (result_tx, mut result_rx) = oneshot::channel();
        let (settled_tx, settled_rx) = oneshot::channel();
        self.inner.tasks.spawn(async move {
            let operation = ActiveOperationGuard {
                state: state.clone(),
                project_id: task_project_id,
                generation,
            };
            let mut result = state
                .run_workspace_probe(request, tunnels, task_cancellation)
                .await;
            if task_control.publish() {
                if result_tx.send(result.clone()).is_err() {
                    task_control.cancel();
                }
                if !task_control.public_result_was_accepted().await {
                    result = Err(ProvisionErrorCode::Cancelled.into());
                }
            } else {
                result = Err(ProvisionErrorCode::Cancelled.into());
                let _ = result_tx.send(result.clone());
            }
            if result
                .as_ref()
                .is_err_and(ProvisionFailure::has_cleanup_failure)
            {
                state
                    .inner
                    .shutdown_failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(result.as_ref().unwrap_err().clone());
            }
            drop(operation);
            let _ = settled_tx.send(result);
        });
        match tokio::time::timeout(self.inner.deadlines.operation, &mut result_rx).await {
            Ok(Ok(result)) if control.complete() => {
                self.finish_operation(&project_id, generation);
                result
            }
            Ok(Ok(_)) => settled_rx
                .await
                .unwrap_or_else(|_| Err(ProvisionErrorCode::AbnormalExit.into())),
            Ok(Err(_)) => Err(ProvisionErrorCode::AbnormalExit.into()),
            Err(_) => {
                control.cancel();
                let _ = tokio::time::timeout(self.inner.deadlines.cleanup, settled_rx).await;
                Err(ProvisionErrorCode::Timeout.into())
            }
        }
    }

    pub(crate) fn cancel_provisioning(&self, project_id: &str) -> bool {
        let active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active
            .get(project_id)
            .and_then(|operation| operation.control.as_ref())
            .is_some_and(|control| control.cancel())
    }

    pub(crate) async fn cancel_and_cleanup(
        &self,
        project_id: &str,
        tunnels: TunnelState,
    ) -> Result<CredentialRollbackStatus, ProvisionFailure> {
        let project_id = canonical_project_id(project_id)?;
        let reservation = self.reserve_cleanup(&project_id)?;
        let deletion_generation = self.agent_deletion_generation(&project_id);
        self.cancel_provisioning(&project_id);
        let state = self.clone();
        let (result_tx, result_rx) = oneshot::channel();
        self.inner.tasks.spawn(async move {
            loop {
                let notified = state.inner.activity.notified();
                if !state
                    .inner
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains_key(&project_id)
                {
                    break;
                }
                notified.await;
            }
            let delete = state
                .delete_agent_credential_owned(&project_id, deletion_generation)
                .await;
            let retry = state.retry_pending_revocation(&project_id, &tunnels).await;
            let result = match delete {
                Ok(()) => state
                    .cleanup_status(&project_id)
                    .map(CredentialRollbackStatus::deleted),
                Err(failure) => Err(retry
                    .err()
                    .map_or(failure.clone(), |cleanup| failure.with_cleanup(cleanup))),
            };
            drop(reservation);
            let _ = result_tx.send(result);
        });
        match tokio::time::timeout(self.inner.deadlines.cleanup, result_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ProvisionErrorCode::AbnormalExit.into()),
            Err(_) => Err(ProvisionErrorCode::Timeout.into()),
        }
    }

    pub(crate) async fn shutdown_all(&self, tunnels: TunnelState) -> Result<(), ProvisionFailure> {
        self.inner.shutting_down.store(true, Ordering::Release);
        {
            let active = self
                .inner
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for operation in active.values() {
                if let Some(control) = operation.control.as_ref() {
                    control.cancel();
                }
            }
        }

        let deadline = tokio::time::Instant::now() + self.inner.deadlines.operation;
        loop {
            let notified = self.inner.activity.notified();
            if !self.has_active_operations() {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(ProvisionErrorCode::Timeout.into());
            }
        }
        while !self.inner.tasks.is_empty() {
            if tokio::time::timeout_at(deadline, tokio::time::sleep(Duration::from_millis(1)))
                .await
                .is_err()
            {
                return Err(ProvisionErrorCode::Timeout.into());
            }
        }

        let active_failure = self
            .inner
            .shutdown_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .next();

        if let Some(error) = self.inner.pending_deletion_load_error {
            return Err(error.into());
        }
        let pending_deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for deletion in pending_deletions {
            let reservation = self.reserve_cleanup(&deletion.project_id)?;
            let result = self
                .delete_agent_credential_owned(&deletion.project_id, deletion.generation)
                .await;
            drop(reservation);
            result?;
        }
        if !self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return Err(ProvisionErrorCode::CredentialDeletionPending.into());
        }

        let pending_projects = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut retry_failure = None;
        for project_id in pending_projects {
            if let Err(failure) = self.retry_pending_revocation(&project_id, &tunnels).await {
                retry_failure.get_or_insert(failure);
            }
        }

        if let Some(failure) = retry_failure {
            return Err(failure);
        }
        if !self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return Err(ProvisionErrorCode::RevocationFailed.into());
        }
        active_failure
            .filter(|failure| {
                failure.primary != ProvisionErrorCode::RevocationFailed
                    && !failure
                        .cleanup
                        .contains(&ProvisionErrorCode::RevocationFailed)
            })
            .map_or(Ok(()), Err)
    }

    pub(crate) fn has_active_operations(&self) -> bool {
        let reserved = !self
            .inner
            .rollback_reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        reserved
            || !self
                .inner
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }

    pub(crate) async fn delete(
        &self,
        project_id: &str,
        tunnels: TunnelState,
    ) -> Result<DeleteCredentialStatus, ProvisionFailure> {
        let project_id = canonical_project_id(project_id)?;
        let reservation = self.reserve_cleanup(&project_id)?;
        let generation = self.agent_deletion_generation(&project_id);
        let state = self.clone();
        let (result_tx, result_rx) = oneshot::channel();
        self.inner.tasks.spawn(async move {
            let delete = state
                .delete_agent_credential_owned(&project_id, generation)
                .await;
            let cleanup = state.retry_pending_revocation(&project_id, &tunnels).await;
            let result = merge_cleanup(delete, cleanup);
            drop(reservation);
            let _ = result_tx.send(result);
        });
        match tokio::time::timeout(self.inner.deadlines.operation, result_rx).await {
            Ok(Ok(Ok(()))) => Ok(DeleteCredentialStatus { deleted: true }),
            Ok(Ok(Err(failure))) => Err(failure),
            Ok(Err(_)) => Err(ProvisionErrorCode::AbnormalExit.into()),
            Err(_) => Err(ProvisionErrorCode::Timeout.into()),
        }
    }

    fn begin_operation(
        &self,
        project_id: &str,
        control: Option<Arc<OperationControl>>,
    ) -> Result<uuid::Uuid, ProvisionFailure> {
        if let Some(error) = self.inner.pending_deletion_load_error {
            return Err(error.into());
        }
        if let Some(error) = *self
            .inner
            .pending_deletion_write_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Err(error.into());
        }
        let reservations = self
            .inner
            .rollback_reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reservations.contains(project_id) {
            return Err(ProvisionErrorCode::AlreadyRunning.into());
        }
        let pending_deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending_deletions.contains_key(project_id) {
            return Err(ProvisionErrorCode::AlreadyRunning.into());
        }
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ProvisionErrorCode::Cancelled.into());
        }
        if active.contains_key(project_id) {
            return Err(ProvisionErrorCode::AlreadyRunning.into());
        }
        let generation = uuid::Uuid::new_v4();
        active.insert(
            project_id.to_owned(),
            ActiveCredentialOperation {
                generation,
                control,
            },
        );
        drop(pending_deletions);
        drop(reservations);
        Ok(generation)
    }

    fn reserve_cleanup(
        &self,
        project_id: &str,
    ) -> Result<CleanupReservationGuard, ProvisionFailure> {
        if let Some(error) = self.inner.pending_deletion_load_error {
            return Err(error.into());
        }
        let mut reservations = self
            .inner
            .rollback_reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !reservations.insert(project_id.to_owned()) {
            return Err(ProvisionErrorCode::AlreadyRunning.into());
        }
        Ok(CleanupReservationGuard {
            state: self.clone(),
            project_id: project_id.to_owned(),
        })
    }

    fn agent_deletion_generation(&self, project_id: &str) -> uuid::Uuid {
        self.inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .map(|deletion| deletion.generation)
            .or_else(|| {
                self.inner
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(project_id)
                    .map(|operation| operation.generation)
            })
            .unwrap_or_else(uuid::Uuid::new_v4)
    }

    fn persist_pending_agent_deletions(
        &self,
        deletions: &HashMap<String, PendingAgentDeletion>,
    ) -> Result<(), ProvisionFailure> {
        let encoded = encode_pending_agent_deletions(deletions)?;
        self.inner
            .backend
            .replace_pending_agent_deletions(&encoded)
            .map_err(backend_failure)
    }

    fn note_pending_deletion_write(&self, result: &Result<(), ProvisionFailure>) {
        *self
            .inner
            .pending_deletion_write_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            result.as_ref().err().map(|failure| failure.primary);
    }

    fn ensure_pending_agent_deletion(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
    ) -> Result<(), ProvisionFailure> {
        let mut deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match deletions.get(project_id) {
            Some(deletion) if deletion.generation != generation => {
                return Err(ProvisionErrorCode::AlreadyRunning.into());
            }
            Some(_) => {}
            None => {
                deletions.insert(
                    project_id.to_owned(),
                    PendingAgentDeletion {
                        project_id: project_id.to_owned(),
                        generation,
                        last_error: ProvisionErrorCode::CredentialDeletionPending,
                    },
                );
            }
        }
        let result = self.persist_pending_agent_deletions(&deletions);
        drop(deletions);
        self.note_pending_deletion_write(&result);
        result
    }

    fn mark_pending_agent_deletion_failure(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
        error: ProvisionErrorCode,
    ) -> Result<(), ProvisionFailure> {
        let mut deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deletion = deletions
            .get_mut(project_id)
            .filter(|deletion| deletion.generation == generation)
            .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::AlreadyRunning))?;
        deletion.last_error = error;
        let result = self.persist_pending_agent_deletions(&deletions);
        drop(deletions);
        self.note_pending_deletion_write(&result);
        result
    }

    fn clear_pending_agent_deletion(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
    ) -> Result<(), ProvisionFailure> {
        let mut deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if deletions
            .get(project_id)
            .is_none_or(|deletion| deletion.generation != generation)
        {
            return Err(ProvisionErrorCode::AlreadyRunning.into());
        }
        let mut persisted = deletions.clone();
        persisted.remove(project_id);
        let result = self.persist_pending_agent_deletions(&persisted);
        if result.is_ok() {
            deletions.remove(project_id);
        }
        drop(deletions);
        self.note_pending_deletion_write(&result);
        result
    }

    async fn delete_agent_credential(&self, project_id: &str) -> Result<(), ProvisionFailure> {
        let backend = Arc::clone(&self.inner.backend);
        let project_id = project_id.to_owned();
        tokio::task::spawn_blocking(move || backend.delete(&project_id))
            .await
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?
            .map_err(backend_failure)
    }

    async fn delete_agent_credential_owned(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
    ) -> Result<(), ProvisionFailure> {
        self.ensure_pending_agent_deletion(project_id, generation)?;
        match self.delete_agent_credential(project_id).await {
            Ok(()) => self.clear_pending_agent_deletion(project_id, generation),
            Err(failure) => {
                let persisted = self.mark_pending_agent_deletion_failure(
                    project_id,
                    generation,
                    failure.primary,
                );
                Err(persisted
                    .err()
                    .map_or(failure.clone(), |cleanup| failure.with_cleanup(cleanup)))
            }
        }
    }

    async fn retry_pending_agent_deletion(&self, project_id: &str) -> Result<(), ProvisionFailure> {
        let generation = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .map(|deletion| deletion.generation);
        match generation {
            Some(generation) => {
                self.delete_agent_credential_owned(project_id, generation)
                    .await
            }
            None => Ok(()),
        }
    }

    fn retry_pending_agent_deletions_blocking(&self) {
        if self.inner.pending_deletion_load_error.is_some() {
            return;
        }
        let deletions = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for deletion in deletions {
            match self.inner.backend.delete(&deletion.project_id) {
                Ok(()) => {
                    let _ = self
                        .clear_pending_agent_deletion(&deletion.project_id, deletion.generation);
                }
                Err(failure) => {
                    let failure = backend_failure(failure);
                    let _ = self.mark_pending_agent_deletion_failure(
                        &deletion.project_id,
                        deletion.generation,
                        failure.primary,
                    );
                }
            }
        }
    }

    async fn rollback_cancelled_provision(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
        result: Result<ProvisionStatus, ProvisionFailure>,
    ) -> Result<ProvisionStatus, ProvisionFailure> {
        let mut cancelled = ProvisionFailure::from(ProvisionErrorCode::Cancelled);
        if let Err(failure) = result {
            if failure.primary == ProvisionErrorCode::Cancelled {
                cancelled.cleanup.extend(failure.cleanup);
            } else {
                cancelled = cancelled.with_cleanup(failure);
            }
        }
        merge_cleanup(
            Err(cancelled),
            self.delete_agent_credential_owned(project_id, generation)
                .await,
        )
    }

    fn finish_operation(&self, project_id: &str, generation: uuid::Uuid) {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active
            .get(project_id)
            .is_some_and(|operation| operation.generation == generation)
        {
            active.remove(project_id);
            self.inner.activity.notify_waiters();
        }
    }

    fn register_pending_revocation(
        &self,
        project_id: &str,
        ssh_host_alias: &str,
        login_token: Zeroizing<String>,
    ) -> uuid::Uuid {
        let generation = uuid::Uuid::new_v4();
        self.inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                project_id.to_owned(),
                PendingRevocation {
                    generation,
                    ssh_host_alias: ssh_host_alias.to_owned(),
                    login_token: Some(Arc::new(login_token)),
                    last_error: None,
                },
            );
        generation
    }

    fn register_pending_tunnel_cleanup(
        &self,
        project_id: &str,
        ssh_host_alias: &str,
        error: ProvisionErrorCode,
    ) {
        self.inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(project_id.to_owned())
            .and_modify(|pending| pending.last_error = Some(error))
            .or_insert_with(|| PendingRevocation {
                generation: uuid::Uuid::new_v4(),
                ssh_host_alias: ssh_host_alias.to_owned(),
                login_token: None,
                last_error: Some(error),
            });
    }

    fn mark_revoked(&self, project_id: &str, generation: uuid::Uuid) {
        if let Some(pending) = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(project_id)
            && pending.generation == generation
        {
            pending.login_token = None;
            pending.last_error = None;
        }
    }

    fn finish_pending_cleanup(&self, project_id: &str, generation: uuid::Uuid) {
        let mut pending = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending
            .get(project_id)
            .is_some_and(|entry| entry.generation == generation && entry.login_token.is_none())
        {
            pending.remove(project_id);
        }
    }

    fn pending_generation(&self, project_id: &str) -> Option<uuid::Uuid> {
        self.inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .map(|pending| pending.generation)
    }

    fn mark_revocation_failure(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
        error: ProvisionErrorCode,
    ) {
        if let Some(pending) = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(project_id)
            && pending.generation == generation
        {
            pending.last_error = Some(error);
        }
    }

    pub(crate) fn cleanup_status(
        &self,
        project_id: &str,
    ) -> Result<CredentialCleanupStatus, ProvisionFailure> {
        let project_id = canonical_project_id(project_id)?;
        if let Some(error) = self.inner.pending_deletion_load_error {
            return Err(error.into());
        }
        let pending_agent_deletion = self
            .inner
            .pending_agent_deletions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&project_id)
            .cloned();
        let pending = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = pending.get(&project_id).cloned();
        drop(pending);
        let reserved = self
            .inner
            .rollback_reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&project_id);
        let active = reserved
            || self
                .inner
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&project_id);
        Ok(CredentialCleanupStatus {
            active,
            pending_agent_deletion: pending_agent_deletion.is_some(),
            pending_revocation: entry
                .as_ref()
                .is_some_and(|pending| pending.login_token.is_some()),
            pending_tunnel_cleanup: entry
                .as_ref()
                .is_some_and(|pending| pending.login_token.is_none()),
            last_error: pending_agent_deletion
                .map(|pending| pending.last_error)
                .or_else(|| entry.and_then(|pending| pending.last_error)),
        })
    }

    pub(crate) async fn retry_cleanup(
        &self,
        project_id: &str,
        tunnels: TunnelState,
    ) -> Result<CredentialCleanupStatus, ProvisionFailure> {
        let project_id = canonical_project_id(project_id)?;
        let reservation = self.reserve_cleanup(&project_id)?;
        let state = self.clone();
        let (result_tx, result_rx) = oneshot::channel();
        self.inner.tasks.spawn(async move {
            let deletion = state.retry_pending_agent_deletion(&project_id).await;
            let revocation = state.retry_pending_revocation(&project_id, &tunnels).await;
            let result = merge_cleanup(deletion, revocation);
            drop(reservation);
            let result = result.and_then(|()| state.cleanup_status(&project_id));
            let _ = result_tx.send(result);
        });
        match tokio::time::timeout(self.inner.deadlines.cleanup, result_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ProvisionErrorCode::AbnormalExit.into()),
            Err(_) => Err(ProvisionErrorCode::Timeout.into()),
        }
    }

    async fn retry_pending_revocation(
        &self,
        project_id: &str,
        tunnels: &TunnelState,
    ) -> Result<(), ProvisionFailure> {
        let Some(pending) = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .cloned()
        else {
            return Ok(());
        };
        let tunnel_key = provision_tunnel_key(project_id);
        let acquire_tunnels = tunnels.clone();
        let acquire_key = tunnel_key.clone();
        let acquire_host = pending.ssh_host_alias.clone();
        let acquire = tokio::task::spawn_blocking(move || {
            acquire_tunnels.get_or_create(&acquire_key, &acquire_host, REMOTE_SERVER_PORT)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| result.map_err(|_| ProvisionErrorCode::TunnelAcquireFailed.into()));
        let revoke = match (acquire, pending.login_token.as_ref()) {
            (Ok(port), Some(login_token)) => {
                self.revoke_login_token(port, project_id, pending.generation, login_token)
                    .await
            }
            (Ok(_), None) => Ok(()),
            (Err(failure), _) => Err(failure),
        };
        if let Err(failure) = &revoke {
            self.mark_revocation_failure(project_id, pending.generation, failure.primary);
        }
        let cleanup_tunnels = tunnels.clone();
        let cleanup_key = tunnel_key;
        let cleanup_host = pending.ssh_host_alias;
        let cleanup = tokio::task::spawn_blocking(move || {
            cleanup_tunnels.close_project(&cleanup_key, &cleanup_host)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| {
            result.map_err(|_| ProvisionFailure::from(ProvisionErrorCode::TunnelCleanupFailed))
        });
        if let Err(failure) = &cleanup {
            self.mark_revocation_failure(project_id, pending.generation, failure.primary);
        }
        if revoke.is_ok() && cleanup.is_ok() {
            self.finish_pending_cleanup(project_id, pending.generation);
        }
        merge_cleanup(revoke, cleanup)
    }

    async fn revoke_login_token(
        &self,
        port: u16,
        project_id: &str,
        generation: uuid::Uuid,
        login_token: &Zeroizing<String>,
    ) -> Result<(), ProvisionFailure> {
        let empty = Zeroizing::new(Vec::new());
        let revoke = post_json(
            port,
            "/api/auth/logout",
            &empty,
            Some(login_token),
            &CancellationToken::new(),
            self.inner.deadlines,
        )
        .await
        .and_then(decode_empty_envelope)
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::RevocationFailed));
        if revoke.is_ok() {
            self.mark_revoked(project_id, generation);
        }
        revoke
    }

    async fn run_provision(
        &self,
        request: ValidatedProvisionRequest,
        tunnels: TunnelState,
        cancellation: CancellationToken,
    ) -> Result<ProvisionStatus, ProvisionFailure> {
        self.retry_pending_revocation(&request.project_id, &tunnels)
            .await?;
        eprintln!("[fns-provision] step=tunnel starting host={}", request.ssh_host_alias);
        let project_id = request.project_id.clone();
        let tunnel_key = provision_tunnel_key(&request.project_id);
        let tunnel_host = request.ssh_host_alias.clone();
        let acquire_tunnels = tunnels.clone();
        let acquire_key = tunnel_key.clone();
        let acquire_host = tunnel_host.clone();
        let acquire = tokio::task::spawn_blocking(move || {
            acquire_tunnels.get_or_create(&acquire_key, &acquire_host, REMOTE_SERVER_PORT)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| result.map_err(|_| ProvisionErrorCode::TunnelAcquireFailed.into()));

        let core = match acquire {
            Ok(port) => self.provision_over_http(port, request, cancellation).await,
            Err(failure) => Err(failure),
        };

        let cleanup_tunnels = tunnels.clone();
        let cleanup_host_metadata = tunnel_host.clone();
        let cleanup = tokio::task::spawn_blocking(move || {
            cleanup_tunnels.close_project(&tunnel_key, &tunnel_host)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| {
            result.map_err(|_failure: TunnelFailure| {
                ProvisionFailure::from(ProvisionErrorCode::TunnelCleanupFailed)
            })
        });
        if let Some(generation) = self.pending_generation(&project_id) {
            if let Err(failure) = &cleanup {
                self.mark_revocation_failure(&project_id, generation, failure.primary);
            } else {
                self.finish_pending_cleanup(&project_id, generation);
            }
        } else if let Err(failure) = &cleanup {
            self.register_pending_tunnel_cleanup(
                &project_id,
                &cleanup_host_metadata,
                failure.primary,
            );
        }
        merge_cleanup(core, cleanup)
    }

    async fn run_workspace_probe(
        &self,
        request: ValidatedWorkspaceProbeRequest,
        tunnels: TunnelState,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceProbeStatus, ProvisionFailure> {
        let backend = Arc::clone(&self.inner.backend);
        let load_project_id = request.project_id.clone();
        let token = tokio::task::spawn_blocking(move || backend.load(&load_project_id))
            .await
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?
            .map_err(backend_failure)?
            .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::CredentialMissing))?;
        if cancellation.is_cancelled() {
            return Err(ProvisionErrorCode::Cancelled.into());
        }

        let tunnel_key = provision_tunnel_key(&request.project_id);
        let tunnel_host = request.ssh_host_alias.clone();
        let acquire_tunnels = tunnels.clone();
        let acquire_key = tunnel_key.clone();
        let acquire_host = tunnel_host.clone();
        let acquire = tokio::task::spawn_blocking(move || {
            acquire_tunnels.get_or_create(&acquire_key, &acquire_host, REMOTE_SERVER_PORT)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| result.map_err(|_| ProvisionErrorCode::TunnelAcquireFailed.into()));

        let core = match acquire {
            Ok(port) => {
                self.probe_workspace_over_websocket(port, &request, &token, &cancellation)
                    .await
            }
            Err(failure) => Err(failure),
        };
        let cleanup_tunnels = tunnels.clone();
        let cleanup_project_id = request.project_id.clone();
        let cleanup_host_metadata = tunnel_host.clone();
        let cleanup = tokio::task::spawn_blocking(move || {
            cleanup_tunnels.close_project(&tunnel_key, &tunnel_host)
        })
        .await
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))
        .and_then(|result| {
            result.map_err(|_| ProvisionFailure::from(ProvisionErrorCode::TunnelCleanupFailed))
        });
        if let Err(failure) = &cleanup {
            self.register_pending_tunnel_cleanup(
                &cleanup_project_id,
                &cleanup_host_metadata,
                failure.primary,
            );
        }
        merge_cleanup(core, cleanup)
    }

    async fn probe_workspace_over_websocket(
        &self,
        port: u16,
        request: &ValidatedWorkspaceProbeRequest,
        token: &SecretToken,
        cancellation: &CancellationToken,
    ) -> Result<WorkspaceProbeStatus, ProvisionFailure> {
        let endpoint =
            WorkspaceEndpoint::parse(&format!("ws://127.0.0.1:{port}/api/user/workspace-sync/v2"))
                .map_err(map_transport_failure)?;
        let stream = cancellable_transport_timeout(
            self.inner.deadlines.connect,
            cancellation,
            fns_transport::socket::connect(&endpoint, token, env!("CARGO_PKG_VERSION")),
        )
        .await?;
        let (mut writer, mut reader) = fns_transport::socket::split(stream);
        let core = probe_workspace_session(
            &mut writer,
            &mut reader,
            request.workspace_id,
            cancellation,
            self.inner.deadlines.io,
        )
        .await;
        let close = transport_timeout(self.inner.deadlines.io, writer.close())
            .await
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::WorkspaceProbeCleanupFailed));
        merge_cleanup(core, close)
    }

    async fn provision_over_http(
        &self,
        port: u16,
        request: ValidatedProvisionRequest,
        cancellation: CancellationToken,
    ) -> Result<ProvisionStatus, ProvisionFailure> {
        eprintln!("[fns-provision] step=login starting port={port}");
        let login_body = json_body(&LoginRequestBody {
            credentials: request.username.as_str(),
            password: request.password.as_str(),
        })?;
        let login_response = match post_json(
            port,
            "/api/user/login",
            &login_body,
            None,
            &cancellation,
            self.inner.deadlines,
        )
        .await
        {
            Ok(response) => {
                eprintln!("[fns-provision] step=login status=http_ok status_code={}", response.status);
                response
            }
            Err(_) if cancellation.is_cancelled() => {
                return Err(ProvisionErrorCode::Cancelled.into());
            }
            Err(failure) => {
                eprintln!("[fns-provision] step=login status=FAILED code={:?}", failure.primary);
                return Err(failure);
            }
        };
        let login: LoginData = decode_envelope(login_response, Endpoint::Login).map_err(|e| {
            eprintln!("[fns-provision] step=login status=DECODE_FAILED code={:?}", e.primary);
            e
        })?;
        eprintln!("[fns-provision] step=login status=decoded_ok");
        let revocation_generation = self.register_pending_revocation(
            &request.project_id,
            &request.ssh_host_alias,
            login.token,
        );
        let login_token = self
            .inner
            .pending_revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&request.project_id)
            .filter(|pending| pending.generation == revocation_generation)
            .and_then(|pending| pending.login_token.as_ref().map(Arc::clone))
            .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?;

        let token_body = json_body(&TokenIssueBody {
            client_type: "fns-agent",
            protocol: "ws",
            client: "fns-agent",
            function: "workspace_rw",
            expired_days: AGENT_TOKEN_EXPIRY_DAYS,
        })?;
        let core = async {
            if cancellation.is_cancelled() {
                return Err(ProvisionErrorCode::Cancelled.into());
            }
            eprintln!("[fns-provision] step=token_issue starting");
            let token_response = match post_json(
                port,
                "/api/token",
                &token_body,
                Some(&login_token),
                &cancellation,
                self.inner.deadlines,
            )
            .await
            {
                Ok(response) => {
                    eprintln!("[fns-provision] step=token_issue status=http_ok status_code={}", response.status);
                    response
                }
                Err(_) if cancellation.is_cancelled() => {
                    return Err(ProvisionErrorCode::Cancelled.into());
                }
                Err(failure) => {
                    eprintln!("[fns-provision] step=token_issue status=FAILED code={:?}", failure.primary);
                    return Err(failure);
                }
            };
            let issued: TokenData = decode_envelope(token_response, Endpoint::Token).map_err(|e| {
                eprintln!("[fns-provision] step=token_issue status=DECODE_FAILED code={:?}", e.primary);
                e
            })?;
            if issued.scope != "p:ws c:fns-agent f:workspace_rw" {
                return Err(ProvisionErrorCode::ScopeMismatch.into());
            }
            if issued.client_type != "fns-agent" {
                return Err(ProvisionErrorCode::ClientTypeMismatch.into());
            }
            validate_token_expiry(&issued.expired_at)?;
            if cancellation.is_cancelled() {
                return Err(ProvisionErrorCode::Cancelled.into());
            }
            let token = SecretToken::from_private_ipc(issued.token.as_bytes().to_vec())
                .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
            let backend = Arc::clone(&self.inner.backend);
            let project_id = request.project_id.clone();
            let cleanup_backend = Arc::clone(&backend);
            let cleanup_project_id = project_id.clone();
            tokio::task::spawn_blocking(move || backend.store(&project_id, &token))
                .await
                .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?
                .map_err(backend_failure)?;
            if cancellation.is_cancelled() {
                tokio::task::spawn_blocking(move || cleanup_backend.delete(&cleanup_project_id))
                    .await
                    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?
                    .map_err(backend_failure)?;
                return Err(ProvisionErrorCode::Cancelled.into());
            }
            Ok(ProvisionStatus { provisioned: true })
        }
        .await;

        let revoke = self
            .revoke_login_token(
                port,
                &request.project_id,
                revocation_generation,
                &login_token,
            )
            .await;
        merge_cleanup(core, revoke)
    }
}

impl Default for CredentialState {
    fn default() -> Self {
        Self::production()
    }
}

impl fmt::Debug for CredentialState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialState")
    }
}

impl CredentialProvider for CredentialState {
    fn token_for_project(&self, project_id: &str) -> Result<SecretToken, AgentErrorCode> {
        let project_id =
            canonical_project_id(project_id).map_err(|_| AgentErrorCode::InsecureCredential)?;
        self.inner
            .backend
            .load(&project_id)
            .map_err(|_| AgentErrorCode::InsecureCredential)?
            .ok_or(AgentErrorCode::AuthRequired)
    }
}

fn backend_failure(failure: CredentialBackendFailure) -> ProvisionFailure {
    match failure {
        CredentialBackendFailure::Access => ProvisionErrorCode::CredentialAccess.into(),
        CredentialBackendFailure::Integrity => ProvisionErrorCode::CredentialIntegrity.into(),
    }
}

fn canonical_project_id(project_id: &str) -> Result<String, ProvisionFailure> {
    let parsed = uuid::Uuid::parse_str(project_id)
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::InvalidProjectId))?;
    if parsed.is_nil() || parsed.to_string() != project_id {
        return Err(ProvisionErrorCode::InvalidProjectId.into());
    }
    Ok(project_id.to_owned())
}

pub(crate) fn provision_tunnel_key(project_id: &str) -> String {
    format!("provision:{project_id}")
}

struct ValidatedProvisionRequest {
    project_id: String,
    ssh_host_alias: String,
    username: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl TryFrom<ProvisionRequest> for ValidatedProvisionRequest {
    type Error = ProvisionFailure;

    fn try_from(request: ProvisionRequest) -> Result<Self, Self::Error> {
        let project_id = canonical_project_id(&request.project_id)?;
        if request.ssh_host_alias.trim().is_empty()
            || request.ssh_host_alias.len() > 255
            || request.username.is_empty()
            || request.username.len() > 512
            || request.password.is_empty()
            || request.password.len() > 4096
            || request
                .ssh_host_alias
                .bytes()
                .any(|byte| byte.is_ascii_control())
            || request.username.bytes().any(|byte| byte.is_ascii_control())
            || request.password.bytes().any(|byte| byte == 0)
        {
            return Err(ProvisionErrorCode::InvalidRequest.into());
        }
        Ok(Self {
            project_id,
            ssh_host_alias: request.ssh_host_alias,
            username: request.username,
            password: request.password,
        })
    }
}

struct ValidatedWorkspaceProbeRequest {
    project_id: String,
    ssh_host_alias: String,
    workspace_id: WorkspaceId,
}

impl TryFrom<WorkspaceProbeRequest> for ValidatedWorkspaceProbeRequest {
    type Error = ProvisionFailure;

    fn try_from(request: WorkspaceProbeRequest) -> Result<Self, Self::Error> {
        let project_id = canonical_project_id(&request.project_id)?;
        if request.ssh_host_alias.trim().is_empty()
            || request.ssh_host_alias.len() > 255
            || request
                .ssh_host_alias
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(ProvisionErrorCode::InvalidRequest.into());
        }
        let workspace_id = WorkspaceId::parse(&request.workspace_id)
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::InvalidRequest))?;
        if workspace_id.as_uuid().is_nil() {
            return Err(ProvisionErrorCode::InvalidRequest.into());
        }
        Ok(Self {
            project_id,
            ssh_host_alias: request.ssh_host_alias,
            workspace_id,
        })
    }
}

fn merge_cleanup<T>(
    primary: Result<T, ProvisionFailure>,
    cleanup: Result<(), ProvisionFailure>,
) -> Result<T, ProvisionFailure> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(primary.with_cleanup(cleanup)),
    }
}

async fn probe_workspace_session(
    writer: &mut SocketWriter,
    reader: &mut SocketReader,
    workspace_id: WorkspaceId,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<WorkspaceProbeStatus, ProvisionFailure> {
    let client_id = ClientId::parse(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?;
    let hello_request_id = RequestId::parse(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?;
    let hello = encode_request(
        WorkspaceAction::WorkspaceHello,
        hello_request_id,
        MessageBody::HelloRequest(WorkspaceHelloRequest {
            protocol_version: "2".to_owned(),
            client_id,
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: vec![
                "binary_chunks".to_owned(),
                "conflicts".to_owned(),
                "snapshot_v1".to_owned(),
            ],
        }),
    )
    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::WorkspaceProbeFailed))?;
    cancellable_transport_timeout(timeout, cancellation, writer.send_text(hello)).await?;

    let hello_response = next_probe_frame(reader, writer, cancellation, timeout).await?;
    match hello_response {
        DecodedFrame {
            action: WorkspaceAction::WorkspaceHello,
            envelope:
                DecodedEnvelope::Success {
                    request_id: Some(request_id),
                    body: MessageBody::HelloResponse(_),
                },
            ..
        } if request_id == hello_request_id => {}
        DecodedFrame {
            envelope: DecodedEnvelope::Failure { error, .. },
            ..
        } => return Err(map_workspace_failure(error.code)),
        _ => return Err(ProvisionErrorCode::WorkspaceProbeFailed.into()),
    }

    let subscribe_request_id = RequestId::parse(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::AbnormalExit))?;
    let subscribe = encode_request(
        WorkspaceAction::WorkspaceSubscribe,
        subscribe_request_id,
        MessageBody::SubscribeRequest(WorkspaceSubscribeRequest {
            workspace_id,
            client_id,
            last_ack_revision: WorkspaceRevision::ZERO,
        }),
    )
    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::WorkspaceProbeFailed))?;
    cancellable_transport_timeout(timeout, cancellation, writer.send_text(subscribe)).await?;

    let acceptance = next_probe_frame(reader, writer, cancellation, timeout).await?;
    match acceptance {
        DecodedFrame {
            action: WorkspaceAction::WorkspaceSnapshotBegin,
            envelope:
                DecodedEnvelope::Success {
                    request_id: None,
                    body: MessageBody::SnapshotBegin(begin),
                },
            ..
        } if begin.workspace_id == workspace_id => Ok(WorkspaceProbeStatus {
            accepted: true,
            workspace_id: workspace_id.to_string(),
        }),
        DecodedFrame {
            action: WorkspaceAction::WorkspaceSnapshotBegin,
            envelope: DecodedEnvelope::Success { .. },
            ..
        } => Err(ProvisionErrorCode::WorkspaceIdentityMismatch.into()),
        DecodedFrame {
            envelope:
                DecodedEnvelope::Failure {
                    request_id: Some(request_id),
                    error,
                },
            ..
        } if request_id == subscribe_request_id => Err(map_workspace_failure(error.code)),
        _ => Err(ProvisionErrorCode::WorkspaceProbeFailed.into()),
    }
}

async fn next_probe_frame(
    reader: &mut SocketReader,
    writer: &mut SocketWriter,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<DecodedFrame, ProvisionFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let message = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProvisionErrorCode::Cancelled.into()),
            result = tokio::time::timeout_at(deadline, reader.next()) => match result {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => return Err(map_transport_failure(error)),
                Ok(None) => return Err(ProvisionErrorCode::WorkspaceProbeFailed.into()),
                Err(_) => return Err(ProvisionErrorCode::Timeout.into()),
            }
        };
        match message {
            InboundMessage::Text(frame) => {
                return decode_server_text_frame(&frame)
                    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::WorkspaceProbeFailed));
            }
            InboundMessage::Ping(bytes) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(ProvisionErrorCode::Timeout.into());
                }
                cancellable_transport_timeout(remaining, cancellation, writer.send_pong(bytes))
                    .await?;
            }
            InboundMessage::Pong(_) => {}
            InboundMessage::Binary(_) => {
                return Err(ProvisionErrorCode::WorkspaceProbeFailed.into());
            }
            InboundMessage::Close => {
                return Err(ProvisionErrorCode::WorkspaceAccessRejected.into());
            }
        }
    }
}

async fn cancellable_transport_timeout<T>(
    duration: Duration,
    cancellation: &CancellationToken,
    operation: impl Future<Output = Result<T, fns_transport::TransportError>>,
) -> Result<T, ProvisionFailure> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(ProvisionErrorCode::Cancelled.into()),
        result = tokio::time::timeout(duration, operation) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(map_transport_failure(error)),
            Err(_) => Err(ProvisionErrorCode::Timeout.into()),
        }
    }
}

async fn transport_timeout<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T, fns_transport::TransportError>>,
) -> Result<T, ProvisionFailure> {
    match tokio::time::timeout(duration, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(map_transport_failure(error)),
        Err(_) => Err(ProvisionErrorCode::Timeout.into()),
    }
}

fn map_transport_failure(error: fns_transport::TransportError) -> ProvisionFailure {
    match error.code() {
        TransportErrorCode::AuthenticationRejected => {
            ProvisionErrorCode::AuthenticationRejected.into()
        }
        TransportErrorCode::Forbidden => ProvisionErrorCode::Forbidden.into(),
        TransportErrorCode::RequestTimeout
        | TransportErrorCode::IdleTimeout
        | TransportErrorCode::TransferTimeout
        | TransportErrorCode::ShutdownTimeout => ProvisionErrorCode::Timeout.into(),
        TransportErrorCode::InvalidConfiguration | TransportErrorCode::Protocol => {
            ProvisionErrorCode::WorkspaceProbeFailed.into()
        }
        TransportErrorCode::Network
        | TransportErrorCode::Core
        | TransportErrorCode::Filesystem
        | TransportErrorCode::StateCorrupt
        | TransportErrorCode::ConflictUnavailable
        | TransportErrorCode::ConflictRevisionStale
        | TransportErrorCode::ConflictResolutionChanged
        | TransportErrorCode::ConflictWaitingBlobs
        | TransportErrorCode::ConflictAutomaticResolutionPending
        | TransportErrorCode::ConflictResolutionPending
        | TransportErrorCode::ConflictRefreshRequired
        | TransportErrorCode::ConflictSelectedSideDeleted
        | TransportErrorCode::MergeFileRequired
        | TransportErrorCode::MergeContentUnavailable
        | TransportErrorCode::ResourceLimit => ProvisionErrorCode::Network.into(),
    }
}

fn map_workspace_failure(code: WorkspaceV2ErrorCode) -> ProvisionFailure {
    match code {
        WorkspaceV2ErrorCode::Unauthenticated => ProvisionErrorCode::AuthenticationRejected.into(),
        WorkspaceV2ErrorCode::Forbidden => ProvisionErrorCode::Forbidden.into(),
        _ => ProvisionErrorCode::WorkspaceAccessRejected.into(),
    }
}

#[derive(Serialize)]
struct LoginRequestBody<'a> {
    credentials: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenIssueBody<'a> {
    client_type: &'a str,
    protocol: &'a str,
    client: &'a str,
    function: &'a str,
    expired_days: u16,
}

fn json_body(value: &impl Serialize) -> Result<Zeroizing<Vec<u8>>, ProvisionFailure> {
    let mut body = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *body, value)
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::InvalidRequest))?;
    Ok(body)
}

struct HttpResponse {
    status: u16,
    body: Zeroizing<Vec<u8>>,
}

async fn post_json(
    port: u16,
    path: &str,
    body: &Zeroizing<Vec<u8>>,
    authorization: Option<&Zeroizing<String>>,
    cancellation: &CancellationToken,
    deadlines: ProvisionDeadlines,
) -> Result<HttpResponse, ProvisionFailure> {
    let mut stream = cancellable_timeout(
        deadlines.connect,
        cancellation,
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await?;
    let mut request = Zeroizing::new(Vec::new());
    write!(
        &mut *request,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Client: webgui\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::Network))?;
    if let Some(token) = authorization {
        request.extend_from_slice(b"Authorization: Bearer ");
        request.extend_from_slice(token.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(body);
    if cancellation.is_cancelled() {
        return Err(ProvisionErrorCode::Cancelled.into());
    }
    // Once bytes can reach the server, finish the bounded exchange so any
    // returned login token remains available for explicit revocation.
    bounded_io_timeout(deadlines.io, stream.write_all(&request)).await?;
    let response = bounded_provision_timeout(deadlines.io, read_http_response(&mut stream)).await;
    match &response {
        Ok(resp) => eprintln!("[fns-provision] post_json path={path} ok status={} body_len={}", resp.status, resp.body.len()),
        Err(failure) => eprintln!("[fns-provision] post_json path={path} FAILED code={:?}", failure.primary),
    }
    response
}

async fn cancellable_timeout<T>(
    duration: Duration,
    cancellation: &CancellationToken,
    operation: impl Future<Output = std::io::Result<T>>,
) -> Result<T, ProvisionFailure> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(ProvisionErrorCode::Cancelled.into()),
        result = tokio::time::timeout(duration, operation) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(ProvisionErrorCode::Network.into()),
            Err(_) => Err(ProvisionErrorCode::Timeout.into()),
        }
    }
}

async fn bounded_io_timeout<T>(
    duration: Duration,
    operation: impl Future<Output = std::io::Result<T>>,
) -> Result<T, ProvisionFailure> {
    match tokio::time::timeout(duration, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(ProvisionErrorCode::Network.into()),
        Err(_) => Err(ProvisionErrorCode::Timeout.into()),
    }
}

async fn bounded_provision_timeout<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T, ProvisionFailure>>,
) -> Result<T, ProvisionFailure> {
    match tokio::time::timeout(duration, operation).await {
        Ok(result) => result,
        Err(_) => Err(ProvisionErrorCode::Timeout.into()),
    }
}

async fn read_http_response(stream: &mut TcpStream) -> Result<HttpResponse, ProvisionFailure> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut header_end: Option<usize> = None;
    let mut expected_body: Option<usize> = None;
    loop {
        if let Some(header_end) = header_end
            && expected_body.is_some_and(|length| {
                header_end
                    .checked_add(length)
                    .is_some_and(|end| bytes.len() >= end)
            })
        {
            break;
        }
        let mut chunk = [0_u8; 2048];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::Network))?;
        if read == 0 {
            eprintln!("[fns-provision] read_http_response: EOF after {} bytes, header_end={}", bytes.len(), header_end.is_some());
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(ProvisionErrorCode::ResponseTooLarge.into());
        }
        if header_end.is_none() {
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = position + 4;
                if end > MAX_HTTP_HEADER_BYTES {
                    return Err(ProvisionErrorCode::MalformedResponse.into());
                }
                expected_body = response_content_length(&bytes[..end])
                    .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
                if expected_body.is_some_and(|length| {
                    end.checked_add(length)
                        .is_none_or(|total| total > MAX_HTTP_RESPONSE_BYTES)
                }) {
                    return Err(ProvisionErrorCode::ResponseTooLarge.into());
                }
                header_end = Some(end);
            } else if bytes.len() > MAX_HTTP_HEADER_BYTES {
                return Err(ProvisionErrorCode::MalformedResponse.into());
            }
        }
    }
    let header_end =
        header_end.ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
    let (status, content_length) = parse_response_head(&bytes[..header_end])
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
    let body_end = content_length.map_or(Ok(bytes.len()), |length| {
        header_end
            .checked_add(length)
            .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::ResponseTooLarge))
    })?;
    if bytes.len() < body_end {
        return Err(ProvisionErrorCode::MalformedResponse.into());
    }
    let mut body = Zeroizing::new(Vec::with_capacity(body_end - header_end));
    body.extend_from_slice(&bytes[header_end..body_end]);
    Ok(HttpResponse { status, body })
}

fn response_content_length(headers: &[u8]) -> std::io::Result<Option<usize>> {
    parse_response_head(headers).map(|(_, length)| length)
}

fn parse_response_head(headers: &[u8]) -> std::io::Result<(u16, Option<usize>)> {
    let mut parsed_headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut parsed_headers);
    if !matches!(response.parse(headers), Ok(httparse::Status::Complete(_))) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "malformed_response",
        ));
    }
    let status = response
        .code
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing_status"))?;
    let mut content_length = None;
    for header in response.headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported_transfer_encoding",
            ));
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate_content_length",
                ));
            }
            let value = std::str::from_utf8(header.value)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid_content_length")
                })?;
            content_length = Some(value);
        }
    }
    Ok((status, content_length))
}

#[derive(Clone, Copy)]
enum Endpoint {
    Login,
    Token,
}

#[derive(Deserialize)]
struct Envelope<T> {
    code: i64,
    status: bool,
    data: Option<T>,
}

#[derive(Deserialize)]
struct LoginData {
    token: Zeroizing<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenData {
    scope: String,
    client_type: String,
    expired_at: String,
    token: Zeroizing<String>,
}

fn validate_token_expiry(expired_at: &str) -> Result<(), ProvisionFailure> {
    // The server may return expiry in RFC3339 (e.g. "2027-08-09T22:24:51Z")
    // or SQLite datetime format (e.g. "2027-08-09 22:24:51"). Accept both.
    let expiry = chrono::DateTime::parse_from_rfc3339(expired_at)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(expired_at, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc())
        })
        .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
    let now = chrono::Utc::now();
    let latest = now
        .checked_add_signed(chrono::Duration::days(i64::from(AGENT_TOKEN_EXPIRY_DAYS)))
        .and_then(|value| value.checked_add_signed(TOKEN_EXPIRY_TRANSPORT_SKEW))
        .ok_or_else(|| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
    if expiry <= now || expiry > latest {
        return Err(ProvisionErrorCode::MalformedResponse.into());
    }
    Ok(())
}

fn decode_envelope<T: for<'de> Deserialize<'de>>(
    response: HttpResponse,
    endpoint: Endpoint,
) -> Result<T, ProvisionFailure> {
    match response.status {
        401 => return Err(ProvisionErrorCode::AuthenticationRejected.into()),
        403 => return Err(ProvisionErrorCode::Forbidden.into()),
        200..=299 => {}
        _ => return Err(ProvisionErrorCode::ServerRejected.into()),
    }
    let envelope: Envelope<T> = serde_json::from_slice(&response.body)
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::MalformedResponse))?;
    if !envelope.status {
        return Err(match (endpoint, envelope.code) {
            (Endpoint::Login, 401 | 402) => ProvisionErrorCode::AuthenticationRejected.into(),
            (_, 403) => ProvisionErrorCode::Forbidden.into(),
            _ => ProvisionErrorCode::ServerRejected.into(),
        });
    }
    envelope
        .data
        .ok_or_else(|| ProvisionErrorCode::MalformedResponse.into())
}

fn decode_empty_envelope(response: HttpResponse) -> Result<(), ProvisionFailure> {
    match response.status {
        200..=299 => {}
        _ => return Err(ProvisionErrorCode::RevocationFailed.into()),
    }
    let envelope: Envelope<serde::de::IgnoredAny> = serde_json::from_slice(&response.body)
        .map_err(|_| ProvisionFailure::from(ProvisionErrorCode::RevocationFailed))?;
    if envelope.status {
        Ok(())
    } else {
        Err(ProvisionErrorCode::RevocationFailed.into())
    }
}

#[tauri::command]
pub(crate) async fn provision_workspace_credential(
    request: ProvisionRequest,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<ProvisionStatus, ProvisionFailure> {
    credential_state
        .provision(request, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) async fn reprovision_workspace_credential(
    request: ProvisionRequest,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<ProvisionStatus, ProvisionFailure> {
    credential_state
        .reprovision(request, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) async fn workspace_credential_status(
    project_id: String,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<WorkspaceCredentialStatus, ProvisionFailure> {
    credential_state.status(&project_id).await
}

#[tauri::command]
pub(crate) async fn probe_workspace_access(
    request: WorkspaceProbeRequest,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<WorkspaceProbeStatus, ProvisionFailure> {
    credential_state
        .probe_workspace(request, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) async fn delete_workspace_credential(
    project_id: String,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<DeleteCredentialStatus, ProvisionFailure> {
    credential_state
        .delete(&project_id, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) async fn cancel_workspace_provisioning(
    project_id: String,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<CredentialRollbackStatus, ProvisionFailure> {
    credential_state
        .cancel_and_cleanup(&project_id, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) async fn retry_workspace_credential_cleanup(
    project_id: String,
    tunnel_state: tauri::State<'_, TunnelState>,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<CredentialCleanupStatus, ProvisionFailure> {
    credential_state
        .retry_cleanup(&project_id, tunnel_state.inner().clone())
        .await
}

#[tauri::command]
pub(crate) fn workspace_credential_cleanup_status(
    project_id: String,
    credential_state: tauri::State<'_, CredentialState>,
) -> Result<CredentialCleanupStatus, ProvisionFailure> {
    credential_state.cleanup_status(&project_id)
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialBackend, CredentialBackendFailure, CredentialState, OPERATION_HANDOFF,
        ProvisionDeadlines, ProvisionErrorCode, ProvisionRequest, WorkspaceProbeRequest,
    };
    use crate::ssh_tunnel::{
        TunnelCreateFailure, TunnelFactory, TunnelFailure, TunnelResource, TunnelState,
    };
    use crate::sync::CredentialProvider;
    use fns_agent::error::AgentErrorCode;
    use fns_protocol::{
        DecodedEnvelope, MessageBody, WorkspaceAction, WorkspaceFlow, WorkspaceHelloResponse,
        WorkspaceRevision, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotMode, decode_text_frame,
        encode_success,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const PROJECT_ID: &str = "10000000-0000-4000-8000-000000000001";
    const LOGIN_TOKEN: &str = "LOGIN.JWT.SENTINEL";
    const AGENT_TOKEN: &str = "AGENT.JWT.SENTINEL";
    const WORKSPACE_ID: &str = "20000000-0000-4000-8000-000000000002";
    const WRONG_WORKSPACE_ID: &str = "30000000-0000-4000-8000-000000000003";

    #[derive(Default)]
    struct WakeProbe {
        count: AtomicUsize,
    }

    impl Wake for WakeProbe {
        fn wake(self: Arc<Self>) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct MemoryCredentialBackend {
        values: Mutex<HashMap<String, Vec<u8>>>,
        load_failure: Mutex<Option<CredentialBackendFailure>>,
        store_failure: Mutex<Option<CredentialBackendFailure>>,
        delete_failure: Mutex<Option<CredentialBackendFailure>>,
        pending_agent_deletions: Mutex<Option<Vec<u8>>>,
    }

    impl CredentialBackend for MemoryCredentialBackend {
        fn store(
            &self,
            project_id: &str,
            token: &fns_platform::SecretToken,
        ) -> Result<(), CredentialBackendFailure> {
            if let Some(failure) = *self.store_failure.lock().unwrap() {
                return Err(failure);
            }
            self.values
                .lock()
                .unwrap()
                .insert(project_id.to_owned(), token.with_exposed(<[u8]>::to_vec));
            Ok(())
        }

        fn load(
            &self,
            project_id: &str,
        ) -> Result<Option<fns_platform::SecretToken>, CredentialBackendFailure> {
            if let Some(failure) = *self.load_failure.lock().unwrap() {
                return Err(failure);
            }
            self.values
                .lock()
                .unwrap()
                .get(project_id)
                .cloned()
                .map(fns_platform::SecretToken::from_private_ipc)
                .transpose()
                .map_err(|_| CredentialBackendFailure::Integrity)
        }

        fn delete(&self, project_id: &str) -> Result<(), CredentialBackendFailure> {
            if let Some(failure) = *self.delete_failure.lock().unwrap() {
                return Err(failure);
            }
            self.values.lock().unwrap().remove(project_id);
            Ok(())
        }

        fn load_pending_agent_deletions(
            &self,
        ) -> Result<Option<Vec<u8>>, CredentialBackendFailure> {
            Ok(self.pending_agent_deletions.lock().unwrap().clone())
        }

        fn replace_pending_agent_deletions(
            &self,
            encoded: &[u8],
        ) -> Result<(), CredentialBackendFailure> {
            *self.pending_agent_deletions.lock().unwrap() = Some(encoded.to_vec());
            Ok(())
        }
    }

    struct FixtureTunnelFactory {
        port: u16,
        creates: Arc<AtomicUsize>,
        close_failures: Arc<AtomicUsize>,
        close_attempts: Arc<Mutex<Vec<u64>>>,
        dropped_unclosed: Arc<AtomicUsize>,
        close_delay: Duration,
    }

    impl TunnelFactory for FixtureTunnelFactory {
        fn create(
            &self,
            _tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn TunnelResource>, TunnelCreateFailure> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FixtureTunnel {
                port: self.port,
                identity: 73,
                closed: false,
                close_failures: Arc::clone(&self.close_failures),
                close_attempts: Arc::clone(&self.close_attempts),
                dropped_unclosed: Arc::clone(&self.dropped_unclosed),
                close_delay: self.close_delay,
            }))
        }
    }

    struct FixtureTunnel {
        port: u16,
        identity: u64,
        closed: bool,
        close_failures: Arc<AtomicUsize>,
        close_attempts: Arc<Mutex<Vec<u64>>>,
        dropped_unclosed: Arc<AtomicUsize>,
        close_delay: Duration,
    }

    impl TunnelResource for FixtureTunnel {
        fn local_port(&self) -> u16 {
            self.port
        }

        fn is_alive(&mut self) -> Result<bool, TunnelFailure> {
            Ok(!self.closed)
        }

        fn close(&mut self) -> Result<(), TunnelFailure> {
            self.close_attempts.lock().unwrap().push(self.identity);
            std::thread::sleep(self.close_delay);
            if self
                .close_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(crate::ssh_tunnel::TunnelErrorCode::WaitTimeout.into());
            }
            self.closed = true;
            Ok(())
        }
    }

    impl Drop for FixtureTunnel {
        fn drop(&mut self) {
            if !self.closed {
                self.dropped_unclosed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct TunnelFixture {
        state: TunnelState,
        creates: Arc<AtomicUsize>,
        close_failures: Arc<AtomicUsize>,
        close_attempts: Arc<Mutex<Vec<u64>>>,
        dropped_unclosed: Arc<AtomicUsize>,
    }

    impl TunnelFixture {
        fn new(port: u16, close_failures: usize) -> Self {
            Self::with_close_delay(port, close_failures, Duration::ZERO)
        }

        fn with_close_delay(port: u16, close_failures: usize, close_delay: Duration) -> Self {
            let creates = Arc::new(AtomicUsize::new(0));
            let close_failures = Arc::new(AtomicUsize::new(close_failures));
            let close_attempts = Arc::new(Mutex::new(Vec::new()));
            let dropped_unclosed = Arc::new(AtomicUsize::new(0));
            let state = TunnelState::with_factory(Arc::new(FixtureTunnelFactory {
                port,
                creates: Arc::clone(&creates),
                close_failures: Arc::clone(&close_failures),
                close_attempts: Arc::clone(&close_attempts),
                dropped_unclosed: Arc::clone(&dropped_unclosed),
                close_delay,
            }));
            Self {
                state,
                creates,
                close_failures,
                close_attempts,
                dropped_unclosed,
            }
        }
    }

    #[derive(Clone)]
    enum ResponsePlan {
        Reply(Vec<u8>),
        Delay(Duration, Vec<u8>),
        Hold,
    }

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    struct HttpFixture {
        port: u16,
        requests: Arc<Mutex<Vec<Option<CapturedRequest>>>>,
        task: tokio::task::JoinHandle<()>,
    }

    struct WorkspaceProbeFixture {
        port: u16,
        authorization: Arc<Mutex<Option<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl WorkspaceProbeFixture {
        #[allow(clippy::result_large_err)]
        async fn spawn(response_workspace_id: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let authorization = Arc::new(Mutex::new(None));
            let captured_authorization = Arc::clone(&authorization);
            let task = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut websocket = tokio_tungstenite::accept_hdr_async(
                    stream,
                    move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        assert_eq!(request.uri().path(), "/api/user/workspace-sync/v2");
                        *captured_authorization.lock().unwrap() = request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned);
                        Ok(response)
                    },
                )
                .await
                .unwrap();

                let hello = websocket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap();
                let hello =
                    decode_text_frame(hello.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
                let (hello_request_id, hello_client_id) = match hello.envelope {
                    DecodedEnvelope::Request {
                        request_id,
                        body: MessageBody::HelloRequest(body),
                    } => {
                        assert_eq!(body.protocol_version, "2");
                        assert_eq!(
                            body.capabilities,
                            ["binary_chunks", "conflicts", "snapshot_v1"]
                        );
                        (request_id, body.client_id)
                    }
                    _ => panic!("unexpected hello request"),
                };
                let hello_response = encode_success(
                    WorkspaceAction::WorkspaceHello,
                    WorkspaceFlow::ServerResponse,
                    Some(hello_request_id),
                    MessageBody::HelloResponse(WorkspaceHelloResponse {
                        protocol_version: "2".to_owned(),
                        server_version: "fixture".to_owned(),
                        max_control_frame_bytes: 65_536,
                        max_binary_chunk_bytes: 1_048_576,
                        max_blob_bytes: 5_368_709_120,
                        max_transfers_per_connection: 4,
                        heartbeat_seconds: 25,
                    }),
                )
                .unwrap();
                websocket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        String::from_utf8(hello_response).unwrap().into(),
                    ))
                    .await
                    .unwrap();

                let subscribe = websocket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap();
                let subscribe =
                    decode_text_frame(subscribe.as_bytes(), WorkspaceFlow::ClientRequest).unwrap();
                match subscribe.envelope {
                    DecodedEnvelope::Request {
                        body: MessageBody::SubscribeRequest(body),
                        ..
                    } => {
                        assert_eq!(body.workspace_id.to_string(), WORKSPACE_ID);
                        assert_eq!(body.client_id, hello_client_id);
                    }
                    _ => panic!("unexpected subscribe request"),
                }
                let response_workspace =
                    fns_protocol::WorkspaceId::parse(response_workspace_id).unwrap();
                let stream_id =
                    fns_protocol::StreamId::parse(&uuid::Uuid::new_v4().to_string()).unwrap();
                let snapshot = encode_success(
                    WorkspaceAction::WorkspaceSnapshotBegin,
                    WorkspaceFlow::ServerPush,
                    None,
                    MessageBody::SnapshotBegin(WorkspaceSnapshotBeginMessage {
                        workspace_id: response_workspace,
                        stream_id,
                        mode: WorkspaceSnapshotMode::Snapshot,
                        from_revision: WorkspaceRevision::ZERO,
                        final_revision: WorkspaceRevision::ZERO,
                        entry_count: 0,
                        event_count: 0,
                        conflict_count: 0,
                    }),
                )
                .unwrap();
                websocket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        String::from_utf8(snapshot).unwrap().into(),
                    ))
                    .await
                    .unwrap();
                let _ = tokio::time::timeout(Duration::from_secs(1), websocket.next()).await;
            });
            Self {
                port,
                authorization,
                task,
            }
        }

        async fn finish(self) -> Option<String> {
            tokio::time::timeout(Duration::from_secs(2), self.task)
                .await
                .expect("workspace probe fixture did not stop")
                .unwrap();
            self.authorization.lock().unwrap().clone()
        }
    }

    impl HttpFixture {
        async fn spawn(plans: Vec<ResponsePlan>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(vec![None; plans.len()]));
            let task_requests = Arc::clone(&requests);
            let task = tokio::spawn(async move {
                let mut connections = Vec::new();
                for (index, plan) in plans.into_iter().enumerate() {
                    let (stream, _) = listener.accept().await.unwrap();
                    let request_slot = Arc::clone(&task_requests);
                    connections.push(tokio::spawn(async move {
                        serve_fixture_connection(stream, plan, request_slot, index).await;
                    }));
                }
                for connection in connections {
                    connection.await.unwrap();
                }
            });
            Self {
                port,
                requests,
                task,
            }
        }

        async fn wait_for_requests(&self, count: usize) {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if self
                        .requests
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|request| request.is_some())
                        .count()
                        >= count
                    {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("fixture request was not received");
        }

        async fn finish(self) -> Vec<CapturedRequest> {
            tokio::time::timeout(Duration::from_secs(2), self.task)
                .await
                .expect("fixture did not stop")
                .unwrap();
            Arc::try_unwrap(self.requests)
                .ok()
                .unwrap()
                .into_inner()
                .unwrap()
                .into_iter()
                .map(Option::unwrap)
                .collect()
        }
    }

    async fn serve_fixture_connection(
        mut stream: TcpStream,
        plan: ResponsePlan,
        requests: Arc<Mutex<Vec<Option<CapturedRequest>>>>,
        index: usize,
    ) {
        let request = read_fixture_request(&mut stream).await;
        requests.lock().unwrap()[index] = Some(request);
        match plan {
            ResponsePlan::Reply(response) => {
                stream.write_all(&response).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            ResponsePlan::Delay(delay, response) => {
                tokio::time::sleep(delay).await;
                stream.write_all(&response).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            ResponsePlan::Hold => {
                let mut byte = [0_u8; 1];
                loop {
                    match stream.read(&mut byte).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        }
    }

    async fn read_fixture_request(stream: &mut TcpStream) -> CapturedRequest {
        let mut bytes = Vec::new();
        let header_end = loop {
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "request closed before headers");
            bytes.extend_from_slice(&chunk[..read]);
            assert!(bytes.len() < 64 * 1024, "fixture request exceeded bound");
        };
        let header_text = std::str::from_utf8(&bytes[..header_end]).unwrap();
        let mut lines = header_text.split("\r\n");
        let mut request_line = lines.next().unwrap().split_ascii_whitespace();
        let method = request_line.next().unwrap().to_owned();
        let path = request_line.next().unwrap().to_owned();
        let mut headers = HashMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').unwrap();
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let content_length = headers
            .get("content-length")
            .map_or(0, |value| value.parse::<usize>().unwrap());
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "request closed before body");
            bytes.extend_from_slice(&chunk[..read]);
        }
        CapturedRequest {
            method,
            path,
            headers,
            body: bytes[header_end..header_end + content_length].to_vec(),
        }
    }

    fn response(status: u16, body: impl AsRef<[u8]>) -> ResponsePlan {
        let body = body.as_ref();
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            _ => "Error",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        ResponsePlan::Reply(response)
    }

    fn delayed_response(delay: Duration, status: u16, body: impl AsRef<[u8]>) -> ResponsePlan {
        let ResponsePlan::Reply(response) = response(status, body) else {
            unreachable!();
        };
        ResponsePlan::Delay(delay, response)
    }

    fn delayed_plan(delay: Duration, plan: ResponsePlan) -> ResponsePlan {
        let ResponsePlan::Reply(response) = plan else {
            unreachable!();
        };
        ResponsePlan::Delay(delay, response)
    }

    fn login_response() -> ResponsePlan {
        response(
            200,
            serde_json::to_vec(&json!({
                "code": 0,
                "status": true,
                "message": "success",
                "data": {
                    "uid": 41,
                    "email": "user@example.com",
                    "username": "fixture-user",
                    "token": LOGIN_TOKEN,
                    "tokenId": 7,
                    "avatar": "",
                    "isDeleted": false,
                    "updatedAt": "2026-08-09T00:00:00Z",
                    "createdAt": "2026-08-09T00:00:00Z"
                }
            }))
            .unwrap(),
        )
    }

    fn token_response(agent_token: &str) -> ResponsePlan {
        token_response_with_expiry(agent_token, Some(Value::String(valid_expiry())))
    }

    fn valid_expiry() -> String {
        chrono::Utc::now()
            .checked_add_signed(chrono::Duration::days(29))
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn token_response_with_expiry(agent_token: &str, expired_at: Option<Value>) -> ResponsePlan {
        let mut data = serde_json::Map::from_iter([
            ("id".into(), json!(9)),
            ("scope".into(), json!("p:ws c:fns-agent f:workspace_rw")),
            ("clientType".into(), json!("fns-agent")),
            ("token".into(), json!(agent_token)),
        ]);
        if let Some(expired_at) = expired_at {
            data.insert("expiredAt".into(), expired_at);
        }
        response(
            200,
            serde_json::to_vec(&json!({
                "code": 0,
                "status": true,
                "message": "success",
                "data": data
            }))
            .unwrap(),
        )
    }

    fn logout_response() -> ResponsePlan {
        response(200, br#"{"code":0,"status":true,"message":"success"}"#)
    }

    fn request() -> ProvisionRequest {
        ProvisionRequest {
            project_id: PROJECT_ID.to_owned(),
            ssh_host_alias: "fixture-host".to_owned(),
            username: "fixture-user".to_owned().into(),
            password: "PASSWORD.SENTINEL".to_owned().into(),
        }
    }

    fn deadlines(duration: Duration) -> ProvisionDeadlines {
        ProvisionDeadlines {
            connect: duration,
            io: duration,
            operation: duration.saturating_mul(6),
            cleanup: duration.saturating_mul(6),
        }
    }

    #[test]
    fn credential_provider_distinguishes_missing_from_access_and_integrity_failures() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );

        assert_eq!(
            state.token_for_project(PROJECT_ID).unwrap_err(),
            AgentErrorCode::AuthRequired
        );
        *backend.load_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        assert_eq!(
            state.token_for_project(PROJECT_ID).unwrap_err(),
            AgentErrorCode::InsecureCredential
        );
        *backend.load_failure.lock().unwrap() = Some(CredentialBackendFailure::Integrity);
        assert_eq!(
            state.token_for_project(PROJECT_ID).unwrap_err(),
            AgentErrorCode::InsecureCredential
        );
        let rendered = format!("{state:?}");
        assert!(!rendered.contains("PASSWORD"));
        assert!(!rendered.contains("AGENT.JWT"));
        let rendered_request = format!("{:?}", request());
        assert!(!rendered_request.contains("fixture-user"));
        assert!(!rendered_request.contains("PASSWORD.SENTINEL"));
    }

    #[tokio::test]
    async fn credential_status_distinguishes_missing_available_and_unreadable() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );

        assert!(!state.status(PROJECT_ID).await.unwrap().available);
        backend
            .values
            .lock()
            .unwrap()
            .insert(PROJECT_ID.to_owned(), AGENT_TOKEN.as_bytes().to_vec());
        assert!(state.status(PROJECT_ID).await.unwrap().available);

        *backend.load_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        assert_eq!(
            state.status(PROJECT_ID).await.unwrap_err().primary,
            ProvisionErrorCode::CredentialAccess
        );
        assert_eq!(
            state.status("not-a-project-id").await.unwrap_err().primary,
            ProvisionErrorCode::InvalidProjectId
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provision_uses_exact_webgui_flow_stores_only_agent_token_and_closes_tunnel() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );

        let status = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap();
        assert!(status.provisioned);
        assert_eq!(
            backend.values.lock().unwrap().get(PROJECT_ID).unwrap(),
            AGENT_TOKEN.as_bytes()
        );
        assert_ne!(
            backend.values.lock().unwrap().get(PROJECT_ID).unwrap(),
            LOGIN_TOKEN.as_bytes()
        );
        assert_eq!(tunnels.creates.load(Ordering::SeqCst), 1);
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
        assert_eq!(tunnels.dropped_unclosed.load(Ordering::SeqCst), 0);

        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            (requests[0].method.as_str(), requests[0].path.as_str()),
            ("POST", "/api/user/login")
        );
        assert_eq!(
            requests[0].headers.get("x-client").map(String::as_str),
            Some("webgui")
        );
        assert!(!requests[0].headers.contains_key("authorization"));
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
            json!({"credentials":"fixture-user","password":"PASSWORD.SENTINEL"})
        );
        assert_eq!(
            (requests[1].method.as_str(), requests[1].path.as_str()),
            ("POST", "/api/token")
        );
        assert_eq!(
            requests[1].headers.get("x-client").map(String::as_str),
            Some("webgui")
        );
        assert_eq!(
            requests[1].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
            json!({
                "clientType":"fns-agent",
                "protocol":"ws",
                "client":"fns-agent",
                "function":"workspace_rw",
                "expiredDays":30
            })
        );
        assert_eq!(
            (requests[2].method.as_str(), requests[2].path.as_str()),
            ("POST", "/api/auth/logout")
        );
        assert_eq!(
            requests[2].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert_eq!(
            requests[2].headers.get("x-client").map(String::as_str),
            Some("webgui")
        );
        assert!(requests[2].body.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workspace_probe_requires_matching_snapshot_identity_and_reaps_the_tunnel() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        backend
            .values
            .lock()
            .unwrap()
            .insert(PROJECT_ID.to_owned(), AGENT_TOKEN.as_bytes().to_vec());
        let state =
            CredentialState::with_backend_and_deadlines(backend, deadlines(Duration::from_secs(1)));

        let accepted_fixture = WorkspaceProbeFixture::spawn(WORKSPACE_ID).await;
        let accepted_tunnels = TunnelFixture::new(accepted_fixture.port, 0);
        let accepted = state
            .probe_workspace(
                WorkspaceProbeRequest {
                    project_id: PROJECT_ID.to_owned(),
                    ssh_host_alias: "fixture-host".to_owned(),
                    workspace_id: WORKSPACE_ID.to_owned(),
                },
                accepted_tunnels.state.clone(),
            )
            .await
            .unwrap();
        assert!(accepted.accepted);
        assert_eq!(accepted.workspace_id, WORKSPACE_ID);
        assert_eq!(*accepted_tunnels.close_attempts.lock().unwrap(), vec![73]);
        assert_eq!(
            accepted_fixture.finish().await.as_deref(),
            Some("Bearer AGENT.JWT.SENTINEL")
        );

        let wrong_fixture = WorkspaceProbeFixture::spawn(WRONG_WORKSPACE_ID).await;
        let wrong_tunnels = TunnelFixture::new(wrong_fixture.port, 0);
        let failure = state
            .probe_workspace(
                WorkspaceProbeRequest {
                    project_id: PROJECT_ID.to_owned(),
                    ssh_host_alias: "fixture-host".to_owned(),
                    workspace_id: WORKSPACE_ID.to_owned(),
                },
                wrong_tunnels.state.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            failure.primary,
            ProvisionErrorCode::WorkspaceIdentityMismatch
        );
        assert_eq!(*wrong_tunnels.close_attempts.lock().unwrap(), vec![73]);
        wrong_fixture.finish().await;
        assert!(!state.has_active_operations());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_cancel_during_probe_cleanup_still_owns_credential_deletion() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        backend
            .values
            .lock()
            .unwrap()
            .insert(PROJECT_ID.to_owned(), AGENT_TOKEN.as_bytes().to_vec());
        let state = Arc::new(CredentialState::with_backend_and_deadlines(
            backend.clone(),
            ProvisionDeadlines {
                connect: Duration::from_millis(500),
                io: Duration::from_millis(500),
                operation: Duration::from_secs(1),
                cleanup: Duration::from_millis(50),
            },
        ));
        let fixture = WorkspaceProbeFixture::spawn(WORKSPACE_ID).await;
        let tunnels = TunnelFixture::with_close_delay(fixture.port, 0, Duration::from_millis(300));
        let probe_state = Arc::clone(&state);
        let probe_tunnels = tunnels.state.clone();
        let probe = tokio::spawn(async move {
            probe_state
                .probe_workspace(
                    WorkspaceProbeRequest {
                        project_id: PROJECT_ID.to_owned(),
                        ssh_host_alias: "fixture-host".to_owned(),
                        workspace_id: WORKSPACE_ID.to_owned(),
                    },
                    probe_tunnels,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !tunnels.close_attempts.lock().unwrap().is_empty() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("probe tunnel cleanup did not start");

        let failure = state
            .cancel_and_cleanup(PROJECT_ID, tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        assert!(state.has_active_operations());
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned cancellation rollback did not settle");
        assert_eq!(
            probe.await.unwrap().unwrap_err().primary,
            ProvisionErrorCode::Cancelled
        );
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
        assert_eq!(tunnels.dropped_unclosed.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture.finish().await.as_deref(),
            Some("Bearer AGENT.JWT.SENTINEL")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_auth_malformed_oversize_and_scope_failures_are_stable_and_secret_free() {
        let cases = [
            (
                response(401, b"{}"),
                ProvisionErrorCode::AuthenticationRejected,
            ),
            (response(403, b"{}"), ProvisionErrorCode::Forbidden),
            (
                response(200, b"not-json"),
                ProvisionErrorCode::MalformedResponse,
            ),
            (
                response(200, vec![b'x'; 65 * 1024]),
                ProvisionErrorCode::ResponseTooLarge,
            ),
        ];
        for (plan, expected) in cases {
            let fixture = HttpFixture::spawn(vec![plan]).await;
            let tunnels = TunnelFixture::new(fixture.port, 0);
            let state = CredentialState::with_backend_and_deadlines(
                Arc::new(MemoryCredentialBackend::default()),
                deadlines(Duration::from_secs(1)),
            );
            let failure = state
                .provision(request(), tunnels.state.clone())
                .await
                .unwrap_err();
            assert_eq!(failure.primary, expected);
            let rendered = format!("{failure:?} {failure}");
            assert!(!rendered.contains("PASSWORD.SENTINEL"));
            assert!(!rendered.contains(LOGIN_TOKEN));
            assert!(!rendered.contains(AGENT_TOKEN));
            assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
            fixture.finish().await;
        }

        let wrong_scope = HttpFixture::spawn(vec![
            login_response(),
            response(
                200,
                br#"{"code":0,"status":true,"data":{"id":9,"scope":"p:ws c:fns-agent f:*","clientType":"fns-agent","expiredAt":"2026-09-08T00:00:00Z","token":"AGENT.JWT.SENTINEL"}}"#,
            ),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(wrong_scope.port, 0);
        let state = CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        );
        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::ScopeMismatch);
        wrong_scope.finish().await;

        let wrong_client = HttpFixture::spawn(vec![
            login_response(),
            response(
                200,
                br#"{"code":0,"status":true,"data":{"id":9,"scope":"p:ws c:fns-agent f:workspace_rw","clientType":"webgui","expiredAt":"2026-09-08T00:00:00Z","token":"AGENT.JWT.SENTINEL"}}"#,
            ),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(wrong_client.port, 0);
        let state = CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        );
        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::ClientTypeMismatch);
        wrong_client.finish().await;

        for expired_at in [
            None,
            Some(json!("not-a-timestamp")),
            Some(json!("1970-01-01T00:00:00Z")),
            Some(json!("9999-12-31T23:59:59Z")),
        ] {
            let fixture = HttpFixture::spawn(vec![
                login_response(),
                token_response_with_expiry(AGENT_TOKEN, expired_at),
                logout_response(),
            ])
            .await;
            let tunnels = TunnelFixture::new(fixture.port, 0);
            let backend = Arc::new(MemoryCredentialBackend::default());
            let state = CredentialState::with_backend_and_deadlines(
                backend.clone(),
                deadlines(Duration::from_secs(1)),
            );
            let failure = state
                .provision(request(), tunnels.state.clone())
                .await
                .unwrap_err();
            assert_eq!(failure.primary, ProvisionErrorCode::MalformedResponse);
            assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
            fixture.finish().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_and_explicit_cancellation_are_bounded_and_still_own_cleanup() {
        let timeout_fixture = HttpFixture::spawn(vec![ResponsePlan::Hold]).await;
        let timeout_tunnels = TunnelFixture::new(timeout_fixture.port, 0);
        let timeout_state = CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_millis(25)),
        );
        let failure = tokio::time::timeout(
            Duration::from_millis(250),
            timeout_state.provision(request(), timeout_tunnels.state.clone()),
        )
        .await
        .expect("public provisioning deadline escaped")
        .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        assert_eq!(*timeout_tunnels.close_attempts.lock().unwrap(), vec![73]);
        timeout_fixture.finish().await;

        let cancel_fixture = HttpFixture::spawn(vec![
            login_response(),
            ResponsePlan::Hold,
            logout_response(),
        ])
        .await;
        let cancel_tunnels = TunnelFixture::new(cancel_fixture.port, 0);
        let cancel_state = Arc::new(CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        ));
        let operation_state = Arc::clone(&cancel_state);
        let operation_tunnels = cancel_tunnels.state.clone();
        let operation = tokio::spawn(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        cancel_fixture.wait_for_requests(2).await;
        assert!(cancel_state.cancel_provisioning(PROJECT_ID));
        let failure = operation.await.unwrap().unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Cancelled);
        assert_eq!(*cancel_tunnels.close_attempts.lock().unwrap(), vec![73]);
        let requests = cancel_fixture.finish().await;
        assert_eq!(requests[2].path, "/api/auth/logout");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn late_token_after_public_timeout_is_never_stored_and_cleanup_is_joined() {
        let late_token_body = serde_json::to_vec(&json!({
            "code": 0,
            "status": true,
            "data": {
                "scope": "p:ws c:fns-agent f:workspace_rw",
                "clientType": "fns-agent",
                "expiredAt": valid_expiry(),
                "token": AGENT_TOKEN
            }
        }))
        .unwrap();
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            delayed_response(Duration::from_millis(300), 200, late_token_body),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            ProvisionDeadlines {
                connect: Duration::from_millis(500),
                io: Duration::from_millis(500),
                operation: Duration::from_millis(100),
                cleanup: Duration::from_secs(1),
            },
        );

        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();

        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(!state.has_active_operations());
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
        let requests = fixture.finish().await;
        assert_eq!(requests[0].path, "/api/user/login");
        assert_eq!(requests[1].path, "/api/token");
        assert_eq!(requests[2].path, "/api/auth/logout");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn late_login_after_public_timeout_is_recovered_and_revoked_without_issuing_a_token() {
        let fixture = HttpFixture::spawn(vec![
            delayed_plan(Duration::from_millis(300), login_response()),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            ProvisionDeadlines {
                connect: Duration::from_millis(500),
                io: Duration::from_millis(500),
                operation: Duration::from_millis(100),
                cleanup: Duration::from_secs(1),
            },
        );

        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();

        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(!state.has_active_operations());
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/api/user/login");
        assert_eq!(requests[1].path, "/api/auth/logout");
        assert_eq!(
            requests[1].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_during_logout_rolls_back_agent_token_and_retains_owned_cleanup() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            delayed_plan(Duration::from_millis(300), logout_response()),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            ProvisionDeadlines {
                connect: Duration::from_millis(500),
                io: Duration::from_millis(500),
                operation: Duration::from_millis(50),
                cleanup: Duration::from_millis(50),
            },
        );

        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        let pending = state.cleanup_status(PROJECT_ID).unwrap();
        assert!(pending.active);
        assert!(pending.pending_revocation);

        let requests = fixture.finish().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned late-logout cleanup did not settle");
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert_eq!(requests[2].path, "/api/auth/logout");
        assert_eq!(
            requests[2].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_cancel_during_logout_rolls_back_agent_token_before_completion() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            delayed_plan(Duration::from_millis(200), logout_response()),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = Arc::new(CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        ));
        let operation_state = Arc::clone(&state);
        let operation_tunnels = tunnels.state.clone();
        let operation = tokio::spawn(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        fixture.wait_for_requests(3).await;

        assert!(state.cancel_provisioning(PROJECT_ID));
        let failure = operation.await.unwrap().unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Cancelled);
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(!state.has_active_operations());
        fixture.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_reservation_prevents_deleting_a_newer_successful_generation() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            delayed_plan(Duration::from_millis(200), logout_response()),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = Arc::new(CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        ));
        let provision_state = Arc::clone(&state);
        let provision_tunnels = tunnels.state.clone();
        let provision = tokio::spawn(async move {
            provision_state
                .provision(request(), provision_tunnels)
                .await
        });
        fixture.wait_for_requests(3).await;

        let cleanup_state = Arc::clone(&state);
        let cleanup_tunnels = tunnels.state.clone();
        let cleanup = tokio::spawn(async move {
            cleanup_state
                .cancel_and_cleanup(PROJECT_ID, cleanup_tunnels)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state
                    .inner
                    .rollback_reservations
                    .lock()
                    .unwrap()
                    .contains(PROJECT_ID)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup reservation was not acquired");

        let rejected_tunnels = TunnelFixture::new(1, 0);
        let rejected = state
            .reprovision(request(), rejected_tunnels.state)
            .await
            .unwrap_err();
        assert_eq!(rejected.primary, ProvisionErrorCode::AlreadyRunning);
        assert_eq!(rejected_tunnels.creates.load(Ordering::SeqCst), 0);

        let rollback = cleanup.await.unwrap().unwrap();
        assert!(rollback.credential_deleted);
        assert!(!rollback.active);
        assert_eq!(
            provision.await.unwrap().unwrap_err().primary,
            ProvisionErrorCode::Cancelled
        );
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        fixture.finish().await;

        let replacement = HttpFixture::spawn(vec![
            login_response(),
            token_response("REPLACEMENT.AGENT.JWT"),
            logout_response(),
        ])
        .await;
        let replacement_tunnels = TunnelFixture::new(replacement.port, 0);
        state
            .reprovision(request(), replacement_tunnels.state.clone())
            .await
            .unwrap();
        replacement.finish().await;
        assert_eq!(
            backend.values.lock().unwrap().get(PROJECT_ID).unwrap(),
            b"REPLACEMENT.AGENT.JWT"
        );
        assert!(!state.has_active_operations());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_past_the_hard_wait_remains_tracked_and_observable_until_it_settles() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            delayed_plan(Duration::from_millis(300), token_response(AGENT_TOKEN)),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            ProvisionDeadlines {
                connect: Duration::from_millis(500),
                io: Duration::from_millis(500),
                operation: Duration::from_millis(50),
                cleanup: Duration::from_millis(50),
            },
        );

        let failure = state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::Timeout);
        let status = state.cleanup_status(PROJECT_ID).unwrap();
        assert!(status.active);
        assert!(status.pending_revocation);
        assert_eq!(status.last_error, None);

        fixture.finish().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tracked cleanup did not settle");
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(!state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_owned_provision_and_surfaces_revocation_cleanup_failure() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            ResponsePlan::Hold,
            response(500, b"{}"),
            response(500, b"{}"),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let state = Arc::new(CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        ));
        let operation_state = Arc::clone(&state);
        let operation_tunnels = tunnels.state.clone();
        let operation = tokio::spawn(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        fixture.wait_for_requests(2).await;

        let shutdown = state.shutdown_all(tunnels.state.clone()).await.unwrap_err();
        assert_eq!(shutdown.primary, ProvisionErrorCode::RevocationFailed);
        let operation_failure = operation.await.unwrap().unwrap_err();
        assert_eq!(operation_failure.primary, ProvisionErrorCode::Cancelled);
        assert_eq!(
            operation_failure.cleanup,
            vec![ProvisionErrorCode::RevocationFailed]
        );
        assert!(!state.has_active_operations());
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73, 73]);
        let requests = fixture.finish().await;
        assert_eq!(requests[2].path, "/api/auth/logout");
        assert_eq!(requests[3].path, "/api/auth/logout");
        assert_eq!(
            requests[2].headers.get("authorization"),
            requests[3].headers.get("authorization")
        );
        assert!(state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);

        let retry_fixture = HttpFixture::spawn(vec![logout_response()]).await;
        let retry_tunnels = TunnelFixture::new(retry_fixture.port, 0);
        state
            .shutdown_all(retry_tunnels.state.clone())
            .await
            .unwrap();
        let retry_requests = retry_fixture.finish().await;
        assert_eq!(
            retry_requests[0]
                .headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert!(!state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);

        let replacement = TunnelFixture::new(1, 0);
        let rejected = state
            .provision(request(), replacement.state)
            .await
            .unwrap_err();
        assert_eq!(rejected.primary, ProvisionErrorCode::Cancelled);
        assert_eq!(replacement.creates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_public_waiter_cannot_detach_login_revocation_or_tunnel_cleanup() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            ResponsePlan::Hold,
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let state = Arc::new(CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        ));
        let operation_state = Arc::clone(&state);
        let operation_tunnels = tunnels.state.clone();
        let operation = tokio::spawn(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        fixture.wait_for_requests(2).await;

        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        let requests = fixture.finish().await;
        assert_eq!(requests[2].path, "/api/auth/logout");
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned provisioning task did not settle");
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
        assert_eq!(tunnels.dropped_unclosed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_waiter_after_internal_success_rolls_back_before_owned_work_settles() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        let operation_state = state.clone();
        let operation_tunnels = tunnels.state.clone();
        let mut operation = Box::pin(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        let wake_probe = Arc::new(WakeProbe::default());
        let waker = Waker::from(Arc::clone(&wake_probe));
        let mut context = Context::from_waker(&waker);
        assert!(matches!(
            operation.as_mut().poll(&mut context),
            Poll::Pending
        ));
        let control = state
            .inner
            .active
            .lock()
            .unwrap()
            .get(PROJECT_ID)
            .and_then(|active| active.control.as_ref())
            .cloned()
            .unwrap();

        fixture.finish().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while control.lifecycle.load(Ordering::Acquire) != OPERATION_HANDOFF
                || wake_probe.count.load(Ordering::SeqCst) == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("published result did not wake the unpolled public waiter");
        assert_eq!(
            backend.values.lock().unwrap().get(PROJECT_ID).unwrap(),
            AGENT_TOKEN.as_bytes()
        );

        drop(operation);
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() || !state.inner.tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("caller-drop rollback did not settle");
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(state.inner.rollback_reservations.lock().unwrap().is_empty());
        assert_eq!(*tunnels.close_attempts.lock().unwrap(), vec![73]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_and_tunnel_cleanup_failures_are_visible_and_tunnel_owner_is_retryable() {
        let revoke_fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            response(500, b"{}"),
        ])
        .await;
        let revoke_tunnels = TunnelFixture::new(revoke_fixture.port, 0);
        let revoke_backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            revoke_backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        let failure = state
            .provision(request(), revoke_tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::RevocationFailed);
        assert_eq!(
            revoke_backend
                .values
                .lock()
                .unwrap()
                .get(PROJECT_ID)
                .unwrap(),
            AGENT_TOKEN.as_bytes()
        );
        assert_eq!(*revoke_tunnels.close_attempts.lock().unwrap(), vec![73]);
        revoke_fixture.finish().await;

        let close_fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            logout_response(),
        ])
        .await;
        let close_tunnels = TunnelFixture::new(close_fixture.port, 1);
        let close_state = CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        );
        let failure = close_state
            .provision(request(), close_tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::TunnelCleanupFailed);
        assert_eq!(close_tunnels.creates.load(Ordering::SeqCst), 1);
        assert_eq!(close_tunnels.dropped_unclosed.load(Ordering::SeqCst), 0);
        assert_eq!(*close_tunnels.close_attempts.lock().unwrap(), vec![73]);
        assert_eq!(close_tunnels.close_failures.load(Ordering::SeqCst), 0);
        let status = close_state.cleanup_status(PROJECT_ID).unwrap();
        assert!(!status.pending_revocation);
        assert!(status.pending_tunnel_cleanup);
        close_state
            .retry_cleanup(PROJECT_ID, close_tunnels.state.clone())
            .await
            .unwrap();
        assert_eq!(*close_tunnels.close_attempts.lock().unwrap(), vec![73, 73]);
        assert_eq!(close_tunnels.dropped_unclosed.load(Ordering::SeqCst), 0);
        let status = close_state.cleanup_status(PROJECT_ID).unwrap();
        assert!(!status.pending_revocation);
        assert!(!status.pending_tunnel_cleanup);
        close_fixture.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reprovision_retries_pending_revocation_with_the_same_login_token() {
        let failed_logout = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            response(500, b"{}"),
        ])
        .await;
        let first_tunnels = TunnelFixture::new(failed_logout.port, 0);
        let state = CredentialState::with_backend_and_deadlines(
            Arc::new(MemoryCredentialBackend::default()),
            deadlines(Duration::from_secs(1)),
        );
        let failure = state
            .provision(request(), first_tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::RevocationFailed);
        failed_logout.finish().await;

        let retry = HttpFixture::spawn(vec![
            logout_response(),
            login_response(),
            token_response("REPLACEMENT.AGENT.JWT"),
            logout_response(),
        ])
        .await;
        let retry_tunnels = TunnelFixture::new(retry.port, 0);
        let status = state
            .reprovision(request(), retry_tunnels.state.clone())
            .await
            .unwrap();
        assert!(status.provisioned);
        let requests = retry.finish().await;
        assert_eq!(requests[0].path, "/api/auth/logout");
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert_eq!(requests[1].path, "/api/user/login");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_removes_agent_token_and_retries_pending_revocation() {
        let failed_logout = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            response(500, b"{}"),
        ])
        .await;
        let first_tunnels = TunnelFixture::new(failed_logout.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        state
            .provision(request(), first_tunnels.state.clone())
            .await
            .unwrap_err();
        failed_logout.finish().await;

        let retry = HttpFixture::spawn(vec![logout_response()]).await;
        let retry_tunnels = TunnelFixture::new(retry.port, 0);
        state
            .delete(PROJECT_ID, retry_tunnels.state.clone())
            .await
            .unwrap();
        let requests = retry.finish().await;
        assert_eq!(requests[0].path, "/api/auth/logout");
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(!state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_logout_does_not_block_agent_token_deletion_or_lose_revocation_owner() {
        let failed_logout = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            response(500, b"{}"),
            response(500, b"{}"),
        ])
        .await;
        let tunnels = TunnelFixture::new(failed_logout.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();
        assert!(backend.values.lock().unwrap().contains_key(PROJECT_ID));

        let failure = state
            .delete(PROJECT_ID, tunnels.state.clone())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::RevocationFailed);
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);
        let requests = failed_logout.finish().await;
        assert_eq!(
            requests[2].headers.get("authorization"),
            requests[3].headers.get("authorization")
        );

        let retry = HttpFixture::spawn(vec![logout_response()]).await;
        let retry_tunnels = TunnelFixture::new(retry.port, 0);
        state
            .retry_cleanup(PROJECT_ID, retry_tunnels.state.clone())
            .await
            .unwrap();
        let requests = retry.finish().await;
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert!(!state.cleanup_status(PROJECT_ID).unwrap().pending_revocation);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_cleanup_after_failed_logout_is_idempotent_and_reports_owned_retry() {
        let failed_logout = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            response(500, b"{}"),
            response(500, b"{}"),
        ])
        .await;
        let tunnels = TunnelFixture::new(failed_logout.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        state
            .provision(request(), tunnels.state.clone())
            .await
            .unwrap_err();

        let rollback = state
            .cancel_and_cleanup(PROJECT_ID, tunnels.state.clone())
            .await
            .unwrap();
        assert!(rollback.credential_deleted);
        assert!(!rollback.active);
        assert!(rollback.pending_revocation);
        assert!(!rollback.pending_tunnel_cleanup);
        assert_eq!(
            rollback.last_error,
            Some(ProvisionErrorCode::RevocationFailed)
        );
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        let requests = failed_logout.finish().await;
        assert_eq!(
            requests[2].headers.get("authorization"),
            requests[3].headers.get("authorization")
        );

        let retry = HttpFixture::spawn(vec![logout_response()]).await;
        let retry_tunnels = TunnelFixture::new(retry.port, 0);
        let rollback = state
            .cancel_and_cleanup(PROJECT_ID, retry_tunnels.state.clone())
            .await
            .unwrap();
        assert!(rollback.credential_deleted);
        assert!(!rollback.active);
        assert!(!rollback.pending_revocation);
        assert!(!rollback.pending_tunnel_cleanup);
        let requests = retry.finish().await;
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer LOGIN.JWT.SENTINEL")
        );
        assert!(!state.has_active_operations());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_agent_deletion_is_pending_blocks_replacement_and_retries_exact_project() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        backend
            .values
            .lock()
            .unwrap()
            .insert(PROJECT_ID.to_owned(), AGENT_TOKEN.as_bytes().to_vec());
        *backend.delete_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );

        let failure = state
            .cancel_and_cleanup(PROJECT_ID, TunnelState::new())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::CredentialAccess);
        assert!(backend.values.lock().unwrap().contains_key(PROJECT_ID));
        let status = serde_json::to_value(state.cleanup_status(PROJECT_ID).unwrap()).unwrap();
        assert_eq!(status.get("pendingAgentDeletion"), Some(&Value::Bool(true)));
        assert_eq!(status.get("lastError"), Some(&json!("credential_access")));
        let encoded = backend
            .pending_agent_deletions
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        let persisted: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(persisted.get("version"), Some(&json!(1)));
        assert_eq!(persisted["deletions"][0]["projectId"], PROJECT_ID);
        assert_eq!(persisted["deletions"][0]["lastError"], "credential_access");
        let deletion_generation = persisted["deletions"][0]["generation"]
            .as_str()
            .unwrap()
            .to_owned();
        let rendered = String::from_utf8(encoded).unwrap();
        assert!(!rendered.contains(AGENT_TOKEN));
        assert!(!rendered.contains(LOGIN_TOKEN));

        state
            .cancel_and_cleanup(PROJECT_ID, TunnelState::new())
            .await
            .unwrap_err();
        let repeated: Value = serde_json::from_slice(
            &backend
                .pending_agent_deletions
                .lock()
                .unwrap()
                .clone()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(repeated["deletions"][0]["generation"], deletion_generation);

        let replacement = TunnelFixture::new(1, 0);
        let rejected = state
            .reprovision(request(), replacement.state)
            .await
            .unwrap_err();
        assert_eq!(rejected.primary, ProvisionErrorCode::AlreadyRunning);
        assert_eq!(replacement.creates.load(Ordering::SeqCst), 0);

        *backend.delete_failure.lock().unwrap() = None;
        let status = state
            .retry_cleanup(PROJECT_ID, TunnelState::new())
            .await
            .unwrap();
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert_eq!(
            serde_json::to_value(status)
                .unwrap()
                .get("pendingAgentDeletion"),
            Some(&Value::Bool(false))
        );
        assert!(!state.has_active_operations());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !state.inner.tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retry cleanup task did not settle");
        assert!(state.inner.rollback_reservations.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pending_agent_deletion_survives_restart_and_shutdown_refuses_until_retry_succeeds() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        backend
            .values
            .lock()
            .unwrap()
            .insert(PROJECT_ID.to_owned(), AGENT_TOKEN.as_bytes().to_vec());
        *backend.delete_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        let first = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        first
            .cancel_and_cleanup(PROJECT_ID, TunnelState::new())
            .await
            .unwrap_err();
        drop(first);

        let restored = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        let status = serde_json::to_value(restored.cleanup_status(PROJECT_ID).unwrap()).unwrap();
        assert_eq!(status.get("pendingAgentDeletion"), Some(&Value::Bool(true)));
        let failure = restored.shutdown_all(TunnelState::new()).await.unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::CredentialAccess);
        assert!(backend.values.lock().unwrap().contains_key(PROJECT_ID));

        *backend.delete_failure.lock().unwrap() = None;
        restored.shutdown_all(TunnelState::new()).await.unwrap();
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        let status = serde_json::to_value(restored.cleanup_status(PROJECT_ID).unwrap()).unwrap();
        assert_eq!(
            status.get("pendingAgentDeletion"),
            Some(&Value::Bool(false))
        );
        assert!(!restored.has_active_operations());
        assert!(restored.inner.tasks.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn caller_drop_deletion_failure_keeps_durable_owner_until_retry() {
        let fixture = HttpFixture::spawn(vec![
            login_response(),
            token_response(AGENT_TOKEN),
            logout_response(),
        ])
        .await;
        let tunnels = TunnelFixture::new(fixture.port, 0);
        let backend = Arc::new(MemoryCredentialBackend::default());
        *backend.delete_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        let operation_state = state.clone();
        let operation_tunnels = tunnels.state.clone();
        let mut operation = Box::pin(async move {
            operation_state
                .provision(request(), operation_tunnels)
                .await
        });
        std::future::poll_fn(|context| match operation.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("provisioning completed during its initial poll"),
        })
        .await;
        let control = state
            .inner
            .active
            .lock()
            .unwrap()
            .get(PROJECT_ID)
            .and_then(|active| active.control.as_ref())
            .cloned()
            .unwrap();
        fixture.finish().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while control.lifecycle.load(Ordering::Acquire) != OPERATION_HANDOFF {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("internal result did not reach the public handoff");

        drop(operation);
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.has_active_operations() || !state.inner.tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("failed caller-drop rollback did not settle");
        assert!(backend.values.lock().unwrap().contains_key(PROJECT_ID));
        let status = state.cleanup_status(PROJECT_ID).unwrap();
        assert!(status.pending_agent_deletion);
        assert_eq!(
            status.last_error,
            Some(ProvisionErrorCode::CredentialAccess)
        );

        *backend.delete_failure.lock().unwrap() = None;
        state
            .retry_cleanup(PROJECT_ID, TunnelState::new())
            .await
            .unwrap();
        assert!(!backend.values.lock().unwrap().contains_key(PROJECT_ID));
        assert!(
            !state
                .cleanup_status(PROJECT_ID)
                .unwrap()
                .pending_agent_deletion
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while !state.inner.tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retry cleanup task did not settle");
        assert!(state.inner.tasks.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_and_reprovision_are_bounded_and_replace_only_the_project_secret() {
        let backend = Arc::new(MemoryCredentialBackend::default());
        let state = CredentialState::with_backend_and_deadlines(
            backend.clone(),
            deadlines(Duration::from_secs(1)),
        );
        for token in ["FIRST.AGENT.JWT", "SECOND.AGENT.JWT"] {
            let fixture = HttpFixture::spawn(vec![
                login_response(),
                token_response(token),
                logout_response(),
            ])
            .await;
            let tunnels = TunnelFixture::new(fixture.port, 0);
            state
                .reprovision(request(), tunnels.state.clone())
                .await
                .unwrap();
            fixture.finish().await;
        }
        assert_eq!(
            backend.values.lock().unwrap().get(PROJECT_ID).unwrap(),
            b"SECOND.AGENT.JWT"
        );
        state.delete(PROJECT_ID, TunnelState::new()).await.unwrap();
        assert_eq!(
            state.token_for_project(PROJECT_ID).unwrap_err(),
            AgentErrorCode::AuthRequired
        );

        *backend.delete_failure.lock().unwrap() = Some(CredentialBackendFailure::Access);
        let failure = state
            .delete(PROJECT_ID, TunnelState::new())
            .await
            .unwrap_err();
        assert_eq!(failure.primary, ProvisionErrorCode::CredentialAccess);
    }
}
