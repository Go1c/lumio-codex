use crate::cli::Timeouts;
use crate::process::{OwnedChild, ProcessOutcome, ProcessSpec, Termination};
use crate::secret::SecretMaterial;
use crate::{HarnessError, Result};
use fns_agent::protocol::{read_worker_frame, write_parent_frame, ParentFrame, WorkerFrame};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncRead;
use tokio::process::{ChildStdin, ChildStdout};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownFrame {
    Stopped,
    Escalate,
}

fn bootstrap_frame_result(frame: WorkerFrame) -> Result<()> {
    match frame {
        WorkerFrame::Ready => Ok(()),
        WorkerFrame::Fatal { code } => Err(HarnessError::ProcessDetail(format!(
            "agent reported fatal before ready: {code:?}"
        ))),
        WorkerFrame::Stopped => Err(HarnessError::Process("agent stopped before ready")),
        WorkerFrame::ConflictsListed { .. }
        | WorkerFrame::ConflictResolved { .. }
        | WorkerFrame::RequestFailed { .. } => Err(HarnessError::Process(
            "agent reported a request response before ready",
        )),
    }
}

async fn wait_for_shutdown_frame<R>(events: &mut R, timeout: Duration) -> ShutdownFrame
where
    R: AsyncRead + Unpin,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, read_worker_frame(&mut *events)).await {
            Ok(Ok(WorkerFrame::Stopped)) => return ShutdownFrame::Stopped,
            Ok(Ok(
                WorkerFrame::ConflictsListed { .. }
                | WorkerFrame::ConflictResolved { .. }
                | WorkerFrame::RequestFailed { .. },
            )) => {}
            Ok(Ok(WorkerFrame::Fatal { .. } | WorkerFrame::Ready)) | Ok(Err(_)) | Err(_) => {
                return ShutdownFrame::Escalate;
            }
        }
    }
}

pub struct OwnedAgent {
    label: String,
    child: OwnedChild,
    control: Option<ChildStdin>,
    events: ChildStdout,
}

impl std::fmt::Debug for OwnedAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedAgent")
            .field("label", &self.label)
            .field("pid", &self.child.pid())
            .field("pgid", &self.child.pgid())
            .finish()
    }
}

impl OwnedAgent {
    pub fn launch(label: impl Into<String>, binary: &Path) -> Result<Self> {
        let label = label.into();
        let mut child =
            OwnedChild::spawn(ProcessSpec::control(label.clone(), binary, ["__worker"]))?;
        let control = child
            .take_stdin()
            .ok_or(HarnessError::Process("agent control pipe was unavailable"))?;
        let events = child
            .take_stdout()
            .ok_or(HarnessError::Process("agent event pipe was unavailable"))?;
        Ok(Self {
            label,
            child,
            control: Some(control),
            events,
        })
    }

    pub async fn bootstrap(
        &mut self,
        config: fns_agent::AgentConfig,
        secret: &SecretMaterial,
        timeouts: Timeouts,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let bootstrap = ParentFrame::Bootstrap {
            config: Box::new(config),
            token: secret.agent_secret(),
        };
        let bootstrap_result = match self.control.as_mut() {
            Some(control) => {
                tokio::time::timeout(timeouts.startup, write_parent_frame(control, &bootstrap))
                    .await
                    .map_err(|_| HarnessError::Timeout("agent bootstrap write"))?
            }
            None => return Err(HarnessError::Process("agent control pipe was closed")),
        };
        bootstrap_result?;

        let ready = tokio::select! {
            result = tokio::time::timeout(timeouts.startup, read_worker_frame(&mut self.events)) => {
                match result {
                    Ok(Ok(frame)) => bootstrap_frame_result(frame),
                    Ok(Err(error)) => Err(error.into()),
                    Err(_) => Err(HarnessError::Timeout("agent ready")),
                }
            }
            () = cancellation.cancelled() => Err(HarnessError::Process("agent startup cancelled")),
        };
        ready
    }

    pub fn pid(&self) -> rustix::process::Pid {
        self.child.pid()
    }

    pub fn pgid(&self) -> rustix::process::Pid {
        self.child.pgid()
    }

    pub async fn force_cleanup(&mut self, timeouts: Timeouts) -> Result<ProcessOutcome> {
        self.control.take();
        self.child
            .terminate_and_reap(timeouts.term_grace, timeouts.kill)
            .await
    }

    pub async fn shutdown(
        &mut self,
        timeouts: Timeouts,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutcome> {
        let shutdown_written = match self.control.as_mut() {
            Some(control) => tokio::time::timeout(
                timeouts.term_grace,
                write_parent_frame(control, &ParentFrame::Shutdown),
            )
            .await
            .is_ok_and(|result| result.is_ok()),
            None => false,
        };
        if !shutdown_written {
            self.control.take();
            return self
                .child
                .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                .await;
        }

        enum Stop {
            Frame,
            Exited(std::process::ExitStatus),
            Escalate,
        }
        let stop = tokio::select! {
            frame = wait_for_shutdown_frame(&mut self.events, timeouts.hook) => {
                match frame {
                    ShutdownFrame::Stopped => Stop::Frame,
                    ShutdownFrame::Escalate => Stop::Escalate,
                }
            }
            result = self.child.wait() => Stop::Exited(result?),
            () = cancellation.cancelled() => Stop::Escalate,
        };
        self.control.take();
        match stop {
            Stop::Frame => match tokio::time::timeout(timeouts.term_grace, self.child.wait()).await
            {
                Ok(result) => {
                    self.child
                        .complete_reaped(
                            result?,
                            Termination::Exited,
                            timeouts.term_grace,
                            timeouts.kill,
                        )
                        .await
                }
                Err(_) => {
                    self.child
                        .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                        .await
                }
            },
            Stop::Exited(status) => {
                self.child
                    .complete_reaped(
                        status,
                        Termination::Exited,
                        timeouts.term_grace,
                        timeouts.kill,
                    )
                    .await
            }
            Stop::Escalate => {
                self.child
                    .terminate_and_reap(timeouts.term_grace, timeouts.kill)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fns_agent::{protocol::write_worker_frame, AgentErrorCode};

    fn request_id() -> fns_protocol::RequestId {
        fns_protocol::RequestId::parse("90000000-0000-4000-8000-000000000001").expect("request ID")
    }

    #[test]
    fn bootstrap_rejects_request_responses_before_ready() {
        let error = bootstrap_frame_result(WorkerFrame::RequestFailed {
            request_id: request_id(),
            code: AgentErrorCode::Core,
        })
        .expect_err("request response must not satisfy bootstrap readiness");
        assert!(error.to_string().contains("request response before ready"));
    }

    #[tokio::test]
    async fn shutdown_drains_pending_request_responses_before_stopped() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let write = tokio::spawn(async move {
            write_worker_frame(
                &mut writer,
                &WorkerFrame::RequestFailed {
                    request_id: request_id(),
                    code: AgentErrorCode::Core,
                },
            )
            .await
            .expect("request response");
            write_worker_frame(&mut writer, &WorkerFrame::Stopped)
                .await
                .expect("stopped response");
        });

        assert_eq!(
            wait_for_shutdown_frame(&mut reader, Duration::from_secs(1)).await,
            ShutdownFrame::Stopped
        );
        write.await.expect("writer task");
    }
}
