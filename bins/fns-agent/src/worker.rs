//! Hidden child entrypoint that owns the full daemon and its shutdown watchdog.

use crate::daemon;
use crate::protocol::{ParentFrame, WorkerFrame, read_parent_frame, write_worker_frame};
use crate::{AgentError, AgentErrorCode};
use std::sync::mpsc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const CHILD_WATCHDOG_DEADLINE: Duration = Duration::from_secs(25);
const ENGINE_RPC_DEADLINE: Duration = Duration::from_secs(5);

pub async fn run() -> Result<(), AgentError> {
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let bootstrap = read_parent_frame(&mut input)
        .await?
        .ok_or_else(|| AgentError::new(AgentErrorCode::Protocol))?;
    let ParentFrame::Bootstrap { config, token } = bootstrap else {
        return Err(AgentError::new(AgentErrorCode::Protocol));
    };
    let config = *config;
    let state_dir = config.state_dir.clone();
    let workspace_id = config.workspace_id;
    let token = token.into_token()?;
    let lease_result = fns_platform::StateDirLease::acquire(&state_dir).map_err(|error| {
        if error.code() == fns_platform::PlatformErrorCode::AlreadyRunning {
            AgentError::new(AgentErrorCode::AlreadyRunning)
        } else {
            AgentError::new(AgentErrorCode::Filesystem)
        }
    });
    let _lease = match lease_result {
        Ok(lease) => lease,
        Err(error) => {
            let _ =
                write_worker_frame(&mut output, &WorkerFrame::Fatal { code: error.code() }).await;
            return Err(error);
        }
    };

    let shutdown = CancellationToken::new();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let daemon_shutdown = shutdown.clone();
    let mut daemon_task = tokio::spawn(async move {
        daemon::run_supervised(config, token, daemon_shutdown, ready_tx).await
    });
    tokio::pin!(ready_rx);

    let engine_handle = tokio::select! {
        biased;
        ready = &mut ready_rx => {
            match ready {
                Ok(handle) => handle,
                Err(_) => {
                    return finish_startup_failure(
                        &mut output,
                        &mut daemon_task,
                        &state_dir,
                        workspace_id,
                    )
                    .await;
                }
            }
        }
        result = &mut daemon_task => {
            return report_daemon_result(&mut output, result, &state_dir, workspace_id, false).await;
        }
    };
    if let Err(error) = write_worker_frame(&mut output, &WorkerFrame::Ready).await {
        shutdown.cancel();
        let watchdog = ShutdownWatchdog::start(state_dir.clone(), workspace_id);
        let _ = (&mut daemon_task).await;
        watchdog.complete();
        let _ = daemon::persist_fatal_status(&state_dir, workspace_id, error.code());
        return Err(error);
    }

    let abnormal_control_eof = loop {
        let frame = tokio::select! {
            frame = read_parent_frame(&mut input) => frame,
            result = &mut daemon_task => {
                return report_daemon_result(
                    &mut output,
                    result,
                    &state_dir,
                    workspace_id,
                    false,
                )
                .await;
            }
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            Ok(None) => break true,
            Err(_) => {
                return finish_control_failure(
                    &mut output,
                    &shutdown,
                    &mut daemon_task,
                    &state_dir,
                    workspace_id,
                    AgentErrorCode::Protocol,
                )
                .await;
            }
        };
        match frame {
            ParentFrame::Shutdown => break false,
            ParentFrame::Bootstrap { .. } => {
                return finish_control_failure(
                    &mut output,
                    &shutdown,
                    &mut daemon_task,
                    &state_dir,
                    workspace_id,
                    AgentErrorCode::Protocol,
                )
                .await;
            }
            request @ (ParentFrame::ListConflicts { .. } | ParentFrame::ResolveConflict { .. }) => {
                let response = tokio::select! {
                    response = execute_rpc(request, &engine_handle) => response,
                    result = &mut daemon_task => {
                        return report_daemon_result(
                            &mut output,
                            result,
                            &state_dir,
                            workspace_id,
                            false,
                        )
                        .await;
                    }
                };
                let response = match response {
                    Ok(response) => response,
                    Err(code) => {
                        return finish_control_failure(
                            &mut output,
                            &shutdown,
                            &mut daemon_task,
                            &state_dir,
                            workspace_id,
                            code,
                        )
                        .await;
                    }
                };
                if write_worker_frame(&mut output, &response).await.is_err() {
                    return finish_control_failure(
                        &mut output,
                        &shutdown,
                        &mut daemon_task,
                        &state_dir,
                        workspace_id,
                        AgentErrorCode::Protocol,
                    )
                    .await;
                }
            }
        }
    };

    shutdown.cancel();
    let watchdog = ShutdownWatchdog::start(state_dir.clone(), workspace_id);
    let result = (&mut daemon_task).await;
    watchdog.complete();
    if abnormal_control_eof {
        let code = match &result {
            Ok(Ok(())) => AgentErrorCode::AbnormalExit,
            Ok(Err(error)) => error.code(),
            Err(_) => AgentErrorCode::Core,
        };
        let _ = daemon::persist_fatal_status(&state_dir, workspace_id, code);
        return match result {
            Ok(Ok(())) => Err(AgentError::new(code)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(AgentError::new(AgentErrorCode::Core)),
        };
    }
    report_daemon_result(&mut output, result, &state_dir, workspace_id, true).await
}

async fn execute_rpc(
    request: ParentFrame,
    handle: &fns_transport::EngineHandle,
) -> Result<WorkerFrame, AgentErrorCode> {
    match request {
        ParentFrame::ListConflicts { request_id } => {
            match tokio::time::timeout(ENGINE_RPC_DEADLINE, handle.list_conflicts()).await {
                Ok(Ok(conflicts)) => Ok(WorkerFrame::ConflictsListed {
                    request_id,
                    conflicts,
                }),
                Ok(Err(error)) => Ok(WorkerFrame::RequestFailed {
                    request_id,
                    code: map_transport_error_code(error.code()),
                }),
                Err(_) => Err(AgentErrorCode::RequestTimeout),
            }
        }
        ParentFrame::ResolveConflict {
            request_id,
            conflict_id,
            conflict_revision,
            choice,
        } => {
            match tokio::time::timeout(
                ENGINE_RPC_DEADLINE,
                handle.resolve_conflict(conflict_id, conflict_revision, choice),
            )
            .await
            {
                Ok(Ok(receipt)) => Ok(WorkerFrame::ConflictResolved {
                    request_id,
                    receipt,
                }),
                Ok(Err(error)) => Ok(WorkerFrame::RequestFailed {
                    request_id,
                    code: map_transport_error_code(error.code()),
                }),
                Err(_) => Err(AgentErrorCode::RequestTimeout),
            }
        }
        ParentFrame::Bootstrap { .. } | ParentFrame::Shutdown => Err(AgentErrorCode::Protocol),
    }
}

async fn finish_control_failure(
    output: &mut (impl tokio::io::AsyncWrite + Unpin),
    shutdown: &CancellationToken,
    daemon_task: &mut tokio::task::JoinHandle<Result<(), AgentError>>,
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
    code: AgentErrorCode,
) -> Result<(), AgentError> {
    let error = AgentError::new(code);
    let _ = daemon::persist_fatal_status(state_dir, workspace_id, code);
    let _ = write_worker_frame(output, &WorkerFrame::Fatal { code }).await;
    shutdown.cancel();
    let watchdog = ShutdownWatchdog::start(state_dir.to_path_buf(), workspace_id);
    let _ = daemon_task.await;
    watchdog.complete();
    Err(error)
}

fn map_transport_error_code(code: fns_transport::TransportErrorCode) -> AgentErrorCode {
    match code {
        fns_transport::TransportErrorCode::InvalidConfiguration => {
            AgentErrorCode::InvalidConfiguration
        }
        fns_transport::TransportErrorCode::AuthenticationRejected => {
            AgentErrorCode::AuthenticationRejected
        }
        fns_transport::TransportErrorCode::Forbidden => AgentErrorCode::Forbidden,
        fns_transport::TransportErrorCode::Network
        | fns_transport::TransportErrorCode::IdleTimeout
        | fns_transport::TransportErrorCode::TransferTimeout => AgentErrorCode::Network,
        fns_transport::TransportErrorCode::RequestTimeout => AgentErrorCode::RequestTimeout,
        fns_transport::TransportErrorCode::Protocol => AgentErrorCode::Protocol,
        fns_transport::TransportErrorCode::Core => AgentErrorCode::Core,
        fns_transport::TransportErrorCode::Filesystem => AgentErrorCode::Filesystem,
        fns_transport::TransportErrorCode::StateCorrupt => AgentErrorCode::StateCorrupt,
        fns_transport::TransportErrorCode::ConflictUnavailable => {
            AgentErrorCode::ConflictUnavailable
        }
        fns_transport::TransportErrorCode::ConflictRevisionStale => {
            AgentErrorCode::ConflictRevisionStale
        }
        fns_transport::TransportErrorCode::ConflictResolutionChanged => {
            AgentErrorCode::ConflictResolutionChanged
        }
        fns_transport::TransportErrorCode::ConflictWaitingBlobs => {
            AgentErrorCode::ConflictWaitingBlobs
        }
        fns_transport::TransportErrorCode::ConflictAutomaticResolutionPending => {
            AgentErrorCode::ConflictAutomaticResolutionPending
        }
        fns_transport::TransportErrorCode::ConflictResolutionPending => {
            AgentErrorCode::ConflictResolutionPending
        }
        fns_transport::TransportErrorCode::ConflictRefreshRequired => {
            AgentErrorCode::ConflictRefreshRequired
        }
        fns_transport::TransportErrorCode::ConflictSelectedSideDeleted => {
            AgentErrorCode::ConflictSelectedSideDeleted
        }
        fns_transport::TransportErrorCode::MergeFileRequired => AgentErrorCode::MergeFileRequired,
        fns_transport::TransportErrorCode::MergeContentUnavailable => {
            AgentErrorCode::MergeContentUnavailable
        }
        fns_transport::TransportErrorCode::ResourceLimit => AgentErrorCode::ResourceLimit,
        fns_transport::TransportErrorCode::ShutdownTimeout => AgentErrorCode::ShutdownTimeout,
    }
}

async fn finish_startup_failure(
    output: &mut (impl tokio::io::AsyncWrite + Unpin),
    daemon_task: &mut tokio::task::JoinHandle<Result<(), AgentError>>,
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
) -> Result<(), AgentError> {
    let result = daemon_task.await;
    report_daemon_result(output, result, state_dir, workspace_id, false).await
}

async fn report_daemon_result(
    output: &mut (impl tokio::io::AsyncWrite + Unpin),
    result: Result<Result<(), AgentError>, tokio::task::JoinError>,
    state_dir: &std::path::Path,
    workspace_id: fns_protocol::WorkspaceId,
    shutdown_requested: bool,
) -> Result<(), AgentError> {
    match result {
        Ok(Ok(())) if shutdown_requested => {
            match write_worker_frame(output, &WorkerFrame::Stopped).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = daemon::persist_fatal_status(
                        state_dir,
                        workspace_id,
                        AgentErrorCode::Protocol,
                    );
                    Err(error)
                }
            }
        }
        Ok(Ok(())) => {
            let error = AgentError::new(AgentErrorCode::AbnormalExit);
            let _ = daemon::persist_fatal_status(state_dir, workspace_id, error.code());
            let _ = write_worker_frame(output, &WorkerFrame::Fatal { code: error.code() }).await;
            Err(error)
        }
        Ok(Err(error)) => {
            let _ = daemon::persist_fatal_status(state_dir, workspace_id, error.code());
            let _ = write_worker_frame(output, &WorkerFrame::Fatal { code: error.code() }).await;
            Err(error)
        }
        Err(_) => {
            let error = AgentError::new(AgentErrorCode::Core);
            let _ = daemon::persist_fatal_status(state_dir, workspace_id, error.code());
            let _ = write_worker_frame(output, &WorkerFrame::Fatal { code: error.code() }).await;
            Err(error)
        }
    }
}

struct ShutdownWatchdog {
    done: mpsc::SyncSender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShutdownWatchdog {
    fn start(state_dir: std::path::PathBuf, workspace_id: fns_protocol::WorkspaceId) -> Self {
        let (done, receiver) = mpsc::sync_channel(1);
        let watchdog_state_dir = state_dir.clone();
        let thread = match std::thread::Builder::new()
            .name("fns-agent-shutdown-watchdog".into())
            .spawn(move || {
                if receiver.recv_timeout(CHILD_WATCHDOG_DEADLINE).is_err() {
                    let _ = daemon::persist_fatal_status(
                        &watchdog_state_dir,
                        workspace_id,
                        AgentErrorCode::ShutdownTimeout,
                    );
                    std::process::exit(
                        AgentError::new(AgentErrorCode::ShutdownTimeout).exit_code(),
                    );
                }
            }) {
            Ok(thread) => thread,
            Err(_) => {
                let _ =
                    daemon::persist_fatal_status(&state_dir, workspace_id, AgentErrorCode::Core);
                std::process::exit(AgentError::new(AgentErrorCode::Core).exit_code());
            }
        };
        Self {
            done,
            thread: Some(thread),
        }
    }

    fn complete(mut self) {
        let _ = self.done.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
