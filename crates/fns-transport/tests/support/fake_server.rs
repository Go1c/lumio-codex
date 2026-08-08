//! A local pre-upgrade-authenticated scripted WebSocket server for transport integration tests.
//!
//! Binds 127.0.0.1:0 and uses tungstenite's `accept_hdr_async` callback to inspect
//! the client request. For reject scenarios, the callback returns an `ErrorResponse`
//! with the desired HTTP status, which tungstenite sends back and the client
//! observes as `Error::Http`.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use http::HeaderValue;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

/// What the fake server observed about the upgrade request.
#[derive(Debug, Default, Clone)]
pub struct UpgradeObservation {
    pub path: Option<String>,
    pub authorization_matched: bool,
    pub client: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub user_agent: Option<String>,
}

/// What kind of fake server to create.
pub enum FakeServerKind {
    /// Require a specific Bearer token and accept the upgrade.
    RequireBearer(String),
    /// Reject the upgrade with a specific HTTP status code.
    RejectUpgrade(http::StatusCode),
}

/// A fake workspace server handle.
pub struct FakeWorkspaceServer {
    pub endpoint: String,
    observation: Arc<Mutex<Option<UpgradeObservation>>>,
}

impl FakeWorkspaceServer {
    /// Create a server that requires a specific Bearer token.
    pub async fn require_bearer(expected_token: String) -> Self {
        Self::start(FakeServerKind::RequireBearer(expected_token)).await
    }

    /// Create a server that rejects the upgrade with a specific status.
    pub async fn reject_upgrade(status: http::StatusCode) -> Self {
        Self::start(FakeServerKind::RejectUpgrade(status)).await
    }

    /// Return the WebSocket endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Read the upgrade observation (available after the connection attempt completes).
    pub fn observation(&self) -> UpgradeObservation {
        self.observation.lock().unwrap().clone().unwrap_or_default()
    }

    #[allow(clippy::result_large_err)]
    async fn start(kind: FakeServerKind) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("ws://127.0.0.1:{}/api/user/workspace-sync/v2", addr.port());

        let observation: Arc<Mutex<Option<UpgradeObservation>>> = Arc::new(Mutex::new(None));
        let obs_clone = Arc::clone(&observation);

        tokio::spawn(async move {
            // Accept one connection then stop.
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };

            match &kind {
                FakeServerKind::RejectUpgrade(status) => {
                    let obs_shared: Arc<Mutex<UpgradeObservation>> =
                        Arc::new(Mutex::new(UpgradeObservation::default()));
                    let obs_for_cb = Arc::clone(&obs_shared);
                    let reject_status = *status;
                    let callback = move |req: &Request, _resp: Response| {
                        let mut obs = obs_for_cb.lock().unwrap();
                        obs.path = Some(req.uri().path().to_string());
                        obs.client = req
                            .headers()
                            .get("X-Client")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);

                        let err_resp = http::Response::builder()
                            .status(reject_status)
                            .body(Some(String::new()))
                            .unwrap();
                        Err(err_resp)
                    };

                    let ws_result = tokio_tungstenite::accept_hdr_async(stream, callback).await;
                    *obs_clone.lock().unwrap() = Some(obs_shared.lock().unwrap().clone());
                    let _ = ws_result;
                }
                FakeServerKind::RequireBearer(expected_token) => {
                    let obs_shared: Arc<Mutex<UpgradeObservation>> =
                        Arc::new(Mutex::new(UpgradeObservation::default()));
                    let obs_for_cb = Arc::clone(&obs_shared);
                    let expected_for_cb = expected_token.clone();
                    let callback = move |req: &Request, resp: Response| {
                        let mut obs = obs_for_cb.lock().unwrap();
                        obs.path = Some(req.uri().path().to_string());

                        if let Some(auth) = req.headers().get("Authorization") {
                            let expected_val = format!("Bearer {expected_for_cb}");
                            let expected_header = HeaderValue::from_str(&expected_val).unwrap();
                            obs.authorization_matched = *auth == expected_header;
                        }

                        obs.client = req
                            .headers()
                            .get("X-Client")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);
                        obs.client_name = req
                            .headers()
                            .get("X-Client-Name")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);
                        obs.client_version = req
                            .headers()
                            .get("X-Client-Version")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);
                        obs.user_agent = req
                            .headers()
                            .get("User-Agent")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);

                        Ok(resp)
                    };

                    let ws_result = tokio_tungstenite::accept_hdr_async(stream, callback).await;
                    *obs_clone.lock().unwrap() = Some(obs_shared.lock().unwrap().clone());

                    if let Ok(mut ws_stream) = ws_result {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_millis(100),
                            ws_stream.next(),
                        )
                        .await;
                    }
                }
            }
        });

        Self {
            endpoint,
            observation,
        }
    }
}
