//! Per-project process supervision for the FNS agent sidecar.

use crate::project::{ProjectClientIdentity, ProjectConfig};
use crate::ssh_tunnel::{TunnelFailure, TunnelState};
use fns_agent::{AgentCommand, AgentConfig, AgentErrorCode, AgentProcess, AgentProcessOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio_util::task::TaskTracker;

const SHUTDOWN_ALL_DEADLINE: Duration = Duration::from_secs(90);
const CONTROL_REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const MAX_RETAINED_CONFLICT_OPERATIONS_PER_PROJECT: usize = 128;
const REMOTE_WORKSPACE_PORT: u16 = 9000;

type RuntimeResult = Result<(), AgentErrorCode>;
type LifecycleResult = Result<(), SyncFailure>;
type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFailure {
    pub primary: AgentErrorCode,
    pub cleanup: Vec<AgentErrorCode>,
}

impl SyncFailure {
    fn primary(primary: AgentErrorCode) -> Self {
        Self {
            primary,
            cleanup: Vec::new(),
        }
    }
}

impl From<AgentErrorCode> for SyncFailure {
    fn from(code: AgentErrorCode) -> Self {
        Self::primary(code)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictControlIdentity {
    pub request_id: fns_protocol::RequestId,
    pub project_generation: uuid::Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolutionOperationPhase {
    Pending,
    Dispatched,
    Queued,
    Failed,
    Cancelled,
}

impl ConflictResolutionOperationPhase {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Queued | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolutionOperationView {
    pub request_id: fns_protocol::RequestId,
    pub project_generation: uuid::Uuid,
    pub conflict_id: fns_protocol::ConflictId,
    pub conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
    pub choice: fns_protocol::WorkspaceConflictChoice,
    pub phase: ConflictResolutionOperationPhase,
    pub receipt: Option<fns_agent::ConflictResolutionReceipt>,
    pub error: Option<AgentErrorCode>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ConflictOperationKey {
    project_id: String,
    request_id: fns_protocol::RequestId,
}

type ConflictOperationResult = Result<fns_agent::ConflictResolutionReceipt, AgentErrorCode>;

struct ConflictOperationRecord {
    project_generation: uuid::Uuid,
    session_generation: uuid::Uuid,
    input: fns_agent::ConflictResolutionInput,
    phase: ConflictResolutionOperationPhase,
    receipt: Option<fns_agent::ConflictResolutionReceipt>,
    error: Option<AgentErrorCode>,
    sequence: u64,
    completion: watch::Sender<Option<ConflictOperationResult>>,
}

impl ConflictOperationRecord {
    fn view(&self, request_id: fns_protocol::RequestId) -> ConflictResolutionOperationView {
        ConflictResolutionOperationView {
            request_id,
            project_generation: self.project_generation,
            conflict_id: self.input.conflict_id,
            conflict_revision: self.input.conflict_revision,
            choice: self.input.choice,
            phase: self.phase,
            receipt: self.receipt,
            error: self.error,
        }
    }
}

#[derive(Clone, Copy)]
struct RestartPolicy {
    max_restarts: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    process_options: AgentProcessOptions,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(5),
            process_options: AgentProcessOptions::default(),
        }
    }
}

pub trait CredentialProvider: Send + Sync + 'static {
    fn token_for_project(
        &self,
        project_id: &str,
    ) -> Result<fns_platform::SecretToken, AgentErrorCode>;
}

#[cfg(test)]
struct UnavailableCredentialProvider;

#[cfg(test)]
impl CredentialProvider for UnavailableCredentialProvider {
    fn token_for_project(
        &self,
        _project_id: &str,
    ) -> Result<fns_platform::SecretToken, AgentErrorCode> {
        Err(AgentErrorCode::AuthRequired)
    }
}

enum SessionCommand {
    Stop,
    CancelStartup {
        generation: uuid::Uuid,
    },
    ListConflicts {
        response: oneshot::Sender<Result<Vec<fns_agent::ConflictView>, AgentErrorCode>>,
    },
    ResolveConflict {
        operation: ConflictOperationKey,
    },
}

struct StartupWaiters {
    generation: uuid::Uuid,
    state: AtomicUsize,
    commands: Weak<mpsc::Sender<SessionCommand>>,
}

impl StartupWaiters {
    const COMPLETE: usize = 1 << (usize::BITS - 1);
    const ACTIVE_MASK: usize = Self::COMPLETE - 1;

    fn new(generation: uuid::Uuid, commands: Weak<mpsc::Sender<SessionCommand>>) -> Arc<Self> {
        Arc::new(Self {
            generation,
            state: AtomicUsize::new(0),
            commands,
        })
    }

    fn register(self: &Arc<Self>) -> StartupWaiter {
        let previous = self.state.fetch_add(1, Ordering::AcqRel);
        assert!(previous & Self::ACTIVE_MASK < Self::ACTIVE_MASK);
        StartupWaiter {
            waiters: Arc::clone(self),
            armed: true,
        }
    }

    fn mark_complete(&self) {
        self.state.fetch_or(Self::COMPLETE, Ordering::AcqRel);
    }

    fn mark_complete_if_active(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & Self::ACTIVE_MASK == 0 {
                return false;
            }
            if state & Self::COMPLETE != 0 {
                return true;
            }
            match self.state.compare_exchange_weak(
                state,
                state | Self::COMPLETE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
    }

    fn active_count(&self) -> usize {
        self.state.load(Ordering::Acquire) & Self::ACTIVE_MASK
    }

    fn release(&self, cancelled: bool) {
        let previous = self.state.fetch_sub(1, Ordering::AcqRel);
        let was_last = previous & Self::ACTIVE_MASK == 1;
        if cancelled
            && was_last
            && previous & Self::COMPLETE == 0
            && let Some(commands) = self.commands.upgrade()
        {
            let _ = commands.try_send(SessionCommand::CancelStartup {
                generation: self.generation,
            });
        }
    }
}

struct StartupWaiter {
    waiters: Arc<StartupWaiters>,
    armed: bool,
}

impl StartupWaiter {
    fn disarm(&mut self) {
        if self.armed {
            self.waiters.release(false);
            self.armed = false;
        }
    }
}

impl Drop for StartupWaiter {
    fn drop(&mut self) {
        if self.armed {
            self.waiters.release(true);
        }
    }
}

trait ManagedAgent: Send {
    fn wait(&mut self) -> RuntimeFuture<'_, RuntimeResult>;
    fn shutdown(&mut self) -> RuntimeFuture<'_, RuntimeResult>;
    fn list_conflicts(
        &mut self,
    ) -> RuntimeFuture<'_, Result<Vec<fns_agent::ConflictView>, AgentErrorCode>> {
        Box::pin(async { Err(AgentErrorCode::Core) })
    }
    fn resolve_conflict(
        &mut self,
        _input: fns_agent::ConflictResolutionInput,
    ) -> RuntimeFuture<'_, Result<fns_agent::ConflictResolutionReceipt, AgentErrorCode>> {
        Box::pin(async { Err(AgentErrorCode::Core) })
    }
}

impl ManagedAgent for AgentProcess {
    fn wait(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
        Box::pin(async move {
            AgentProcess::wait(self)
                .await
                .map(|_| ())
                .map_err(|error| error.code())
        })
    }

    fn shutdown(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
        Box::pin(async move {
            AgentProcess::shutdown(self)
                .await
                .map_err(|error| error.code())
        })
    }

    fn list_conflicts(
        &mut self,
    ) -> RuntimeFuture<'_, Result<Vec<fns_agent::ConflictView>, AgentErrorCode>> {
        Box::pin(async move {
            AgentProcess::list_conflicts(self)
                .await
                .map_err(|error| error.code())
        })
    }

    fn resolve_conflict(
        &mut self,
        input: fns_agent::ConflictResolutionInput,
    ) -> RuntimeFuture<'_, Result<fns_agent::ConflictResolutionReceipt, AgentErrorCode>> {
        Box::pin(async move {
            AgentProcess::resolve_conflict(
                self,
                input.conflict_id,
                input.conflict_revision,
                input.choice,
            )
            .await
            .map_err(|error| error.code())
        })
    }
}

type TunnelCleanupAction = Box<dyn FnOnce() -> LifecycleResult + Send + 'static>;

// A discarded spawn_blocking result drops this lease, so even a tunnel
// published after its caller is gone is closed by its exact generation owner.
struct TunnelLease {
    local_port: u16,
    cleanup: Option<TunnelCleanupAction>,
}

impl TunnelLease {
    fn new(local_port: u16, cleanup: TunnelCleanupAction) -> Self {
        Self {
            local_port,
            cleanup: Some(cleanup),
        }
    }

    #[cfg(test)]
    fn unmanaged(local_port: u16) -> Self {
        Self {
            local_port,
            cleanup: None,
        }
    }

    fn disarm(&mut self) {
        self.cleanup.take();
    }
}

impl Drop for TunnelLease {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take()
            && let Err(failure) = cleanup()
        {
            eprintln!(
                "fns_sync_tunnel_cleanup_failed:code={}",
                stable_failure(&failure)
            );
        }
    }
}

trait SessionRuntime: Send + Sync + 'static {
    fn token_for_project(
        &self,
        project_id: &str,
    ) -> Result<fns_platform::SecretToken, AgentErrorCode>;

    fn open_tunnel<'a>(
        &'a self,
        project_id: &'a str,
        generation: uuid::Uuid,
        ssh_host: &'a str,
        remote_port: u16,
    ) -> RuntimeFuture<'a, Result<TunnelLease, SyncFailure>>;

    fn close_tunnel<'a>(
        &'a self,
        project_id: &'a str,
        generation: uuid::Uuid,
        ssh_host: &'a str,
    ) -> RuntimeFuture<'a, LifecycleResult>;

    fn spawn_agent<'a>(
        &'a self,
        command: AgentCommand,
        config: AgentConfig,
        token: fns_platform::SecretToken,
        options: AgentProcessOptions,
    ) -> RuntimeFuture<'a, Result<Box<dyn ManagedAgent>, AgentErrorCode>>;
}

struct DesktopSessionRuntime {
    credentials: Arc<dyn CredentialProvider>,
    tunnels: TunnelState,
    tasks: TaskTracker,
}

fn spawn_owned_blocking<T, E>(
    tasks: &TaskTracker,
    action: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> RuntimeFuture<'static, Result<T, E>>
where
    T: Send + 'static,
    E: From<AgentErrorCode> + Send + 'static,
{
    let (result_tx, result_rx) = oneshot::channel();
    tasks.spawn(async move {
        let result = tokio::task::spawn_blocking(action)
            .await
            .map_err(|_| E::from(AgentErrorCode::AbnormalExit))
            .and_then(std::convert::identity);
        let _ = result_tx.send(result);
    });
    Box::pin(async move {
        result_rx
            .await
            .unwrap_or_else(|_| Err(E::from(AgentErrorCode::AbnormalExit)))
    })
}

fn sync_failure_from_tunnel(failure: TunnelFailure) -> SyncFailure {
    SyncFailure {
        primary: failure.primary.agent_code(),
        cleanup: failure
            .cleanup
            .into_iter()
            .map(|code| code.agent_code())
            .collect(),
    }
}

impl SessionRuntime for DesktopSessionRuntime {
    fn token_for_project(
        &self,
        project_id: &str,
    ) -> Result<fns_platform::SecretToken, AgentErrorCode> {
        self.credentials.token_for_project(project_id)
    }

    fn open_tunnel<'a>(
        &'a self,
        project_id: &'a str,
        generation: uuid::Uuid,
        ssh_host: &'a str,
        remote_port: u16,
    ) -> RuntimeFuture<'a, Result<TunnelLease, SyncFailure>> {
        let tunnels = self.tunnels.clone();
        let tasks = self.tasks.clone();
        let tunnel_key = sync_tunnel_key(project_id, generation);
        let ssh_host = ssh_host.to_owned();
        spawn_owned_blocking(&tasks, move || -> Result<TunnelLease, SyncFailure> {
            let cleanup_tunnels = tunnels.clone();
            let cleanup_key = tunnel_key.clone();
            let cleanup_host = ssh_host.clone();
            let cleanup: TunnelCleanupAction = Box::new(move || {
                cleanup_tunnels
                    .close_project(&cleanup_key, &cleanup_host)
                    .map_err(sync_failure_from_tunnel)
            });
            let mut lease = TunnelLease::new(0, cleanup);
            let local_port = tunnels
                .get_or_create(&tunnel_key, &ssh_host, remote_port)
                .map_err(sync_failure_from_tunnel)?;
            lease.local_port = local_port;
            Ok(lease)
        })
    }

    fn close_tunnel<'a>(
        &'a self,
        project_id: &'a str,
        generation: uuid::Uuid,
        ssh_host: &'a str,
    ) -> RuntimeFuture<'a, LifecycleResult> {
        let tunnels = self.tunnels.clone();
        let tasks = self.tasks.clone();
        let tunnel_key = sync_tunnel_key(project_id, generation);
        let ssh_host = ssh_host.to_owned();
        spawn_owned_blocking(&tasks, move || {
            tunnels
                .close_project(&tunnel_key, &ssh_host)
                .map_err(sync_failure_from_tunnel)
        })
    }

    fn spawn_agent<'a>(
        &'a self,
        command: AgentCommand,
        config: AgentConfig,
        token: fns_platform::SecretToken,
        options: AgentProcessOptions,
    ) -> RuntimeFuture<'a, Result<Box<dyn ManagedAgent>, AgentErrorCode>> {
        Box::pin(async move {
            AgentProcess::spawn(command, config, token, options)
                .await
                .map(|process| Box::new(process) as Box<dyn ManagedAgent>)
                .map_err(|error| error.code())
        })
    }
}

fn sync_tunnel_key(project_id: &str, generation: uuid::Uuid) -> String {
    format!("sync:{generation}:{project_id}")
}

struct SessionRecord {
    generation: uuid::Uuid,
    commands: Arc<mpsc::Sender<SessionCommand>>,
    readiness: watch::Receiver<Option<LifecycleResult>>,
    startup_waiters: Arc<StartupWaiters>,
    completion: watch::Receiver<Option<LifecycleResult>>,
    actor_abort: Option<tokio::task::AbortHandle>,
    runtime: Arc<dyn SessionRuntime>,
    ssh_host: String,
    stop_requested: bool,
    running: bool,
    local_port: Option<u16>,
    message: String,
    failure: Option<SyncFailure>,
}

struct SessionStart {
    project_id: String,
    command: AgentCommand,
    config: AgentConfig,
    runtime: Arc<dyn SessionRuntime>,
    ssh_host: String,
    remote_port: u16,
    restart_policy: RestartPolicy,
    #[cfg(test)]
    readiness_transition: Option<Arc<ReadinessTransitionHook>>,
}

#[cfg(test)]
struct ReadinessTransitionHook {
    launch_completed: tokio::sync::Notify,
    resume_publication: tokio::sync::Notify,
    stale_cancel_ignored: tokio::sync::Notify,
}

#[derive(Default)]
struct SyncRegistry {
    sessions: HashMap<String, SessionRecord>,
    last_errors: HashMap<String, String>,
    last_failures: HashMap<String, SyncFailure>,
    conflict_operations: HashMap<ConflictOperationKey, ConflictOperationRecord>,
    next_conflict_operation_sequence: u64,
    shutting_down: bool,
}

pub struct SyncState {
    registry: Arc<Mutex<SyncRegistry>>,
    tasks: TaskTracker,
    shutdown_tasks: TaskTracker,
    shutdown_operation: std::sync::Mutex<Option<watch::Receiver<Option<LifecycleResult>>>>,
    shutdown_deadline: Duration,
    control_request_deadline: Duration,
    credentials: Arc<dyn CredentialProvider>,
}

impl SyncState {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_credentials(Arc::new(UnavailableCredentialProvider))
    }

    pub(crate) fn with_credentials(credentials: Arc<dyn CredentialProvider>) -> Self {
        Self::with_credentials_and_shutdown_deadline(credentials, SHUTDOWN_ALL_DEADLINE)
    }

    fn with_credentials_and_shutdown_deadline(
        credentials: Arc<dyn CredentialProvider>,
        shutdown_deadline: Duration,
    ) -> Self {
        Self {
            registry: Arc::new(Mutex::new(SyncRegistry::default())),
            tasks: TaskTracker::new(),
            shutdown_tasks: TaskTracker::new(),
            shutdown_operation: std::sync::Mutex::new(None),
            shutdown_deadline,
            control_request_deadline: CONTROL_REQUEST_DEADLINE,
            credentials,
        }
    }

    async fn start(&self, start: SessionStart) -> LifecycleResult {
        let mut start = Some(start);
        loop {
            let mut registry = self.registry.lock().await;
            if registry.shutting_down {
                return Err(AgentErrorCode::ShutdownTimeout.into());
            }
            let project_id = start
                .as_ref()
                .expect("session start consumed")
                .project_id
                .clone();
            if let Some(session) = registry.sessions.get(&project_id) {
                if !session.stop_requested {
                    let generation = session.generation;
                    let mut readiness = session.readiness.clone();
                    let mut waiter = session.startup_waiters.register();
                    drop(registry);
                    let result = wait_for_signal(&mut readiness).await;
                    waiter.disarm();
                    return match result {
                        Ok(result) => result,
                        Err(code) => {
                            self.cleanup_closed_actor(project_id, generation).await;
                            Err(code)
                        }
                    };
                }
                let mut completion = session.completion.clone();
                let generation = session.generation;
                let runtime = Arc::clone(&session.runtime);
                let ssh_host = session.ssh_host.clone();
                drop(registry);
                match wait_for_signal(&mut completion).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(_failure)) => {
                        match self
                            .cleanup_unresponsive_actor(&project_id, generation, runtime, &ssh_host)
                            .await
                        {
                            Ok(()) => continue,
                            Err(cleanup) => return Err(cleanup),
                        }
                    }
                    Err(failure) => return Err(failure),
                }
            }

            let start = start.take().expect("session start consumed");
            let project_id = start.project_id.clone();
            let generation = uuid::Uuid::new_v4();
            let (commands, receiver) = mpsc::channel(1);
            let commands = Arc::new(commands);
            let startup_waiters = StartupWaiters::new(generation, Arc::downgrade(&commands));
            let mut waiter = startup_waiters.register();
            let (readiness_tx, mut readiness) = watch::channel(None);
            let (completion_tx, completion) = watch::channel(None);
            let (actor_completion_tx, actor_completion) = watch::channel(None);
            registry.last_errors.remove(&start.project_id);
            let monitor_project_id = start.project_id.clone();
            let monitor_runtime = Arc::clone(&start.runtime);
            let monitor_ssh_host = start.ssh_host.clone();
            let monitor_startup_waiters = Arc::clone(&startup_waiters);
            let monitor_readiness = readiness_tx.clone();
            let actor = self.tasks.spawn(session_actor(
                Arc::clone(&self.registry),
                generation,
                start,
                receiver,
                startup_waiters,
                readiness_tx,
                actor_completion_tx,
            ));
            let actor_abort = actor.abort_handle();
            registry.sessions.insert(
                project_id.clone(),
                SessionRecord {
                    generation,
                    commands,
                    readiness: readiness.clone(),
                    startup_waiters: monitor_startup_waiters.clone(),
                    completion,
                    actor_abort: Some(actor_abort),
                    runtime: Arc::clone(&monitor_runtime),
                    ssh_host: monitor_ssh_host.clone(),
                    stop_requested: false,
                    running: false,
                    local_port: None,
                    message: "starting".into(),
                    failure: None,
                },
            );
            self.tasks.spawn(monitor_session_actor(
                actor,
                Arc::clone(&self.registry),
                monitor_project_id,
                generation,
                monitor_runtime,
                monitor_ssh_host,
                monitor_startup_waiters,
                monitor_readiness,
                actor_completion,
                completion_tx,
            ));
            drop(registry);
            let result = wait_for_signal(&mut readiness).await;
            waiter.disarm();
            return match result {
                Ok(result) => result,
                Err(code) => {
                    self.cleanup_closed_actor(project_id, generation).await;
                    Err(code)
                }
            };
        }
    }

    async fn stop(&self, project_id: &str) -> LifecycleResult {
        let stop = {
            let mut registry = self.registry.lock().await;
            let Some(session) = registry.sessions.get_mut(project_id) else {
                return Ok(());
            };
            let should_send = !session.stop_requested;
            session.stop_requested = true;
            session.running = false;
            session.message = "stopping".into();
            (
                should_send.then(|| session.commands.clone()),
                session.completion.clone(),
                session.generation,
                Arc::clone(&session.runtime),
                session.ssh_host.clone(),
            )
        };
        let (commands, mut completion, generation, runtime, ssh_host) = stop;
        if let Some(commands) = commands
            && commands.send(SessionCommand::Stop).await.is_err()
        {
            return self
                .cleanup_unresponsive_actor(project_id, generation, runtime, &ssh_host)
                .await;
        }
        match wait_for_signal(&mut completion).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(failure)) => {
                match self
                    .cleanup_unresponsive_actor(project_id, generation, runtime, &ssh_host)
                    .await
                {
                    Ok(()) => Err(failure),
                    Err(cleanup) => Err(cleanup),
                }
            }
            Err(_) => {
                self.cleanup_unresponsive_actor(project_id, generation, runtime, &ssh_host)
                    .await
            }
        }
    }

    async fn list_conflicts(
        &self,
        project_id: &str,
    ) -> Result<Vec<fns_agent::ConflictView>, SyncFailure> {
        let commands = {
            let registry = self.registry.lock().await;
            let session = registry
                .sessions
                .get(project_id)
                .filter(|session| session.running && !session.stop_requested)
                .ok_or_else(|| SyncFailure::primary(AgentErrorCode::AbnormalExit))?;
            Arc::clone(&session.commands)
        };
        let (response, result) = oneshot::channel();
        tokio::time::timeout(CONTROL_REQUEST_DEADLINE, async move {
            commands
                .send(SessionCommand::ListConflicts { response })
                .await
                .map_err(|_| AgentErrorCode::AbnormalExit)?;
            result.await.map_err(|_| AgentErrorCode::AbnormalExit)?
        })
        .await
        .map_err(|_| SyncFailure::primary(AgentErrorCode::RequestTimeout))?
        .map_err(SyncFailure::primary)
    }

    async fn resolve_conflict(
        &self,
        project_id: &str,
        identity: ConflictControlIdentity,
        input: fns_agent::ConflictResolutionInput,
    ) -> Result<fns_agent::ConflictResolutionReceipt, SyncFailure> {
        let operation = ConflictOperationKey {
            project_id: project_id.to_owned(),
            request_id: identity.request_id,
        };
        let (commands, mut completion) = {
            let mut registry = self.registry.lock().await;
            let (commands, session_generation) = registry
                .sessions
                .get(project_id)
                .filter(|session| session.running && !session.stop_requested)
                .map(|session| (Arc::clone(&session.commands), session.generation))
                .ok_or_else(|| SyncFailure::primary(AgentErrorCode::AbnormalExit))?;
            if let Some(existing) = registry.conflict_operations.get(&operation) {
                if existing.project_generation != identity.project_generation
                    || existing.input != input
                {
                    return Err(SyncFailure::primary(AgentErrorCode::ConflictRequestChanged));
                }
                (None, existing.completion.subscribe())
            } else {
                while registry
                    .conflict_operations
                    .keys()
                    .filter(|key| key.project_id == project_id)
                    .count()
                    >= MAX_RETAINED_CONFLICT_OPERATIONS_PER_PROJECT
                {
                    let oldest = registry
                        .conflict_operations
                        .iter()
                        .filter(|(key, record)| {
                            key.project_id == project_id && record.phase.is_terminal()
                        })
                        .min_by_key(|(_, record)| record.sequence)
                        .map(|(key, _)| key.clone())
                        .ok_or_else(|| SyncFailure::primary(AgentErrorCode::ResourceLimit))?;
                    registry.conflict_operations.remove(&oldest);
                }
                let sequence = registry
                    .next_conflict_operation_sequence
                    .checked_add(1)
                    .ok_or_else(|| SyncFailure::primary(AgentErrorCode::ResourceLimit))?;
                registry.next_conflict_operation_sequence = sequence;
                let (completion_tx, completion) = watch::channel(None);
                registry.conflict_operations.insert(
                    operation.clone(),
                    ConflictOperationRecord {
                        project_generation: identity.project_generation,
                        session_generation,
                        input,
                        phase: ConflictResolutionOperationPhase::Pending,
                        receipt: None,
                        error: None,
                        sequence,
                        completion: completion_tx,
                    },
                );
                (Some(commands), completion)
            }
        };

        let wait = async {
            if let Some(commands) = commands {
                let dispatch = commands.send(SessionCommand::ResolveConflict {
                    operation: operation.clone(),
                });
                tokio::pin!(dispatch);
                tokio::select! {
                    biased;
                    result = wait_for_conflict_operation(&mut completion) => result,
                    result = &mut dispatch => {
                        if result.is_err() {
                            settle_conflict_operation(
                                &self.registry,
                                &operation,
                                Err(AgentErrorCode::AbnormalExit),
                            ).await;
                        }
                        wait_for_conflict_operation(&mut completion).await
                    }
                }
            } else {
                wait_for_conflict_operation(&mut completion).await
            }
        };

        match tokio::time::timeout(self.control_request_deadline, wait).await {
            Ok(result) => result.map_err(SyncFailure::primary),
            Err(_) => {
                fail_pending_conflict_operation(
                    &self.registry,
                    &operation,
                    AgentErrorCode::RequestTimeout,
                )
                .await;
                Err(SyncFailure::primary(AgentErrorCode::RequestTimeout))
            }
        }
    }

    async fn cancel_conflict_request(
        &self,
        project_id: &str,
        identity: ConflictControlIdentity,
    ) -> Result<ConflictResolutionOperationView, SyncFailure> {
        let operation = ConflictOperationKey {
            project_id: project_id.to_owned(),
            request_id: identity.request_id,
        };
        let mut registry = self.registry.lock().await;
        let record = registry
            .conflict_operations
            .get_mut(&operation)
            .ok_or_else(|| SyncFailure::primary(AgentErrorCode::ConflictRequestUnavailable))?;
        if record.project_generation != identity.project_generation {
            return Err(SyncFailure::primary(AgentErrorCode::ConflictRequestChanged));
        }
        if record.phase == ConflictResolutionOperationPhase::Pending {
            cancel_conflict_operation_record(record);
        }
        Ok(record.view(identity.request_id))
    }

    async fn cancel_conflict_generation(
        &self,
        project_id: &str,
        project_generation: uuid::Uuid,
    ) -> Vec<ConflictResolutionOperationView> {
        let mut registry = self.registry.lock().await;
        let mut operations = registry
            .conflict_operations
            .iter_mut()
            .filter(|(key, record)| {
                key.project_id == project_id && record.project_generation == project_generation
            })
            .map(|(key, record)| {
                if record.phase == ConflictResolutionOperationPhase::Pending {
                    cancel_conflict_operation_record(record);
                }
                (record.sequence, record.view(key.request_id))
            })
            .collect::<Vec<_>>();
        operations.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
        operations.into_iter().map(|(_, view)| view).collect()
    }

    async fn conflict_operations(&self, project_id: &str) -> Vec<ConflictResolutionOperationView> {
        let registry = self.registry.lock().await;
        let mut operations = registry
            .conflict_operations
            .iter()
            .filter(|(key, _)| key.project_id == project_id)
            .map(|(key, record)| (record.sequence, record.view(key.request_id)))
            .collect::<Vec<_>>();
        operations.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
        operations.into_iter().map(|(_, view)| view).collect()
    }

    async fn status(&self, project_id: &str) -> SyncStatus {
        let registry = self.registry.lock().await;
        if let Some(session) = registry.sessions.get(project_id) {
            return SyncStatus {
                running: session.running,
                local_port: session.local_port,
                message: session.message.clone(),
                error: session.failure.clone(),
            };
        }
        SyncStatus {
            running: false,
            local_port: None,
            message: registry
                .last_errors
                .get(project_id)
                .cloned()
                .unwrap_or_else(|| "stopped".into()),
            error: registry.last_failures.get(project_id).cloned(),
        }
    }

    pub async fn shutdown_all(&self) -> LifecycleResult {
        let mut completion = {
            let mut operation = self
                .shutdown_operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(completion) = operation.as_ref()
                && match completion.borrow().as_ref() {
                    Some(result) => result.is_ok(),
                    None => completion.has_changed().is_ok(),
                }
            {
                completion.clone()
            } else {
                let (completion_tx, completion) = watch::channel(None);
                let registry = Arc::clone(&self.registry);
                let tasks = self.tasks.clone();
                let deadline = self.shutdown_deadline;
                self.shutdown_tasks.spawn(async move {
                    let result = run_shutdown_owner(registry, tasks, deadline).await;
                    let _ = completion_tx.send(Some(result));
                });
                *operation = Some(completion.clone());
                completion
            }
        };
        wait_for_signal(&mut completion)
            .await
            .unwrap_or(Err(AgentErrorCode::AbnormalExit.into()))
    }

    async fn cleanup_closed_actor(&self, project_id: String, generation: uuid::Uuid) {
        let cleanup = {
            let registry = self.registry.lock().await;
            registry.sessions.get(&project_id).and_then(|session| {
                (session.generation == generation)
                    .then(|| (Arc::clone(&session.runtime), session.ssh_host.clone()))
            })
        };
        if let Some((runtime, ssh_host)) = cleanup {
            let _ = self
                .cleanup_unresponsive_actor(&project_id, generation, runtime, &ssh_host)
                .await;
        }
    }

    async fn cleanup_unresponsive_actor(
        &self,
        project_id: &str,
        generation: uuid::Uuid,
        runtime: Arc<dyn SessionRuntime>,
        ssh_host: &str,
    ) -> LifecycleResult {
        cleanup_unresponsive_actor(&self.registry, project_id, generation, runtime, ssh_host).await
    }
}

async fn wait_for_conflict_operation(
    completion: &mut watch::Receiver<Option<ConflictOperationResult>>,
) -> ConflictOperationResult {
    loop {
        if let Some(result) = *completion.borrow_and_update() {
            return result;
        }
        if completion.changed().await.is_err() {
            return Err(AgentErrorCode::AbnormalExit);
        }
    }
}

fn cancel_conflict_operation_record(record: &mut ConflictOperationRecord) {
    record.phase = ConflictResolutionOperationPhase::Cancelled;
    record.receipt = None;
    record.error = Some(AgentErrorCode::RequestCancelled);
    record
        .completion
        .send_replace(Some(Err(AgentErrorCode::RequestCancelled)));
}

async fn begin_conflict_operation(
    registry: &Mutex<SyncRegistry>,
    operation: &ConflictOperationKey,
    session_generation: uuid::Uuid,
) -> Option<fns_agent::ConflictResolutionInput> {
    let mut registry = registry.lock().await;
    let record = registry.conflict_operations.get_mut(operation)?;
    if record.session_generation != session_generation
        || record.phase != ConflictResolutionOperationPhase::Pending
    {
        return None;
    }
    record.phase = ConflictResolutionOperationPhase::Dispatched;
    Some(record.input)
}

async fn settle_conflict_operation(
    registry: &Mutex<SyncRegistry>,
    operation: &ConflictOperationKey,
    result: ConflictOperationResult,
) {
    let mut registry = registry.lock().await;
    let Some(record) = registry.conflict_operations.get_mut(operation) else {
        return;
    };
    if record.phase.is_terminal() {
        return;
    }
    match result {
        Ok(receipt) => {
            record.phase = ConflictResolutionOperationPhase::Queued;
            record.receipt = Some(receipt);
            record.error = None;
            record.completion.send_replace(Some(Ok(receipt)));
        }
        Err(error) => {
            record.phase = ConflictResolutionOperationPhase::Failed;
            record.receipt = None;
            record.error = Some(error);
            record.completion.send_replace(Some(Err(error)));
        }
    }
}

async fn fail_pending_conflict_operation(
    registry: &Mutex<SyncRegistry>,
    operation: &ConflictOperationKey,
    error: AgentErrorCode,
) {
    let mut registry = registry.lock().await;
    let Some(record) = registry.conflict_operations.get_mut(operation) else {
        return;
    };
    if record.phase != ConflictResolutionOperationPhase::Pending {
        return;
    }
    record.phase = ConflictResolutionOperationPhase::Failed;
    record.receipt = None;
    record.error = Some(error);
    record.completion.send_replace(Some(Err(error)));
}

struct ShutdownSession {
    project_id: String,
    commands: Option<Arc<mpsc::Sender<SessionCommand>>>,
    completion: watch::Receiver<Option<LifecycleResult>>,
    generation: uuid::Uuid,
    actor_abort: Option<tokio::task::AbortHandle>,
    runtime: Arc<dyn SessionRuntime>,
    ssh_host: String,
}

async fn run_shutdown_owner(
    registry: Arc<Mutex<SyncRegistry>>,
    tasks: TaskTracker,
    deadline: Duration,
) -> LifecycleResult {
    let sessions = {
        let mut registry = registry.lock().await;
        registry.shutting_down = true;
        registry
            .sessions
            .iter_mut()
            .map(|(project_id, session)| {
                let should_send = !session.stop_requested;
                session.stop_requested = true;
                session.running = false;
                session.message = "stopping".into();
                ShutdownSession {
                    project_id: project_id.clone(),
                    commands: should_send.then(|| session.commands.clone()),
                    completion: session.completion.clone(),
                    generation: session.generation,
                    actor_abort: session.actor_abort.clone(),
                    runtime: Arc::clone(&session.runtime),
                    ssh_host: session.ssh_host.clone(),
                }
            })
            .collect::<Vec<_>>()
    };
    tasks.close();
    let shutdown = async {
        let mut first_error = None;
        let mut completions = Vec::with_capacity(sessions.len());
        for session in &sessions {
            let Some(commands) = session.commands.as_ref() else {
                let result = cleanup_unresponsive_actor(
                    &registry,
                    &session.project_id,
                    session.generation,
                    Arc::clone(&session.runtime),
                    &session.ssh_host,
                )
                .await;
                if let Err(code) = result {
                    eprintln!(
                        "fns_sync_session_shutdown_failed:project={}:code={}",
                        session.project_id,
                        stable_failure(&code)
                    );
                    first_error.get_or_insert(code);
                }
                continue;
            };
            if commands.send(SessionCommand::Stop).await.is_err() {
                let result = cleanup_unresponsive_actor(
                    &registry,
                    &session.project_id,
                    session.generation,
                    Arc::clone(&session.runtime),
                    &session.ssh_host,
                )
                .await;
                if let Err(code) = result {
                    eprintln!(
                        "fns_sync_session_shutdown_failed:project={}:code={}",
                        session.project_id,
                        stable_failure(&code)
                    );
                    first_error.get_or_insert(code);
                }
                continue;
            }
            completions.push(session);
        }
        for session in completions {
            let mut completion = session.completion.clone();
            let result = match wait_for_signal(&mut completion).await {
                Ok(result) => result,
                Err(_) => {
                    cleanup_unresponsive_actor(
                        &registry,
                        &session.project_id,
                        session.generation,
                        Arc::clone(&session.runtime),
                        &session.ssh_host,
                    )
                    .await
                }
            };
            if let Err(code) = result {
                eprintln!(
                    "fns_sync_session_shutdown_failed:project={}:code={}",
                    session.project_id,
                    stable_failure(&code)
                );
                first_error.get_or_insert(code);
            }
        }
        tasks.wait().await;
        first_error.map_or(Ok(()), Err)
    };
    let result = match tokio::time::timeout(deadline, shutdown).await {
        Ok(result) => {
            let cleanup = match tokio::time::timeout(
                deadline,
                settle_fenced_sessions_once(&registry, &sessions),
            )
            .await
            {
                Ok(cleanup) => cleanup,
                Err(_) => Some(SyncFailure::primary(AgentErrorCode::ShutdownTimeout)),
            };
            match (result, cleanup) {
                (Err(primary), Some(cleanup)) => Err(merge_failures(primary, cleanup)),
                (Ok(()), Some(cleanup)) => Err(cleanup),
                (result, None) => result,
            }
        }
        Err(_) => {
            for session in &sessions {
                if let Some(actor_abort) = session.actor_abort.as_ref() {
                    actor_abort.abort();
                }
            }
            let timeout = SyncFailure::primary(AgentErrorCode::ShutdownTimeout);
            let cleanup = tokio::time::timeout(deadline, async {
                tasks.wait().await;
                settle_fenced_sessions_once(&registry, &sessions).await
            })
            .await
            .ok()
            .flatten();
            match cleanup {
                Some(cleanup) => Err(merge_failures(timeout, cleanup)),
                None => Err(timeout),
            }
        }
    };
    if let Err(failure) = result.as_ref() {
        for session in &sessions {
            retain_session_failure(
                &registry,
                &session.project_id,
                session.generation,
                failure.clone(),
            )
            .await;
        }
    }
    result
}

async fn settle_fenced_sessions_once(
    registry: &Mutex<SyncRegistry>,
    sessions: &[ShutdownSession],
) -> Option<SyncFailure> {
    let mut first_failure = None;
    for session in sessions {
        let owns_generation = registry
            .lock()
            .await
            .sessions
            .get(&session.project_id)
            .is_some_and(|record| record.generation == session.generation);
        if !owns_generation {
            continue;
        }
        if let Err(failure) = cleanup_unresponsive_actor(
            registry,
            &session.project_id,
            session.generation,
            Arc::clone(&session.runtime),
            &session.ssh_host,
        )
        .await
        {
            first_failure = Some(
                first_failure.map_or(failure.clone(), |primary| merge_failures(primary, failure)),
            );
        }
    }
    first_failure
}

async fn cleanup_unresponsive_actor(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    runtime: Arc<dyn SessionRuntime>,
    ssh_host: &str,
) -> LifecycleResult {
    let existing_failure = {
        let registry = registry.lock().await;
        let Some(session) = registry.sessions.get(project_id) else {
            return Ok(());
        };
        if session.generation != generation {
            return Ok(());
        }
        session.failure.clone().or_else(|| {
            session
                .completion
                .borrow()
                .as_ref()
                .and_then(|result| result.as_ref().err().cloned())
        })
    };
    match runtime.close_tunnel(project_id, generation, ssh_host).await {
        Ok(()) => {
            finish_registry_session(
                registry,
                project_id,
                generation,
                existing_failure.as_ref().map(stable_failure),
                existing_failure,
            )
            .await;
            Ok(())
        }
        Err(cleanup) => {
            let failure = existing_failure
                .map_or(cleanup.clone(), |primary| merge_failures(primary, cleanup));
            retain_session_failure(registry, project_id, generation, failure.clone()).await;
            Err(failure)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn monitor_session_actor(
    actor: tokio::task::JoinHandle<()>,
    registry: Arc<Mutex<SyncRegistry>>,
    project_id: String,
    generation: uuid::Uuid,
    runtime: Arc<dyn SessionRuntime>,
    ssh_host: String,
    startup_waiters: Arc<StartupWaiters>,
    readiness: watch::Sender<Option<LifecycleResult>>,
    actor_completion: watch::Receiver<Option<LifecycleResult>>,
    completion: watch::Sender<Option<LifecycleResult>>,
) {
    let join_result = actor.await;
    let actor_result = if join_result.is_ok() {
        actor_completion
            .borrow()
            .clone()
            .unwrap_or_else(|| Err(AgentErrorCode::AbnormalExit.into()))
    } else {
        Err(AgentErrorCode::AbnormalExit.into())
    };
    let actor_message = registry
        .lock()
        .await
        .sessions
        .get(&project_id)
        .filter(|session| session.generation == generation)
        .map(|session| session.message.clone());
    let cleanup = runtime
        .close_tunnel(&project_id, generation, &ssh_host)
        .await;
    let cleanup_succeeded = cleanup.is_ok();
    let result = merge_lifecycle_cleanup(actor_result, cleanup);
    if readiness.borrow().is_none() {
        publish_readiness(&startup_waiters, &readiness, result.clone());
    }
    if cleanup_succeeded {
        let error_message = result.as_ref().err().map(|failure| {
            actor_message
                .filter(|message| !matches!(message.as_str(), "starting" | "running" | "stopping"))
                .unwrap_or_else(|| stable_failure(failure))
        });
        let failure = result.as_ref().err().cloned();
        finish_registry_session(&registry, &project_id, generation, error_message, failure).await;
    } else {
        if let Err(failure) = result.as_ref() {
            retain_session_failure(&registry, &project_id, generation, failure.clone()).await;
        }
    }
    let _ = completion.send(Some(result));
}

#[cfg(test)]
impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    pub local_port: Option<u16>,
    pub message: String,
    pub error: Option<SyncFailure>,
}

struct OwnedLaunch {
    process: Box<dyn ManagedAgent>,
    tunnel: TunnelLease,
}

struct LaunchFailure {
    failure: SyncFailure,
    tunnel: Option<TunnelLease>,
}

async fn close_owned_tunnel(
    runtime: &dyn SessionRuntime,
    project_id: &str,
    generation: uuid::Uuid,
    ssh_host: &str,
    tunnel: Option<&mut TunnelLease>,
) -> LifecycleResult {
    let result = runtime.close_tunnel(project_id, generation, ssh_host).await;
    if result.is_ok()
        && let Some(tunnel) = tunnel
    {
        tunnel.disarm();
    }
    result
}

fn merge_lifecycle_cleanup(primary: LifecycleResult, cleanup: LifecycleResult) -> LifecycleResult {
    match (primary, cleanup) {
        (Err(primary), Err(cleanup)) => {
            observe_cleanup_failure(primary.primary, cleanup.primary);
            Err(merge_failures(primary, cleanup))
        }
        (primary, Ok(())) => primary,
        (Ok(()), Err(cleanup)) => Err(cleanup),
    }
}

fn merge_failures(mut primary: SyncFailure, cleanup: SyncFailure) -> SyncFailure {
    primary.cleanup.push(cleanup.primary);
    primary.cleanup.extend(cleanup.cleanup);
    primary
}

async fn cleanup_owned_launch(
    runtime: &dyn SessionRuntime,
    project_id: &str,
    generation: uuid::Uuid,
    ssh_host: &str,
    launch: &mut OwnedLaunch,
) -> LifecycleResult {
    let process_result = launch.process.shutdown().await;
    let tunnel_result = close_owned_tunnel(
        runtime,
        project_id,
        generation,
        ssh_host,
        Some(&mut launch.tunnel),
    )
    .await;
    merge_cleanup_result(process_result, tunnel_result)
}

async fn session_actor(
    registry: Arc<Mutex<SyncRegistry>>,
    generation: uuid::Uuid,
    start: SessionStart,
    mut commands: mpsc::Receiver<SessionCommand>,
    startup_waiters: Arc<StartupWaiters>,
    readiness: watch::Sender<Option<LifecycleResult>>,
    completion: watch::Sender<Option<LifecycleResult>>,
) {
    let SessionStart {
        project_id,
        command,
        config,
        runtime,
        ssh_host,
        remote_port,
        restart_policy,
        #[cfg(test)]
        readiness_transition,
    } = start;
    let mut has_started = false;
    let mut restart_attempts = 0usize;
    let mut recovery_error = AgentErrorCode::AbnormalExit;

    loop {
        if has_started {
            if restart_attempts >= restart_policy.max_restarts {
                finish_actor(
                    &registry,
                    &project_id,
                    generation,
                    &completion,
                    Err(recovery_error.into()),
                    Some(format!(
                        "recovery_exhausted:{}",
                        stable_code(&recovery_error)
                    )),
                )
                .await;
                return;
            }
            restart_attempts += 1;
            update_session_status(
                &registry,
                &project_id,
                generation,
                false,
                None,
                format!(
                    "recovering:{restart_attempts}/{}:{}",
                    restart_policy.max_restarts,
                    stable_code(&recovery_error)
                ),
            )
            .await;
            let backoff = restart_backoff(restart_policy, restart_attempts);
            let backoff = tokio::time::sleep(backoff);
            tokio::pin!(backoff);
            loop {
                tokio::select! {
                    command = commands.recv() => {
                        match command {
                            Some(SessionCommand::CancelStartup { .. }) => {
                                #[cfg(test)]
                                if let Some(transition) = readiness_transition.as_ref() {
                                    transition.stale_cancel_ignored.notify_one();
                                }
                                continue;
                            }
                            Some(SessionCommand::ListConflicts { response }) => {
                                let _ = response.send(Err(AgentErrorCode::AbnormalExit));
                                continue;
                            }
                            Some(SessionCommand::ResolveConflict { operation }) => {
                                settle_conflict_operation(
                                    &registry,
                                    &operation,
                                    Err(AgentErrorCode::AbnormalExit),
                                ).await;
                                continue;
                            }
                            Some(SessionCommand::Stop) | None => {}
                        }
                        finish_actor(
                            &registry,
                            &project_id,
                            generation,
                            &completion,
                            Ok(()),
                            None,
                        ).await;
                        return;
                    }
                    () = &mut backoff => break,
                }
            }
        }

        let cancel_launch = AtomicBool::new(false);
        let open_completed = AtomicBool::new(false);
        let mut launch = Box::pin(launch_agent(LaunchRequest {
            runtime: runtime.as_ref(),
            project_id: &project_id,
            generation,
            ssh_host: &ssh_host,
            remote_port,
            command: command.clone(),
            config: &config,
            options: restart_policy.process_options,
            cancelled: &cancel_launch,
            open_completed: &open_completed,
        }));
        let launch_result = loop {
            tokio::select! {
                command = commands.recv() => {
                    let command = match command {
                        Some(SessionCommand::ListConflicts { response }) => {
                            let _ = response.send(Err(AgentErrorCode::StartupTimeout));
                            continue;
                        }
                        Some(SessionCommand::ResolveConflict { operation }) => {
                            settle_conflict_operation(
                                &registry,
                                &operation,
                                Err(AgentErrorCode::StartupTimeout),
                            ).await;
                            continue;
                        }
                        command => command,
                    };
                    if let Some(SessionCommand::CancelStartup { generation: cancelled_generation }) = command {
                        if has_started
                            || cancelled_generation != generation
                            || !claim_startup_cancellation(
                                &registry,
                                &project_id,
                                generation,
                                &startup_waiters,
                            ).await
                        {
                            #[cfg(test)]
                            if let Some(transition) = readiness_transition.as_ref() {
                                transition.stale_cancel_ignored.notify_one();
                            }
                            continue;
                        }
                        cancel_launch.store(true, Ordering::Release);
                        startup_waiters.mark_complete();
                        // A completed opener has transferred cleanup to its
                        // lease; an in-flight opener must be joined before the
                        // Stopping generation can be removed.
                        let result = if open_completed.load(Ordering::Acquire) {
                            drop(launch);
                            close_owned_tunnel(
                                runtime.as_ref(),
                                &project_id,
                                generation,
                                &ssh_host,
                                None,
                            ).await
                        } else {
                            match launch.await {
                                Ok(mut launch) => cleanup_owned_launch(
                                    runtime.as_ref(),
                                    &project_id,
                                    generation,
                                    &ssh_host,
                                    &mut launch,
                                ).await,
                                Err(mut failure) => {
                                    close_owned_tunnel(
                                        runtime.as_ref(),
                                        &project_id,
                                        generation,
                                        &ssh_host,
                                        failure.tunnel.as_mut(),
                                    )
                                    .await
                                }
                            }
                        };
                        finish_actor(
                            &registry,
                            &project_id,
                            generation,
                            &completion,
                            result.clone(),
                            result.as_ref().err().map(stable_failure),
                        ).await;
                        return;
                    }
                    update_session_status(
                        &registry,
                        &project_id,
                        generation,
                        false,
                        None,
                        "stopping".into(),
                    ).await;
                    if !has_started {
                        publish_readiness(
                            &startup_waiters,
                            &readiness,
                            Err(AgentErrorCode::AbnormalExit.into()),
                        );
                    }
                    cancel_launch.store(true, Ordering::Release);
                    let result = match launch.await {
                        Ok(mut launch) => cleanup_owned_launch(
                            runtime.as_ref(),
                            &project_id,
                            generation,
                            &ssh_host,
                            &mut launch,
                        ).await,
                        Err(mut failure) => {
                            close_owned_tunnel(
                                runtime.as_ref(),
                                &project_id,
                                generation,
                                &ssh_host,
                                failure.tunnel.as_mut(),
                            )
                            .await
                        }
                    };
                    finish_actor(
                        &registry,
                        &project_id,
                        generation,
                        &completion,
                        result.clone(),
                        result.as_ref().err().map(stable_failure),
                    ).await;
                    return;
                }
                result = &mut launch => break result,
            }
        };

        let mut launch = match launch_result {
            Ok(launch) => launch,
            Err(mut failure) => {
                let launch_failure = failure.failure;
                recovery_error = launch_failure.primary;
                match close_owned_tunnel(
                    runtime.as_ref(),
                    &project_id,
                    generation,
                    &ssh_host,
                    failure.tunnel.as_mut(),
                )
                .await
                {
                    Ok(()) => {}
                    Err(cleanup) => {
                        observe_cleanup_failure(launch_failure.primary, cleanup.primary);
                        let failure = merge_failures(launch_failure, cleanup);
                        if !has_started {
                            publish_readiness(&startup_waiters, &readiness, Err(failure.clone()));
                        }
                        finish_actor(
                            &registry,
                            &project_id,
                            generation,
                            &completion,
                            Err(failure.clone()),
                            Some(stable_failure(&failure)),
                        )
                        .await;
                        return;
                    }
                }
                if !has_started {
                    publish_readiness(&startup_waiters, &readiness, Err(launch_failure.clone()));
                    finish_actor(
                        &registry,
                        &project_id,
                        generation,
                        &completion,
                        Err(launch_failure.clone()),
                        Some(stable_failure(&launch_failure)),
                    )
                    .await;
                    return;
                }
                if !is_retryable(recovery_error) {
                    finish_actor(
                        &registry,
                        &project_id,
                        generation,
                        &completion,
                        Err(recovery_error.into()),
                        Some(stable_code(&recovery_error)),
                    )
                    .await;
                    return;
                }
                continue;
            }
        };
        let local_port = launch.tunnel.local_port;

        #[cfg(test)]
        if !has_started && let Some(transition) = readiness_transition.as_ref() {
            transition.launch_completed.notify_one();
            transition.resume_publication.notified().await;
        }

        if !has_started {
            if !publish_initial_readiness(
                &registry,
                &project_id,
                generation,
                &startup_waiters,
                &readiness,
                local_port,
            )
            .await
            {
                let result = cleanup_owned_launch(
                    runtime.as_ref(),
                    &project_id,
                    generation,
                    &ssh_host,
                    &mut launch,
                )
                .await;
                finish_actor(
                    &registry,
                    &project_id,
                    generation,
                    &completion,
                    result.clone(),
                    result.as_ref().err().map(stable_failure),
                )
                .await;
                return;
            }
            has_started = true;
        } else {
            update_session_status(
                &registry,
                &project_id,
                generation,
                true,
                Some(local_port),
                "running".into(),
            )
            .await;
        }

        loop {
            tokio::select! {
                command = commands.recv() => {
                    match command {
                        Some(SessionCommand::CancelStartup { generation: _ }) => {
                            #[cfg(test)]
                            if let Some(transition) = readiness_transition.as_ref() {
                                transition.stale_cancel_ignored.notify_one();
                            }
                            continue;
                        }
                        Some(SessionCommand::ListConflicts { response }) => {
                            let result = launch.process.list_conflicts().await;
                            let _ = response.send(result);
                            continue;
                        }
                        Some(SessionCommand::ResolveConflict { operation }) => {
                            let Some(input) = begin_conflict_operation(
                                &registry,
                                &operation,
                                generation,
                            ).await else {
                                continue;
                            };
                            let result = launch.process.resolve_conflict(input).await;
                            settle_conflict_operation(&registry, &operation, result).await;
                            continue;
                        }
                        Some(SessionCommand::Stop) | None => {}
                    }
                    update_session_status(
                        &registry,
                        &project_id,
                        generation,
                        false,
                        Some(local_port),
                        "stopping".into(),
                    ).await;
                    let result = cleanup_owned_launch(
                        runtime.as_ref(),
                        &project_id,
                        generation,
                        &ssh_host,
                        &mut launch,
                    ).await;
                    finish_actor(
                        &registry,
                        &project_id,
                        generation,
                        &completion,
                        result.clone(),
                        result.as_ref().err().map(stable_failure),
                    ).await;
                    return;
                }
                result = launch.process.wait() => {
                    recovery_error = result.err().unwrap_or(AgentErrorCode::AbnormalExit);
                    if let Err(cleanup) = close_owned_tunnel(
                        runtime.as_ref(),
                        &project_id,
                        generation,
                        &ssh_host,
                        Some(&mut launch.tunnel),
                    ).await {
                        observe_cleanup_failure(recovery_error, cleanup.primary);
                        let failure =
                            merge_failures(SyncFailure::primary(recovery_error), cleanup);
                        finish_actor(
                            &registry,
                            &project_id,
                            generation,
                            &completion,
                            Err(failure.clone()),
                            Some(stable_failure(&failure)),
                        ).await;
                        return;
                    }
                    if !is_retryable(recovery_error) {
                        finish_actor(
                            &registry,
                            &project_id,
                            generation,
                            &completion,
                            Err(recovery_error.into()),
                            Some(stable_code(&recovery_error)),
                        ).await;
                        return;
                    }
                    break;
                }
            }
        }
    }
}

fn publish_readiness(
    startup_waiters: &StartupWaiters,
    readiness: &watch::Sender<Option<LifecycleResult>>,
    result: LifecycleResult,
) {
    startup_waiters.mark_complete();
    let _ = readiness.send(Some(result));
}

async fn publish_initial_readiness(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    startup_waiters: &StartupWaiters,
    readiness: &watch::Sender<Option<LifecycleResult>>,
    local_port: u16,
) -> bool {
    let mut registry = registry.lock().await;
    let Some(session) = registry.sessions.get_mut(project_id) else {
        startup_waiters.mark_complete();
        return false;
    };
    if session.generation != generation || !startup_waiters.mark_complete_if_active() {
        startup_waiters.mark_complete();
        if session.generation == generation {
            session.stop_requested = true;
            session.running = false;
            session.local_port = None;
            session.message = "stopping".into();
        }
        return false;
    }
    session.running = true;
    session.local_port = Some(local_port);
    session.message = "running".into();
    let _ = readiness.send(Some(Ok(())));
    true
}

async fn claim_startup_cancellation(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    startup_waiters: &StartupWaiters,
) -> bool {
    let mut registry = registry.lock().await;
    let Some(session) = registry.sessions.get_mut(project_id) else {
        return true;
    };
    if session.generation != generation {
        return false;
    }
    if startup_waiters.active_count() != 0 {
        return false;
    }
    session.stop_requested = true;
    session.running = false;
    session.local_port = None;
    session.message = "stopping".into();
    true
}

struct LaunchRequest<'a> {
    runtime: &'a dyn SessionRuntime,
    project_id: &'a str,
    generation: uuid::Uuid,
    ssh_host: &'a str,
    remote_port: u16,
    command: AgentCommand,
    config: &'a AgentConfig,
    options: AgentProcessOptions,
    cancelled: &'a AtomicBool,
    open_completed: &'a AtomicBool,
}

async fn launch_agent(request: LaunchRequest<'_>) -> Result<OwnedLaunch, LaunchFailure> {
    let LaunchRequest {
        runtime,
        project_id,
        generation,
        ssh_host,
        remote_port,
        command,
        config,
        options,
        cancelled,
        open_completed,
    } = request;
    let token = runtime
        .token_for_project(project_id)
        .map_err(|code| LaunchFailure {
            failure: code.into(),
            tunnel: None,
        })?;
    let tunnel = runtime
        .open_tunnel(project_id, generation, ssh_host, remote_port)
        .await
        .map_err(|failure| LaunchFailure {
            failure,
            tunnel: None,
        })?;
    open_completed.store(true, Ordering::Release);
    if cancelled.load(Ordering::Acquire) {
        return Err(LaunchFailure {
            failure: AgentErrorCode::AbnormalExit.into(),
            tunnel: Some(tunnel),
        });
    }
    let config = agent_config_for_port(config, tunnel.local_port);
    // A successful spawn means WorkerFrame::Ready, which is sent only after
    // recovery and the worker's initial durable reconciliation complete.
    let process = match runtime.spawn_agent(command, config, token, options).await {
        Ok(process) => process,
        Err(code) => {
            return Err(LaunchFailure {
                failure: code.into(),
                tunnel: Some(tunnel),
            });
        }
    };
    Ok(OwnedLaunch { process, tunnel })
}

fn agent_config_for_port(template: &AgentConfig, local_port: u16) -> AgentConfig {
    AgentConfig {
        schema_version: template.schema_version.clone(),
        endpoint: format!("ws://127.0.0.1:{local_port}/api/user/workspace-sync/v2"),
        workspace_id: template.workspace_id,
        client_id: template.client_id,
        workspace_root: template.workspace_root.clone(),
        state_dir: template.state_dir.clone(),
        token_file: template.token_file.clone(),
        sync: template.sync.clone(),
        transport: template.transport,
    }
}

fn restart_backoff(policy: RestartPolicy, restart_attempt: usize) -> Duration {
    let exponent = u32::try_from(restart_attempt.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(31);
    policy
        .initial_backoff
        .saturating_mul(2u32.saturating_pow(exponent))
        .min(policy.max_backoff)
}

fn is_retryable(code: AgentErrorCode) -> bool {
    matches!(
        code,
        AgentErrorCode::Network
            | AgentErrorCode::Protocol
            | AgentErrorCode::Core
            | AgentErrorCode::Filesystem
            | AgentErrorCode::SpawnFailed
            | AgentErrorCode::StartupTimeout
            | AgentErrorCode::IdleTimeout
            | AgentErrorCode::TransferTimeout
            | AgentErrorCode::AbnormalExit
            | AgentErrorCode::ShutdownTimeout
    )
}

fn merge_cleanup_result(primary: RuntimeResult, cleanup: LifecycleResult) -> LifecycleResult {
    match (primary, cleanup) {
        (Err(primary), Err(cleanup)) => {
            observe_cleanup_failure(primary, cleanup.primary);
            Err(merge_failures(SyncFailure::primary(primary), cleanup))
        }
        (Err(primary), Ok(())) => Err(primary.into()),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn observe_cleanup_failure(primary: AgentErrorCode, cleanup: AgentErrorCode) {
    eprintln!(
        "fns_sync_cleanup_failed:primary={}:cleanup={}",
        stable_code(&primary),
        stable_code(&cleanup)
    );
}

async fn update_session_status(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    running: bool,
    local_port: Option<u16>,
    message: String,
) {
    let mut registry = registry.lock().await;
    if let Some(session) = registry.sessions.get_mut(project_id)
        && session.generation == generation
    {
        session.running = running;
        session.local_port = local_port;
        session.message = message;
    }
}

async fn retain_session_failure(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    failure: SyncFailure,
) {
    let mut registry = registry.lock().await;
    if let Some(session) = registry.sessions.get_mut(project_id)
        && session.generation == generation
    {
        session.running = false;
        session.local_port = None;
        session.message = stable_failure(&failure);
        session.failure = Some(failure);
    }
}

async fn finish_actor(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    completion: &watch::Sender<Option<LifecycleResult>>,
    result: LifecycleResult,
    error_message: Option<String>,
) {
    if let Some(session) = registry.lock().await.sessions.get_mut(project_id)
        && session.generation == generation
    {
        session.stop_requested = true;
        session.running = false;
        session.local_port = None;
        if let Some(error_message) = error_message {
            session.message = error_message;
        }
    }
    let _ = completion.send(Some(result));
}

async fn finish_registry_session(
    registry: &Mutex<SyncRegistry>,
    project_id: &str,
    generation: uuid::Uuid,
    error_message: Option<String>,
    failure: Option<SyncFailure>,
) {
    let mut registry = registry.lock().await;
    let owns_session = registry
        .sessions
        .get(project_id)
        .is_some_and(|session| session.generation == generation);
    if !owns_session {
        return;
    }
    let operation_error = failure
        .as_ref()
        .map_or(AgentErrorCode::AbnormalExit, |failure| failure.primary);
    for (key, record) in &mut registry.conflict_operations {
        if key.project_id == project_id
            && record.session_generation == generation
            && !record.phase.is_terminal()
        {
            record.phase = ConflictResolutionOperationPhase::Failed;
            record.receipt = None;
            record.error = Some(operation_error);
            record.completion.send_replace(Some(Err(operation_error)));
        }
    }
    registry.sessions.remove(project_id);
    if let Some(error) = error_message {
        registry.last_errors.insert(project_id.to_owned(), error);
    } else {
        registry.last_errors.remove(project_id);
    }
    if let Some(failure) = failure {
        registry
            .last_failures
            .insert(project_id.to_owned(), failure);
    } else {
        registry.last_failures.remove(project_id);
    }
}

async fn wait_for_signal(
    receiver: &mut watch::Receiver<Option<LifecycleResult>>,
) -> Result<LifecycleResult, SyncFailure> {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return Ok(result);
        }
        if receiver.changed().await.is_err() {
            return Err(AgentErrorCode::AbnormalExit.into());
        }
    }
}

fn project_state_dir(project_id: &str) -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|directories| directories.config_dir().join("fns-workspace"))
        .unwrap_or_else(|| PathBuf::from(".config/fns-workspace"));
    base.join(format!("projects-{project_id}")).join("state")
}

fn resolve_project_client_id(
    state_dir: &std::path::Path,
    project_id: &str,
    workspace_id: fns_protocol::WorkspaceId,
) -> Result<fns_protocol::ClientId, AgentErrorCode> {
    match fns_sync_core::read_persisted_identity(state_dir.join("state.sqlite"))
        .map_err(map_persisted_identity_error)?
    {
        Some(identity) if identity.workspace_id != workspace_id => {
            Err(AgentErrorCode::InvalidConfiguration)
        }
        Some(identity) => Ok(identity.client_id),
        None => ProjectClientIdentity::load_or_create_in(state_dir, project_id)
            .map(ProjectClientIdentity::get)
            .map_err(|_| AgentErrorCode::Filesystem),
    }
}

fn map_persisted_identity_error(error: fns_sync_core::SyncError) -> AgentErrorCode {
    match error {
        fns_sync_core::SyncError::InvalidConfiguration { .. } => {
            AgentErrorCode::InvalidConfiguration
        }
        fns_sync_core::SyncError::CorruptState { .. } => AgentErrorCode::StateCorrupt,
        fns_sync_core::SyncError::StorageUnavailable
        | fns_sync_core::SyncError::Filesystem(_)
        | fns_sync_core::SyncError::ScanIncomplete => AgentErrorCode::Filesystem,
        _ => AgentErrorCode::Core,
    }
}

fn bundled_agent_command() -> Result<AgentCommand, AgentErrorCode> {
    let executable = std::env::current_exe().map_err(|_| AgentErrorCode::SpawnFailed)?;
    let parent = executable.parent().ok_or(AgentErrorCode::SpawnFailed)?;
    Ok(AgentCommand::new(parent.join(if cfg!(windows) {
        "fns-agent.exe"
    } else {
        "fns-agent"
    }))
    .arg("__worker"))
}

fn stable_code(code: &AgentErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "core".into())
}

fn stable_failure(failure: &SyncFailure) -> String {
    if failure.cleanup.is_empty() {
        return stable_code(&failure.primary);
    }
    format!(
        "primary={};cleanup={}",
        stable_code(&failure.primary),
        failure
            .cleanup
            .iter()
            .map(stable_code)
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(crate) fn stable_error_code(failure: &SyncFailure) -> String {
    stable_failure(failure)
}

#[tauri::command]
pub async fn start_sync(
    project_id: String,
    tunnel_state: tauri::State<'_, TunnelState>,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<SyncStatus, SyncFailure> {
    let project = ProjectConfig::list_all()
        .map_err(|_| SyncFailure::primary(AgentErrorCode::Filesystem))?
        .into_iter()
        .find(|project| project.id.to_string() == project_id)
        .ok_or_else(|| SyncFailure::primary(AgentErrorCode::InvalidConfiguration))?;
    let state_dir = project_state_dir(&project_id);
    std::fs::create_dir_all(&project.local_root)
        .map_err(|_| SyncFailure::primary(AgentErrorCode::Filesystem))?;
    std::fs::create_dir_all(&state_dir)
        .map_err(|_| SyncFailure::primary(AgentErrorCode::Filesystem))?;
    let workspace_id = fns_protocol::WorkspaceId::parse(&project.workspace_id.to_string())
        .map_err(|_| SyncFailure::primary(AgentErrorCode::InvalidConfiguration))?;
    let client_id = resolve_project_client_id(&state_dir, &project_id, workspace_id)
        .map_err(SyncFailure::primary)?;
    let config = AgentConfig {
        schema_version: "fns-agent-config/1".into(),
        endpoint: String::new(),
        workspace_id,
        client_id,
        workspace_root: PathBuf::from(&project.local_root),
        token_file: state_dir.join("unused-private-pipe-token"),
        state_dir,
        sync: fns_agent::config::AgentSyncConfig {
            includes: project.sync.includes,
            excludes: project.sync.excludes,
            protect_secrets: project.sync.protect_secrets,
        },
        transport: fns_agent::config::AgentTransportConfig {
            max_active_transfers: 2,
        },
    };
    let start_result = sync_state
        .start(SessionStart {
            project_id: project_id.clone(),
            command: bundled_agent_command().map_err(SyncFailure::primary)?,
            config,
            runtime: Arc::new(DesktopSessionRuntime {
                credentials: Arc::clone(&sync_state.credentials),
                tunnels: tunnel_state.inner().clone(),
                tasks: sync_state.tasks.clone(),
            }),
            ssh_host: project.ssh_host_alias.clone(),
            remote_port: REMOTE_WORKSPACE_PORT,
            restart_policy: RestartPolicy::default(),
            #[cfg(test)]
            readiness_transition: None,
        })
        .await;
    start_result?;
    Ok(sync_state.status(&project_id).await)
}

#[tauri::command]
pub async fn stop_sync(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> LifecycleResult {
    sync_state.stop(&project_id).await
}

#[tauri::command]
pub async fn sync_status(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<SyncStatus, String> {
    Ok(sync_state.status(&project_id).await)
}

#[tauri::command]
pub async fn list_sync_conflicts(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<Vec<fns_agent::ConflictView>, SyncFailure> {
    sync_state.list_conflicts(&project_id).await
}

#[tauri::command]
pub async fn resolve_sync_conflict(
    project_id: String,
    identity: ConflictControlIdentity,
    input: fns_agent::ConflictResolutionInput,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<fns_agent::ConflictResolutionReceipt, SyncFailure> {
    sync_state
        .resolve_conflict(&project_id, identity, input)
        .await
}

#[tauri::command]
pub async fn cancel_sync_conflict_request(
    project_id: String,
    identity: ConflictControlIdentity,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<ConflictResolutionOperationView, SyncFailure> {
    sync_state
        .cancel_conflict_request(&project_id, identity)
        .await
}

#[tauri::command]
pub async fn cancel_sync_conflict_generation(
    project_id: String,
    project_generation: uuid::Uuid,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<Vec<ConflictResolutionOperationView>, String> {
    Ok(sync_state
        .cancel_conflict_generation(&project_id, project_generation)
        .await)
}

#[tauri::command]
pub async fn list_sync_conflict_operations(
    project_id: String,
    sync_state: tauri::State<'_, SyncState>,
) -> Result<Vec<ConflictResolutionOperationView>, String> {
    Ok(sync_state.conflict_operations(&project_id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_tunnel::{TunnelFactory, TunnelResource};
    use std::collections::{HashSet, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex as StdMutex, OnceLock};
    use tokio::sync::Notify;

    #[test]
    fn transport_timeouts_are_recoverable() {
        assert!(is_retryable(AgentErrorCode::IdleTimeout));
        assert!(is_retryable(AgentErrorCode::TransferTimeout));
    }

    enum SpawnPlan {
        Running,
        Crash(AgentErrorCode),
        SpawnError(AgentErrorCode),
        Blocked {
            entered: Arc<Notify>,
            release: Arc<Notify>,
        },
        BlockedError {
            entered: Arc<Notify>,
            release: Arc<Notify>,
            code: AgentErrorCode,
        },
        ShutdownError(AgentErrorCode),
        BlockedShutdown {
            entered: Arc<Notify>,
            release: Arc<Notify>,
        },
        Panic(Arc<Notify>),
    }

    enum WaitPlan {
        Running,
        Crash(AgentErrorCode),
        Panic(Arc<Notify>),
    }

    struct FakeProcess {
        wait_plan: WaitPlan,
        shutdown_result: RuntimeResult,
        shutdown_gate: Option<(Arc<Notify>, Arc<Notify>)>,
        shutdowns: Arc<AtomicUsize>,
        conflict_lists: Arc<AtomicUsize>,
        conflict_resolutions: Arc<StdMutex<Vec<fns_agent::ConflictResolutionInput>>>,
        conflict_list_error: Option<AgentErrorCode>,
        conflict_resolution_error: Option<AgentErrorCode>,
        conflict_resolution_gate: Option<(Arc<Notify>, Arc<Notify>)>,
    }

    impl ManagedAgent for FakeProcess {
        fn wait(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
            Box::pin(async move {
                match self.wait_plan {
                    WaitPlan::Running => std::future::pending().await,
                    WaitPlan::Crash(code) => {
                        tokio::task::yield_now().await;
                        Err(code)
                    }
                    WaitPlan::Panic(ref panicked) => {
                        panicked.notify_one();
                        panic!("fixture actor panic");
                    }
                }
            })
        }

        fn shutdown(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            let result = self.shutdown_result;
            let gate = self.shutdown_gate.clone();
            Box::pin(async move {
                if let Some((entered, release)) = gate {
                    entered.notify_one();
                    release.notified().await;
                }
                result
            })
        }

        fn list_conflicts(
            &mut self,
        ) -> RuntimeFuture<'_, Result<Vec<fns_agent::ConflictView>, AgentErrorCode>> {
            self.conflict_lists.fetch_add(1, Ordering::SeqCst);
            let error = self.conflict_list_error;
            Box::pin(async move { error.map_or_else(|| Ok(Vec::new()), Err) })
        }

        fn resolve_conflict(
            &mut self,
            input: fns_agent::ConflictResolutionInput,
        ) -> RuntimeFuture<'_, Result<fns_agent::ConflictResolutionReceipt, AgentErrorCode>>
        {
            self.conflict_resolutions.lock().unwrap().push(input);
            let error = self.conflict_resolution_error;
            let gate = self.conflict_resolution_gate.clone();
            Box::pin(async move {
                if let Some((entered, release)) = gate {
                    entered.notify_one();
                    release.notified().await;
                }
                if let Some(error) = error {
                    return Err(error);
                }
                Ok(fns_agent::ConflictResolutionReceipt {
                    status: fns_agent::ConflictResolutionReceiptStatus::Queued,
                    operation_id: fns_protocol::OperationId::parse(
                        "10000000-0000-4000-8000-000000000099",
                    )
                    .unwrap(),
                })
            })
        }
    }

    #[derive(Debug)]
    struct LaunchRecord {
        endpoint: String,
        token: String,
    }

    struct FakeRuntime {
        plans: StdMutex<VecDeque<SpawnPlan>>,
        token_requests: AtomicUsize,
        tunnel_opens: AtomicUsize,
        tunnel_closes: AtomicUsize,
        open_tunnels: StdMutex<HashSet<(String, uuid::Uuid)>>,
        shutdowns: Arc<AtomicUsize>,
        conflict_lists: Arc<AtomicUsize>,
        conflict_resolutions: Arc<StdMutex<Vec<fns_agent::ConflictResolutionInput>>>,
        conflict_list_error: StdMutex<Option<AgentErrorCode>>,
        conflict_resolution_error: StdMutex<Option<AgentErrorCode>>,
        conflict_resolution_gate: StdMutex<Option<(Arc<Notify>, Arc<Notify>)>>,
        launches: StdMutex<Vec<LaunchRecord>>,
        close_error: StdMutex<Option<AgentErrorCode>>,
        persistent_close_error: AtomicBool,
        close_attempts: StdMutex<Vec<(String, uuid::Uuid)>>,
        block_close: AtomicBool,
        close_entered: Notify,
        close_release: Notify,
    }

    impl FakeRuntime {
        fn new(plans: impl IntoIterator<Item = SpawnPlan>) -> Arc<Self> {
            Arc::new(Self {
                plans: StdMutex::new(plans.into_iter().collect()),
                token_requests: AtomicUsize::new(0),
                tunnel_opens: AtomicUsize::new(0),
                tunnel_closes: AtomicUsize::new(0),
                open_tunnels: StdMutex::new(HashSet::new()),
                shutdowns: Arc::new(AtomicUsize::new(0)),
                conflict_lists: Arc::new(AtomicUsize::new(0)),
                conflict_resolutions: Arc::new(StdMutex::new(Vec::new())),
                conflict_list_error: StdMutex::new(None),
                conflict_resolution_error: StdMutex::new(None),
                conflict_resolution_gate: StdMutex::new(None),
                launches: StdMutex::new(Vec::new()),
                close_error: StdMutex::new(None),
                persistent_close_error: AtomicBool::new(false),
                close_attempts: StdMutex::new(Vec::new()),
                block_close: AtomicBool::new(false),
                close_entered: Notify::new(),
                close_release: Notify::new(),
            })
        }

        fn spawn_count(&self) -> usize {
            self.launches.lock().unwrap().len()
        }
    }

    impl SessionRuntime for FakeRuntime {
        fn token_for_project(
            &self,
            _project_id: &str,
        ) -> Result<fns_platform::SecretToken, AgentErrorCode> {
            let sequence = self.token_requests.fetch_add(1, Ordering::SeqCst) + 1;
            fns_platform::SecretToken::from_private_ipc(
                format!("fixture-token-{sequence}").into_bytes(),
            )
            .map_err(|_| AgentErrorCode::AuthRequired)
        }

        fn open_tunnel<'a>(
            &'a self,
            project_id: &'a str,
            generation: uuid::Uuid,
            _ssh_host: &'a str,
            _remote_port: u16,
        ) -> RuntimeFuture<'a, Result<TunnelLease, SyncFailure>> {
            let sequence = self.tunnel_opens.fetch_add(1, Ordering::SeqCst);
            self.open_tunnels
                .lock()
                .unwrap()
                .insert((project_id.to_owned(), generation));
            Box::pin(async move {
                Ok(TunnelLease::unmanaged(
                    19050 + u16::try_from(sequence).unwrap(),
                ))
            })
        }

        fn close_tunnel<'a>(
            &'a self,
            project_id: &'a str,
            generation: uuid::Uuid,
            _ssh_host: &'a str,
        ) -> RuntimeFuture<'a, LifecycleResult> {
            self.close_attempts
                .lock()
                .unwrap()
                .push((project_id.to_owned(), generation));
            let block = self.block_close.load(Ordering::SeqCst);
            let persistent_error = self.persistent_close_error.load(Ordering::SeqCst);
            let one_shot_error = self.close_error.lock().unwrap().take();
            let key = (project_id.to_owned(), generation);
            Box::pin(async move {
                if block {
                    self.close_entered.notify_one();
                    self.close_release.notified().await;
                }
                if persistent_error {
                    return Err(AgentErrorCode::Network.into());
                }
                if let Some(code) = one_shot_error {
                    return Err(code.into());
                }
                if self.open_tunnels.lock().unwrap().remove(&key) {
                    self.tunnel_closes.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            })
        }

        fn spawn_agent<'a>(
            &'a self,
            _command: AgentCommand,
            config: AgentConfig,
            token: fns_platform::SecretToken,
            _options: AgentProcessOptions,
        ) -> RuntimeFuture<'a, Result<Box<dyn ManagedAgent>, AgentErrorCode>> {
            let plan = self
                .plans
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(SpawnPlan::Running);
            let token = token.with_exposed(|bytes| String::from_utf8(bytes.to_vec()).unwrap());
            self.launches.lock().unwrap().push(LaunchRecord {
                endpoint: config.endpoint,
                token,
            });
            let shutdowns = Arc::clone(&self.shutdowns);
            let conflict_lists = Arc::clone(&self.conflict_lists);
            let conflict_resolutions = Arc::clone(&self.conflict_resolutions);
            let conflict_list_error = *self.conflict_list_error.lock().unwrap();
            let conflict_resolution_error = *self.conflict_resolution_error.lock().unwrap();
            let conflict_resolution_gate = self.conflict_resolution_gate.lock().unwrap().clone();
            Box::pin(async move {
                let (wait_plan, shutdown_result, shutdown_gate) = match plan {
                    SpawnPlan::Running => (WaitPlan::Running, Ok(()), None),
                    SpawnPlan::Crash(code) => (WaitPlan::Crash(code), Ok(()), None),
                    SpawnPlan::SpawnError(code) => return Err(code),
                    SpawnPlan::Blocked { entered, release } => {
                        entered.notify_one();
                        release.notified().await;
                        (WaitPlan::Running, Ok(()), None)
                    }
                    SpawnPlan::BlockedError {
                        entered,
                        release,
                        code,
                    } => {
                        entered.notify_one();
                        release.notified().await;
                        return Err(code);
                    }
                    SpawnPlan::ShutdownError(code) => (WaitPlan::Running, Err(code), None),
                    SpawnPlan::BlockedShutdown { entered, release } => {
                        (WaitPlan::Running, Ok(()), Some((entered, release)))
                    }
                    SpawnPlan::Panic(panicked) => (WaitPlan::Panic(panicked), Ok(()), None),
                };
                Ok(Box::new(FakeProcess {
                    wait_plan,
                    shutdown_result,
                    shutdown_gate,
                    shutdowns,
                    conflict_lists,
                    conflict_resolutions,
                    conflict_list_error,
                    conflict_resolution_error,
                    conflict_resolution_gate,
                }) as Box<dyn ManagedAgent>)
            })
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct TunnelIdentity {
        project_id: String,
        generation: uuid::Uuid,
        ssh_host: String,
        remote_port: u16,
    }

    struct BlockingOpenControl {
        opener_started: Notify,
        opener_published: Notify,
        opener_release: (StdMutex<bool>, Condvar),
        close_observed: (StdMutex<usize>, Condvar),
        cleanup_release: Notify,
        open_future_dropped: AtomicBool,
        entry: StdMutex<Option<TunnelIdentity>>,
    }

    impl BlockingOpenControl {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                opener_started: Notify::new(),
                opener_published: Notify::new(),
                opener_release: (StdMutex::new(false), Condvar::new()),
                close_observed: (StdMutex::new(0), Condvar::new()),
                cleanup_release: Notify::new(),
                open_future_dropped: AtomicBool::new(false),
                entry: StdMutex::new(None),
            })
        }

        fn release_opener(&self) {
            *self.opener_release.0.lock().unwrap() = true;
            self.opener_release.1.notify_all();
        }

        fn wait_for_opener_release(&self) {
            let mut released = self.opener_release.0.lock().unwrap();
            while !*released {
                released = self.opener_release.1.wait(released).unwrap();
            }
        }

        fn record_close(&self) {
            let mut closes = self.close_observed.0.lock().unwrap();
            *closes += 1;
            self.close_observed.1.notify_all();
        }

        fn wait_for_close(&self) {
            let mut closes = self.close_observed.0.lock().unwrap();
            while *closes == 0 {
                closes = self.close_observed.1.wait(closes).unwrap();
            }
        }
    }

    struct OpenFutureGuard {
        control: Arc<BlockingOpenControl>,
        armed: bool,
    }

    impl Drop for OpenFutureGuard {
        fn drop(&mut self) {
            if self.armed {
                self.control
                    .open_future_dropped
                    .store(true, Ordering::Release);
            }
        }
    }

    struct TrackedProcess {
        active_children: Arc<AtomicUsize>,
        running: bool,
    }

    impl TrackedProcess {
        fn stop(&mut self) {
            if self.running {
                self.running = false;
                self.active_children.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    impl ManagedAgent for TrackedProcess {
        fn wait(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
            Box::pin(std::future::pending())
        }

        fn shutdown(&mut self) -> RuntimeFuture<'_, RuntimeResult> {
            self.stop();
            Box::pin(async { Ok(()) })
        }
    }

    impl Drop for TrackedProcess {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct BlockingOpenRuntime {
        control: Arc<BlockingOpenControl>,
        first_open: AtomicBool,
        first_close: AtomicBool,
        active_children: Arc<AtomicUsize>,
        spawns: AtomicUsize,
    }

    impl BlockingOpenRuntime {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                control: BlockingOpenControl::new(),
                first_open: AtomicBool::new(true),
                first_close: AtomicBool::new(true),
                active_children: Arc::new(AtomicUsize::new(0)),
                spawns: AtomicUsize::new(0),
            })
        }

        fn identity(
            project_id: &str,
            generation: uuid::Uuid,
            ssh_host: &str,
            remote_port: u16,
        ) -> TunnelIdentity {
            TunnelIdentity {
                project_id: project_id.to_owned(),
                generation,
                ssh_host: ssh_host.to_owned(),
                remote_port,
            }
        }
    }

    impl SessionRuntime for BlockingOpenRuntime {
        fn token_for_project(
            &self,
            _project_id: &str,
        ) -> Result<fns_platform::SecretToken, AgentErrorCode> {
            fns_platform::SecretToken::from_private_ipc(b"blocking-open-token".to_vec())
                .map_err(|_| AgentErrorCode::AuthRequired)
        }

        fn open_tunnel<'a>(
            &'a self,
            project_id: &'a str,
            generation: uuid::Uuid,
            ssh_host: &'a str,
            remote_port: u16,
        ) -> RuntimeFuture<'a, Result<TunnelLease, SyncFailure>> {
            let identity = Self::identity(project_id, generation, ssh_host, remote_port);
            if !self.first_open.swap(false, Ordering::SeqCst) {
                *self.control.entry.lock().unwrap() = Some(identity.clone());
                let cleanup_control = Arc::clone(&self.control);
                return Box::pin(async move {
                    Ok(TunnelLease::new(
                        19051,
                        Box::new(move || {
                            let mut entry = cleanup_control.entry.lock().unwrap();
                            if entry.as_ref() == Some(&identity) {
                                entry.take();
                            }
                            Ok(())
                        }),
                    ))
                });
            }

            let control = Arc::clone(&self.control);
            Box::pin(async move {
                let mut guard = OpenFutureGuard {
                    control: Arc::clone(&control),
                    armed: true,
                };
                let blocking_control = Arc::clone(&control);
                let result = tokio::task::spawn_blocking(move || {
                    blocking_control.opener_started.notify_one();
                    blocking_control.wait_for_opener_release();
                    if blocking_control.open_future_dropped.load(Ordering::Acquire) {
                        blocking_control.wait_for_close();
                    }
                    *blocking_control.entry.lock().unwrap() = Some(identity.clone());
                    blocking_control.opener_published.notify_one();
                    let cleanup_control = Arc::clone(&blocking_control);
                    Ok(TunnelLease::new(
                        19050,
                        Box::new(move || {
                            let mut entry = cleanup_control.entry.lock().unwrap();
                            if entry.as_ref() == Some(&identity) {
                                entry.take();
                            }
                            Ok(())
                        }),
                    ))
                })
                .await
                .map_err(|_| AgentErrorCode::Network)?;
                guard.armed = false;
                result
            })
        }

        fn close_tunnel<'a>(
            &'a self,
            project_id: &'a str,
            generation: uuid::Uuid,
            ssh_host: &'a str,
        ) -> RuntimeFuture<'a, LifecycleResult> {
            let control = Arc::clone(&self.control);
            let project_id = project_id.to_owned();
            let ssh_host = ssh_host.to_owned();
            let block = self.first_close.swap(false, Ordering::SeqCst);
            Box::pin(async move {
                {
                    let mut entry = control.entry.lock().unwrap();
                    if entry.as_ref().is_some_and(|entry| {
                        entry.project_id == project_id
                            && entry.generation == generation
                            && entry.ssh_host == ssh_host
                    }) {
                        entry.take();
                    }
                }
                control.record_close();
                if block {
                    control.cleanup_release.notified().await;
                }
                Ok(())
            })
        }

        fn spawn_agent<'a>(
            &'a self,
            _command: AgentCommand,
            _config: AgentConfig,
            _token: fns_platform::SecretToken,
            _options: AgentProcessOptions,
        ) -> RuntimeFuture<'a, Result<Box<dyn ManagedAgent>, AgentErrorCode>> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            self.active_children.fetch_add(1, Ordering::SeqCst);
            let active_children = Arc::clone(&self.active_children);
            Box::pin(async move {
                Ok(Box::new(TrackedProcess {
                    active_children,
                    running: true,
                }) as Box<dyn ManagedAgent>)
            })
        }
    }

    struct DesktopOpenControl {
        opener_started: Notify,
        opener_release: (StdMutex<bool>, Condvar),
        active_tunnels: AtomicUsize,
        close_count: AtomicUsize,
        close_failures: AtomicUsize,
        open_count: AtomicUsize,
        panic_next: AtomicBool,
        keys: StdMutex<Vec<String>>,
    }

    impl DesktopOpenControl {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                opener_started: Notify::new(),
                opener_release: (StdMutex::new(false), Condvar::new()),
                active_tunnels: AtomicUsize::new(0),
                close_count: AtomicUsize::new(0),
                close_failures: AtomicUsize::new(0),
                open_count: AtomicUsize::new(0),
                panic_next: AtomicBool::new(false),
                keys: StdMutex::new(Vec::new()),
            })
        }

        fn release_opener(&self) {
            *self.opener_release.0.lock().unwrap() = true;
            self.opener_release.1.notify_all();
        }
    }

    struct DesktopOpenFactory {
        control: Arc<DesktopOpenControl>,
    }

    impl TunnelFactory for DesktopOpenFactory {
        fn create(
            &self,
            tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn TunnelResource>, crate::ssh_tunnel::TunnelCreateFailure> {
            let sequence = self.control.open_count.fetch_add(1, Ordering::SeqCst);
            self.control
                .keys
                .lock()
                .unwrap()
                .push(tunnel_key.to_owned());
            self.control.opener_started.notify_one();
            if sequence == 0 {
                let mut released = self.control.opener_release.0.lock().unwrap();
                while !*released {
                    released = self.control.opener_release.1.wait(released).unwrap();
                }
            }
            assert!(
                !self.control.panic_next.swap(false, Ordering::SeqCst),
                "fixture opener panic"
            );
            self.control.active_tunnels.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(DesktopOpenTunnel {
                port: 19100 + u16::try_from(sequence).unwrap(),
                control: Arc::clone(&self.control),
                closed: false,
            }))
        }
    }

    struct DesktopOpenTunnel {
        port: u16,
        control: Arc<DesktopOpenControl>,
        closed: bool,
    }

    impl DesktopOpenTunnel {
        fn close_once(&mut self) {
            if !self.closed {
                self.closed = true;
                self.control.active_tunnels.fetch_sub(1, Ordering::SeqCst);
                self.control.close_count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    impl TunnelResource for DesktopOpenTunnel {
        fn local_port(&self) -> u16 {
            self.port
        }

        fn is_alive(&mut self) -> Result<bool, TunnelFailure> {
            Ok(!self.closed)
        }

        fn close(&mut self) -> Result<(), TunnelFailure> {
            if self
                .control
                .close_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |failures| {
                    failures.checked_sub(1)
                })
                .is_ok()
            {
                return Err(crate::ssh_tunnel::TunnelErrorCode::WaitTimeout.into());
            }
            self.close_once();
            Ok(())
        }
    }

    impl Drop for DesktopOpenTunnel {
        fn drop(&mut self) {
            self.close_once();
        }
    }

    struct DesktopOpenCredentials;

    impl CredentialProvider for DesktopOpenCredentials {
        fn token_for_project(
            &self,
            _project_id: &str,
        ) -> Result<fns_platform::SecretToken, AgentErrorCode> {
            fns_platform::SecretToken::from_private_ipc(b"desktop-open-token".to_vec())
                .map_err(|_| AgentErrorCode::AuthRequired)
        }
    }

    fn state() -> Arc<SyncState> {
        Arc::new(SyncState::with_credentials(Arc::new(
            UnavailableCredentialProvider,
        )))
    }

    fn state_with_deadlines(
        control_request_deadline: Duration,
        shutdown_deadline: Duration,
    ) -> Arc<SyncState> {
        let mut state = SyncState::with_credentials_and_shutdown_deadline(
            Arc::new(UnavailableCredentialProvider),
            shutdown_deadline,
        );
        state.control_request_deadline = control_request_deadline;
        Arc::new(state)
    }

    fn conflict_identity(request_id: &str, project_generation: &str) -> ConflictControlIdentity {
        ConflictControlIdentity {
            request_id: fns_protocol::RequestId::parse(request_id).unwrap(),
            project_generation: uuid::Uuid::parse_str(project_generation).unwrap(),
        }
    }

    fn conflict_input(
        conflict_revision: &str,
        choice: fns_agent::WorkspaceConflictChoice,
    ) -> fns_agent::ConflictResolutionInput {
        fns_agent::ConflictResolutionInput {
            conflict_id: fns_agent::ConflictId::parse("10000000-0000-4000-8000-000000000031")
                .unwrap(),
            conflict_revision: fns_agent::WorkspaceConflictRevision::parse(conflict_revision)
                .unwrap(),
            choice,
        }
    }

    async fn wait_for_conflict_phase(
        state: &SyncState,
        project_id: &str,
        request_id: fns_protocol::RequestId,
        phase: ConflictResolutionOperationPhase,
    ) -> ConflictResolutionOperationView {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(operation) = state
                    .conflict_operations(project_id)
                    .await
                    .into_iter()
                    .find(|operation| {
                        operation.request_id == request_id && operation.phase == phase
                    })
                {
                    return operation;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("conflict operation did not reach the expected phase")
    }

    fn session_start<R>(project_id: &str, runtime: Arc<R>) -> SessionStart
    where
        R: SessionRuntime,
    {
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        let root = ROOT
            .get_or_init(|| std::env::temp_dir().join("fns-desktop-actor-fixture"))
            .clone();
        SessionStart {
            project_id: project_id.into(),
            command: AgentCommand::new("fixture-agent"),
            config: AgentConfig {
                schema_version: "fns-agent-config/1".into(),
                endpoint: String::new(),
                workspace_id: fns_protocol::WorkspaceId::parse(
                    "10000000-0000-4000-8000-000000000001",
                )
                .unwrap(),
                client_id: fns_protocol::ClientId::parse("20000000-0000-4000-8000-000000000002")
                    .unwrap(),
                workspace_root: root.join(project_id).join("workspace"),
                state_dir: root.join(project_id).join("state"),
                token_file: root.join(project_id).join("unused-token"),
                sync: fns_agent::config::AgentSyncConfig {
                    includes: vec!["**/*".into()],
                    excludes: Vec::new(),
                    protect_secrets: true,
                },
                transport: fns_agent::config::AgentTransportConfig {
                    max_active_transfers: 2,
                },
            },
            runtime,
            ssh_host: "fixture-host".into(),
            remote_port: REMOTE_WORKSPACE_PORT,
            restart_policy: RestartPolicy {
                max_restarts: 2,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(2),
                process_options: AgentProcessOptions {
                    startup_timeout: Duration::from_secs(1),
                    shutdown_timeout: Duration::from_secs(1),
                },
            },
            readiness_transition: None,
        }
    }

    async fn wait_until(check: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("condition timed out");
    }

    async fn wait_for_stopping(state: &SyncState, project_id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.status(project_id).await.message == "stopping" {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("session did not enter stopping");
    }

    #[test]
    fn unavailable_provider_fails_closed() {
        let error = UnavailableCredentialProvider
            .token_for_project("project")
            .unwrap_err();
        assert_eq!(error, AgentErrorCode::AuthRequired);
    }

    #[tokio::test]
    async fn authentication_failure_does_not_open_a_tunnel_or_spawn_an_agent() {
        let control = DesktopOpenControl::new();
        let runtime = Arc::new(DesktopSessionRuntime {
            credentials: Arc::new(UnavailableCredentialProvider),
            tunnels: TunnelState::with_factory(Arc::new(DesktopOpenFactory {
                control: Arc::clone(&control),
            })),
            tasks: TaskTracker::new(),
        });
        let state = state();

        let failure = state
            .start(session_start(
                "authentication-failure",
                Arc::clone(&runtime),
            ))
            .await
            .unwrap_err();

        assert_eq!(failure, SyncFailure::primary(AgentErrorCode::AuthRequired));
        wait_until(|| {
            state
                .registry
                .try_lock()
                .is_ok_and(|registry| registry.sessions.is_empty())
        })
        .await;
        runtime.tasks.close();
        runtime.tasks.wait().await;
        assert_eq!(control.open_count.load(Ordering::SeqCst), 0);
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 0);
        assert_eq!(control.close_count.load(Ordering::SeqCst), 0);
        assert!(state.registry.lock().await.sessions.is_empty());
    }

    #[tokio::test]
    async fn conflict_commands_route_to_the_running_generation_and_return_a_receipt() {
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        let state = state();
        state
            .start(session_start("conflict-control", Arc::clone(&runtime)))
            .await
            .unwrap();

        assert_eq!(state.list_conflicts("conflict-control").await.unwrap(), []);
        let input = fns_agent::ConflictResolutionInput {
            conflict_id: fns_agent::ConflictId::parse("10000000-0000-4000-8000-000000000031")
                .unwrap(),
            conflict_revision: fns_agent::WorkspaceConflictRevision::parse("7").unwrap(),
            choice: fns_agent::WorkspaceConflictChoice::Incoming,
        };
        let identity = conflict_identity(
            "30000000-0000-4000-8000-000000000031",
            "40000000-0000-4000-8000-000000000031",
        );
        let receipt = state
            .resolve_conflict("conflict-control", identity, input)
            .await
            .unwrap();

        assert_eq!(
            receipt.status,
            fns_agent::ConflictResolutionReceiptStatus::Queued
        );
        assert_eq!(runtime.conflict_lists.load(Ordering::SeqCst), 1);
        assert_eq!(*runtime.conflict_resolutions.lock().unwrap(), vec![input]);
        assert_eq!(state.stop("conflict-control").await, Ok(()));
        assert_eq!(
            state.list_conflicts("conflict-control").await,
            Err(SyncFailure::primary(AgentErrorCode::AbnormalExit))
        );
    }

    #[tokio::test]
    async fn conflict_command_errors_keep_their_stable_code_at_the_tauri_boundary() {
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        *runtime.conflict_list_error.lock().unwrap() = Some(AgentErrorCode::StateCorrupt);
        *runtime.conflict_resolution_error.lock().unwrap() =
            Some(AgentErrorCode::MergeFileRequired);
        let state = state();
        state
            .start(session_start("conflict-errors", Arc::clone(&runtime)))
            .await
            .unwrap();

        assert_eq!(
            state.list_conflicts("conflict-errors").await,
            Err(SyncFailure::primary(AgentErrorCode::StateCorrupt))
        );
        let input = fns_agent::ConflictResolutionInput {
            conflict_id: fns_agent::ConflictId::parse("10000000-0000-4000-8000-000000000031")
                .unwrap(),
            conflict_revision: fns_agent::WorkspaceConflictRevision::parse("7").unwrap(),
            choice: fns_agent::WorkspaceConflictChoice::Merged,
        };
        let identity = conflict_identity(
            "30000000-0000-4000-8000-000000000032",
            "40000000-0000-4000-8000-000000000032",
        );
        assert_eq!(
            state
                .resolve_conflict("conflict-errors", identity, input)
                .await,
            Err(SyncFailure::primary(AgentErrorCode::MergeFileRequired))
        );
        assert_eq!(
            serde_json::to_value(SyncFailure::primary(AgentErrorCode::MergeFileRequired)).unwrap(),
            serde_json::json!({
                "primary": "merge_file_required",
                "cleanup": [],
            })
        );

        assert_eq!(state.stop("conflict-errors").await, Ok(()));
    }

    #[tokio::test]
    async fn conflict_request_generation_cancels_pending_but_preserves_dispatched_work() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        *runtime.conflict_resolution_gate.lock().unwrap() =
            Some((Arc::clone(&entered), Arc::clone(&release)));
        let state = state();
        state
            .start(session_start("conflict-cancel", Arc::clone(&runtime)))
            .await
            .unwrap();

        let first_identity = conflict_identity(
            "30000000-0000-4000-8000-000000000041",
            "40000000-0000-4000-8000-000000000041",
        );
        let second_identity = conflict_identity(
            "30000000-0000-4000-8000-000000000042",
            "40000000-0000-4000-8000-000000000041",
        );
        let first_input = conflict_input("7", fns_agent::WorkspaceConflictChoice::Incoming);
        let second_input = conflict_input("8", fns_agent::WorkspaceConflictChoice::Current);

        let first = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-cancel", first_identity, first_input)
                    .await
            })
        };
        entered.notified().await;
        let second = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-cancel", second_identity, second_input)
                    .await
            })
        };
        wait_for_conflict_phase(
            &state,
            "conflict-cancel",
            second_identity.request_id,
            ConflictResolutionOperationPhase::Pending,
        )
        .await;

        let cancelled = state
            .cancel_conflict_generation("conflict-cancel", first_identity.project_generation)
            .await;
        assert_eq!(
            cancelled
                .iter()
                .find(|operation| operation.request_id == first_identity.request_id)
                .unwrap()
                .phase,
            ConflictResolutionOperationPhase::Dispatched
        );
        assert_eq!(
            cancelled
                .iter()
                .find(|operation| operation.request_id == second_identity.request_id)
                .unwrap()
                .phase,
            ConflictResolutionOperationPhase::Cancelled
        );
        assert_eq!(
            second.await.unwrap(),
            Err(SyncFailure::primary(AgentErrorCode::RequestCancelled))
        );

        release.notify_one();
        assert!(first.await.unwrap().is_ok());
        wait_for_conflict_phase(
            &state,
            "conflict-cancel",
            first_identity.request_id,
            ConflictResolutionOperationPhase::Queued,
        )
        .await;
        assert_eq!(
            *runtime.conflict_resolutions.lock().unwrap(),
            vec![first_input]
        );
        assert_eq!(state.stop("conflict-cancel").await, Ok(()));
    }

    #[tokio::test]
    async fn duplicate_conflict_request_dispatches_once_and_rejects_changed_input() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        *runtime.conflict_resolution_gate.lock().unwrap() =
            Some((Arc::clone(&entered), Arc::clone(&release)));
        let state = state();
        state
            .start(session_start("conflict-duplicate", Arc::clone(&runtime)))
            .await
            .unwrap();
        let identity = conflict_identity(
            "30000000-0000-4000-8000-000000000043",
            "40000000-0000-4000-8000-000000000043",
        );
        let input = conflict_input("9", fns_agent::WorkspaceConflictChoice::Incoming);

        let first = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-duplicate", identity, input)
                    .await
            })
        };
        entered.notified().await;
        let duplicate = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-duplicate", identity, input)
                    .await
            })
        };
        tokio::task::yield_now().await;
        release.notify_one();

        let first_receipt = first.await.unwrap().unwrap();
        let duplicate_receipt = duplicate.await.unwrap().unwrap();
        assert_eq!(first_receipt, duplicate_receipt);
        assert_eq!(*runtime.conflict_resolutions.lock().unwrap(), vec![input]);
        assert_eq!(
            state
                .resolve_conflict(
                    "conflict-duplicate",
                    identity,
                    conflict_input("10", fns_agent::WorkspaceConflictChoice::Current),
                )
                .await,
            Err(SyncFailure::primary(AgentErrorCode::ConflictRequestChanged))
        );
        assert_eq!(state.stop("conflict-duplicate").await, Ok(()));
    }

    #[tokio::test]
    async fn timed_out_conflict_request_keeps_its_late_queued_receipt() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        *runtime.conflict_resolution_gate.lock().unwrap() =
            Some((Arc::clone(&entered), Arc::clone(&release)));
        let state = state_with_deadlines(Duration::from_millis(20), Duration::from_secs(1));
        state
            .start(session_start("conflict-late", Arc::clone(&runtime)))
            .await
            .unwrap();
        let identity = conflict_identity(
            "30000000-0000-4000-8000-000000000044",
            "40000000-0000-4000-8000-000000000044",
        );
        let input = conflict_input("11", fns_agent::WorkspaceConflictChoice::Incoming);
        let request = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-late", identity, input)
                    .await
            })
        };
        entered.notified().await;
        assert_eq!(
            request.await.unwrap(),
            Err(SyncFailure::primary(AgentErrorCode::RequestTimeout))
        );
        let cancellation = state
            .cancel_conflict_request("conflict-late", identity)
            .await
            .unwrap();
        assert_eq!(
            cancellation.phase,
            ConflictResolutionOperationPhase::Dispatched
        );

        release.notify_one();
        let operation = wait_for_conflict_phase(
            &state,
            "conflict-late",
            identity.request_id,
            ConflictResolutionOperationPhase::Queued,
        )
        .await;
        assert_eq!(
            operation.receipt.unwrap().status,
            fns_agent::ConflictResolutionReceiptStatus::Queued
        );
        assert_eq!(operation.error, None);
        assert_eq!(state.stop("conflict-late").await, Ok(()));
    }

    #[tokio::test]
    async fn app_shutdown_gives_an_in_flight_conflict_request_a_terminal_failure() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        *runtime.conflict_resolution_gate.lock().unwrap() =
            Some((Arc::clone(&entered), Arc::clone(&release)));
        let state = state_with_deadlines(Duration::from_secs(1), Duration::from_millis(20));
        state
            .start(session_start("conflict-shutdown", Arc::clone(&runtime)))
            .await
            .unwrap();
        let identity = conflict_identity(
            "30000000-0000-4000-8000-000000000045",
            "40000000-0000-4000-8000-000000000045",
        );
        let input = conflict_input("12", fns_agent::WorkspaceConflictChoice::Current);
        let request = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                state
                    .resolve_conflict("conflict-shutdown", identity, input)
                    .await
            })
        };
        entered.notified().await;

        assert_eq!(
            state.shutdown_all().await,
            Err(SyncFailure::primary(AgentErrorCode::ShutdownTimeout))
        );
        assert!(request.await.unwrap().is_err());
        let operation = wait_for_conflict_phase(
            &state,
            "conflict-shutdown",
            identity.request_id,
            ConflictResolutionOperationPhase::Failed,
        )
        .await;
        assert_eq!(operation.error, Some(AgentErrorCode::AbnormalExit));
        release.notify_one();
    }

    #[test]
    fn primary_and_cleanup_failures_keep_both_structured_identities() {
        let failure = merge_cleanup_result(
            Err(AgentErrorCode::SpawnFailed),
            Err(SyncFailure::primary(AgentErrorCode::Network)),
        )
        .unwrap_err();
        assert_eq!(failure.primary, AgentErrorCode::SpawnFailed);
        assert_eq!(failure.cleanup, vec![AgentErrorCode::Network]);
    }

    #[tokio::test]
    async fn startup_and_stop_double_failures_reach_completion_and_status() {
        let startup_runtime =
            FakeRuntime::new([SpawnPlan::SpawnError(AgentErrorCode::SpawnFailed)]);
        *startup_runtime.close_error.lock().unwrap() = Some(AgentErrorCode::Network);
        let startup_state = state();
        let startup_failure = startup_state
            .start(session_start(
                "startup-double-failure",
                Arc::clone(&startup_runtime),
            ))
            .await
            .unwrap_err();
        assert_eq!(startup_failure.primary, AgentErrorCode::SpawnFailed);
        assert_eq!(startup_failure.cleanup, vec![AgentErrorCode::Network]);
        let startup_status = startup_state.status("startup-double-failure").await;
        assert_eq!(startup_status.error, Some(startup_failure));
        assert_eq!(
            startup_status.message,
            "primary=spawn_failed;cleanup=network"
        );

        let stop_runtime =
            FakeRuntime::new([SpawnPlan::ShutdownError(AgentErrorCode::ShutdownTimeout)]);
        let stop_state = state();
        stop_state
            .start(session_start(
                "stop-double-failure",
                Arc::clone(&stop_runtime),
            ))
            .await
            .unwrap();
        *stop_runtime.close_error.lock().unwrap() = Some(AgentErrorCode::Network);
        let stop_failure = stop_state.stop("stop-double-failure").await.unwrap_err();
        assert_eq!(stop_failure.primary, AgentErrorCode::ShutdownTimeout);
        assert_eq!(stop_failure.cleanup, vec![AgentErrorCode::Network]);
        assert_eq!(
            stop_state.status("stop-double-failure").await.error,
            Some(stop_failure)
        );
    }

    #[test]
    fn project_state_is_backend_owned() {
        assert!(
            std::path::Path::new(&project_state_dir("project")).ends_with("projects-project/state")
        );
    }

    fn identity_workspace_id() -> fns_protocol::WorkspaceId {
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000201").unwrap()
    }

    fn other_identity_workspace_id() -> fns_protocol::WorkspaceId {
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000202").unwrap()
    }

    fn durable_client_id() -> fns_protocol::ClientId {
        fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000203").unwrap()
    }

    #[test]
    fn missing_state_database_generates_and_reuses_file_identity() {
        let state_dir = tempfile::tempdir().unwrap();
        let first =
            resolve_project_client_id(state_dir.path(), "new-project", identity_workspace_id())
                .unwrap();

        assert!(!state_dir.path().join("state.sqlite").exists());
        assert!(state_dir.path().join("client-new-project.json").is_file());
        assert_eq!(
            resolve_project_client_id(state_dir.path(), "new-project", identity_workspace_id(),)
                .unwrap(),
            first
        );
    }

    #[test]
    fn persisted_state_identity_wins_when_file_identity_is_missing() {
        let state_dir = tempfile::tempdir().unwrap();
        let state = fns_sync_core::SqliteState::open(
            state_dir.path().join("state.sqlite"),
            identity_workspace_id(),
            durable_client_id(),
        )
        .unwrap();
        drop(state);

        assert!(!state_dir.path().join("client-project.json").exists());
        assert_eq!(
            resolve_project_client_id(state_dir.path(), "project", identity_workspace_id())
                .unwrap(),
            durable_client_id()
        );
    }

    #[test]
    fn persisted_state_identity_wins_when_file_identity_has_drifted() {
        let state_dir = tempfile::tempdir().unwrap();
        let drifted = ProjectClientIdentity::load_or_create_in(state_dir.path(), "project")
            .unwrap()
            .get();
        assert_ne!(drifted, durable_client_id());
        let state = fns_sync_core::SqliteState::open(
            state_dir.path().join("state.sqlite"),
            identity_workspace_id(),
            durable_client_id(),
        )
        .unwrap();
        drop(state);

        assert_eq!(
            resolve_project_client_id(state_dir.path(), "project", identity_workspace_id())
                .unwrap(),
            durable_client_id()
        );
    }

    #[test]
    fn persisted_state_workspace_mismatch_fails_without_fallback() {
        let state_dir = tempfile::tempdir().unwrap();
        let state = fns_sync_core::SqliteState::open(
            state_dir.path().join("state.sqlite"),
            identity_workspace_id(),
            durable_client_id(),
        )
        .unwrap();
        drop(state);

        assert_eq!(
            resolve_project_client_id(state_dir.path(), "project", other_identity_workspace_id()),
            Err(AgentErrorCode::InvalidConfiguration)
        );
        assert!(!state_dir.path().join("client-project.json").exists());
    }

    #[test]
    fn corrupt_persisted_state_fails_without_generating_file_identity() {
        let state_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            state_dir.path().join("state.sqlite"),
            b"not a sqlite database",
        )
        .unwrap();

        assert_eq!(
            resolve_project_client_id(state_dir.path(), "project", identity_workspace_id()),
            Err(AgentErrorCode::StateCorrupt)
        );
        assert!(!state_dir.path().join("client-project.json").exists());
    }

    #[tokio::test]
    async fn stale_generation_cannot_overwrite_successor_error_state() {
        let old_generation = uuid::Uuid::new_v4();
        let successor_generation = uuid::Uuid::new_v4();
        let (commands, _receiver) = mpsc::channel(1);
        let commands = Arc::new(commands);
        let startup_waiters = StartupWaiters::new(successor_generation, Arc::downgrade(&commands));
        let (_readiness_tx, readiness) = watch::channel(Some(Ok(())));
        let (_completion_tx, completion) = watch::channel(None);
        let runtime = FakeRuntime::new([]);
        let registry = Mutex::new(SyncRegistry::default());
        registry.lock().await.sessions.insert(
            "project".into(),
            SessionRecord {
                generation: successor_generation,
                commands,
                readiness,
                startup_waiters,
                completion,
                actor_abort: None,
                runtime,
                ssh_host: "fixture-host".into(),
                stop_requested: false,
                running: true,
                local_port: Some(19050),
                message: "running".into(),
                failure: None,
            },
        );

        finish_registry_session(
            &registry,
            "project",
            old_generation,
            Some(stable_code(&AgentErrorCode::AbnormalExit)),
            Some(SyncFailure::primary(AgentErrorCode::AbnormalExit)),
        )
        .await;

        let registry = registry.lock().await;
        assert!(registry.sessions.contains_key("project"));
        assert!(!registry.last_errors.contains_key("project"));
    }

    #[tokio::test]
    async fn repeated_start_stop_and_restart_are_idempotent() {
        let runtime = FakeRuntime::new([SpawnPlan::Running, SpawnPlan::Running]);
        let state = state();

        state
            .start(session_start("project", Arc::clone(&runtime)))
            .await
            .unwrap();
        state
            .start(session_start("project", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 1);

        let (first_stop, second_stop) = tokio::join!(state.stop("project"), state.stop("project"));
        assert_eq!(first_stop, Ok(()));
        assert_eq!(second_stop, Ok(()));
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.status("project").await.message, "stopped");

        state
            .start(session_start("project", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 2);
        state.stop("project").await.unwrap();
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 2);
        assert_eq!(state.registry.lock().await.sessions.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn simultaneous_starts_join_the_same_failed_readiness_outcome() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::BlockedError {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            code: AgentErrorCode::SpawnFailed,
        }]);
        let state = state();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_state = Arc::clone(&state);
        let first_runtime = Arc::clone(&runtime);
        let first_barrier = Arc::clone(&barrier);
        let mut first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_state
                .start(session_start("simultaneous-failure", first_runtime))
                .await
        });
        let second_state = Arc::clone(&state);
        let second_runtime = Arc::clone(&runtime);
        let second_barrier = Arc::clone(&barrier);
        let mut second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_state
                .start(session_start("simultaneous-failure", second_runtime))
                .await
        });

        barrier.wait().await;
        entered.notified().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut first)
                .await
                .is_err(),
            "first start returned before shared readiness"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err(),
            "second start returned before shared readiness"
        );

        release.notify_one();
        assert_eq!(
            first.await.unwrap(),
            Err(SyncFailure::primary(AgentErrorCode::SpawnFailed))
        );
        assert_eq!(
            second.await.unwrap(),
            Err(SyncFailure::primary(AgentErrorCode::SpawnFailed))
        );
        assert_eq!(runtime.spawn_count(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zero_waiter_cancel_joins_started_blocking_open_before_cleanup_and_restart() {
        let runtime = BlockingOpenRuntime::new();
        let state = state();
        let start_state = Arc::clone(&state);
        let start_runtime = Arc::clone(&runtime);
        let start = tokio::spawn(async move {
            start_state
                .start(session_start("blocking-open", start_runtime))
                .await
        });

        runtime.control.opener_started.notified().await;
        start.abort();
        let _ = start.await;
        wait_for_stopping(&state, "blocking-open").await;
        assert!(
            state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("blocking-open")
        );

        runtime.control.release_opener();
        runtime.control.opener_published.notified().await;
        runtime.control.cleanup_release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("blocking-open")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled blocking open did not finish cleanup");

        assert_eq!(*runtime.control.entry.lock().unwrap(), None);
        assert_eq!(runtime.active_children.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.spawns.load(Ordering::SeqCst), 0);
        assert_eq!(state.status("blocking-open").await.message, "stopped");
        tokio::task::yield_now().await;
        assert_eq!(*runtime.control.entry.lock().unwrap(), None);
        assert_eq!(state.status("blocking-open").await.message, "stopped");

        state
            .start(session_start("blocking-open", Arc::clone(&runtime)))
            .await
            .unwrap();
        let restart_generation = state
            .registry
            .lock()
            .await
            .sessions
            .get("blocking-open")
            .unwrap()
            .generation;
        assert_eq!(runtime.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_children.load(Ordering::SeqCst), 1);
        assert_eq!(
            *runtime.control.entry.lock().unwrap(),
            Some(BlockingOpenRuntime::identity(
                "blocking-open",
                restart_generation,
                "fixture-host",
                REMOTE_WORKSPACE_PORT,
            ))
        );
        state.stop("blocking-open").await.unwrap();
        assert_eq!(*runtime.control.entry.lock().unwrap(), None);
        assert_eq!(runtime.active_children.load(Ordering::SeqCst), 0);
        assert!(state.registry.lock().await.sessions.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn desktop_runtime_owns_aborted_opener_through_publication_and_join_errors() {
        let control = DesktopOpenControl::new();
        let tasks = TaskTracker::new();
        let runtime = Arc::new(DesktopSessionRuntime {
            credentials: Arc::new(DesktopOpenCredentials),
            tunnels: TunnelState::with_factory(Arc::new(DesktopOpenFactory {
                control: Arc::clone(&control),
            })),
            tasks: tasks.clone(),
        });
        let first_generation = uuid::Uuid::new_v4();
        let open_runtime = Arc::clone(&runtime);
        let open = tokio::spawn(async move {
            open_runtime
                .open_tunnel(
                    "aborted-open",
                    first_generation,
                    "fixture-host",
                    REMOTE_WORKSPACE_PORT,
                )
                .await
        });

        control.opener_started.notified().await;
        open.abort();
        let _ = open.await;
        control.release_opener();
        wait_until(|| control.close_count.load(Ordering::SeqCst) == 1).await;
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 0);

        let successor_generation = uuid::Uuid::new_v4();
        let mut successor = runtime
            .open_tunnel(
                "aborted-open",
                successor_generation,
                "fixture-host",
                REMOTE_WORKSPACE_PORT,
            )
            .await
            .unwrap();
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime
                .close_tunnel("aborted-open", successor_generation, "fixture-host")
                .await,
            Ok(())
        );
        successor.disarm();
        drop(successor);
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 0);
        assert_eq!(control.close_count.load(Ordering::SeqCst), 2);

        let fallback_generation = uuid::Uuid::new_v4();
        let mut fallback = runtime
            .open_tunnel(
                "fallback-open",
                fallback_generation,
                "fixture-host",
                REMOTE_WORKSPACE_PORT,
            )
            .await
            .unwrap();
        control.close_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            close_owned_tunnel(
                runtime.as_ref(),
                "fallback-open",
                fallback_generation,
                "fixture-host",
                Some(&mut fallback),
            )
            .await,
            Err(SyncFailure::primary(AgentErrorCode::ShutdownTimeout))
        );
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 1);
        drop(fallback);
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 0);

        control.panic_next.store(true, Ordering::SeqCst);
        let join_error_generation = uuid::Uuid::new_v4();
        assert!(matches!(
            runtime
                .open_tunnel(
                    "join-error",
                    join_error_generation,
                    "fixture-host",
                    REMOTE_WORKSPACE_PORT,
                )
                .await,
            Err(failure) if failure == SyncFailure::primary(AgentErrorCode::AbnormalExit)
        ));

        let recovered_generation = uuid::Uuid::new_v4();
        let mut recovered = runtime
            .open_tunnel(
                "join-error",
                recovered_generation,
                "fixture-host",
                REMOTE_WORKSPACE_PORT,
            )
            .await
            .expect("join-error generation poisoned tunnel restart");
        runtime
            .close_tunnel("join-error", recovered_generation, "fixture-host")
            .await
            .unwrap();
        recovered.disarm();

        tasks.close();
        tasks.wait().await;
        assert_eq!(
            *control.keys.lock().unwrap(),
            vec![
                sync_tunnel_key("aborted-open", first_generation),
                sync_tunnel_key("aborted-open", successor_generation),
                sync_tunnel_key("fallback-open", fallback_generation),
                sync_tunnel_key("join-error", join_error_generation),
                sync_tunnel_key("join-error", recovered_generation),
            ]
        );
        assert_eq!(control.active_tunnels.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_cancelled_start_waiters_cancel_startup_and_allow_restart() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([
            SpawnPlan::Blocked {
                entered: Arc::clone(&entered),
                release,
            },
            SpawnPlan::Running,
        ]);
        let state = state();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_state = Arc::clone(&state);
        let first_runtime = Arc::clone(&runtime);
        let first_barrier = Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_state
                .start(session_start("cancel-all", first_runtime))
                .await
        });
        let second_state = Arc::clone(&state);
        let second_runtime = Arc::clone(&runtime);
        let second_barrier = Arc::clone(&barrier);
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_state
                .start(session_start("cancel-all", second_runtime))
                .await
        });

        barrier.wait().await;
        entered.notified().await;
        first.abort();
        second.abort();
        let _ = first.await;
        let _ = second.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime.tunnel_closes.load(Ordering::SeqCst) == 1
                    && state.registry.lock().await.sessions.is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled startup was not cleaned up");

        state
            .start(session_start("cancel-all", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 2);
        state.stop("cancel-all").await.unwrap();
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_one_start_waiter_keeps_startup_for_the_other() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Blocked {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }]);
        let state = state();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_state = Arc::clone(&state);
        let first_runtime = Arc::clone(&runtime);
        let first_barrier = Arc::clone(&barrier);
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_state
                .start(session_start("cancel-one", first_runtime))
                .await
        });
        let second_state = Arc::clone(&state);
        let second_runtime = Arc::clone(&runtime);
        let second_barrier = Arc::clone(&barrier);
        let mut second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_state
                .start(session_start("cancel-one", second_runtime))
                .await
        });

        barrier.wait().await;
        entered.notified().await;
        first.abort();
        let _ = first.await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err(),
            "remaining start did not stay joined to readiness"
        );
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 0);

        release.notify_one();
        assert_eq!(second.await.unwrap(), Ok(()));
        state.stop("cancel-one").await.unwrap();
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_startup_cancel_is_ignored_when_readiness_waiters_are_active() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Blocked {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }]);
        let state = state();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_state = Arc::clone(&state);
        let first_runtime = Arc::clone(&runtime);
        let first_barrier = Arc::clone(&barrier);
        let mut first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_state
                .start(session_start("stale-cancel", first_runtime))
                .await
        });
        let second_state = Arc::clone(&state);
        let second_runtime = Arc::clone(&runtime);
        let second_barrier = Arc::clone(&barrier);
        let mut second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_state
                .start(session_start("stale-cancel", second_runtime))
                .await
        });

        barrier.wait().await;
        entered.notified().await;
        let (commands, generation) = {
            let registry = state.registry.lock().await;
            let session = registry.sessions.get("stale-cancel").unwrap();
            (session.commands.clone(), session.generation)
        };
        commands
            .send(SessionCommand::CancelStartup { generation })
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut first)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err()
        );

        release.notify_one();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));
        state.stop("stale-cancel").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_queued_after_launch_is_ignored_for_a_later_readiness_waiter() {
        let transition = Arc::new(ReadinessTransitionHook {
            launch_completed: tokio::sync::Notify::new(),
            resume_publication: tokio::sync::Notify::new(),
            stale_cancel_ignored: tokio::sync::Notify::new(),
        });
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        let state = state();
        let mut first_start = session_start("ready-transition", Arc::clone(&runtime));
        first_start.readiness_transition = Some(Arc::clone(&transition));
        let first_state = Arc::clone(&state);
        let first = tokio::spawn(async move { first_state.start(first_start).await });

        transition.launch_completed.notified().await;
        first.abort();
        let _ = first.await;

        let second_state = Arc::clone(&state);
        let second_runtime = Arc::clone(&runtime);
        let second = tokio::spawn(async move {
            second_state
                .start(session_start("ready-transition", second_runtime))
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let active = state
                    .registry
                    .lock()
                    .await
                    .sessions
                    .get("ready-transition")
                    .map(|session| session.startup_waiters.active_count());
                if active == Some(1) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("later caller did not register as a readiness waiter");

        transition.resume_publication.notify_one();
        assert_eq!(second.await.unwrap(), Ok(()));
        tokio::time::timeout(
            Duration::from_secs(2),
            transition.stale_cancel_ignored.notified(),
        )
        .await
        .expect("queued startup cancellation was not ignored after readiness");
        assert!(state.status("ready-transition").await.running);
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 0);
        assert!(
            state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("ready-transition")
        );

        state.stop("ready-transition").await.unwrap();
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        assert!(state.registry.lock().await.sessions.is_empty());
    }

    #[tokio::test]
    async fn crash_restarts_with_fresh_tunnel_token_and_ready_reconcile() {
        let runtime = FakeRuntime::new([
            SpawnPlan::Crash(AgentErrorCode::AbnormalExit),
            SpawnPlan::Running,
        ]);
        let state = state();
        state
            .start(session_start("project", Arc::clone(&runtime)))
            .await
            .unwrap();

        wait_until(|| runtime.spawn_count() == 2).await;
        {
            let launches = runtime.launches.lock().unwrap();
            assert_eq!(launches.len(), 2);
            assert_ne!(launches[0].token, launches[1].token);
            assert!(launches[0].endpoint.contains(":19050/"));
            assert!(launches[1].endpoint.contains(":19051/"));
        }
        assert_eq!(runtime.token_requests.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.tunnel_opens.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        let status = state.status("project").await;
        assert!(status.running);
        assert_eq!(status.local_port, Some(19051));

        state.stop("project").await.unwrap();
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actor_panic_after_ready_is_monitored_and_allows_immediate_restart() {
        let panicked = Arc::new(Notify::new());
        let runtime =
            FakeRuntime::new([SpawnPlan::Panic(Arc::clone(&panicked)), SpawnPlan::Running]);
        let state = state();

        state
            .start(session_start("actor-panic", Arc::clone(&runtime)))
            .await
            .unwrap();
        panicked.notified().await;

        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("actor-panic")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panicked actor was not finalized by its monitor");

        state
            .start(session_start("actor-panic", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 2);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        state.stop("actor-panic").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actor_abort_after_ready_is_monitored_and_allows_immediate_restart() {
        let runtime = FakeRuntime::new([SpawnPlan::Running, SpawnPlan::Running]);
        let state = state();
        state
            .start(session_start("actor-abort", Arc::clone(&runtime)))
            .await
            .unwrap();
        let actor_abort = state
            .registry
            .lock()
            .await
            .sessions
            .get("actor-abort")
            .and_then(|session| session.actor_abort.clone())
            .unwrap();

        actor_abort.abort();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("actor-abort")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted actor was not finalized by its monitor");

        state
            .start(session_start("actor-abort", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 2);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        state.stop("actor-abort").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actor_command_channel_close_reaps_resources_and_settles_completion() {
        let runtime = FakeRuntime::new([SpawnPlan::Running, SpawnPlan::Running]);
        let state = state();
        state
            .start(session_start("channel-close", Arc::clone(&runtime)))
            .await
            .unwrap();
        let mut completion = state
            .registry
            .lock()
            .await
            .sessions
            .remove("channel-close")
            .unwrap()
            .completion;

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), wait_for_signal(&mut completion))
                .await
                .expect("channel-close actor did not settle")
                .unwrap(),
            Ok(())
        );
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);

        state
            .start(session_start("channel-close", Arc::clone(&runtime)))
            .await
            .unwrap();
        assert_eq!(runtime.spawn_count(), 2);
        state.stop("channel-close").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_shutdown_waiter_cannot_detach_timeout_escalation() {
        let shutdown_entered = Arc::new(Notify::new());
        let shutdown_release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::BlockedShutdown {
            entered: Arc::clone(&shutdown_entered),
            release: shutdown_release,
        }]);
        let state = Arc::new(SyncState::with_credentials_and_shutdown_deadline(
            Arc::new(UnavailableCredentialProvider),
            Duration::from_millis(25),
        ));
        state
            .start(session_start("shutdown-cancel", Arc::clone(&runtime)))
            .await
            .unwrap();

        let shutdown_state = Arc::clone(&state);
        let shutdown = tokio::spawn(async move { shutdown_state.shutdown_all().await });
        shutdown_entered.notified().await;
        shutdown.abort();
        let _ = shutdown.await;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.registry.lock().await.sessions.is_empty() && state.tasks.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled shutdown waiter detached actor cleanup");
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        let mut terminal = state
            .shutdown_operation
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), wait_for_signal(&mut terminal))
                .await
                .expect("caller-independent shutdown owner did not publish its terminal result")
                .unwrap(),
            Err(SyncFailure::primary(AgentErrorCode::ShutdownTimeout))
        );
        assert_eq!(state.shutdown_all().await, Ok(()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn post_deadline_cleanup_is_bounded_retained_and_retryable_by_generation() {
        let shutdown_entered = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::BlockedShutdown {
            entered: Arc::clone(&shutdown_entered),
            release: Arc::new(Notify::new()),
        }]);
        runtime.persistent_close_error.store(true, Ordering::SeqCst);
        let state = SyncState::with_credentials_and_shutdown_deadline(
            Arc::new(UnavailableCredentialProvider),
            Duration::from_millis(20),
        );
        state
            .start(session_start("shutdown-retry", Arc::clone(&runtime)))
            .await
            .unwrap();
        let generation = state
            .registry
            .lock()
            .await
            .sessions
            .get("shutdown-retry")
            .unwrap()
            .generation;

        let first_failure = tokio::time::timeout(Duration::from_millis(250), state.shutdown_all())
            .await
            .expect("post-deadline cleanup did not return a bounded failure")
            .unwrap_err();
        assert_eq!(first_failure.primary, AgentErrorCode::ShutdownTimeout);
        assert!(
            first_failure.cleanup.contains(&AgentErrorCode::Network),
            "permanent tunnel cleanup was not retained in the terminal result"
        );
        let retained_status = state.status("shutdown-retry").await;
        assert_eq!(retained_status.error, Some(first_failure));
        assert!(
            state
                .registry
                .lock()
                .await
                .sessions
                .get("shutdown-retry")
                .is_some_and(|session| session.generation == generation),
            "failed cleanup lost its exact generation owner"
        );
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 0);
        assert!(runtime.close_attempts.lock().unwrap().iter().all(
            |(project, attempted_generation)| {
                project == "shutdown-retry" && *attempted_generation == generation
            }
        ));

        runtime
            .persistent_close_error
            .store(false, Ordering::SeqCst);
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(250), state.shutdown_all())
                .await
                .expect("cleanup retry did not terminate"),
            Ok(())
        );
        assert!(state.registry.lock().await.sessions.is_empty());
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.shutdown_all().await, Ok(()));
        assert_eq!(
            runtime.tunnel_closes.load(Ordering::SeqCst),
            1,
            "successful retry double-closed the exact tunnel"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn post_deadline_task_wait_returns_while_the_tracker_retains_cleanup_ownership() {
        let shutdown_entered = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::BlockedShutdown {
            entered: Arc::clone(&shutdown_entered),
            release: Arc::new(Notify::new()),
        }]);
        runtime.block_close.store(true, Ordering::SeqCst);
        let state = SyncState::with_credentials_and_shutdown_deadline(
            Arc::new(UnavailableCredentialProvider),
            Duration::from_millis(20),
        );
        state
            .start(session_start("tracked-cleanup", Arc::clone(&runtime)))
            .await
            .unwrap();
        let generation = state
            .registry
            .lock()
            .await
            .sessions
            .get("tracked-cleanup")
            .unwrap()
            .generation;

        let first_failure = tokio::time::timeout(Duration::from_millis(250), state.shutdown_all())
            .await
            .expect("TaskTracker wait escaped the post-deadline bound")
            .unwrap_err();
        assert_eq!(first_failure.primary, AgentErrorCode::ShutdownTimeout);
        assert!(
            !state.tasks.is_empty(),
            "blocked monitor was detached instead of remaining tracker-owned"
        );
        assert!(
            state
                .registry
                .lock()
                .await
                .sessions
                .get("tracked-cleanup")
                .is_some_and(|session| session.generation == generation)
        );

        runtime.block_close.store(false, Ordering::SeqCst);
        runtime.close_release.notify_waiters();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(250), state.shutdown_all())
                .await
                .expect("tracked cleanup retry did not terminate"),
            Ok(())
        );
        tokio::time::timeout(Duration::from_millis(250), async {
            while !state.tasks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tracked cleanup task did not join after release");
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        assert!(
            runtime
                .close_attempts
                .lock()
                .unwrap()
                .iter()
                .all(|(_, attempted_generation)| *attempted_generation == generation)
        );
    }

    #[tokio::test]
    async fn persistent_cleanup_failure_is_visible_while_session_is_retained() {
        let runtime = FakeRuntime::new([SpawnPlan::Running]);
        let state = state();
        state
            .start(session_start("persistent-cleanup", Arc::clone(&runtime)))
            .await
            .unwrap();
        runtime.persistent_close_error.store(true, Ordering::SeqCst);

        let failure = state.stop("persistent-cleanup").await.unwrap_err();
        assert_eq!(failure.primary, AgentErrorCode::Network);
        assert_eq!(
            state.status("persistent-cleanup").await.error,
            Some(failure)
        );
        assert!(
            state
                .registry
                .lock()
                .await
                .sessions
                .contains_key("persistent-cleanup")
        );
    }

    #[tokio::test]
    async fn recovery_has_a_hard_retry_limit_and_terminal_status() {
        let runtime = FakeRuntime::new([
            SpawnPlan::Crash(AgentErrorCode::AbnormalExit),
            SpawnPlan::SpawnError(AgentErrorCode::SpawnFailed),
            SpawnPlan::SpawnError(AgentErrorCode::SpawnFailed),
        ]);
        let state = state();
        state
            .start(session_start("project", Arc::clone(&runtime)))
            .await
            .unwrap();

        wait_until(|| {
            state
                .registry
                .try_lock()
                .is_ok_and(|registry| registry.sessions.is_empty())
        })
        .await;
        assert_eq!(runtime.spawn_count(), 3);
        assert_eq!(runtime.token_requests.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.tunnel_opens.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 3);
        assert_eq!(
            state.status("project").await.message,
            "recovery_exhausted:spawn_failed"
        );
        assert_eq!(state.stop("project").await, Ok(()));
    }

    #[tokio::test]
    async fn startup_wait_is_cancellable_but_spawned_process_is_still_reaped() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = FakeRuntime::new([SpawnPlan::Blocked {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }]);
        let state = state();
        let start_state = Arc::clone(&state);
        let start_runtime = Arc::clone(&runtime);
        let mut start = tokio::spawn(async move {
            start_state
                .start(session_start("project", start_runtime))
                .await
        });
        entered.notified().await;

        let stop_state = Arc::clone(&state);
        let stop = tokio::spawn(async move { stop_state.stop("project").await });
        let start_result = tokio::time::timeout(Duration::from_millis(100), &mut start)
            .await
            .expect("start waiter was not cancelled")
            .unwrap();
        assert_eq!(
            start_result,
            Err(SyncFailure::primary(AgentErrorCode::AbnormalExit))
        );
        assert!(!stop.is_finished());

        release.notify_one();
        assert_eq!(stop.await.unwrap(), Ok(()));
        assert_eq!(runtime.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.tunnel_closes.load(Ordering::SeqCst), 1);
        assert!(state.registry.lock().await.sessions.is_empty());
    }

    #[tokio::test]
    async fn shutdown_all_propagates_actor_and_tunnel_errors() {
        let process_error =
            FakeRuntime::new([SpawnPlan::ShutdownError(AgentErrorCode::ShutdownTimeout)]);
        let process_state = state();
        process_state
            .start(session_start("process-error", Arc::clone(&process_error)))
            .await
            .unwrap();
        assert_eq!(
            process_state.shutdown_all().await,
            Err(SyncFailure::primary(AgentErrorCode::ShutdownTimeout))
        );
        assert_eq!(
            process_state.status("process-error").await.message,
            "shutdown_timeout"
        );

        let tunnel_error = FakeRuntime::new([SpawnPlan::Running]);
        *tunnel_error.close_error.lock().unwrap() = Some(AgentErrorCode::Network);
        let tunnel_state = state();
        tunnel_state
            .start(session_start("tunnel-error", Arc::clone(&tunnel_error)))
            .await
            .unwrap();
        assert_eq!(
            tunnel_state.shutdown_all().await,
            Err(SyncFailure::primary(AgentErrorCode::Network))
        );
        assert_eq!(tunnel_state.status("tunnel-error").await.message, "network");
    }
}
