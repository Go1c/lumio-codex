//! Session supervision: one reconnecting workspace-sync session per project.
//!
//! The supervisor owns the loop 交互设计 6.3 describes — connect, run, back off,
//! connect again — and keeps the shared snapshot the status bar reads. What it
//! does *not* own is the connection itself: that is a [`SessionDriver`], so the
//! loop can be tested against scripted outcomes instead of a live server.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use fns_platform::SecretToken;
use fns_transport::{EngineHandle, WorkspaceEndpoint};

use super::status::{SyncSnapshot, engine_progress};
use super::{DETAIL_CONNECTING, DETAIL_NO_ENDPOINT};

/// How a single connection attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionOutcome {
    /// The socket came up and the session later ended. Reaching this state
    /// means the server accepted our Hello, so the ladder starts over.
    Ended(&'static str),
    /// The session never came up; keep climbing the ladder.
    Unreachable(&'static str),
    /// Credentials or configuration are wrong. Retrying cannot help.
    Fatal(&'static str),
}

/// Everything a connection attempt needs. Deliberately not `Clone`: exactly one
/// supervisor owns it.
pub struct SessionContext {
    pub workspace_id: fns_protocol::WorkspaceId,
    pub client_id: fns_protocol::ClientId,
    pub engine: EngineHandle,
    pub shutdown: CancellationToken,
    pub credentials: Arc<dyn CredentialSource>,
    pub snapshot: Arc<Mutex<SyncSnapshot>>,
}

impl SessionContext {
    /// Mark the session live and refresh the counters behind it.
    ///
    /// A driver calls this the moment its socket is up, so the status bar can
    /// leave 「离线」 without waiting for the session to end.
    pub async fn mark_connected(&self) {
        publish(&self.snapshot, |state| {
            state.connected = true;
            state.retry_at = None;
            state.detail = None;
        });
        refresh_progress(&self.engine, &self.snapshot).await;
    }
}

/// Where a session's endpoint and bearer token come from.
///
/// Split out so the supervisor never reads the keychain or the environment
/// directly, and so tests can supply neither.
pub trait CredentialSource: Send + Sync {
    /// The loopback endpoint the agent is reachable on, if one is configured.
    fn endpoint(&self) -> Option<WorkspaceEndpoint>;
    /// The agent's bearer token, if one is stored.
    fn token(&self) -> Option<SecretToken>;
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait SessionDriver: Send + Sync + 'static {
    fn run<'a>(&'a self, context: &'a SessionContext) -> BoxFuture<'a, SessionOutcome>;
}

/// Drives a real workspace-sync-v2 session over the transport crate.
pub struct RemoteDriver {
    pkg_version: String,
}

impl RemoteDriver {
    pub fn new(pkg_version: impl Into<String>) -> Self {
        Self {
            pkg_version: pkg_version.into(),
        }
    }
}

impl SessionDriver for RemoteDriver {
    fn run<'a>(&'a self, context: &'a SessionContext) -> BoxFuture<'a, SessionOutcome> {
        Box::pin(async move {
            let (Some(endpoint), Some(token)) =
                (context.credentials.endpoint(), context.credentials.token())
            else {
                // No agent has been published for this project yet. Reporting
                // 「离线」 with a reason is the honest answer; inventing progress
                // would not be.
                return SessionOutcome::Unreachable(DETAIL_NO_ENDPOINT);
            };

            let stream =
                match fns_transport::socket::connect(&endpoint, &token, &self.pkg_version).await {
                    Ok(stream) => stream,
                    Err(error) if error.retryable() => {
                        return SessionOutcome::Unreachable(super::DETAIL_UNREACHABLE);
                    }
                    Err(error) => return SessionOutcome::Fatal(fatal_reason(error.code())),
                };
            context.mark_connected().await;

            let (session, mut writer) = fns_transport::session::Session::new(
                stream,
                context.engine.clone(),
                context.workspace_id,
                context.client_id,
                self.pkg_version.clone(),
            );
            match session.run(&mut writer, context.shutdown.clone()).await {
                fns_transport::session::SessionResult::Closed => {
                    SessionOutcome::Ended(super::DETAIL_CLOSED)
                }
                fns_transport::session::SessionResult::Error(error) if error.retryable() => {
                    SessionOutcome::Ended(super::DETAIL_DROPPED)
                }
                fns_transport::session::SessionResult::Error(error) => {
                    SessionOutcome::Fatal(fatal_reason(error.code()))
                }
            }
        })
    }
}

fn fatal_reason(code: fns_transport::TransportErrorCode) -> &'static str {
    match code {
        fns_transport::TransportErrorCode::AuthenticationRejected => super::DETAIL_UNAUTHORIZED,
        fns_transport::TransportErrorCode::Forbidden => super::DETAIL_FORBIDDEN,
        fns_transport::TransportErrorCode::InvalidConfiguration => super::DETAIL_BAD_ENDPOINT,
        _ => super::DETAIL_PROTOCOL,
    }
}

/// Run the connect/run/back-off loop until the project is closed.
///
/// Returns when the token is cancelled or a connection attempt fails in a way
/// that retrying cannot fix.
pub async fn supervise(context: SessionContext, driver: Arc<dyn SessionDriver>) {
    let mut ladder = super::backoff::RetryLadder::new();
    let snapshot = Arc::clone(&context.snapshot);

    loop {
        if context.shutdown.is_cancelled() {
            break;
        }

        publish(&snapshot, |state| {
            state.connected = false;
            state.retry_at = None;
            state.detail = Some(DETAIL_CONNECTING);
        });

        let outcome = driver.run(&context).await;
        refresh_progress(&context.engine, &snapshot).await;

        let detail = match outcome {
            SessionOutcome::Ended(detail) => {
                ladder.reset();
                detail
            }
            SessionOutcome::Unreachable(detail) => detail,
            SessionOutcome::Fatal(detail) => {
                publish(&snapshot, |state| {
                    state.connected = false;
                    state.retry_at = None;
                    state.detail = Some(detail);
                });
                break;
            }
        };

        if context.shutdown.is_cancelled() {
            break;
        }

        let delay = ladder.next_delay();
        let retry_at = Instant::now() + delay;
        publish(&snapshot, |state| {
            state.connected = false;
            state.retry_at = Some(retry_at);
            state.detail = Some(detail);
        });

        tokio::select! {
            _ = context.shutdown.cancelled() => break,
            _ = tokio::time::sleep_until(retry_at) => {}
        }
    }

    let _ = context.engine.shutdown().await;
}

/// Re-read the engine's counters into the shared snapshot.
pub async fn refresh_progress(engine: &EngineHandle, snapshot: &Arc<Mutex<SyncSnapshot>>) {
    let Ok(Ok(progress)) = engine.with_engine(|engine| engine_progress(engine)).await else {
        return;
    };
    publish(snapshot, |state| state.progress = progress);
}

fn publish(snapshot: &Arc<Mutex<SyncSnapshot>>, update: impl FnOnce(&mut SyncSnapshot)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}
