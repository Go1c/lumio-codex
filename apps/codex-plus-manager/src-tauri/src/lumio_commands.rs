//! Lumio 的 Tauri 命令面。这一层是秘密的终点：访问 / 刷新令牌、临时 2FA token 与
//! API Key 明文只活在 [`LumioSession`] 的进程内状态里，任何 payload 都只带三态状态与
//! 稳定错误码，不带凭据本身。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use codex_plus_core::lumio::account::ensure_desktop_key;
use codex_plus_core::lumio::api::{AccountProfile, AuthOutcome, LumioApiClient, RegisterRequest};
use codex_plus_core::lumio::config_takeover::{self, TakeoverHealth, TakeoverRequest};
use codex_plus_core::lumio::credentials::{CredentialStatus, CredentialStore};
use codex_plus_core::lumio::errors::redact;
use codex_plus_core::lumio::session::AuthSession;
use codex_plus_core::lumio::{launch, product};
use serde::Serialize;

const SESSION_EXPIRED: &str = "AUTH_SESSION_EXPIRED";
const SERVICE_UNAVAILABLE: &str = "SERVICE_UNAVAILABLE";
const KEY_PROVISION_FAILED: &str = "KEY_PROVISION_FAILED";
const KEY_STORAGE_UNAVAILABLE: &str = "KEY_STORAGE_UNAVAILABLE";
const RESTORE_FAILED: &str = "CODEX_RESTORE_FAILED";
const APP_NOT_FOUND: &str = "CODEX_APP_NOT_FOUND";
const UNKNOWN: &str = "UNKNOWN";

const HEALTH_NOT_APPLIED: &str = "not-applied";
const HEALTH_HEALTHY: &str = "healthy";
const HEALTH_CONFLICTED: &str = "conflicted";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioCommandResult<T> {
    pub ok: bool,
    pub error_code: Option<String>,
    pub payload: Option<T>,
}

impl<T> LumioCommandResult<T> {
    fn ok(payload: T) -> Self {
        Self {
            ok: true,
            error_code: None,
            payload: Some(payload),
        }
    }

    fn failed(code: &str) -> Self {
        Self {
            ok: false,
            error_code: Some(code.to_string()),
            payload: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioCodexAppPayload {
    pub path: String,
    pub version: Option<String>,
    pub source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioAccountPayload {
    pub email: String,
    pub balance: f64,
    pub plan_label: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioBootstrapPayload {
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub codex_app: Option<LumioCodexAppPayload>,
    pub account: Option<LumioAccountPayload>,
    pub telemetry_enabled: bool,
    pub auto_update_enabled: bool,
    pub credential_status: CredentialStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioAgreementDocumentPayload {
    pub id: String,
    pub title: String,
    pub content_md: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioServiceSettingsPayload {
    pub registration_enabled: bool,
    pub email_verify_enabled: bool,
    pub email_suffix_whitelist: Vec<String>,
    pub password_reset_enabled: bool,
    pub agreement_enabled: bool,
    pub agreement_revision: String,
    pub agreement_documents: Vec<LumioAgreementDocumentPayload>,
    pub default_model: Option<String>,
    pub site_base_url: String,
    pub payment_path: String,
    /// 账户相关网页（重置密码 / 支持）所在源，与官网 `site_base_url` 分开。
    pub api_base_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioUpdateReminderPayload {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub download_url: String,
    pub release_summary: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioVerifyCodePayload {
    pub countdown: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioAuthPayload {
    pub requires_two_factor: bool,
    pub masked_email: Option<String>,
    pub account: Option<LumioAccountPayload>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioProvisionStepPayload {
    pub step: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioTakeoverHealthPayload {
    pub health: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioTelemetryPayload {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioExportLogsPayload {
    pub path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioEmptyPayload {}

/// 进程内会话。令牌与桌面 Key 都由 [`AuthSession`] 持有，不进 payload、不进日志。
pub struct LumioSession {
    client: LumioApiClient,
    auth: AuthSession,
    pending_two_factor: Mutex<Option<String>>,
    account: Mutex<Option<AccountProfile>>,
    model: Mutex<Option<String>>,
    accepted_agreement: Mutex<Option<String>>,
    codex_app: Mutex<Option<PathBuf>>,
    telemetry_enabled: AtomicBool,
}

impl LumioSession {
    pub fn new() -> anyhow::Result<Self> {
        let client = LumioApiClient::new(product::API_BASE_URL)?;
        let auth = AuthSession::new(CredentialStore::default_store()?);

        Ok(Self {
            client,
            auth,
            pending_two_factor: Mutex::new(None),
            account: Mutex::new(None),
            model: Mutex::new(None),
            accepted_agreement: Mutex::new(None),
            codex_app: Mutex::new(None),
            telemetry_enabled: AtomicBool::new(false),
        })
    }

    fn adopt(&self, outcome: AuthOutcome) -> Result<LumioAuthPayload, String> {
        match outcome {
            AuthOutcome::TwoFactorRequired {
                temp_token,
                masked_email,
            } => {
                *lock(&self.pending_two_factor) = Some(temp_token);
                Ok(LumioAuthPayload {
                    requires_two_factor: true,
                    masked_email: Some(masked_email),
                    account: None,
                })
            }
            AuthOutcome::Tokens { tokens, profile } => {
                *lock(&self.pending_two_factor) = None;
                self.auth.adopt_tokens(
                    tokens.access_token,
                    tokens.refresh_token,
                    profile.email.clone(),
                )?;
                *lock(&self.account) = Some(profile.clone());
                Ok(LumioAuthPayload {
                    requires_two_factor: false,
                    masked_email: None,
                    account: Some(account_payload(&profile)),
                })
            }
        }
    }

    fn forget(&self) {
        *lock(&self.pending_two_factor) = None;
        *lock(&self.account) = None;
        *lock(&self.model) = None;
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn account_payload(profile: &AccountProfile) -> LumioAccountPayload {
    LumioAccountPayload {
        email: profile.email.clone(),
        balance: profile.balance,
        plan_label: None,
    }
}

fn codex_app_payload(path: &std::path::Path, source: &'static str) -> LumioCodexAppPayload {
    LumioCodexAppPayload {
        version: codex_plus_core::app_paths::codex_app_version(path),
        path: path.to_string_lossy().into_owned(),
        source,
    }
}

fn result<T>(outcome: Result<T, String>) -> Result<LumioCommandResult<T>, ()> {
    Ok(match outcome {
        Ok(payload) => LumioCommandResult::ok(payload),
        Err(code) => LumioCommandResult::failed(&code),
    })
}

#[tauri::command]
pub fn lumio_bootstrap(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioBootstrapPayload>, ()> {
    let codex_app = lock(&session.codex_app)
        .clone()
        .map(|path| codex_app_payload(&path, "manual"))
        .or_else(|| {
            codex_plus_core::app_paths::resolve_codex_app_dir(None)
                .map(|path| codex_app_payload(&path, "automatic"))
        });

    // 离线启动时余额未知，先用缓存邮箱把 UI 推进到 provisioning，由 `verify-account` 拉真实数值。
    let account = lock(&session.account)
        .as_ref()
        .map(account_payload)
        .or_else(|| {
            session.auth.email().map(|email| LumioAccountPayload {
                email,
                balance: 0.0,
                plan_label: None,
            })
        });

    result(Ok(LumioBootstrapPayload {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        codex_app,
        account,
        telemetry_enabled: session.telemetry_enabled.load(Ordering::SeqCst),
        auto_update_enabled: true,
        credential_status: session.auth.credential_status(),
    }))
}

#[tauri::command]
pub async fn lumio_public_settings(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioServiceSettingsPayload>, ()> {
    let outcome = session.client.public_settings().await.map(|settings| {
        *lock(&session.model) = settings.default_model.clone();
        LumioServiceSettingsPayload {
            registration_enabled: settings.registration_enabled,
            email_verify_enabled: settings.email_verify_enabled,
            email_suffix_whitelist: settings.email_suffix_whitelist,
            password_reset_enabled: settings.password_reset_enabled,
            agreement_enabled: settings.agreement_enabled,
            agreement_revision: settings.agreement_revision,
            agreement_documents: settings
                .agreement_documents
                .into_iter()
                .map(|document| LumioAgreementDocumentPayload {
                    id: document.id,
                    title: document.title,
                    content_md: document.content_md,
                })
                .collect(),
            default_model: settings.default_model,
            site_base_url: product::SITE_BASE_URL.to_string(),
            payment_path: product::PAYMENT_PATH.to_string(),
            api_base_url: product::API_BASE_URL.trim_end_matches('/').to_string(),
        }
    });
    result(outcome)
}

#[tauri::command]
pub async fn lumio_send_verify_code(
    session: tauri::State<'_, LumioSession>,
    email: String,
) -> Result<LumioCommandResult<LumioVerifyCodePayload>, ()> {
    let outcome = session
        .client
        .send_verify_code(&email)
        .await
        .map(|countdown| LumioVerifyCodePayload { countdown });
    result(outcome)
}

#[tauri::command]
pub async fn lumio_register(
    session: tauri::State<'_, LumioSession>,
    email: String,
    password: String,
    verify_code: String,
    accepted_revision: String,
) -> Result<LumioCommandResult<LumioAuthPayload>, ()> {
    *lock(&session.accepted_agreement) = Some(accepted_revision);
    let request = RegisterRequest {
        email,
        password,
        verify_code: non_empty(verify_code),
        invitation_code: None,
    };
    let outcome = match session.client.register(&request).await {
        Ok(outcome) => session.adopt(outcome),
        Err(code) => Err(code),
    };
    result(outcome)
}

#[tauri::command]
pub async fn lumio_login(
    session: tauri::State<'_, LumioSession>,
    email: String,
    password: String,
) -> Result<LumioCommandResult<LumioAuthPayload>, ()> {
    let outcome = match session.client.login(&email, &password).await {
        Ok(outcome) => session.adopt(outcome),
        Err(code) => Err(code),
    };
    result(outcome)
}

#[tauri::command]
pub async fn lumio_login_two_factor(
    session: tauri::State<'_, LumioSession>,
    code: String,
) -> Result<LumioCommandResult<LumioAuthPayload>, ()> {
    let pending = lock(&session.pending_two_factor).clone();
    let outcome = match pending {
        // 挑战 token 只在后端进程里；前端拿不到它，也就无法伪造这一步。
        Some(temp_token) => match session.client.login_two_factor(&temp_token, &code).await {
            Ok(outcome) => session.adopt(outcome),
            Err(error) => Err(error),
        },
        None => Err(SESSION_EXPIRED.to_string()),
    };
    result(outcome)
}

#[tauri::command]
pub async fn lumio_logout(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioEmptyPayload>, ()> {
    if let Some(refresh_token) = session.auth.refresh_token() {
        // 服务端撤销失败不该把用户困在已登录状态：本地凭据照样清掉。
        let _ = session.client.logout(&refresh_token).await;
    }
    session.forget();
    let outcome = session.auth.clear().map(|()| LumioEmptyPayload {});
    result(outcome)
}

#[tauri::command]
pub async fn lumio_refresh_account(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioAccountPayload>, ()> {
    let client = &session.client;
    let outcome = session
        .auth
        .with_access_token(client, move |token| async move { client.me(&token).await })
        .await
        .map(|profile| {
            let payload = account_payload(&profile);
            *lock(&session.account) = Some(profile);
            payload
        });
    result(outcome)
}

#[tauri::command]
pub async fn lumio_provision_step(
    session: tauri::State<'_, LumioSession>,
    step: String,
) -> Result<LumioCommandResult<LumioProvisionStepPayload>, ()> {
    let outcome = run_provision_step(&session, &step)
        .await
        .map(|()| LumioProvisionStepPayload { step });
    result(outcome)
}

async fn run_provision_step(session: &LumioSession, step: &str) -> Result<(), String> {
    let client = &session.client;
    match step {
        "verify-account" => {
            let profile = session
                .auth
                .with_access_token(client, move |token| async move { client.me(&token).await })
                .await?;
            *lock(&session.account) = Some(profile);
            Ok(())
        }
        "prepare-connection" => {
            let key = session
                .auth
                .with_access_token(client, move |token| async move {
                    ensure_desktop_key(client, &token).await
                })
                .await?;
            session.auth.set_api_key(key)
        }
        "sync-models" => {
            let key = session
                .auth
                .api_key()
                .ok_or_else(|| KEY_PROVISION_FAILED.to_string())?;
            let models = client.models(&key).await?;
            let preferred = lock(&session.model).clone();
            let selected = preferred
                .filter(|model| models.iter().any(|candidate| candidate == model))
                .or_else(|| models.first().cloned())
                .ok_or_else(|| SERVICE_UNAVAILABLE.to_string())?;
            *lock(&session.model) = Some(selected);
            Ok(())
        }
        "write-config" => {
            let api_key = session
                .auth
                .api_key()
                .ok_or_else(|| KEY_PROVISION_FAILED.to_string())?;
            let model = lock(&session.model)
                .clone()
                .ok_or_else(|| SERVICE_UNAVAILABLE.to_string())?;
            let state_dir =
                product::state_dir().ok_or_else(|| KEY_STORAGE_UNAVAILABLE.to_string())?;
            config_takeover::apply_takeover(
                &codex_plus_core::codex_home::default_codex_home_dir(),
                &state_dir,
                &TakeoverRequest {
                    model,
                    api_key,
                    base_url: gateway_base_url(),
                },
            )
            .map(|_| ())
        }
        _ => Err(UNKNOWN.to_string()),
    }
}

#[tauri::command]
pub fn lumio_takeover_health(
    _session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioTakeoverHealthPayload>, ()> {
    let Some(state_dir) = product::state_dir() else {
        return result(Err(KEY_STORAGE_UNAVAILABLE.to_string()));
    };
    result(Ok(takeover_health_payload(
        config_takeover::check_takeover(
            &codex_plus_core::codex_home::default_codex_home_dir(),
            &state_dir,
        ),
    )))
}

/// `health` 的三个取值是跨语言契约：前端按它们分支。取值本身由
/// `tests/lumio_contract_literals.rs` 用实际序列化钉住。
pub fn takeover_health_payload(health: TakeoverHealth) -> LumioTakeoverHealthPayload {
    match health {
        TakeoverHealth::NotApplied => LumioTakeoverHealthPayload {
            health: HEALTH_NOT_APPLIED.to_string(),
            error_code: None,
        },
        TakeoverHealth::Healthy => LumioTakeoverHealthPayload {
            health: HEALTH_HEALTHY.to_string(),
            error_code: None,
        },
        TakeoverHealth::Conflicted { error_code } => LumioTakeoverHealthPayload {
            health: HEALTH_CONFLICTED.to_string(),
            error_code: Some(error_code),
        },
    }
}

#[tauri::command]
pub fn lumio_restore_config(
    _session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioEmptyPayload>, ()> {
    let outcome = match product::state_dir() {
        Some(state_dir) => config_takeover::restore(
            &codex_plus_core::codex_home::default_codex_home_dir(),
            &state_dir,
        )
        .map(|()| LumioEmptyPayload {}),
        None => Err(RESTORE_FAILED.to_string()),
    };
    result(outcome)
}

#[tauri::command]
pub fn lumio_launch_codex(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioEmptyPayload>, ()> {
    let app_dir = lock(&session.codex_app)
        .clone()
        .or_else(|| codex_plus_core::app_paths::resolve_codex_app_dir(None));
    let outcome = match app_dir {
        Some(path) => launch::launch_official_codex(&path).map(|()| LumioEmptyPayload {}),
        None => Err(APP_NOT_FOUND.to_string()),
    };
    result(outcome)
}

#[tauri::command]
pub fn lumio_detect_codex_app(
    _session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<Option<LumioCodexAppPayload>>, ()> {
    result(Ok(codex_plus_core::app_paths::resolve_codex_app_dir(None)
        .map(|path| codex_app_payload(&path, "automatic"))))
}

#[tauri::command]
pub fn lumio_select_codex_app(
    session: tauri::State<'_, LumioSession>,
    path: String,
) -> Result<LumioCommandResult<LumioCodexAppPayload>, ()> {
    let outcome = launch::validate_selected_app(std::path::Path::new(&path)).map(|resolved| {
        let payload = codex_app_payload(&resolved, "manual");
        *lock(&session.codex_app) = Some(resolved);
        payload
    });
    result(outcome)
}

#[tauri::command]
pub fn lumio_open_browser(
    _session: tauri::State<'_, LumioSession>,
    url: String,
) -> Result<LumioCommandResult<LumioEmptyPayload>, ()> {
    result(launch::open_in_browser(&url).map(|()| LumioEmptyPayload {}))
}

#[tauri::command]
pub async fn lumio_check_update(
    _session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioUpdateReminderPayload>, ()> {
    let outcome =
        codex_plus_core::lumio::update_check::check_update_reminder(env!("CARGO_PKG_VERSION"))
            .await
            .map(|reminder| LumioUpdateReminderPayload {
                current_version: reminder.current_version,
                latest_version: reminder.latest_version,
                update_available: reminder.update_available,
                download_url: reminder.download_url,
                release_summary: reminder.release_summary,
            });
    result(outcome)
}

#[tauri::command]
pub fn lumio_set_telemetry(
    session: tauri::State<'_, LumioSession>,
    enabled: bool,
) -> Result<LumioCommandResult<LumioTelemetryPayload>, ()> {
    session.telemetry_enabled.store(enabled, Ordering::SeqCst);
    result(Ok(LumioTelemetryPayload { enabled }))
}

#[tauri::command]
pub fn lumio_export_logs(
    session: tauri::State<'_, LumioSession>,
) -> Result<LumioCommandResult<LumioExportLogsPayload>, ()> {
    result(export_logs(&session).map(|path| LumioExportLogsPayload { path }))
}

fn export_logs(session: &LumioSession) -> Result<String, String> {
    let dir = product::log_dir().ok_or_else(|| KEY_STORAGE_UNAVAILABLE.to_string())?;
    let takeover = product::state_dir()
        .map(|state_dir| {
            match config_takeover::check_takeover(
                &codex_plus_core::codex_home::default_codex_home_dir(),
                &state_dir,
            ) {
                TakeoverHealth::NotApplied => HEALTH_NOT_APPLIED,
                TakeoverHealth::Healthy => HEALTH_HEALTHY,
                TakeoverHealth::Conflicted { .. } => HEALTH_CONFLICTED,
            }
        })
        .unwrap_or(HEALTH_NOT_APPLIED);

    // 只写状态与稳定码。诊断文件里没有任何服务端响应体，redact 是最后一道保险而非唯一防线。
    let report = redact(&format!(
        "product={}\nversion={}\nplatform={}\narch={}\ncredential_status={}\ntakeover_health={}\ntelemetry_enabled={}\n",
        product::PRODUCT_NAME,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        credential_status_label(session.auth.credential_status()),
        takeover,
        session.telemetry_enabled.load(Ordering::SeqCst),
    ));

    let path = dir.join("lumio-diagnostics.log");
    codex_plus_core::settings::atomic_write(&path, report.as_bytes())
        .map_err(|_| KEY_STORAGE_UNAVAILABLE.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn credential_status_label(status: CredentialStatus) -> &'static str {
    match status {
        CredentialStatus::Present => "present",
        CredentialStatus::Missing => "missing",
        CredentialStatus::Invalid => "invalid",
    }
}

fn gateway_base_url() -> String {
    format!("{}v1", product::API_BASE_URL)
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}
