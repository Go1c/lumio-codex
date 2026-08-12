//! Control-plane client (`services/cchaven-control`).
//!
//! Response shapes mirror `internal/api/handler_oauth.go`, `handler_me.go` and
//! `internal/service` one-for-one: success bodies are wrapped in `{"data": …}`,
//! failures in `{"error":{"code","message","details"}}`.
//!
//! A [`ControlClient::Mock`] variant serves the same shapes offline so the app
//! is fully developable without a running control plane (see `README.md`).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// OAuth client id registered in `migrations/0002_seed.sql`.
pub const CLIENT_ID: &str = "cchaven-desktop";
/// Scopes the desktop app requests; the seed row allows exactly these three.
pub const SCOPE: &str = "profile workspace offline_access";

/// 控制面 API 主机（运维文档 `docs/ops/01-architecture.md` 的权威取值）。
const DEFAULT_API_BASE: &str = "https://api.cc.lumiogame.com";
/// CC 产品站：下载、文档、账户页。
const DEFAULT_WEB_BASE: &str = "https://cc.lumiogame.com";
/// 统一门户（Lumio 账号中心）。注册、登录与桌面授权确认页都在这里——
/// 账号已收口到 Sub2API，只有门户上才有可用的账号中心会话。
const DEFAULT_PORTAL_BASE: &str = "https://lumiogame.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// --- Wire types (must stay in sync with the Go service) ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Entitlement {
    /// `active` | `trialing` | `none`
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub days_left: i32,
    #[serde(default)]
    pub bonus_days_total: i32,
    #[serde(default)]
    pub expiring_soon: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Activation {
    #[serde(default)]
    pub trial_granted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_expires_at: Option<String>,
    #[serde(default)]
    pub trial_denied_reuse: bool,
    #[serde(default)]
    pub inviter_bonus_days: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub activation: Option<Activation>,
    #[serde(default)]
    pub entitlement: Option<Entitlement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct UserView {
    #[serde(default)]
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResponse {
    pub user: UserView,
    pub entitlement: Entitlement,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
pub struct Notice {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub days_left: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub entitlement: Entitlement,
    #[serde(default)]
    pub notices: Vec<Notice>,
}

/// Device facts reported with the token exchange and every heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub app_version: String,
    pub os_version: String,
    pub arch: String,
}

// --- Errors ---

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlError {
    /// Machine-readable code from the service, or `network` when unreachable.
    pub code: String,
    /// zh-CN message; the service already localises 6.2 fixed copy.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl ControlError {
    pub fn network(detail: impl std::fmt::Display) -> Self {
        Self {
            code: "network".into(),
            message: format!("无法连接服务器。（{detail}）"),
            status: None,
        }
    }

    /// True when the session is gone for good and the UI must return to login.
    pub fn is_session_expired(&self) -> bool {
        matches!(
            self.code.as_str(),
            "unauthorized" | "invalid_grant" | "session_expired" | "token_revoked"
        ) || self.status == Some(401)
    }

    pub fn is_network(&self) -> bool {
        self.code == "network"
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

// --- Configuration ---

#[derive(Debug, Clone)]
pub struct ControlConfig {
    pub api_base: String,
    pub web_base: String,
    /// 统一门户，授权确认页开在这里。
    pub portal_base: String,
    pub mock: bool,
}

impl ControlConfig {
    /// `CCHAVEN_API_BASE` / `CCHAVEN_WEB_BASE` / `CCHAVEN_PORTAL_BASE` point at a
    /// local control plane and portal; `CCHAVEN_CONTROL_MOCK=0|1` forces the
    /// backing implementation. Debug builds default to the mock because the
    /// control plane needs PostgreSQL.
    pub fn from_env() -> Self {
        let mock = match std::env::var("CCHAVEN_CONTROL_MOCK").as_deref() {
            Ok("0") | Ok("false") => false,
            Ok(_) => true,
            Err(_) => cfg!(debug_assertions),
        };
        Self {
            api_base: env_or("CCHAVEN_API_BASE", DEFAULT_API_BASE),
            web_base: env_or("CCHAVEN_WEB_BASE", DEFAULT_WEB_BASE),
            portal_base: env_or("CCHAVEN_PORTAL_BASE", DEFAULT_PORTAL_BASE),
            mock,
        }
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
        .trim_end_matches('/')
        .to_string()
}

// --- Client ---

pub enum ControlClient {
    Http {
        config: ControlConfig,
        http: reqwest::Client,
    },
    Mock {
        config: ControlConfig,
        state: Mutex<MockState>,
    },
}

impl ControlClient {
    pub fn from_env() -> Self {
        Self::new(ControlConfig::from_env())
    }

    pub fn new(config: ControlConfig) -> Self {
        if config.mock {
            return Self::Mock {
                config,
                state: Mutex::new(MockState::from_env()),
            };
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("CCHaven-Desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self::Http { config, http }
    }

    pub fn config(&self) -> &ControlConfig {
        match self {
            Self::Http { config, .. } | Self::Mock { config, .. } => config,
        }
    }

    pub fn is_mock(&self) -> bool {
        matches!(self, Self::Mock { .. })
    }

    /// Build the browser URL for 5.1「通过浏览器登录」.
    ///
    /// 页面开在统一门户：账号已收口到 Sub2API，授权时的用户身份由门户的账号中心
    /// 会话决定（控制面的 `POST /api/v1/oauth/authorize` 只认 Sub2API 令牌）。
    /// 查询参数与 PKCE 契约保持不变，回跳仍走本机回环。
    pub fn authorize_url(&self, redirect_uri: &str, code_challenge: &str, state: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .finish();
        format!("{}/authorize?{}", self.config().portal_base, query)
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
        device: &DeviceInfo,
    ) -> Result<TokenResponse, ControlError> {
        match self {
            Self::Mock { state, .. } => {
                let mut state = state.lock().map_err(mock_poisoned)?;
                state.exchange_code(code)
            }
            Self::Http { .. } => {
                self.post_json(
                    "/api/v1/oauth/token",
                    None,
                    &serde_json::json!({
                        "grant_type": "authorization_code",
                        "code": code,
                        "code_verifier": code_verifier,
                        "client_id": CLIENT_ID,
                        "redirect_uri": redirect_uri,
                        "device_id": device.device_id,
                    }),
                )
                .await
            }
        }
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenResponse, ControlError> {
        match self {
            Self::Mock { state, .. } => {
                let mut state = state.lock().map_err(mock_poisoned)?;
                state.refresh(refresh_token)
            }
            Self::Http { .. } => {
                self.post_json(
                    "/api/v1/oauth/token",
                    None,
                    &serde_json::json!({
                        "grant_type": "refresh_token",
                        "refresh_token": refresh_token,
                    }),
                )
                .await
            }
        }
    }

    pub async fn revoke(&self, refresh_token: &str) -> Result<(), ControlError> {
        match self {
            Self::Mock { state, .. } => {
                let mut state = state.lock().map_err(mock_poisoned)?;
                state.revoke();
                Ok(())
            }
            Self::Http { config, http } => {
                let response = http
                    .post(format!("{}/api/v1/oauth/revoke", config.api_base))
                    .json(&serde_json::json!({ "token": refresh_token }))
                    .send()
                    .await
                    .map_err(ControlError::network)?;
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(decode_error(response).await)
                }
            }
        }
    }

    pub async fn me(&self, access_token: &str) -> Result<MeResponse, ControlError> {
        match self {
            Self::Mock { state, .. } => {
                let state = state.lock().map_err(mock_poisoned)?;
                state.me()
            }
            Self::Http { config, http } => {
                let response = http
                    .get(format!("{}/api/v1/me", config.api_base))
                    .bearer_auth(access_token)
                    .send()
                    .await
                    .map_err(ControlError::network)?;
                decode::<MeResponse>(response).await
            }
        }
    }

    pub async fn heartbeat(
        &self,
        access_token: &str,
        device: &DeviceInfo,
    ) -> Result<HeartbeatResponse, ControlError> {
        match self {
            Self::Mock { state, .. } => {
                let state = state.lock().map_err(mock_poisoned)?;
                state.heartbeat()
            }
            Self::Http { .. } => {
                self.post_json("/api/v1/app/heartbeat", Some(access_token), device)
                    .await
            }
        }
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        bearer: Option<&str>,
        body: &impl Serialize,
    ) -> Result<T, ControlError> {
        let Self::Http { config, http } = self else {
            return Err(ControlError {
                code: "unsupported".into(),
                message: "mock 客户端不支持该请求。".into(),
                status: None,
            });
        };
        let mut request = http
            .post(format!("{}{path}", config.api_base))
            .json(body)
            .timeout(REQUEST_TIMEOUT);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(ControlError::network)?;
        decode::<T>(response).await
    }
}

async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ControlError> {
    if !response.status().is_success() {
        return Err(decode_error(response).await);
    }
    response
        .json::<Envelope<T>>()
        .await
        .map(|envelope| envelope.data)
        .map_err(|e| ControlError {
            code: "malformed_response".into(),
            message: format!("服务器返回了无法解析的内容。（{e}）"),
            status: None,
        })
}

async fn decode_error(response: reqwest::Response) -> ControlError {
    let status = response.status().as_u16();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => ControlError {
            code: envelope.error.code,
            message: envelope.error.message,
            status: Some(status),
        },
        Err(_) => ControlError {
            code: format!("http_{status}"),
            message: format!("服务器返回错误（HTTP {status}）。"),
            status: Some(status),
        },
    }
}

fn mock_poisoned<T>(_: T) -> ControlError {
    ControlError {
        code: "internal".into(),
        message: "内部状态失效，请重试。".into(),
        status: None,
    }
}

// --- Mock backend ---

/// Deterministic stand-in for the control plane.
///
/// Reserved codes drive the failure paths that the login page must cover:
/// `invalid` → `invalid_grant`, `offline` → network error.
pub struct MockState {
    email: String,
    days_left: i32,
    trialing: bool,
    invited: bool,
    trial_claimed: bool,
    signed_out: bool,
    force_offline: bool,
}

impl MockState {
    pub fn from_env() -> Self {
        Self {
            email: env_or("CCHAVEN_MOCK_EMAIL", "mary@example.com"),
            days_left: std::env::var("CCHAVEN_MOCK_DAYS_LEFT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(23),
            trialing: std::env::var("CCHAVEN_MOCK_SUBSCRIBED").as_deref() != Ok("1"),
            invited: std::env::var("CCHAVEN_MOCK_INVITED").as_deref() != Ok("0"),
            trial_claimed: false,
            signed_out: false,
            force_offline: std::env::var("CCHAVEN_MOCK_OFFLINE").as_deref() == Ok("1"),
        }
    }

    fn guard_offline(&self) -> Result<(), ControlError> {
        if self.force_offline {
            return Err(ControlError::network("mock 离线模式"));
        }
        Ok(())
    }

    fn entitlement(&self) -> Entitlement {
        Entitlement {
            status: if self.trialing {
                "trialing".into()
            } else {
                "active".into()
            },
            kind: if self.trialing {
                "trial".into()
            } else {
                "monthly".into()
            },
            expires_at: Some(rfc3339_in_days(i64::from(self.days_left))),
            days_left: self.days_left,
            bonus_days_total: 0,
            expiring_soon: self.days_left <= 3,
        }
    }

    fn exchange_code(&mut self, code: &str) -> Result<TokenResponse, ControlError> {
        self.guard_offline()?;
        if code.is_empty() || code == "invalid" {
            return Err(ControlError {
                code: "invalid_grant".into(),
                message: "授权码无效或已过期，请重新登录。".into(),
                status: Some(400),
            });
        }
        self.signed_out = false;

        // The trial is granted once, on the first APP login of an invited account.
        let activation = if self.invited && !self.trial_claimed {
            self.trial_claimed = true;
            Activation {
                trial_granted: true,
                trial_expires_at: Some(rfc3339_in_days(30)),
                trial_denied_reuse: false,
                inviter_bonus_days: 0,
            }
        } else {
            Activation {
                trial_granted: false,
                trial_expires_at: None,
                trial_denied_reuse: self.invited,
                inviter_bonus_days: 0,
            }
        };

        Ok(TokenResponse {
            access_token: "mock-access-token".into(),
            refresh_token: "mock-refresh-token".into(),
            token_type: "Bearer".into(),
            expires_in: 900,
            activation: Some(activation),
            entitlement: Some(self.entitlement()),
        })
    }

    fn refresh(&mut self, refresh_token: &str) -> Result<TokenResponse, ControlError> {
        self.guard_offline()?;
        if self.signed_out || refresh_token.is_empty() || refresh_token == "expired" {
            return Err(ControlError {
                code: "invalid_grant".into(),
                message: "登录已过期，请重新登录。".into(),
                status: Some(401),
            });
        }
        Ok(TokenResponse {
            access_token: "mock-access-token".into(),
            refresh_token: "mock-refresh-token".into(),
            token_type: "Bearer".into(),
            expires_in: 900,
            activation: None,
            entitlement: Some(self.entitlement()),
        })
    }

    fn revoke(&mut self) {
        self.signed_out = true;
    }

    fn me(&self) -> Result<MeResponse, ControlError> {
        self.guard_offline()?;
        Ok(MeResponse {
            user: UserView {
                id: "1".into(),
                email: self.email.clone(),
                display_name: String::new(),
                created_at: rfc3339_in_days(-90),
            },
            entitlement: self.entitlement(),
        })
    }

    fn heartbeat(&self) -> Result<HeartbeatResponse, ControlError> {
        self.guard_offline()?;
        let entitlement = self.entitlement();
        let notices = if entitlement.expiring_soon {
            vec![Notice {
                kind: "expiring_soon".into(),
                days_left: entitlement.days_left,
            }]
        } else {
            Vec::new()
        };
        Ok(HeartbeatResponse {
            entitlement,
            notices,
        })
    }
}

/// RFC 3339 timestamp `days` from now, matching Go's `time.Time` marshalling.
fn rfc3339_in_days(days: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    format_rfc3339(now + days * 86_400)
}

/// Format a Unix timestamp as UTC RFC 3339 (civil-from-days, no date crate).
fn format_rfc3339(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Howard Hinnant's `civil_from_days`, shifted to the 0000-03-01 era.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> ControlClient {
        ControlClient::new(ControlConfig {
            api_base: "http://127.0.0.1:0".into(),
            web_base: "https://cc.lumiogame.com".into(),
            portal_base: "https://lumiogame.com".into(),
            mock: true,
        })
    }

    #[test]
    fn authorize_url_targets_the_unified_portal_with_pkce() {
        let url = mock().authorize_url("http://127.0.0.1:53682/callback", "chal", "st4te");
        // 授权确认页必须开在账号中心所在的门户：只有那里能拿到 Sub2API 会话。
        assert!(url.starts_with("https://lumiogame.com/authorize?"));
        // PKCE 契约一个字都不能变——存量客户端与控制面都按它对齐。
        assert!(url.contains("client_id=cchaven-desktop"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback"));
        assert!(url.contains("scope=profile+workspace+offline_access"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st4te"));
    }

    #[test]
    fn defaults_target_the_lumio_domains() {
        assert_eq!(DEFAULT_WEB_BASE, "https://cc.lumiogame.com");
        assert_eq!(DEFAULT_API_BASE, "https://api.cc.lumiogame.com");
        assert_eq!(DEFAULT_PORTAL_BASE, "https://lumiogame.com");
    }

    #[test]
    fn env_or_trims_trailing_slash_and_ignores_empty_values() {
        // 覆盖能力是本地联调的唯一入口，同时末尾斜杠必须去掉，
        // 否则拼出来的地址会变成 //authorize。
        assert_eq!(env_or("CCHAVEN_UNSET_FOR_TEST", "https://x.test/"), "https://x.test");
    }

    #[tokio::test]
    async fn mock_grants_the_trial_only_on_the_first_app_login() {
        let client = mock();
        let device = DeviceInfo {
            device_id: "dev".into(),
            app_version: "0.1.0".into(),
            os_version: "15.0".into(),
            arch: "aarch64".into(),
        };

        let first = client
            .exchange_code("code", "verifier", "http://127.0.0.1:1/callback", &device)
            .await
            .expect("first exchange");
        let activation = first.activation.expect("activation");
        assert!(activation.trial_granted);
        assert!(activation.trial_expires_at.is_some());

        let second = client
            .exchange_code("code", "verifier", "http://127.0.0.1:1/callback", &device)
            .await
            .expect("second exchange");
        let activation = second.activation.expect("activation");
        assert!(!activation.trial_granted);
        assert!(activation.trial_denied_reuse);
    }

    #[tokio::test]
    async fn mock_rejects_a_bad_code_with_the_invalid_grant_code() {
        let device = DeviceInfo {
            device_id: "dev".into(),
            app_version: "0.1.0".into(),
            os_version: "15.0".into(),
            arch: "aarch64".into(),
        };
        let error = mock()
            .exchange_code(
                "invalid",
                "verifier",
                "http://127.0.0.1:1/callback",
                &device,
            )
            .await
            .expect_err("must fail");
        assert_eq!(error.code, "invalid_grant");
        assert!(error.is_session_expired());
    }

    #[tokio::test]
    async fn revoking_invalidates_later_refreshes() {
        let client = mock();
        assert!(client.refresh("mock-refresh-token").await.is_ok());
        client.revoke("mock-refresh-token").await.expect("revoke");
        let error = client
            .refresh("mock-refresh-token")
            .await
            .expect_err("must fail");
        assert!(error.is_session_expired());
        assert_eq!(error.message, "登录已过期，请重新登录。");
    }

    #[test]
    fn network_errors_are_recognisable() {
        let error = ControlError::network("connection refused");
        assert!(error.is_network());
        assert!(!error.is_session_expired());
    }

    #[test]
    fn formats_timestamps_like_go() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_770_000_000), "2026-02-02T02:40:00Z");
        // Leap day survives the civil-from-days conversion.
        assert_eq!(format_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }
}
