//! Account access for the desktop app (交互设计 3.4 / 5.1 / 5.6).
//!
//! The app never sees a password: it opens the system browser against the
//! control plane's `/authorize` page and receives an authorization code on a
//! loopback port. Only the refresh token is persisted, and only in the system
//! keychain; the access token lives in memory for the life of the process.

pub mod keychain;
pub mod loopback;
pub mod pkce;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::control::{
    Activation, ControlClient, ControlError, DeviceInfo, Entitlement, HeartbeatResponse, Notice,
};
use keychain::Secrets;
use loopback::LoopbackServer;
use pkce::Pkce;

/// How long the login page waits before showing 「等待授权超时」.
const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh a little before the real expiry so a request never races the clock.
const ACCESS_TOKEN_SKEW: Duration = Duration::from_secs(60);

/// Event emitted when browser authorization finishes (either way).
pub const EVENT_LOGIN_COMPLETED: &str = "auth://login-completed";
pub const EVENT_LOGIN_FAILED: &str = "auth://login-failed";

// --- Views handed to the frontend ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub email: String,
    pub entitlement: Option<Entitlement>,
    /// Present only right after a first APP login that granted a trial.
    pub activation: Option<Activation>,
}

/// Result of `auth_restore_session` — mirrors the three branches of 3.4.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum RestoreOutcome {
    /// No refresh token, or the token was rejected.
    SignedOut {
        /// 6.2 fixed copy when a stored session was rejected.
        message: Option<String>,
    },
    SignedIn {
        session: SessionView,
    },
    /// Network unreachable but a refresh token exists: read-only offline mode.
    Offline {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStarted {
    pub authorize_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginFailure {
    pub code: String,
    pub message: String,
    /// Whether the login page should offer the 「离线使用」 escape hatch.
    pub network: bool,
}

// --- Internal state ---

struct AccessToken {
    value: String,
    fetched_at: Instant,
    lifetime: Duration,
}

impl AccessToken {
    fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() + ACCESS_TOKEN_SKEW < self.lifetime
    }
}

#[derive(Default)]
struct Session {
    email: String,
    access: Option<AccessToken>,
    entitlement: Option<Entitlement>,
}

struct PendingLogin {
    pkce: Pkce,
    redirect_uri: String,
    authorize_url: String,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Everything the account layer needs, managed by Tauri as shared state.
pub struct AuthState {
    control: ControlClient,
    secrets: Secrets,
    device: DeviceInfo,
    session: Mutex<Session>,
    pending: Mutex<Option<PendingLogin>>,
}

impl AuthState {
    pub fn from_env() -> Self {
        Self {
            control: ControlClient::from_env(),
            secrets: Secrets::from_env(),
            device: device_info(),
            session: Mutex::new(Session::default()),
            pending: Mutex::new(None),
        }
    }

    pub fn control(&self) -> &ControlClient {
        &self.control
    }

    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// A valid access token, refreshing silently when the cached one aged out.
    pub async fn access_token(&self) -> Result<String, ControlError> {
        if let Some(token) = self.cached_access_token() {
            return Ok(token);
        }
        let refresh_token = self
            .secrets
            .refresh_token()
            .map_err(secret_error)?
            .ok_or_else(session_expired)?;
        let tokens = self.control.refresh(&refresh_token).await?;
        self.adopt_tokens(
            &tokens.access_token,
            tokens.expires_in,
            &tokens.refresh_token,
        )?;
        if let Some(entitlement) = tokens.entitlement.clone() {
            self.set_entitlement(entitlement);
        }
        Ok(tokens.access_token)
    }

    fn cached_access_token(&self) -> Option<String> {
        let session = self.session.lock().ok()?;
        session
            .access
            .as_ref()
            .filter(|token| token.is_fresh())
            .map(|token| token.value.clone())
    }

    fn adopt_tokens(
        &self,
        access_token: &str,
        expires_in: i64,
        refresh_token: &str,
    ) -> Result<(), ControlError> {
        if !refresh_token.is_empty() {
            self.secrets
                .store_refresh_token(refresh_token)
                .map_err(secret_error)?;
        }
        let mut session = self.session.lock().map_err(poisoned)?;
        session.access = Some(AccessToken {
            value: access_token.to_string(),
            fetched_at: Instant::now(),
            lifetime: Duration::from_secs(expires_in.max(60) as u64),
        });
        Ok(())
    }

    fn set_entitlement(&self, entitlement: Entitlement) {
        if let Ok(mut session) = self.session.lock() {
            session.entitlement = Some(entitlement);
        }
    }

    fn set_email(&self, email: String) {
        if let Ok(mut session) = self.session.lock() {
            session.email = email;
        }
    }

    fn snapshot(&self) -> SessionView {
        let session = self.session.lock().ok();
        SessionView {
            email: session
                .as_ref()
                .map(|s| s.email.clone())
                .unwrap_or_default(),
            entitlement: session.as_ref().and_then(|s| s.entitlement.clone()),
            activation: None,
        }
    }

    fn clear(&self) {
        if let Ok(mut session) = self.session.lock() {
            *session = Session::default();
        }
    }

    /// Exchange an authorization code and hydrate the session (5.1 授权成功).
    async fn complete_login(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<SessionView, ControlError> {
        let tokens = self
            .control
            .exchange_code(code, verifier, redirect_uri, &self.device)
            .await?;
        self.adopt_tokens(
            &tokens.access_token,
            tokens.expires_in,
            &tokens.refresh_token,
        )?;
        if let Some(entitlement) = tokens.entitlement.clone() {
            self.set_entitlement(entitlement);
        }

        // The token response carries entitlement but not the email; /me does.
        if let Ok(me) = self.control.me(&tokens.access_token).await {
            self.set_email(me.user.email);
            self.set_entitlement(me.entitlement);
        }

        let mut view = self.snapshot();
        view.activation = tokens.activation;
        Ok(view)
    }
}

fn poisoned<T>(_: T) -> ControlError {
    ControlError {
        code: "internal".into(),
        message: "内部状态失效，请重启应用。".into(),
        status: None,
    }
}

fn secret_error(error: keychain::SecretError) -> ControlError {
    ControlError {
        code: "keychain".into(),
        message: error.to_string(),
        status: None,
    }
}

fn session_expired() -> ControlError {
    ControlError {
        code: "session_expired".into(),
        // 6.2 fixed copy.
        message: "登录已过期，请重新登录。".into(),
        status: Some(401),
    }
}

/// Device facts reported to the control plane (`device_id` is stable per install).
fn device_info() -> DeviceInfo {
    DeviceInfo {
        device_id: crate::project::device_id(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os_version: os_version(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS.to_string()
    }
}

// --- Tauri commands ---

/// Start browser authorization. Returns as soon as the browser is opened; the
/// outcome arrives as an event so the login page can sit in 「等待授权」.
#[tauri::command]
pub async fn auth_begin_login(
    app: AppHandle,
    state: tauri::State<'_, AuthState>,
) -> Result<LoginStarted, ControlError> {
    cancel_pending(&state);

    let server = LoopbackServer::bind().await.map_err(|e| ControlError {
        code: e.code().into(),
        message: e.message(),
        status: None,
    })?;
    let redirect_uri = server.redirect_uri();
    let pkce = Pkce::generate().map_err(|message| ControlError {
        code: "internal".into(),
        message,
        status: None,
    })?;
    let authorize_url = state
        .control
        .authorize_url(&redirect_uri, pkce.challenge(), pkce.state());

    let expected_state = pkce.state().to_string();
    let verifier = pkce.verifier().to_string();
    let redirect_for_task = redirect_uri.clone();
    let app_for_task = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let outcome = server
            .wait_for_code(&expected_state, AUTHORIZE_TIMEOUT)
            .await;
        let app_state = app_for_task.state::<AuthState>();
        match outcome {
            Ok(code) => {
                match app_state
                    .complete_login(&code, &verifier, &redirect_for_task)
                    .await
                {
                    Ok(session) => {
                        let _ = app_for_task.emit(EVENT_LOGIN_COMPLETED, session);
                    }
                    Err(error) => {
                        let _ = app_for_task.emit(EVENT_LOGIN_FAILED, login_failure(&error));
                    }
                }
            }
            Err(error) => {
                let _ = app_for_task.emit(
                    EVENT_LOGIN_FAILED,
                    LoginFailure {
                        code: error.code().into(),
                        message: error.message(),
                        network: false,
                    },
                );
            }
        }
    });

    if let Ok(mut pending) = state.pending.lock() {
        *pending = Some(PendingLogin {
            pkce,
            redirect_uri: redirect_uri.clone(),
            authorize_url: authorize_url.clone(),
            task,
        });
    }

    open_browser(&authorize_url)?;
    Ok(LoginStarted {
        authorize_url,
        redirect_uri,
    })
}

/// 「重新打开浏览器」 — reuse the pending attempt instead of restarting it.
#[tauri::command]
pub fn auth_reopen_browser(state: tauri::State<'_, AuthState>) -> Result<String, ControlError> {
    let url = state
        .pending
        .lock()
        .map_err(poisoned)?
        .as_ref()
        .map(|pending| pending.authorize_url.clone())
        .ok_or_else(|| ControlError {
            code: "no_pending_login".into(),
            message: "没有正在进行的登录，请重新发起。".into(),
            status: None,
        })?;
    open_browser(&url)?;
    Ok(url)
}

/// 「取消」 on the waiting state.
#[tauri::command]
pub fn auth_cancel_login(state: tauri::State<'_, AuthState>) {
    cancel_pending(&state);
}

/// Fallback for 5.1 超时态: the user pastes the code shown on `/authorize`.
#[tauri::command]
pub async fn auth_submit_manual_code(
    code: String,
    state: tauri::State<'_, AuthState>,
) -> Result<SessionView, ControlError> {
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(ControlError {
            code: "invalid_request".into(),
            message: "请粘贴浏览器中显示的授权码。".into(),
            status: None,
        });
    }

    let (verifier, redirect_uri) = {
        let pending = state.pending.lock().map_err(poisoned)?;
        let pending = pending.as_ref().ok_or_else(|| ControlError {
            code: "no_pending_login".into(),
            message: "登录会话已失效，请重新发起登录。".into(),
            status: None,
        })?;
        (
            pending.pkce.verifier().to_string(),
            pending.redirect_uri.clone(),
        )
    };

    let session = state
        .complete_login(&code, &verifier, &redirect_uri)
        .await?;
    cancel_pending(&state);
    Ok(session)
}

/// Silent login at startup (3.4 flow chart).
#[tauri::command]
pub async fn auth_restore_session(
    state: tauri::State<'_, AuthState>,
) -> Result<RestoreOutcome, ControlError> {
    let Some(refresh_token) = state.secrets.refresh_token().map_err(secret_error)? else {
        return Ok(RestoreOutcome::SignedOut { message: None });
    };

    match state.control.refresh(&refresh_token).await {
        Ok(tokens) => {
            state.adopt_tokens(
                &tokens.access_token,
                tokens.expires_in,
                &tokens.refresh_token,
            )?;
            if let Some(entitlement) = tokens.entitlement.clone() {
                state.set_entitlement(entitlement);
            }
            if let Ok(me) = state.control.me(&tokens.access_token).await {
                state.set_email(me.user.email);
                state.set_entitlement(me.entitlement);
            }
            Ok(RestoreOutcome::SignedIn {
                session: state.snapshot(),
            })
        }
        Err(error) if error.is_network() => Ok(RestoreOutcome::Offline {
            message: error.message,
        }),
        Err(error) => {
            // The stored token is dead — drop it so the next launch is clean.
            let _ = state.secrets.clear_refresh_token();
            state.clear();
            Ok(RestoreOutcome::SignedOut {
                message: Some(if error.is_session_expired() {
                    "登录已过期，请重新登录。".into()
                } else {
                    error.message
                }),
            })
        }
    }
}

/// 「退出登录」: revoke server-side, then clear the keychain no matter what.
#[tauri::command]
pub async fn auth_logout(state: tauri::State<'_, AuthState>) -> Result<(), ControlError> {
    let refresh_token = state.secrets.refresh_token().map_err(secret_error)?;
    if let Some(token) = refresh_token {
        // A revoke failure must not strand the user in a signed-in shell.
        let _ = state.control.revoke(&token).await;
    }
    state.secrets.clear_refresh_token().map_err(secret_error)?;
    state.clear();
    cancel_pending(&state);
    Ok(())
}

/// Periodic heartbeat: reports the device and refreshes entitlement + notices.
#[tauri::command]
pub async fn auth_heartbeat(
    state: tauri::State<'_, AuthState>,
) -> Result<HeartbeatResponse, ControlError> {
    let token = state.access_token().await?;
    let result = state.control.heartbeat(&token, &state.device).await?;
    state.set_entitlement(result.entitlement.clone());
    Ok(result)
}

/// Current in-memory session, used when the shell remounts.
#[tauri::command]
pub fn auth_session(state: tauri::State<'_, AuthState>) -> SessionView {
    state.snapshot()
}

/// Open an external link in the system browser (账户菜单的四个 ↗ 入口).
#[tauri::command]
pub fn open_external(url: String) -> Result<(), ControlError> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(ControlError {
            code: "invalid_request".into(),
            message: "只能打开 http/https 链接。".into(),
            status: None,
        });
    }
    open_browser(&url)
}

fn open_browser(url: &str) -> Result<(), ControlError> {
    open::that_detached(url).map_err(|e| ControlError {
        code: "browser".into(),
        message: format!("无法打开系统浏览器。（{e}）"),
        status: None,
    })
}

fn cancel_pending(state: &tauri::State<'_, AuthState>) {
    if let Ok(mut slot) = state.pending.lock()
        && let Some(pending) = slot.take()
    {
        pending.task.abort();
    }
}

fn login_failure(error: &ControlError) -> LoginFailure {
    LoginFailure {
        code: error.code.clone(),
        message: error.message.clone(),
        network: error.is_network(),
    }
}

/// Notices are surfaced verbatim by the shell banner; re-exported for clarity.
pub type LoginNotice = Notice;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlConfig;

    fn state() -> AuthState {
        AuthState {
            control: ControlClient::new(ControlConfig {
                api_base: "http://127.0.0.1:0".into(),
                web_base: "https://cc.lumiogame.com".into(),
                portal_base: "https://lumiogame.com".into(),
                mock: true,
            }),
            secrets: Secrets::new(Box::new(keychain::MemoryStore::new())),
            device: DeviceInfo {
                device_id: "dev".into(),
                app_version: "0.1.0".into(),
                os_version: "15.0".into(),
                arch: "aarch64".into(),
            },
            session: Mutex::new(Session::default()),
            pending: Mutex::new(None),
        }
    }

    #[tokio::test]
    async fn completing_a_login_stores_the_refresh_token_in_the_keychain() {
        let state = state();
        let session = state
            .complete_login("code", "verifier", "http://127.0.0.1:1/callback")
            .await
            .expect("login");

        assert_eq!(session.email, "mary@example.com");
        assert!(session.activation.expect("activation").trial_granted);
        assert_eq!(
            state.secrets.refresh_token().expect("read"),
            Some("mock-refresh-token".to_string())
        );
    }

    #[tokio::test]
    async fn a_cached_access_token_is_reused_until_it_ages_out() {
        let state = state();
        state
            .complete_login("code", "verifier", "http://127.0.0.1:1/callback")
            .await
            .expect("login");
        assert_eq!(
            state.access_token().await.expect("token"),
            "mock-access-token"
        );

        // Force the cached token to look expired; the refresh path must kick in.
        {
            let mut session = state.session.lock().expect("lock");
            if let Some(access) = session.access.as_mut() {
                access.lifetime = Duration::from_secs(0);
            }
        }
        assert_eq!(
            state.access_token().await.expect("token"),
            "mock-access-token"
        );
    }

    #[tokio::test]
    async fn without_a_stored_token_the_access_path_reports_session_expiry() {
        let state = state();
        let error = state.access_token().await.expect_err("must fail");
        assert_eq!(error.message, "登录已过期，请重新登录。");
    }

    #[test]
    fn only_http_links_can_be_opened_externally() {
        let error = open_external("file:///etc/passwd".into()).expect_err("must reject");
        assert_eq!(error.code, "invalid_request");
    }
}
