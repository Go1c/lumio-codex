//! Daemon: recovery-first watcher/transport orchestration with bounded shutdown.
//!
//! Startup order: open and recover the engine, start observation, reconcile, then
//! connect transport. Normal shutdown quiesces the watcher before engine close.
//! A parent process supervisor, not this in-process engine thread, supplies the
//! hard deadline by killing and reaping the complete worker process if needed.

use crate::config::AgentConfig;
use crate::error::{AgentError, AgentErrorCode, AgentPhase};
use crate::status::AgentStatus;

use fns_observability::RuntimeDiagnostics;
use fns_sync_core::{SyncDiagnostics, SyncEngine, SyncEngineConfig};
use fns_transport::{
    EngineWorker, JitterSource, ReconnectPolicy, ReconnectSchedule, SessionConnectionPhase,
    SessionRuntimeStatus, TransportDiagnostics, UuidJitter, WorkspaceEndpoint,
};
use std::sync::Arc;

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{
    Arc as StdArc,
    atomic::{AtomicUsize as StdAtomicUsize, Ordering as StdOrdering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
const BRIDGE_RECEIVE_TICK: Duration = Duration::from_millis(25);
const ENGINE_SUBMISSION_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

#[cfg(test)]
async fn run_with_shutdown_signal<Signal>(
    config: AgentConfig,
    token: fns_platform::SecretToken,
    signal: Signal,
) -> Result<(), AgentError>
where
    Signal: Future<Output = Result<(), AgentError>> + Send + 'static,
{
    let state_dir = config.state_dir.clone();
    let workspace_id = config.workspace_id;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    let mut signal = tokio::spawn(async move {
        let result = signal.await;
        signal_shutdown.cancel();
        result
    });
    let runtime_result = run_until_shutdown(config, token, shutdown, None).await;
    let signal_result = if signal.is_finished() {
        match (&mut signal).await {
            Ok(result) => Some(result),
            Err(_) => Some(Err(AgentError::new(AgentErrorCode::Core))),
        }
    } else {
        signal.abort();
        match signal.await {
            Err(error) if error.is_cancelled() => None,
            Ok(result) => Some(result),
            Err(_) => Some(Err(AgentError::new(AgentErrorCode::Core))),
        }
    };
    match runtime_result {
        Err(error) => Err(error),
        Ok(()) => match signal_result {
            Some(Err(error)) => {
                write_status_error(&state_dir, workspace_id, error.code())?;
                Err(error)
            }
            Some(Ok(())) | None => Ok(()),
        },
    }
}

/// Run the agent daemon in embedded mode (e.g., inside a Tauri desktop app).
///
/// Unlike `run()`, this does NOT install a SIGINT/SIGTERM handler.
/// The caller controls shutdown by cancelling the `external_shutdown` token.
/// When the token is cancelled, the current session (if any) stops and the
/// function returns `Ok(())`.
#[cfg(test)]
async fn run_embedded(
    config: AgentConfig,
    token: fns_platform::SecretToken,
    external_shutdown: tokio_util::sync::CancellationToken,
) -> Result<(), AgentError> {
    run_until_shutdown(config, token, external_shutdown, None).await
}

/// Run under the worker control protocol and report readiness only after
/// recovery, watcher startup, and the initial durable reconciliation complete.
pub async fn run_supervised(
    config: AgentConfig,
    token: fns_platform::SecretToken,
    external_shutdown: tokio_util::sync::CancellationToken,
    ready: tokio::sync::oneshot::Sender<fns_transport::EngineHandle>,
) -> Result<(), AgentError> {
    run_until_shutdown(config, token, external_shutdown, Some(ready)).await
}

async fn run_until_shutdown(
    config: AgentConfig,
    token: fns_platform::SecretToken,
    shutdown: tokio_util::sync::CancellationToken,
    ready: Option<tokio::sync::oneshot::Sender<fns_transport::EngineHandle>>,
) -> Result<(), AgentError> {
    let workspace_id = config.workspace_id;
    let state_dir = config.state_dir.clone();
    let endpoint = WorkspaceEndpoint::parse(&config.endpoint)
        .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))?;
    let diagnostics =
        crate::obs::open_agent_diagnostics(&state_dir, &workspace_id.to_string(), None);
    crate::obs::emit_lifecycle(&diagnostics, "starting", "agent starting", vec![]);

    write_status(&state_dir, AgentPhase::Starting, workspace_id)?;
    write_status(&state_dir, AgentPhase::Recovering, workspace_id)?;

    let (mut watcher, worker, handle) = match start_local_runtime(&config).await {
        Ok(runtime) => runtime,
        Err(error) => {
            write_status_error(&state_dir, workspace_id, error.code())?;
            crate::obs::emit_lifecycle(
                &diagnostics,
                "fatal",
                "local runtime failed",
                vec![(
                    "errorCode",
                    serde_json::to_value(error.code()).unwrap_or(serde_json::Value::Null),
                )],
            );
            return Err(error);
        }
    };
    let watcher_failure = watcher.failure_token();
    let queued_watcher_batches = watcher.queued_batches_handle();
    let run_result = match write_status(&state_dir, AgentPhase::Connecting, workspace_id) {
        Ok(()) => {
            if ready.is_some_and(|ready| ready.send(handle.clone()).is_err()) {
                return finalize_runtime(
                    &mut watcher,
                    worker,
                    handle,
                    &state_dir,
                    workspace_id,
                    &diagnostics,
                    Err(AgentError::new(AgentErrorCode::Protocol)),
                )
                .await;
            }
            run_transport_loop(
                &config,
                &token,
                &endpoint,
                &handle,
                shutdown,
                watcher_failure,
                queued_watcher_batches,
                &diagnostics,
            )
            .await
        }
        Err(error) => Err(error),
    };
    finalize_runtime(
        &mut watcher,
        worker,
        handle,
        &state_dir,
        workspace_id,
        &diagnostics,
        run_result,
    )
    .await
}

async fn start_local_runtime(
    config: &AgentConfig,
) -> Result<(WatcherRuntime, EngineWorker, fns_transport::EngineHandle), AgentError> {
    let rule_config = configured_sync_rules(config);
    let watcher_rules = compile_sync_rules(rule_config.clone())?;
    let engine_config = SyncEngineConfig::new(
        config.workspace_id,
        config.client_id,
        &config.workspace_root,
        &config.state_dir,
    )
    .with_sync_rules(rule_config);
    let engine = SyncEngine::open(engine_config).map_err(map_sync_error)?;
    let (worker, handle) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| EngineWorker::spawn(engine)))
            .map_err(|_| AgentError::new(AgentErrorCode::Core))?;
    match start_watcher_and_reconcile(&config.workspace_root, watcher_rules, handle.clone()).await {
        Ok(watcher) => Ok((watcher, worker, handle)),
        Err(error) => {
            let cleanup = shutdown_engine_worker(worker, handle).await;
            Err(cleanup.err().unwrap_or(error))
        }
    }
}

async fn run_transport_loop(
    config: &AgentConfig,
    token: &fns_platform::SecretToken,
    endpoint: &WorkspaceEndpoint,
    handle: &fns_transport::EngineHandle,
    shutdown: tokio_util::sync::CancellationToken,
    watcher_failure: tokio_util::sync::CancellationToken,
    queued_watcher_batches: StdArc<StdAtomicUsize>,
    diagnostics: &RuntimeDiagnostics,
) -> Result<(), AgentError> {
    let workspace_id = config.workspace_id;
    let state_dir = &config.state_dir;

    let mut schedule = ReconnectSchedule::new(ReconnectPolicy::default(), UuidJitter);

    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        if watcher_failure.is_cancelled() {
            return Err(AgentError::new(AgentErrorCode::Core));
        }

        let connect_result = tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = watcher_failure.cancelled() => {
                return Err(AgentError::new(AgentErrorCode::Core));
            }
            res = fns_transport::socket::connect(endpoint, token, "0.1.0") => res,
        };

        let reconnect_error_code = match connect_result {
            Ok(stream) => {
                let transport_diag = TransportDiagnostics::new(Arc::new(diagnostics.clone()));
                let (session, mut writer, mut session_status_rx) =
                    fns_transport::session::Session::new_observed(
                        stream,
                        handle.clone(),
                        workspace_id,
                        config.client_id,
                        "0.1.0".into(),
                    );
                let session = session.with_diagnostics(transport_diag);
                let mut current_status = *session_status_rx.borrow_and_update();
                write_session_status(
                    state_dir,
                    workspace_id,
                    handle,
                    &queued_watcher_batches,
                    diagnostics,
                    current_status,
                    schedule.attempt(),
                )
                .await?;
                let mut reached_online = false;
                let mut status_channel_open = true;
                let mut status_tick = tokio::time::interval(STATUS_REFRESH_INTERVAL);
                status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                status_tick.tick().await;
                let mut session_run = Box::pin(session.run(&mut writer, shutdown.clone()));

                let result = loop {
                    tokio::select! {
                        _ = watcher_failure.cancelled() => {
                            return Err(AgentError::new(AgentErrorCode::Core));
                        }
                        result = &mut session_run => break result,
                        changed = session_status_rx.changed(), if status_channel_open => {
                            if changed.is_err() {
                                status_channel_open = false;
                                continue;
                            }
                            current_status = *session_status_rx.borrow_and_update();
                            reset_reconnect_after_online(
                                &mut schedule,
                                &mut reached_online,
                                current_status.phase,
                            );
                            write_session_status(
                                state_dir,
                                workspace_id,
                                handle,
                                &queued_watcher_batches,
                                diagnostics,
                                current_status,
                                schedule.attempt(),
                            )
                            .await?;
                        }
                        _ = status_tick.tick() => {
                            write_session_status(
                                state_dir,
                                workspace_id,
                                handle,
                                &queued_watcher_batches,
                                diagnostics,
                                current_status,
                                schedule.attempt(),
                            )
                            .await?;
                        }
                    }
                };
                match result {
                    fns_transport::session::SessionResult::Closed => {
                        if shutdown.is_cancelled() {
                            return Ok(());
                        }
                        AgentErrorCode::Network
                    }
                    fns_transport::session::SessionResult::Error(e) => {
                        let error = map_transport_error(e.code());
                        if !e.retryable() {
                            return Err(error);
                        }
                        error.code()
                    }
                }
            }
            Err(e) => {
                let error = map_transport_error(e.code());
                if !e.retryable() {
                    return Err(error);
                }
                error.code()
            }
        };

        let delay = schedule.next_delay();
        let attempt = schedule.attempt();
        TransportDiagnostics::new(Arc::new(diagnostics.clone()))
            .on_reconnect(attempt, &format!("{reconnect_error_code:?}"));
        write_reconnect_status(
            state_dir,
            workspace_id,
            handle,
            &queued_watcher_batches,
            diagnostics,
            attempt,
            reconnect_error_code,
        )
        .await?;
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = watcher_failure.cancelled() => {
                return Err(AgentError::new(AgentErrorCode::Core));
            }
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

fn reset_reconnect_after_online<J: JitterSource>(
    schedule: &mut ReconnectSchedule<J>,
    reached_online: &mut bool,
    phase: SessionConnectionPhase,
) {
    if phase == SessionConnectionPhase::Online && !*reached_online {
        schedule.reset();
        *reached_online = true;
    }
}

async fn finalize_runtime(
    watcher: &mut WatcherRuntime,
    worker: EngineWorker,
    handle: fns_transport::EngineHandle,
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    diagnostics: &RuntimeDiagnostics,
    run_result: Result<(), AgentError>,
) -> Result<(), AgentError> {
    let stopping_result = if run_result.is_ok() {
        write_status(state_dir, AgentPhase::Stopping, workspace_id)
    } else {
        Ok(())
    };
    let watcher_result = watcher.shutdown(SHUTDOWN_GRACE).await;
    let engine_result = shutdown_engine_worker(worker, handle).await;
    let error = watcher_result
        .err()
        .or_else(|| run_result.err())
        .or_else(|| stopping_result.err())
        .or_else(|| engine_result.err());
    if let Some(error) = error {
        write_status_error(state_dir, workspace_id, error.code())?;
        crate::obs::emit_lifecycle(
            diagnostics,
            "fatal",
            "agent stopped with error",
            vec![(
                "errorCode",
                serde_json::to_value(error.code()).unwrap_or(serde_json::Value::Null),
            )],
        );
        return Err(error);
    }
    write_status_stopped(state_dir, workspace_id)?;
    crate::obs::emit_lifecycle(diagnostics, "stopped", "agent stopped", vec![]);
    Ok(())
}

async fn shutdown_engine_worker(
    worker: EngineWorker,
    handle: fns_transport::EngineHandle,
) -> Result<(), AgentError> {
    let shutdown_result = match tokio::time::timeout(SHUTDOWN_GRACE, handle.shutdown()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(map_transport_error(error.code())),
        Err(_) => Err(AgentError::new(AgentErrorCode::ShutdownTimeout)),
    };
    drop(handle);
    let join_result = worker
        .join()
        .map_err(|error| map_transport_error(error.code()));
    shutdown_result.and(join_result)
}

fn map_sync_error(error: fns_sync_core::SyncError) -> AgentError {
    match error {
        fns_sync_core::SyncError::InvalidConfiguration { .. } => {
            AgentError::new(AgentErrorCode::InvalidConfiguration)
        }
        fns_sync_core::SyncError::Filesystem(_) | fns_sync_core::SyncError::ScanIncomplete => {
            AgentError::new(AgentErrorCode::Filesystem)
        }
        _ => AgentError::new(AgentErrorCode::Core),
    }
}

#[cfg(all(test, unix))]
async fn wait_for_shutdown_signal_from<RegisterTerminate, Terminate, CtrlC>(
    register_terminate: RegisterTerminate,
    ctrl_c: CtrlC,
) -> Result<(), AgentError>
where
    RegisterTerminate: FnOnce() -> std::io::Result<Terminate>,
    Terminate: Future<Output = Option<()>>,
    CtrlC: Future<Output = std::io::Result<()>>,
{
    let terminate = register_terminate().map_err(|_| AgentError::new(AgentErrorCode::Core))?;
    tokio::pin!(terminate);
    tokio::pin!(ctrl_c);
    tokio::select! {
        result = &mut ctrl_c => result.map_err(|_| AgentError::new(AgentErrorCode::Core)),
        result = &mut terminate => result.ok_or_else(|| AgentError::new(AgentErrorCode::Core)),
    }
}

fn map_transport_error(code: fns_transport::TransportErrorCode) -> AgentError {
    match code {
        fns_transport::TransportErrorCode::InvalidConfiguration => {
            AgentError::new(AgentErrorCode::InvalidConfiguration)
        }
        fns_transport::TransportErrorCode::AuthenticationRejected => {
            AgentError::new(AgentErrorCode::AuthenticationRejected)
        }
        fns_transport::TransportErrorCode::Forbidden => AgentError::new(AgentErrorCode::Forbidden),
        fns_transport::TransportErrorCode::Network => AgentError::new(AgentErrorCode::Network),
        fns_transport::TransportErrorCode::RequestTimeout => {
            AgentError::new(AgentErrorCode::RequestTimeout)
        }
        fns_transport::TransportErrorCode::IdleTimeout => {
            AgentError::new(AgentErrorCode::IdleTimeout)
        }
        fns_transport::TransportErrorCode::TransferTimeout => {
            AgentError::new(AgentErrorCode::TransferTimeout)
        }
        fns_transport::TransportErrorCode::Protocol => AgentError::new(AgentErrorCode::Protocol),
        fns_transport::TransportErrorCode::Core => AgentError::new(AgentErrorCode::Core),
        fns_transport::TransportErrorCode::Filesystem => {
            AgentError::new(AgentErrorCode::Filesystem)
        }
        fns_transport::TransportErrorCode::StateCorrupt => {
            AgentError::new(AgentErrorCode::StateCorrupt)
        }
        fns_transport::TransportErrorCode::ConflictUnavailable => {
            AgentError::new(AgentErrorCode::ConflictUnavailable)
        }
        fns_transport::TransportErrorCode::ConflictRevisionStale => {
            AgentError::new(AgentErrorCode::ConflictRevisionStale)
        }
        fns_transport::TransportErrorCode::ConflictResolutionChanged => {
            AgentError::new(AgentErrorCode::ConflictResolutionChanged)
        }
        fns_transport::TransportErrorCode::ConflictWaitingBlobs => {
            AgentError::new(AgentErrorCode::ConflictWaitingBlobs)
        }
        fns_transport::TransportErrorCode::ConflictAutomaticResolutionPending => {
            AgentError::new(AgentErrorCode::ConflictAutomaticResolutionPending)
        }
        fns_transport::TransportErrorCode::ConflictResolutionPending => {
            AgentError::new(AgentErrorCode::ConflictResolutionPending)
        }
        fns_transport::TransportErrorCode::ConflictRefreshRequired => {
            AgentError::new(AgentErrorCode::ConflictRefreshRequired)
        }
        fns_transport::TransportErrorCode::ConflictSelectedSideDeleted => {
            AgentError::new(AgentErrorCode::ConflictSelectedSideDeleted)
        }
        fns_transport::TransportErrorCode::MergeFileRequired => {
            AgentError::new(AgentErrorCode::MergeFileRequired)
        }
        fns_transport::TransportErrorCode::MergeContentUnavailable => {
            AgentError::new(AgentErrorCode::MergeContentUnavailable)
        }
        fns_transport::TransportErrorCode::ResourceLimit => {
            AgentError::new(AgentErrorCode::ResourceLimit)
        }
        fns_transport::TransportErrorCode::ShutdownTimeout => {
            AgentError::new(AgentErrorCode::ShutdownTimeout)
        }
    }
}

type BridgeFuture = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send + 'static>>;
type BridgeJoin = tokio_util::task::AbortOnDropHandle<Result<(), AgentError>>;

struct WatcherRuntime {
    watcher: Option<fns_fs::PlatformWatcher>,
    cancellation: tokio_util::sync::CancellationToken,
    failure: tokio_util::sync::CancellationToken,
    queued_batches: StdArc<StdAtomicUsize>,
    bridge: Option<BridgeJoin>,
}

impl WatcherRuntime {
    fn failure_token(&self) -> tokio_util::sync::CancellationToken {
        self.failure.clone()
    }

    fn queued_batches_handle(&self) -> StdArc<StdAtomicUsize> {
        StdArc::clone(&self.queued_batches)
    }

    async fn shutdown(&mut self, grace: Duration) -> Result<(), AgentError> {
        self.cancellation.cancel();
        self.watcher.take();
        let Some(mut bridge) = self.bridge.take() else {
            return Ok(());
        };
        match tokio::time::timeout(grace, &mut bridge).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AgentError::new(AgentErrorCode::Core)),
            Err(_) => {
                bridge.abort();
                match bridge.await {
                    Err(error) if !error.is_cancelled() => {
                        Err(AgentError::new(AgentErrorCode::Core))
                    }
                    Ok(_) | Err(_) => Err(AgentError::new(AgentErrorCode::ShutdownTimeout)),
                }
            }
        }
    }
}

impl Drop for WatcherRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.watcher.take();
        if let Some(bridge) = &self.bridge {
            bridge.abort();
        }
    }
}

fn configured_sync_rules(config: &AgentConfig) -> fns_fs::SyncRuleConfig {
    fns_fs::SyncRuleConfig {
        includes: config.sync.includes.clone(),
        excludes: config.sync.excludes.clone(),
        protect_secrets: config.sync.protect_secrets,
    }
}

fn compile_sync_rules(config: fns_fs::SyncRuleConfig) -> Result<fns_fs::SyncRules, AgentError> {
    fns_fs::SyncRules::compile(config)
        .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))
}

fn start_watcher(
    workspace_root: &Path,
    rules: fns_fs::SyncRules,
    handle: fns_transport::EngineHandle,
) -> Result<WatcherRuntime, AgentError> {
    let runtime =
        tokio::runtime::Handle::try_current().map_err(|_| AgentError::new(AgentErrorCode::Core))?;
    start_watcher_with(
        workspace_root,
        rules,
        handle,
        |root| fns_fs::start_platform_watcher(root, fns_fs::WATCH_QUEUE_CAPACITY),
        move |future| {
            Ok(tokio_util::task::AbortOnDropHandle::new(
                runtime.spawn(future),
            ))
        },
    )
}

fn start_watcher_with<StartPlatform, SpawnBridge>(
    workspace_root: &Path,
    rules: fns_fs::SyncRules,
    handle: fns_transport::EngineHandle,
    start_platform: StartPlatform,
    spawn_bridge: SpawnBridge,
) -> Result<WatcherRuntime, AgentError>
where
    StartPlatform:
        FnOnce(
            &fns_fs::RootedWorkspace,
        )
            -> Result<(fns_fs::PlatformWatcher, fns_fs::WatchReceiver), fns_fs::FsError>,
    SpawnBridge: FnOnce(BridgeFuture) -> Result<BridgeJoin, AgentError>,
{
    let root = fns_fs::RootedWorkspace::open(workspace_root)
        .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
    let (watcher, receiver) =
        start_platform(&root).map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
    let cancellation = tokio_util::sync::CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let failure = tokio_util::sync::CancellationToken::new();
    let task_failure = failure.clone();
    let queued_batches = StdArc::new(StdAtomicUsize::new(0));
    let task_queued_batches = StdArc::clone(&queued_batches);
    let task: BridgeFuture = Box::pin(async move {
        let result = run_watch_bridge(
            receiver,
            rules,
            handle,
            task_cancellation,
            task_queued_batches,
        )
        .await;
        if result.is_err() {
            task_failure.cancel();
        }
        result
    });
    let bridge = spawn_bridge(task)?;
    Ok(WatcherRuntime {
        watcher: Some(watcher),
        cancellation,
        failure,
        queued_batches,
        bridge: Some(bridge),
    })
}

async fn start_watcher_and_reconcile(
    workspace_root: &Path,
    rules: fns_fs::SyncRules,
    handle: fns_transport::EngineHandle,
) -> Result<WatcherRuntime, AgentError> {
    let mut watcher = start_watcher(workspace_root, rules, handle.clone())?;
    if let Err(error) = submit_engine_changes(&handle, vec![fns_fs::FsChange::RescanRequired]).await
    {
        let cleanup = watcher.shutdown(SHUTDOWN_GRACE).await;
        return Err(cleanup.err().unwrap_or(error));
    }
    Ok(watcher)
}

async fn run_watch_bridge(
    receiver: fns_fs::WatchReceiver,
    rules: fns_fs::SyncRules,
    handle: fns_transport::EngineHandle,
    cancellation: tokio_util::sync::CancellationToken,
    queued_batches: StdArc<StdAtomicUsize>,
) -> Result<(), AgentError> {
    let mut coalescer = fns_fs::EventCoalescer::with_rules(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
        rules,
    );
    let mut tick = tokio::time::interval(BRIDGE_RECEIVE_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => {
                return submit_bridge_changes(
                    &handle,
                    vec![fns_fs::FsChange::RescanRequired],
                    &queued_batches,
                ).await;
            }
            _ = tick.tick() => {
                let mut disconnected = false;
                for _ in 0..fns_fs::WATCH_QUEUE_CAPACITY {
                    match receiver.try_recv_detailed() {
                        Ok(message) => {
                            process_watch_message(
                                &mut coalescer,
                                &handle,
                                message,
                                &queued_batches,
                            )
                            .await?;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                if disconnected {
                    if cancellation.is_cancelled() {
                        return submit_bridge_changes(
                            &handle,
                            vec![fns_fs::FsChange::RescanRequired],
                            &queued_batches,
                        ).await;
                    }
                    return Err(AgentError::new(AgentErrorCode::Filesystem));
                }
                let changes = coalescer
                    .flush_ready(Instant::now(), &ConservativePrior)
                    .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
                if !changes.is_empty() {
                    submit_bridge_changes(&handle, changes, &queued_batches).await?;
                }
            }
        }
    }
}

async fn process_watch_message(
    coalescer: &mut fns_fs::EventCoalescer,
    handle: &fns_transport::EngineHandle,
    message: fns_fs::WatchMessage,
    queued_batches: &StdArc<StdAtomicUsize>,
) -> Result<(), AgentError> {
    match message {
        fns_fs::WatchMessage::Event(event) => {
            let push = coalescer.push(event);
            let changes = coalescer
                .flush_ready(Instant::now(), &ConservativePrior)
                .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
            if push == fns_fs::CoalescePush::RescanRequired {
                submit_bridge_changes(
                    handle,
                    vec![fns_fs::FsChange::RescanRequired],
                    queued_batches,
                )
                .await
            } else {
                submit_bridge_changes(handle, changes, queued_batches).await
            }
        }
        fns_fs::WatchMessage::Gap(_) => {
            let discard_at = Instant::now()
                .checked_add(fns_fs::RENAME_WINDOW.max(fns_fs::DEBOUNCE_WINDOW))
                .ok_or_else(|| AgentError::new(AgentErrorCode::Core))?;
            drop(
                coalescer
                    .flush_ready(discard_at, &ConservativePrior)
                    .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?,
            );
            submit_bridge_changes(
                handle,
                vec![fns_fs::FsChange::RescanRequired],
                queued_batches,
            )
            .await
        }
    }
}

async fn submit_bridge_changes(
    handle: &fns_transport::EngineHandle,
    changes: Vec<fns_fs::FsChange>,
    queued_batches: &StdArc<StdAtomicUsize>,
) -> Result<(), AgentError> {
    if changes.is_empty() {
        return Ok(());
    }
    let _pending = PendingWatcherBatch::new(StdArc::clone(queued_batches));
    submit_engine_changes(handle, changes).await
}

struct PendingWatcherBatch(StdArc<StdAtomicUsize>);

impl PendingWatcherBatch {
    fn new(counter: StdArc<StdAtomicUsize>) -> Self {
        counter.fetch_add(1, StdOrdering::AcqRel);
        Self(counter)
    }
}

impl Drop for PendingWatcherBatch {
    fn drop(&mut self) {
        self.0.fetch_sub(1, StdOrdering::AcqRel);
    }
}

async fn submit_engine_changes(
    handle: &fns_transport::EngineHandle,
    changes: Vec<fns_fs::FsChange>,
) -> Result<(), AgentError> {
    match tokio::time::timeout(
        ENGINE_SUBMISSION_TIMEOUT,
        handle.record_local_changes(changes),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(map_transport_error(error.code())),
        Err(_) => Err(AgentError::new(AgentErrorCode::Core)),
    }
}

/// Conservative prior lookup that always returns None (no engine state query).
/// The engine deduplicates echo events internally.
struct ConservativePrior;

impl fns_fs::PriorEntryLookup for ConservativePrior {
    fn signature(&self, _path: &fns_protocol::WorkspacePath) -> Option<fns_fs::EntrySignature> {
        None
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

async fn write_session_status(
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    handle: &fns_transport::EngineHandle,
    queued_watcher_batches: &StdArc<StdAtomicUsize>,
    diagnostics: &RuntimeDiagnostics,
    session: SessionRuntimeStatus,
    reconnect_attempt: u32,
) -> Result<(), AgentError> {
    let (phase, connected) = match session.phase {
        SessionConnectionPhase::Handshaking => (AgentPhase::Connecting, false),
        SessionConnectionPhase::Subscribing => (AgentPhase::Subscribing, true),
        SessionConnectionPhase::Online => (AgentPhase::Online, true),
    };
    write_runtime_status(
        state_dir,
        workspace_id,
        handle,
        queued_watcher_batches,
        diagnostics,
        phase,
        connected,
        session.active_transfers,
        reconnect_attempt,
        None,
    )
    .await
}

async fn write_reconnect_status(
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    handle: &fns_transport::EngineHandle,
    queued_watcher_batches: &StdArc<StdAtomicUsize>,
    diagnostics: &RuntimeDiagnostics,
    reconnect_attempt: u32,
    last_error_code: AgentErrorCode,
) -> Result<(), AgentError> {
    diagnostics.bump_connection_generation();
    write_runtime_status(
        state_dir,
        workspace_id,
        handle,
        queued_watcher_batches,
        diagnostics,
        AgentPhase::Connecting,
        false,
        0,
        reconnect_attempt,
        Some(last_error_code),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn write_runtime_status(
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    handle: &fns_transport::EngineHandle,
    queued_watcher_batches: &StdArc<StdAtomicUsize>,
    diagnostics: &RuntimeDiagnostics,
    phase: AgentPhase,
    connected: bool,
    active_transfers: usize,
    reconnect_attempt: u32,
    last_error_code: Option<AgentErrorCode>,
) -> Result<(), AgentError> {
    let queued_watcher_batches = queued_watcher_batches.load(StdOrdering::Acquire);
    let engine =
        match tokio::time::timeout(ENGINE_SUBMISSION_TIMEOUT, handle.runtime_status()).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => return Err(map_transport_error(error.code())),
            Err(_) => return Err(AgentError::new(AgentErrorCode::Core)),
        };
    let status = AgentStatus {
        schema_version: "fns-agent-status/1".into(),
        running: true,
        phase,
        pid: Some(std::process::id()),
        connected,
        workspace_id,
        last_ack_revision: engine.last_ack_revision,
        pending_commands: engine.pending_commands,
        queued_watcher_batches,
        active_transfers,
        reconnect_attempt,
        last_error_code,
        updated_at_ms: now_ms(),
    };
    status
        .write_to(&state_dir.join("runtime-status.json"))
        .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))?;
    let phase_name = serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let error_name = last_error_code.and_then(|code| {
        serde_json::to_value(code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
    });
    // Best-effort durable diagnostic event; never fails the status path.
    crate::obs::emit_status_snapshot(
        diagnostics,
        &phase_name,
        connected,
        engine.pending_commands,
        queued_watcher_batches,
        active_transfers,
        &engine.last_ack_revision.to_string(),
        reconnect_attempt,
        error_name.as_deref(),
    );
    // Boundary events on the real agent path (outbox/stream/apply/cursor/watcher).
    let sync_diag = SyncDiagnostics::new(Arc::new(diagnostics.clone()));
    sync_diag.on_outbox_snapshot(
        engine.outbox_queued,
        engine.outbox_dispatched,
        engine.outbox_awaiting_blob,
        engine.outbox_blocked_conflict,
    );
    if engine.stream_active {
        sync_diag.on_stream_advance(
            "active",
            &engine.last_applied_revision.to_string(),
            0,
            false,
        );
    }
    sync_diag.on_apply_progress(engine.last_applied_revision.get(), 0);
    sync_diag.on_cursor(
        &engine.last_ack_revision.to_string(),
        &engine.last_applied_revision.to_string(),
        engine.pending_ack,
        engine.pending_segment_ack,
    );
    crate::obs::emit_watcher(
        diagnostics,
        "watcher.queue.snapshot",
        "watcher queue depth",
        queued_watcher_batches,
    );
    Ok(())
}

fn previous_status(state_dir: &Path, workspace_id: fns_protocol::WorkspaceId) -> AgentStatus {
    let status = AgentStatus::read_or_stored(&state_dir.join("runtime-status.json"), workspace_id);
    if status.workspace_id == workspace_id {
        status
    } else {
        AgentStatus::stopped(workspace_id)
    }
}

fn write_status(
    state_dir: &std::path::Path,
    phase: AgentPhase,
    workspace_id: fns_protocol::WorkspaceId,
) -> Result<(), AgentError> {
    let mut status = previous_status(state_dir, workspace_id);
    status.schema_version = "fns-agent-status/1".into();
    status.running = true;
    status.phase = phase;
    status.pid = Some(std::process::id());
    status.connected = phase == AgentPhase::Online || phase == AgentPhase::Subscribing;
    status.active_transfers = 0;
    status.reconnect_attempt = 0;
    status.last_error_code = None;
    status.updated_at_ms = now_ms();
    let path = state_dir.join("runtime-status.json");
    status
        .write_to(&path)
        .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))
}

fn write_status_error(
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
    code: AgentErrorCode,
) -> Result<(), AgentError> {
    let mut status = previous_status(state_dir, workspace_id);
    status.schema_version = "fns-agent-status/1".into();
    status.running = false;
    status.phase = AgentPhase::Fatal;
    status.pid = None;
    status.connected = false;
    status.queued_watcher_batches = 0;
    status.active_transfers = 0;
    status.last_error_code = Some(code);
    status.updated_at_ms = now_ms();
    let path = state_dir.join("runtime-status.json");
    status
        .write_to(&path)
        .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))
}

pub(crate) fn persist_fatal_status(
    state_dir: &Path,
    workspace_id: fns_protocol::WorkspaceId,
    code: AgentErrorCode,
) -> Result<(), AgentError> {
    write_status_error(state_dir, workspace_id, code)
}

fn write_status_stopped(
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
) -> Result<(), AgentError> {
    let mut status = previous_status(state_dir, workspace_id);
    status.schema_version = "fns-agent-status/1".into();
    status.running = false;
    status.phase = AgentPhase::Stopped;
    status.pid = None;
    status.connected = false;
    status.queued_watcher_batches = 0;
    status.active_transfers = 0;
    status.reconnect_attempt = 0;
    status.last_error_code = None;
    status.updated_at_ms = now_ms();
    let path = state_dir.join("runtime-status.json");
    status
        .write_to(&path)
        .map_err(|_| AgentError::new(AgentErrorCode::Filesystem))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    };

    use fns_fs::{FsError, SyncRuleConfig, SyncRules};

    struct ActiveBridge {
        active: Arc<AtomicUsize>,
        stopped: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl ActiveBridge {
        fn new(active: Arc<AtomicUsize>, stopped: tokio::sync::oneshot::Sender<()>) -> Self {
            active.fetch_add(1, AtomicOrdering::SeqCst);
            Self {
                active,
                stopped: Some(stopped),
            }
        }
    }

    impl Drop for ActiveBridge {
        fn drop(&mut self) {
            self.active.fetch_sub(1, AtomicOrdering::SeqCst);
            if let Some(stopped) = self.stopped.take() {
                let _result = stopped.send(());
            }
        }
    }

    fn rules_config() -> SyncRuleConfig {
        SyncRuleConfig {
            includes: Vec::new(),
            excludes: Vec::new(),
            protect_secrets: true,
        }
    }

    fn agent_config(workspace: &Path, state: &Path) -> AgentConfig {
        AgentConfig {
            schema_version: "fns-agent-config/1".into(),
            endpoint: "ws://127.0.0.1:1/api/user/workspace-sync/v2".into(),
            workspace_id: fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001")
                .unwrap(),
            client_id: fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002")
                .unwrap(),
            workspace_root: workspace.to_path_buf(),
            state_dir: state.to_path_buf(),
            token_file: state.join("token"),
            sync: crate::config::AgentSyncConfig {
                includes: Vec::new(),
                excludes: Vec::new(),
                protect_secrets: true,
            },
            transport: crate::config::AgentTransportConfig {
                max_active_transfers: 1,
            },
        }
    }

    fn engine(
        workspace: &Path,
        state: &Path,
        rules: SyncRuleConfig,
    ) -> (EngineWorker, fns_transport::EngineHandle) {
        let config = SyncEngineConfig::new(
            fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
            fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap(),
            workspace,
            state,
        )
        .with_sync_rules(rules);
        EngineWorker::spawn(SyncEngine::open(config).unwrap())
    }

    fn has_mutation(commands: &[fns_sync_core::SyncCommand], path: &str) -> bool {
        commands.iter().any(|command| {
            command
                .mutation()
                .is_ok_and(|mutation| mutation.path.as_str() == path)
        })
    }

    async fn wait_for_mutation(handle: &fns_transport::EngineHandle, path: &str) -> bool {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let commands = handle.pending_commands(32).await.unwrap();
                if has_mutation(&commands, path) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    async fn stop_engine(worker: EngineWorker, handle: &fns_transport::EngineHandle) {
        handle.shutdown().await.unwrap();
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn startup_reconciliation_records_a_preexisting_file() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("existing.txt"), b"existing").unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);

        let mut watcher = start_watcher_and_reconcile(workspace.path(), rules, handle.clone())
            .await
            .unwrap();
        let commands = handle.pending_commands(32).await.unwrap();
        let found = has_mutation(&commands, "existing.txt");

        watcher.shutdown(Duration::from_secs(2)).await.unwrap();
        stop_engine(worker, &handle).await;
        assert!(found, "initial reconciliation did not queue existing.txt");
    }

    #[tokio::test]
    async fn isolated_real_file_event_flushes_after_debounce() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        let mut watcher = start_watcher_and_reconcile(workspace.path(), rules, handle.clone())
            .await
            .unwrap();

        std::fs::write(workspace.path().join("isolated.txt"), b"isolated").unwrap();
        let found = wait_for_mutation(&handle, "isolated.txt").await;

        watcher.shutdown(Duration::from_secs(2)).await.unwrap();
        stop_engine(worker, &handle).await;
        assert!(found, "isolated event was not flushed by the timer");
    }

    #[test]
    fn invalid_rules_return_a_stable_configuration_error() {
        let config = SyncRuleConfig {
            includes: vec!["[".into()],
            excludes: Vec::new(),
            protect_secrets: true,
        };

        let error = match compile_sync_rules(config) {
            Ok(_) => panic!("invalid rules unexpectedly compiled"),
            Err(error) => error,
        };
        assert_eq!(error.code(), AgentErrorCode::InvalidConfiguration);
    }

    #[tokio::test]
    async fn embedded_invalid_rules_write_fatal_status() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut config = agent_config(workspace.path(), state.path());
        let workspace_id = config.workspace_id;
        config.sync.includes = vec!["[".into()];
        let token = fns_platform::SecretToken::from_bytes_for_test(b"test-token");

        let error = run_embedded(config, token, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap_err();
        let status =
            AgentStatus::read_or_stored(&state.path().join("runtime-status.json"), workspace_id);

        assert_eq!(error.code(), AgentErrorCode::InvalidConfiguration);
        assert_eq!(status.phase, AgentPhase::Fatal);
        assert!(!status.running);
        assert_eq!(
            status.last_error_code,
            Some(AgentErrorCode::InvalidConfiguration)
        );
    }

    #[tokio::test]
    async fn embedded_missing_root_write_fatal_status() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut config = agent_config(workspace.path(), state.path());
        let workspace_id = config.workspace_id;
        config.workspace_root = workspace.path().join("missing");
        let token = fns_platform::SecretToken::from_bytes_for_test(b"test-token");

        let error = run_embedded(config, token, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap_err();
        let status =
            AgentStatus::read_or_stored(&state.path().join("runtime-status.json"), workspace_id);

        assert_eq!(error.code(), AgentErrorCode::Filesystem);
        assert_eq!(status.phase, AgentPhase::Fatal);
        assert!(!status.running);
        assert_eq!(status.last_error_code, Some(AgentErrorCode::Filesystem));
    }

    #[tokio::test]
    async fn root_creation_failure_is_returned() {
        let workspace = tempfile::tempdir().unwrap();
        let missing_root = workspace.path().join("missing");
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);

        let error = match start_watcher(&missing_root, rules, handle.clone()) {
            Ok(_) => panic!("missing root unexpectedly started"),
            Err(error) => error,
        };

        stop_engine(worker, &handle).await;
        assert_eq!(error.code(), AgentErrorCode::Filesystem);
    }

    #[tokio::test]
    async fn platform_watcher_creation_failure_is_returned() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);

        let error = match start_watcher_with(
            workspace.path(),
            rules,
            handle.clone(),
            |_root| {
                Err(FsError::Io {
                    operation: "injected watcher failure",
                })
            },
            |future| {
                Ok(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                    future,
                )))
            },
        ) {
            Ok(_) => panic!("injected watcher failure unexpectedly started"),
            Err(error) => error,
        };

        stop_engine(worker, &handle).await;
        assert_eq!(error.code(), AgentErrorCode::Filesystem);
    }

    #[tokio::test]
    async fn bridge_spawn_failure_is_returned() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);

        let error = match start_watcher_with(
            workspace.path(),
            rules,
            handle.clone(),
            |root| fns_fs::start_platform_watcher(root, 32),
            |_future| Err(AgentError::new(AgentErrorCode::Core)),
        ) {
            Ok(_) => panic!("injected bridge failure unexpectedly started"),
            Err(error) => error,
        };

        stop_engine(worker, &handle).await;
        assert_eq!(error.code(), AgentErrorCode::Core);
    }

    #[tokio::test]
    async fn initial_reconciliation_submission_failure_is_returned() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        stop_engine(worker, &handle).await;

        let error = match start_watcher_and_reconcile(workspace.path(), rules, handle).await {
            Ok(_) => panic!("disconnected engine unexpectedly reconciled"),
            Err(error) => error,
        };
        assert_eq!(error.code(), AgentErrorCode::Core);
    }

    #[tokio::test]
    async fn bridge_submission_failure_is_observable() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        let mut watcher = start_watcher_and_reconcile(workspace.path(), rules, handle.clone())
            .await
            .unwrap();
        stop_engine(worker, &handle).await;

        std::fs::write(workspace.path().join("failure.txt"), b"failure").unwrap();
        tokio::time::timeout(Duration::from_secs(3), watcher.failure_token().cancelled())
            .await
            .unwrap();
        let error = watcher.shutdown(Duration::from_secs(2)).await.unwrap_err();
        assert_eq!(error.code(), AgentErrorCode::Core);
    }

    #[tokio::test]
    async fn shutdown_reconciles_late_work_and_joins_the_bridge() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        let mut watcher = start_watcher_and_reconcile(workspace.path(), rules, handle.clone())
            .await
            .unwrap();

        std::fs::write(workspace.path().join("late.txt"), b"late").unwrap();
        watcher.shutdown(Duration::from_secs(2)).await.unwrap();
        let commands = handle.pending_commands(32).await.unwrap();
        let found = has_mutation(&commands, "late.txt");
        let joined = watcher.bridge.is_none();

        stop_engine(worker, &handle).await;
        assert!(found, "shutdown did not reconcile late.txt");
        assert!(joined, "bridge thread remained owned after shutdown");
    }

    #[test]
    fn status_write_failure_is_returned() {
        let state_path = tempfile::NamedTempFile::new().unwrap();
        let workspace_id =
            fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap();

        let error =
            write_status(state_path.path(), AgentPhase::Starting, workspace_id).unwrap_err();

        assert_eq!(error.code(), AgentErrorCode::Filesystem);
    }

    struct ZeroJitter;

    impl JitterSource for ZeroJitter {
        fn sample_inclusive(&mut self, _upper: u32) -> u32 {
            0
        }
    }

    #[test]
    fn online_transition_resets_reconnect_schedule_once() {
        let mut schedule = ReconnectSchedule::new(ReconnectPolicy::default(), ZeroJitter);
        let _ = schedule.next_delay();
        let _ = schedule.next_delay();
        assert_eq!(schedule.attempt(), 2);
        let mut reached_online = false;

        reset_reconnect_after_online(
            &mut schedule,
            &mut reached_online,
            SessionConnectionPhase::Subscribing,
        );
        assert_eq!(schedule.attempt(), 2);
        reset_reconnect_after_online(
            &mut schedule,
            &mut reached_online,
            SessionConnectionPhase::Online,
        );
        assert_eq!(schedule.attempt(), 0);

        let _ = schedule.next_delay();
        reset_reconnect_after_online(
            &mut schedule,
            &mut reached_online,
            SessionConnectionPhase::Online,
        );
        assert_eq!(schedule.attempt(), 1);
    }

    #[test]
    fn transport_errors_keep_their_observable_category() {
        assert_eq!(
            map_transport_error(fns_transport::TransportErrorCode::ResourceLimit).code(),
            AgentErrorCode::ResourceLimit
        );
        assert_eq!(
            map_transport_error(fns_transport::TransportErrorCode::Network).code(),
            AgentErrorCode::Network
        );
        assert_eq!(
            map_transport_error(fns_transport::TransportErrorCode::IdleTimeout).code(),
            AgentErrorCode::IdleTimeout
        );
        assert_eq!(
            map_transport_error(fns_transport::TransportErrorCode::TransferTimeout).code(),
            AgentErrorCode::TransferTimeout
        );
    }

    #[tokio::test]
    async fn reconnect_status_records_the_actual_retryable_error() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config());
        let queued = StdArc::new(StdAtomicUsize::new(0));
        let workspace_id = agent_config(workspace.path(), state.path()).workspace_id;
        let diagnostics = crate::obs::open_agent_diagnostics(
            state.path(),
            &workspace_id.to_string(),
            Some("run-status-reuse".into()),
        );

        write_reconnect_status(
            state.path(),
            workspace_id,
            &handle,
            &queued,
            &diagnostics,
            3,
            AgentErrorCode::ResourceLimit,
        )
        .await
        .unwrap();
        let status: AgentStatus = serde_json::from_slice(
            &std::fs::read(state.path().join("runtime-status.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(status.phase, AgentPhase::Connecting);
        assert!(!status.connected);
        assert_eq!(status.reconnect_attempt, 3);
        assert_eq!(status.last_error_code, Some(AgentErrorCode::ResourceLimit));

        write_reconnect_status(
            state.path(),
            workspace_id,
            &handle,
            &queued,
            &diagnostics,
            4,
            AgentErrorCode::TransferTimeout,
        )
        .await
        .unwrap();
        let status: AgentStatus = serde_json::from_slice(
            &std::fs::read(state.path().join("runtime-status.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(status.reconnect_attempt, 4);
        assert_eq!(
            status.last_error_code,
            Some(AgentErrorCode::TransferTimeout)
        );
        let diagnostic_lines =
            std::fs::read_to_string(state.path().join("diagnostics").join("events.jsonl")).unwrap();
        let status_events: Vec<fns_observability::DiagnosticEvent> = diagnostic_lines
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|event: &fns_observability::DiagnosticEvent| {
                event.event_name == "agent.status.published"
            })
            .collect();
        assert_eq!(status_events.len(), 2);
        assert!(status_events.iter().all(|event| {
            event.run_id == "run-status-reuse"
                && event.fields.contains_key("queuedWatcherBatches")
                && event.fields.contains_key("lastErrorCode")
        }));

        stop_engine(worker, &handle).await;
    }

    #[tokio::test]
    async fn online_status_uses_engine_watcher_and_session_metrics() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("pending.txt"), b"pending").unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config());
        handle
            .record_local_changes(vec![fns_fs::FsChange::RescanRequired])
            .await
            .unwrap();
        let queued = StdArc::new(StdAtomicUsize::new(2));
        let workspace_id = agent_config(workspace.path(), state.path()).workspace_id;
        let diagnostics = crate::obs::open_agent_diagnostics(
            state.path(),
            &workspace_id.to_string(),
            Some("run-online-status".into()),
        );

        write_session_status(
            state.path(),
            workspace_id,
            &handle,
            &queued,
            &diagnostics,
            SessionRuntimeStatus {
                phase: SessionConnectionPhase::Online,
                active_transfers: 3,
            },
            0,
        )
        .await
        .unwrap();
        let status: AgentStatus = serde_json::from_slice(
            &std::fs::read(state.path().join("runtime-status.json")).unwrap(),
        )
        .unwrap();

        assert_eq!(status.phase, AgentPhase::Online);
        assert!(status.running && status.connected);
        assert_eq!(
            status.last_ack_revision,
            fns_protocol::WorkspaceRevision::ZERO
        );
        assert_eq!(status.pending_commands, 1);
        assert_eq!(status.queued_watcher_batches, 2);
        assert_eq!(status.active_transfers, 3);
        stop_engine(worker, &handle).await;
    }

    #[tokio::test]
    async fn dropping_watcher_runtime_aborts_bridge_without_orphan() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        let active = Arc::new(AtomicUsize::new(0));
        let task_active = Arc::clone(&active);
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (stopped_sender, stopped_receiver) = tokio::sync::oneshot::channel();
        let watcher = start_watcher_with(
            workspace.path(),
            rules,
            handle.clone(),
            |root| fns_fs::start_platform_watcher(root, 32),
            move |_bridge_future| {
                Ok(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                    async move {
                        let _active = ActiveBridge::new(task_active, stopped_sender);
                        let _result = started_sender.send(());
                        std::future::pending::<()>().await;
                        Ok(())
                    },
                )))
            },
        )
        .unwrap();
        started_receiver.await.unwrap();
        assert_eq!(active.load(AtomicOrdering::SeqCst), 1);

        drop(watcher);
        tokio::time::timeout(Duration::from_secs(1), stopped_receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);

        stop_engine(worker, &handle).await;
    }

    #[tokio::test]
    async fn watcher_shutdown_timeout_aborts_and_awaits_bridge() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let rules_config = rules_config();
        let rules = SyncRules::compile(rules_config.clone()).unwrap();
        let (worker, handle) = engine(workspace.path(), state.path(), rules_config);
        let active = Arc::new(AtomicUsize::new(0));
        let task_active = Arc::clone(&active);
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (stopped_sender, stopped_receiver) = tokio::sync::oneshot::channel();
        let mut watcher = start_watcher_with(
            workspace.path(),
            rules,
            handle.clone(),
            |root| fns_fs::start_platform_watcher(root, 32),
            move |_bridge_future| {
                Ok(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                    async move {
                        let _active = ActiveBridge::new(task_active, stopped_sender);
                        let _result = started_sender.send(());
                        std::future::pending::<()>().await;
                        Ok(())
                    },
                )))
            },
        )
        .unwrap();
        started_receiver.await.unwrap();

        let error = watcher
            .shutdown(Duration::from_millis(1))
            .await
            .unwrap_err();
        tokio::time::timeout(Duration::from_secs(1), stopped_receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(error.code(), AgentErrorCode::ShutdownTimeout);
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
        assert!(watcher.bridge.is_none());

        stop_engine(worker, &handle).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_registration_failure_is_returned() {
        let result = wait_for_shutdown_signal_from(
            || -> std::io::Result<std::future::Pending<Option<()>>> {
                Err(std::io::Error::other("injected registration failure"))
            },
            std::future::pending::<std::io::Result<()>>(),
        )
        .await;

        assert_eq!(result.unwrap_err().code(), AgentErrorCode::Core);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ctrl_c_await_failure_is_returned() {
        let result = wait_for_shutdown_signal_from(
            || Ok(std::future::pending::<Option<()>>()),
            std::future::ready(Err(std::io::Error::other("injected ctrl-c failure"))),
        )
        .await;

        assert_eq!(result.unwrap_err().code(), AgentErrorCode::Core);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_terminate_channel_is_returned() {
        let result = wait_for_shutdown_signal_from(
            || Ok(std::future::ready(None)),
            std::future::pending::<std::io::Result<()>>(),
        )
        .await;

        assert_eq!(result.unwrap_err().code(), AgentErrorCode::Core);
    }

    #[tokio::test]
    async fn standalone_signal_error_finalizes_runtime_before_returning() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let config = agent_config(workspace.path(), state.path());
        let workspace_id = config.workspace_id;
        let token = fns_platform::SecretToken::from_bytes_for_test(b"test-token");

        let result = run_with_shutdown_signal(config, token, async {
            Err(AgentError::new(AgentErrorCode::Core))
        })
        .await;
        let status =
            AgentStatus::read_or_stored(&state.path().join("runtime-status.json"), workspace_id);

        assert_eq!(result.unwrap_err().code(), AgentErrorCode::Core);
        assert_eq!(status.phase, AgentPhase::Fatal);
        assert_eq!(status.last_error_code, Some(AgentErrorCode::Core));
    }

    #[tokio::test]
    async fn embedded_cancelled_start_runs_full_finalizer() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let config = agent_config(workspace.path(), state.path());
        let workspace_id = config.workspace_id;
        let token = fns_platform::SecretToken::from_bytes_for_test(b"test-token");
        let shutdown = tokio_util::sync::CancellationToken::new();
        shutdown.cancel();

        run_embedded(config, token, shutdown).await.unwrap();
        let status =
            AgentStatus::read_or_stored(&state.path().join("runtime-status.json"), workspace_id);

        assert_eq!(status.phase, AgentPhase::Stopped);
        assert!(!status.running);
    }

    #[tokio::test]
    async fn finalize_runtime_stops_watcher_engine_and_writes_fatal_status() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let config = agent_config(workspace.path(), state.path());
        let workspace_id = config.workspace_id;
        let (mut watcher, worker, handle) = start_local_runtime(&config).await.unwrap();
        let diagnostics = crate::obs::open_agent_diagnostics(
            state.path(),
            &workspace_id.to_string(),
            Some("run-finalize".into()),
        );

        let result = finalize_runtime(
            &mut watcher,
            worker,
            handle,
            state.path(),
            workspace_id,
            &diagnostics,
            Err(AgentError::new(AgentErrorCode::Network)),
        )
        .await;
        let status =
            AgentStatus::read_or_stored(&state.path().join("runtime-status.json"), workspace_id);

        assert_eq!(result.unwrap_err().code(), AgentErrorCode::Network);
        assert!(watcher.bridge.is_none());
        assert_eq!(status.phase, AgentPhase::Fatal);
        assert_eq!(status.last_error_code, Some(AgentErrorCode::Network));
    }
}
