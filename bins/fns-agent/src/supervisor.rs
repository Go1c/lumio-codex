//! Bounded child-process ownership for a complete fns-agent daemon.

use crate::protocol::{
    ParentFrame, SecretBytes, WorkerFrame, read_worker_frame_optional, write_parent_frame,
};
use crate::{AgentConfig, AgentError, AgentErrorCode, AgentPhase, AgentStatus};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

const RPC_TIMEOUT: Duration = Duration::from_secs(7);
const MAX_PENDING_RPCS: usize = 64;
const EVENT_CHANNEL_CAPACITY: usize = 8;

#[derive(Clone)]
pub struct AgentCommand {
    program: PathBuf,
    args: Vec<OsString>,
}

impl AgentCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

impl std::fmt::Debug for AgentCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentCommand")
            .field("program", &self.program)
            .field("argument_count", &self.args.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AgentProcessOptions {
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
}

impl Default for AgentProcessOptions {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<fns_protocol::RequestId, PendingRpc>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RpcKind {
    ListConflicts,
    ResolveConflict,
}

enum RpcResponse {
    Conflicts(Vec<fns_sync_core::ConflictView>),
    Resolution(fns_sync_core::ConflictResolutionReceipt),
}

struct PendingRpc {
    kind: RpcKind,
    response: oneshot::Sender<Result<RpcResponse, AgentErrorCode>>,
}

struct PendingGuard {
    request_id: fns_protocol::RequestId,
    pending: PendingMap,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        lock_pending(&self.pending).remove(&self.request_id);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessEvent {
    Ready,
    Stopped,
    Fatal(AgentErrorCode),
    ProtocolFailure,
    StreamClosed,
}

pub struct AgentProcess {
    child: Child,
    control: Option<ChildStdin>,
    events: mpsc::Receiver<ProcessEvent>,
    event_reader: Option<tokio::task::JoinHandle<()>>,
    pending: PendingMap,
    poisoned: Arc<AtomicBool>,
    state_dir: PathBuf,
    workspace_id: fns_protocol::WorkspaceId,
    shutdown_timeout: Duration,
    reaped: Option<ExitStatus>,
    control_closed: bool,
}

impl std::fmt::Debug for AgentProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentProcess")
            .field("pid", &self.id())
            .field("reaped", &self.reaped.is_some())
            .finish()
    }
}

impl AgentProcess {
    pub async fn spawn(
        command: AgentCommand,
        config: AgentConfig,
        token: fns_platform::SecretToken,
        options: AgentProcessOptions,
    ) -> Result<Self, AgentError> {
        let state_dir = config.state_dir.clone();
        let workspace_id = config.workspace_id;
        let mut child = match Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return Err(AgentError::new(AgentErrorCode::SpawnFailed)),
        };
        let Some(control) = child.stdin.take() else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            persist_fatal_after_reap(&state_dir, workspace_id, AgentErrorCode::SpawnFailed);
            return Err(AgentError::after_reap(AgentErrorCode::SpawnFailed));
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            persist_fatal_after_reap(&state_dir, workspace_id, AgentErrorCode::SpawnFailed);
            return Err(AgentError::after_reap(AgentErrorCode::SpawnFailed));
        };
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let poisoned = Arc::new(AtomicBool::new(false));
        let (event_tx, events) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let event_reader = tokio::spawn(read_events(
            stdout,
            Arc::clone(&pending),
            Arc::clone(&poisoned),
            event_tx,
        ));
        let mut process = Self {
            child,
            control: Some(control),
            events,
            event_reader: Some(event_reader),
            pending,
            poisoned,
            state_dir,
            workspace_id,
            shutdown_timeout: options.shutdown_timeout,
            reaped: None,
            control_closed: false,
        };

        let secret = token.with_exposed(|bytes| SecretBytes::new(bytes.to_vec()));
        let bootstrap = ParentFrame::Bootstrap {
            config: Box::new(config),
            token: secret,
        };
        let write_result = match process.control.as_mut() {
            Some(control) => write_parent_frame(control, &bootstrap).await,
            None => Err(AgentError::new(AgentErrorCode::Protocol)),
        };
        if write_result.is_err() {
            process.terminate_and_reap().await?;
            persist_fatal_after_reap(
                &process.state_dir,
                process.workspace_id,
                AgentErrorCode::Protocol,
            );
            return Err(AgentError::after_reap(AgentErrorCode::Protocol));
        }

        match tokio::time::timeout(options.startup_timeout, process.await_ready()).await {
            Ok(Ok(())) => Ok(process),
            Ok(Err(error)) => {
                let code = error.code();
                process.terminate_and_reap().await?;
                if code != AgentErrorCode::AlreadyRunning {
                    process.persist_fatal_after_reap(code);
                }
                Err(AgentError::after_reap(code))
            }
            Err(_) => {
                process.terminate_and_reap().await?;
                process.persist_fatal_after_reap(AgentErrorCode::StartupTimeout);
                Err(AgentError::after_reap(AgentErrorCode::StartupTimeout))
            }
        }
    }

    pub fn id(&self) -> Option<u32> {
        if self.reaped.is_some() {
            None
        } else {
            self.child.id()
        }
    }

    pub fn is_reaped(&self) -> bool {
        self.reaped.is_some()
    }

    pub fn close_control(&mut self) {
        self.control.take();
        self.control_closed = true;
    }

    /// Force termination for explicit crash-recovery workflows. The method
    /// does not return until the child has been reaped.
    pub async fn force_kill_and_reap(&mut self) -> Result<ExitStatus, AgentError> {
        if let Some(status) = self.reaped {
            return Ok(status);
        }
        self.close_control();
        let _ = self.child.start_kill();
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| AgentError::new(AgentErrorCode::Core))?;
        self.reaped = Some(status);
        self.poisoned.store(true, Ordering::Release);
        fail_pending(&self.pending, AgentErrorCode::AbnormalExit);
        self.finish_event_reader().await;
        self.persist_fatal_after_reap(AgentErrorCode::AbnormalExit);
        Ok(status)
    }

    pub async fn list_conflicts(&mut self) -> Result<Vec<fns_sync_core::ConflictView>, AgentError> {
        let request_id = new_request_id();
        match self
            .rpc(
                request_id,
                RpcKind::ListConflicts,
                ParentFrame::ListConflicts { request_id },
            )
            .await?
        {
            RpcResponse::Conflicts(conflicts) => Ok(conflicts),
            RpcResponse::Resolution(_) => self.fail_protocol().await,
        }
    }

    pub async fn resolve_conflict(
        &mut self,
        conflict_id: fns_protocol::ConflictId,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
        choice: fns_protocol::WorkspaceConflictChoice,
    ) -> Result<fns_sync_core::ConflictResolutionReceipt, AgentError> {
        let request_id = new_request_id();
        match self
            .rpc(
                request_id,
                RpcKind::ResolveConflict,
                ParentFrame::ResolveConflict {
                    request_id,
                    conflict_id,
                    conflict_revision,
                    choice,
                },
            )
            .await?
        {
            RpcResponse::Resolution(receipt) => Ok(receipt),
            RpcResponse::Conflicts(_) => self.fail_protocol().await,
        }
    }

    pub async fn shutdown(&mut self) -> Result<(), AgentError> {
        if self.reaped.is_some() {
            return Ok(());
        }

        if let Some(control) = self.control.as_mut()
            && write_parent_frame(control, &ParentFrame::Shutdown)
                .await
                .is_err()
        {
            self.close_control();
        }

        match tokio::time::timeout(self.shutdown_timeout, self.await_stopped()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                let code = error.code();
                self.terminate_and_reap().await?;
                self.persist_fatal_after_reap(code);
                Err(AgentError::after_reap(code))
            }
            Err(_) => {
                self.terminate_and_reap().await?;
                self.persist_fatal_after_reap(AgentErrorCode::ShutdownTimeout);
                Err(AgentError::after_reap(AgentErrorCode::ShutdownTimeout))
            }
        }
    }

    pub async fn wait(&mut self) -> Result<ExitStatus, AgentError> {
        if let Some(status) = self.reaped {
            return Ok(status);
        }

        match self.next_event().await? {
            ProcessEvent::Stopped => {
                let status = self.reap_expected_exit().await?;
                if status.success() {
                    Ok(status)
                } else {
                    self.persist_fatal_after_reap(AgentErrorCode::AbnormalExit);
                    Err(AgentError::after_reap(AgentErrorCode::AbnormalExit))
                }
            }
            ProcessEvent::Fatal(code) => {
                self.terminate_and_reap().await?;
                self.persist_fatal_after_reap(code);
                Err(AgentError::after_reap(code))
            }
            ProcessEvent::ProtocolFailure => {
                if self.control_closed {
                    let status = self.reap_expected_exit().await?;
                    if status.success() {
                        return Ok(status);
                    }
                    let code = exit_error_code(status);
                    self.persist_fatal_after_reap(code);
                    return Err(AgentError::after_reap(code));
                }
                self.fail_protocol().await
            }
            ProcessEvent::StreamClosed => {
                let status = self.reap_expected_exit().await?;
                if self.control_closed && status.success() {
                    return Ok(status);
                }
                let code = exit_error_code(status);
                self.persist_fatal_after_reap(code);
                Err(AgentError::after_reap(code))
            }
            ProcessEvent::Ready => self.fail_protocol().await,
        }
    }

    async fn await_ready(&mut self) -> Result<(), AgentError> {
        match self.next_event().await? {
            ProcessEvent::Ready => Ok(()),
            ProcessEvent::Fatal(code) => Err(AgentError::new(code)),
            ProcessEvent::Stopped | ProcessEvent::ProtocolFailure => {
                Err(AgentError::new(AgentErrorCode::Protocol))
            }
            ProcessEvent::StreamClosed => Err(AgentError::new(AgentErrorCode::AbnormalExit)),
        }
    }

    async fn await_stopped(&mut self) -> Result<(), AgentError> {
        match self.next_event().await? {
            ProcessEvent::Stopped => {
                let status = self.reap_expected_exit().await?;
                if status.success() {
                    Ok(())
                } else {
                    Err(AgentError::after_reap(AgentErrorCode::AbnormalExit))
                }
            }
            ProcessEvent::Fatal(code) => Err(AgentError::new(code)),
            ProcessEvent::Ready | ProcessEvent::ProtocolFailure | ProcessEvent::StreamClosed => {
                Err(AgentError::new(AgentErrorCode::Protocol))
            }
        }
    }

    async fn rpc(
        &mut self,
        request_id: fns_protocol::RequestId,
        kind: RpcKind,
        frame: ParentFrame,
    ) -> Result<RpcResponse, AgentError> {
        if self.reaped.is_some() || self.poisoned.load(Ordering::Acquire) {
            return self.fail_protocol().await;
        }
        let (response_tx, response_rx) = oneshot::channel();
        let guard = match register_pending(&self.pending, request_id, kind, response_tx) {
            Ok(guard) => guard,
            Err(AgentErrorCode::Protocol) => {
                self.poisoned.store(true, Ordering::Release);
                return self.fail_protocol().await;
            }
            Err(code) => return Err(AgentError::new(code)),
        };
        let write_result = match self.control.as_mut() {
            Some(control) => write_parent_frame(control, &frame).await,
            None => Err(AgentError::new(AgentErrorCode::Protocol)),
        };
        if write_result.is_err() {
            drop(guard);
            return self.fail_protocol().await;
        }

        let result = tokio::time::timeout(RPC_TIMEOUT, response_rx).await;
        drop(guard);
        match result {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(code))) if self.poisoned.load(Ordering::Acquire) => {
                self.terminate_and_reap().await?;
                self.persist_fatal_after_reap(code);
                Err(AgentError::after_reap(code))
            }
            Ok(Ok(Err(code))) => Err(AgentError::new(code)),
            Ok(Err(_)) => self.fail_protocol().await,
            Err(_) => {
                self.terminate_and_reap().await?;
                self.persist_fatal_after_reap(AgentErrorCode::RequestTimeout);
                Err(AgentError::after_reap(AgentErrorCode::RequestTimeout))
            }
        }
    }

    async fn next_event(&mut self) -> Result<ProcessEvent, AgentError> {
        self.events
            .recv()
            .await
            .ok_or_else(|| AgentError::new(AgentErrorCode::Protocol))
    }

    async fn fail_protocol<T>(&mut self) -> Result<T, AgentError> {
        self.poisoned.store(true, Ordering::Release);
        fail_pending(&self.pending, AgentErrorCode::Protocol);
        self.terminate_and_reap().await?;
        self.persist_fatal_after_reap(AgentErrorCode::Protocol);
        Err(AgentError::after_reap(AgentErrorCode::Protocol))
    }

    async fn reap_expected_exit(&mut self) -> Result<ExitStatus, AgentError> {
        if let Some(status) = self.reaped {
            return Ok(status);
        }
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| AgentError::new(AgentErrorCode::Core))?;
        self.reaped = Some(status);
        self.finish_event_reader().await;
        Ok(status)
    }

    async fn terminate_and_reap(&mut self) -> Result<(), AgentError> {
        self.close_control();
        self.poisoned.store(true, Ordering::Release);
        fail_pending(&self.pending, AgentErrorCode::AbnormalExit);
        if self.reaped.is_some() {
            self.finish_event_reader().await;
            return Ok(());
        }
        let _ = self.child.start_kill();
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| AgentError::new(AgentErrorCode::Core))?;
        self.reaped = Some(status);
        self.finish_event_reader().await;
        Ok(())
    }

    async fn finish_event_reader(&mut self) {
        if let Some(reader) = self.event_reader.take() {
            reader.abort();
            let _ = reader.await;
        }
    }

    fn persist_fatal_after_reap(&self, code: AgentErrorCode) {
        debug_assert!(self.reaped.is_some());
        persist_fatal_after_reap(&self.state_dir, self.workspace_id, code);
    }
}

async fn read_events(
    mut stdout: ChildStdout,
    pending: PendingMap,
    poisoned: Arc<AtomicBool>,
    events: mpsc::Sender<ProcessEvent>,
) {
    let mut ready = false;
    loop {
        let frame = match read_worker_frame_optional(&mut stdout).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                poisoned.store(true, Ordering::Release);
                fail_pending(&pending, AgentErrorCode::Protocol);
                let _ = events.send(ProcessEvent::StreamClosed).await;
                return;
            }
            Err(_) => {
                poison_reader(&pending, &poisoned, &events, AgentErrorCode::Protocol).await;
                return;
            }
        };
        match frame {
            WorkerFrame::Ready if !ready => {
                ready = true;
                if events.send(ProcessEvent::Ready).await.is_err() {
                    return;
                }
            }
            WorkerFrame::Ready => {
                poison_reader(&pending, &poisoned, &events, AgentErrorCode::Protocol).await;
                return;
            }
            WorkerFrame::Stopped if ready => {
                poisoned.store(true, Ordering::Release);
                fail_pending(&pending, AgentErrorCode::AbnormalExit);
                let _ = events.send(ProcessEvent::Stopped).await;
                return;
            }
            WorkerFrame::Fatal { code } => {
                poisoned.store(true, Ordering::Release);
                fail_pending(&pending, code);
                let _ = events.send(ProcessEvent::Fatal(code)).await;
                return;
            }
            WorkerFrame::Stopped => {
                poison_reader(&pending, &poisoned, &events, AgentErrorCode::Protocol).await;
                return;
            }
            response @ (WorkerFrame::ConflictsListed { .. }
            | WorkerFrame::ConflictResolved { .. }
            | WorkerFrame::RequestFailed { .. })
                if ready =>
            {
                if route_response(&pending, response).is_err() {
                    poison_reader(&pending, &poisoned, &events, AgentErrorCode::Protocol).await;
                    return;
                }
            }
            WorkerFrame::ConflictsListed { .. }
            | WorkerFrame::ConflictResolved { .. }
            | WorkerFrame::RequestFailed { .. } => {
                poison_reader(&pending, &poisoned, &events, AgentErrorCode::Protocol).await;
                return;
            }
        }
    }
}

fn route_response(pending: &PendingMap, frame: WorkerFrame) -> Result<(), ()> {
    let (request_id, response) = match frame {
        WorkerFrame::ConflictsListed {
            request_id,
            conflicts,
        } => (request_id, Ok(RpcResponse::Conflicts(conflicts))),
        WorkerFrame::ConflictResolved {
            request_id,
            receipt,
        } => (request_id, Ok(RpcResponse::Resolution(receipt))),
        WorkerFrame::RequestFailed { request_id, code } => (request_id, Err(code)),
        WorkerFrame::Ready | WorkerFrame::Stopped | WorkerFrame::Fatal { .. } => return Err(()),
    };
    let Some(request) = lock_pending(pending).remove(&request_id) else {
        return Err(());
    };
    let expected = matches!(
        (&request.kind, &response),
        (RpcKind::ListConflicts, Ok(RpcResponse::Conflicts(_)))
            | (RpcKind::ResolveConflict, Ok(RpcResponse::Resolution(_)))
            | (_, Err(_))
    );
    if !expected {
        let _ = request.response.send(Err(AgentErrorCode::Protocol));
        return Err(());
    }
    let _ = request.response.send(response);
    Ok(())
}

async fn poison_reader(
    pending: &PendingMap,
    poisoned: &AtomicBool,
    events: &mpsc::Sender<ProcessEvent>,
    code: AgentErrorCode,
) {
    poisoned.store(true, Ordering::Release);
    fail_pending(pending, code);
    let event = if code == AgentErrorCode::Protocol {
        ProcessEvent::ProtocolFailure
    } else {
        ProcessEvent::Fatal(code)
    };
    let _ = events.send(event).await;
}

fn fail_pending(pending: &PendingMap, code: AgentErrorCode) {
    let pending = std::mem::take(&mut *lock_pending(pending));
    for (_, request) in pending {
        let _ = request.response.send(Err(code));
    }
}

fn register_pending(
    pending: &PendingMap,
    request_id: fns_protocol::RequestId,
    kind: RpcKind,
    response: oneshot::Sender<Result<RpcResponse, AgentErrorCode>>,
) -> Result<PendingGuard, AgentErrorCode> {
    let mut requests = lock_pending(pending);
    if requests.len() >= MAX_PENDING_RPCS {
        return Err(AgentErrorCode::ResourceLimit);
    }
    match requests.entry(request_id) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(PendingRpc { kind, response });
        }
        std::collections::hash_map::Entry::Occupied(_) => {
            return Err(AgentErrorCode::Protocol);
        }
    }
    Ok(PendingGuard {
        request_id,
        pending: Arc::clone(pending),
    })
}

fn lock_pending(
    pending: &PendingMap,
) -> std::sync::MutexGuard<'_, HashMap<fns_protocol::RequestId, PendingRpc>> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn new_request_id() -> fns_protocol::RequestId {
    fns_protocol::RequestId::parse(&uuid::Uuid::new_v4().to_string())
        .expect("Uuid v4 always has canonical requestId form")
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        self.control.take();
        self.poisoned.store(true, Ordering::Release);
        fail_pending(&self.pending, AgentErrorCode::AbnormalExit);
        if let Some(reader) = self.event_reader.take() {
            reader.abort();
        }
        if self.reaped.is_some() {
            return;
        }

        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.reaped = Some(status);
                self.persist_fatal_after_reap(AgentErrorCode::AbnormalExit);
                return;
            }
            Ok(None) => {}
            Err(_) => {
                eprintln!("fns_agent_drop_reap_failed:stage=probe");
                return;
            }
        }
        if self.child.start_kill().is_err() {
            eprintln!("fns_agent_drop_reap_failed:stage=kill");
            return;
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = Some(status);
                    self.persist_fatal_after_reap(AgentErrorCode::AbnormalExit);
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => {
                    eprintln!("fns_agent_drop_reap_failed:stage=timeout");
                    return;
                }
                Err(_) => {
                    eprintln!("fns_agent_drop_reap_failed:stage=wait");
                    return;
                }
            }
        }
    }
}

fn persist_fatal_after_reap(
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    code: AgentErrorCode,
) {
    #[cfg(test)]
    run_before_fatal_persist_hook(state_dir);

    persist_fatal(state_dir, workspace_id, code);
}

fn persist_fatal(state_dir: &Path, workspace_id: fns_protocol::WorkspaceId, code: AgentErrorCode) {
    let _lease = match fns_platform::StateDirLease::acquire(state_dir) {
        Ok(lease) => lease,
        Err(error) if error.code() == fns_platform::PlatformErrorCode::AlreadyRunning => return,
        Err(error) => {
            observe_fatal_persist_failure(state_dir, "lease", error.code());
            return;
        }
    };
    let path = state_dir.join("runtime-status.json");
    let mut status = AgentStatus::read_or_stored(&path, workspace_id);
    status.running = false;
    status.phase = AgentPhase::Fatal;
    status.pid = None;
    status.connected = false;
    status.last_error_code = Some(code);
    status.updated_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64);
    if status.write_to(&path).is_err() {
        observe_fatal_persist_failure(
            state_dir,
            "status_write",
            fns_platform::PlatformErrorCode::Io,
        );
    }
}

fn observe_fatal_persist_failure(
    state_dir: &Path,
    stage: &'static str,
    code: fns_platform::PlatformErrorCode,
) {
    let diagnostic = format!(
        "fns_agent_fatal_status_persist_failed:stage={stage}:code={}",
        stable_platform_code(code)
    );
    eprintln!("{diagnostic}");
    #[cfg(test)]
    run_fatal_persist_failure_hook(state_dir, diagnostic);
    #[cfg(not(test))]
    let _ = state_dir;
}

fn stable_platform_code(code: fns_platform::PlatformErrorCode) -> &'static str {
    match code {
        fns_platform::PlatformErrorCode::UnsupportedPlatform => "unsupported_platform",
        fns_platform::PlatformErrorCode::InvalidProjectId => "invalid_project_id",
        fns_platform::PlatformErrorCode::InvalidCredentialPath => "invalid_credential_path",
        fns_platform::PlatformErrorCode::CredentialAccess => "credential_access",
        fns_platform::PlatformErrorCode::CredentialInteractionNotAllowed => {
            "credential_interaction_not_allowed"
        }
        fns_platform::PlatformErrorCode::InvalidFileType => "invalid_file_type",
        fns_platform::PlatformErrorCode::InsecurePermissions => "insecure_permissions",
        fns_platform::PlatformErrorCode::WrongOwner => "wrong_owner",
        fns_platform::PlatformErrorCode::InvalidSecret => "invalid_secret",
        fns_platform::PlatformErrorCode::AlreadyRunning => "already_running",
        fns_platform::PlatformErrorCode::CorruptLock => "corrupt_lock",
        fns_platform::PlatformErrorCode::Io => "io",
    }
}

#[cfg(test)]
type BeforeFatalPersistHook = std::sync::Arc<dyn Fn() + Send + Sync>;
#[cfg(test)]
type FatalPersistFailureHook = std::sync::Arc<dyn Fn(String) + Send + Sync>;

#[cfg(test)]
static BEFORE_FATAL_PERSIST_HOOKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, BeforeFatalPersistHook>>,
> = std::sync::OnceLock::new();
#[cfg(test)]
static FATAL_PERSIST_FAILURE_HOOKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, FatalPersistFailureHook>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
fn before_fatal_persist_hooks()
-> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, BeforeFatalPersistHook>> {
    BEFORE_FATAL_PERSIST_HOOKS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn fatal_persist_failure_hooks()
-> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, FatalPersistFailureHook>> {
    FATAL_PERSIST_FAILURE_HOOKS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn run_before_fatal_persist_hook(state_dir: &Path) {
    let hook = before_fatal_persist_hooks()
        .lock()
        .unwrap()
        .get(state_dir)
        .cloned();
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
fn run_fatal_persist_failure_hook(state_dir: &Path, diagnostic: String) {
    let hook = fatal_persist_failure_hooks()
        .lock()
        .unwrap()
        .get(state_dir)
        .cloned();
    if let Some(hook) = hook {
        hook(diagnostic);
    }
}

fn exit_error_code(status: ExitStatus) -> AgentErrorCode {
    if status.code() == Some(AgentError::new(AgentErrorCode::ShutdownTimeout).exit_code()) {
        AgentErrorCode::ShutdownTimeout
    } else {
        AgentErrorCode::AbnormalExit
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    struct HookGuard {
        state_dir: PathBuf,
    }

    struct FailureHookGuard {
        state_dir: PathBuf,
    }

    impl Drop for HookGuard {
        fn drop(&mut self) {
            before_fatal_persist_hooks()
                .lock()
                .unwrap()
                .remove(&self.state_dir);
        }
    }

    impl Drop for FailureHookGuard {
        fn drop(&mut self) {
            fatal_persist_failure_hooks()
                .lock()
                .unwrap()
                .remove(&self.state_dir);
        }
    }

    enum FailureKind {
        TimedOut,
        Failed,
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_child_cannot_overwrite_successor_live_status() {
        for _ in 0..8 {
            assert_successor_status_wins(FailureKind::TimedOut).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_child_cannot_overwrite_successor_live_status() {
        for _ in 0..8 {
            assert_successor_status_wins(FailureKind::Failed).await;
        }
    }

    async fn assert_successor_status_wins(kind: FailureKind) {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let state_dir = config.state_dir.clone();
        let workspace_id = config.workspace_id;
        let (reached_tx, reached_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let resume_rx = Arc::new(Mutex::new(resume_rx));
        let hook: BeforeFatalPersistHook = Arc::new(move || {
            reached_tx.send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        });
        before_fatal_persist_hooks()
            .lock()
            .unwrap()
            .insert(state_dir.clone(), hook);
        let _hook_guard = HookGuard {
            state_dir: state_dir.clone(),
        };

        let (mode, expected_code, startup_timeout) = match kind {
            FailureKind::TimedOut => (
                "never-ready",
                AgentErrorCode::StartupTimeout,
                Duration::from_millis(500),
            ),
            FailureKind::Failed => (
                "fatal-before-ready",
                AgentErrorCode::Core,
                Duration::from_secs(5),
            ),
        };
        let failure = tokio::spawn(AgentProcess::spawn(
            fixture_command(mode),
            config,
            test_token(),
            AgentProcessOptions {
                startup_timeout,
                shutdown_timeout: Duration::from_millis(250),
            },
        ));

        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .expect("old supervisor did not reach fatal persistence");
        let successor_lease = fns_platform::StateDirLease::acquire(&state_dir).unwrap();
        let mut live = AgentStatus::stopped(workspace_id);
        live.running = true;
        live.phase = AgentPhase::Online;
        live.pid = Some(std::process::id());
        live.connected = true;
        live.updated_at_ms = 42;
        let status_path = state_dir.join("runtime-status.json");
        live.write_to(&status_path).unwrap();

        resume_tx.send(()).unwrap();
        let error = failure.await.unwrap().unwrap_err();
        assert_eq!(error.code(), expected_code);
        assert!(error.reaped());
        let stored: AgentStatus =
            serde_json::from_slice(&std::fs::read(status_path).unwrap()).unwrap();
        assert_eq!(stored, live);
        drop(successor_lease);
    }

    #[test]
    fn credential_interaction_failure_has_a_stable_diagnostic_code() {
        assert_eq!(
            stable_platform_code(fns_platform::PlatformErrorCode::CredentialInteractionNotAllowed),
            "credential_interaction_not_allowed"
        );
    }

    #[test]
    fn fatal_persist_lease_failure_emits_a_stable_path_free_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state-as-file");
        std::fs::write(&state_dir, b"not a directory").unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&observations);
        let hook: FatalPersistFailureHook = Arc::new(move |diagnostic| {
            captured.lock().unwrap().push(diagnostic);
        });
        fatal_persist_failure_hooks()
            .lock()
            .unwrap()
            .insert(state_dir.clone(), hook);
        let _guard = FailureHookGuard {
            state_dir: state_dir.clone(),
        };

        persist_fatal_after_reap(
            &state_dir,
            test_config(dir.path()).workspace_id,
            AgentErrorCode::AbnormalExit,
        );

        assert_eq!(
            *observations.lock().unwrap(),
            ["fns_agent_fatal_status_persist_failed:stage=lease:code=io"]
        );
    }

    #[test]
    fn fatal_status_write_failure_emits_a_stable_path_free_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        std::fs::create_dir(config.state_dir.join("runtime-status.json")).unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&observations);
        let hook: FatalPersistFailureHook = Arc::new(move |diagnostic| {
            captured.lock().unwrap().push(diagnostic);
        });
        fatal_persist_failure_hooks()
            .lock()
            .unwrap()
            .insert(config.state_dir.clone(), hook);
        let _guard = FailureHookGuard {
            state_dir: config.state_dir.clone(),
        };

        persist_fatal_after_reap(
            &config.state_dir,
            config.workspace_id,
            AgentErrorCode::AbnormalExit,
        );

        assert_eq!(
            *observations.lock().unwrap(),
            ["fns_agent_fatal_status_persist_failed:stage=status_write:code=io"]
        );
    }

    fn fixture_command(mode: &str) -> AgentCommand {
        static FIXTURE_BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        let binary = FIXTURE_BINARY.get_or_init(|| {
            let source =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/supervisor_child.rs");
            let build_dir = std::env::temp_dir().join(format!(
                "fns-supervisor-unit-fixture-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&build_dir).unwrap();
            let binary = build_dir.join("supervisor-child");
            let output = std::process::Command::new("rustc")
                .args(["--edition=2024", "-o"])
                .arg(&binary)
                .arg(source)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "fixture build failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        });
        AgentCommand::new(binary).arg(mode)
    }

    fn test_config(root: &Path) -> AgentConfig {
        let workspace_root = root.join("workspace");
        let state_dir = root.join("state");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        AgentConfig {
            schema_version: "fns-agent-config/1".into(),
            endpoint: "ws://127.0.0.1:9/api/user/workspace-sync/v2".into(),
            workspace_id: fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000002")
                .unwrap(),
            client_id: fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000001")
                .unwrap(),
            workspace_root,
            state_dir,
            token_file: root.join("unused-token-file"),
            sync: crate::config::AgentSyncConfig {
                includes: vec!["**".into()],
                excludes: Vec::new(),
                protect_secrets: true,
            },
            transport: crate::config::AgentTransportConfig {
                max_active_transfers: 2,
            },
        }
    }

    fn test_token() -> fns_platform::SecretToken {
        fns_platform::SecretToken::from_private_ipc(b"test-token".to_vec()).unwrap()
    }
}
