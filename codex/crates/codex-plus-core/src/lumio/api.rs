//! Sub2API HTTP 客户端。所有失败路径只向上抛 Lumio 稳定错误码，
//! 服务端原文（reason 之外的 message、响应体、reqwest 的 Display）永不越过这一层。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::errors::{network_error_code, normalize_reason};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const KEY_PAGE_SIZE: &str = "100";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublicSettings {
    pub registration_enabled: bool,
    pub email_verify_enabled: bool,
    pub email_suffix_whitelist: Vec<String>,
    pub password_reset_enabled: bool,
    pub agreement_enabled: bool,
    pub agreement_revision: String,
    pub agreement_documents: Vec<AgreementDocument>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AgreementDocument {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "content_md")]
    pub content_md: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invitation_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountProfile {
    pub id: i64,
    pub email: String,
    pub balance: f64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthOutcome {
    Tokens {
        tokens: TokenPair,
        profile: AccountProfile,
    },
    TwoFactorRequired {
        temp_token: String,
        masked_email: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GroupSummary {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiKeyRecord {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub group_id: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i64>,
}

/// 服务端的统一响应信封。限流中间件不走这个信封，所以 `code` 与 `data` 都要能缺席。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    reason: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct RawPublicSettings {
    #[serde(default)]
    registration_enabled: bool,
    #[serde(default)]
    email_verify_enabled: bool,
    #[serde(default)]
    registration_email_suffix_whitelist: Vec<String>,
    #[serde(default)]
    password_reset_enabled: bool,
    #[serde(default)]
    login_agreement_enabled: bool,
    #[serde(default)]
    login_agreement_revision: String,
    #[serde(default)]
    login_agreement_documents: Vec<AgreementDocument>,
    #[serde(default)]
    ccswitch_default_model_openai: String,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    email: String,
    #[serde(default)]
    balance: f64,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct RawAuthResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    user: Option<RawUser>,
}

#[derive(Debug, Deserialize)]
struct RawTwoFactorChallenge {
    #[serde(default)]
    requires_2fa: bool,
    #[serde(default)]
    temp_token: String,
    #[serde(default)]
    user_email_masked: String,
}

#[derive(Debug, Deserialize)]
struct RawSendVerifyCode {
    #[serde(default)]
    countdown: u32,
}

#[derive(Debug, Deserialize)]
struct RawPage<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct RawModelList {
    #[serde(default = "Vec::new")]
    data: Vec<RawModel>,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    #[serde(default)]
    id: String,
}

pub struct LumioApiClient {
    http: reqwest::Client,
    base_url: String,
}

impl LumioApiClient {
    pub fn new(base_url: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("LumioCodex/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub async fn public_settings(&self) -> Result<PublicSettings, String> {
        let response = self
            .send(self.http.get(self.url("/api/v1/settings/public")))
            .await?;
        let raw: RawPublicSettings = read_envelope(response).await?;
        Ok(PublicSettings {
            registration_enabled: raw.registration_enabled,
            email_verify_enabled: raw.email_verify_enabled,
            email_suffix_whitelist: raw.registration_email_suffix_whitelist,
            password_reset_enabled: raw.password_reset_enabled,
            agreement_enabled: raw.login_agreement_enabled,
            agreement_revision: raw.login_agreement_revision,
            agreement_documents: raw.login_agreement_documents,
            default_model: non_empty(raw.ccswitch_default_model_openai),
        })
    }

    pub async fn send_verify_code(&self, email: &str) -> Result<u32, String> {
        let request = self
            .http
            .post(self.url("/api/v1/auth/send-verify-code"))
            .json(&serde_json::json!({ "email": email }));
        let response = self.send(request).await?;
        let raw: RawSendVerifyCode = read_envelope(response).await?;
        Ok(raw.countdown)
    }

    pub async fn register(&self, req: &RegisterRequest) -> Result<AuthOutcome, String> {
        let request = self.http.post(self.url("/api/v1/auth/register")).json(req);
        let response = self.send(request).await?;
        read_auth_outcome(response).await
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<AuthOutcome, String> {
        let request = self
            .http
            .post(self.url("/api/v1/auth/login"))
            .json(&serde_json::json!({ "email": email, "password": password }));
        let response = self.send(request).await?;
        read_auth_outcome(response).await
    }

    pub async fn login_two_factor(
        &self,
        temp_token: &str,
        code: &str,
    ) -> Result<AuthOutcome, String> {
        let request = self
            .http
            .post(self.url("/api/v1/auth/login/2fa"))
            .json(&serde_json::json!({ "temp_token": temp_token, "totp_code": code }));
        let response = self.send(request).await?;
        read_auth_outcome(response).await
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, String> {
        let request = self
            .http
            .post(self.url("/api/v1/auth/refresh"))
            .json(&serde_json::json!({ "refresh_token": refresh_token }));
        let response = self.send(request).await?;
        let raw: RawAuthResponse = read_envelope(response).await?;
        Ok(token_pair(&raw))
    }

    pub async fn logout(&self, refresh_token: &str) -> Result<(), String> {
        let request = self
            .http
            .post(self.url("/api/v1/auth/logout"))
            .json(&serde_json::json!({ "refresh_token": refresh_token }));
        let response = self.send(request).await?;
        let _: serde_json::Value = read_envelope(response).await?;
        Ok(())
    }

    pub async fn me(&self, access_token: &str) -> Result<AccountProfile, String> {
        let request = self
            .http
            .get(self.url("/api/v1/auth/me"))
            .bearer_auth(access_token);
        let response = self.send(request).await?;
        let raw: RawUser = read_envelope(response).await?;
        Ok(profile(raw))
    }

    pub async fn available_groups(&self, access_token: &str) -> Result<Vec<GroupSummary>, String> {
        let request = self
            .http
            .get(self.url("/api/v1/groups/available"))
            .bearer_auth(access_token);
        let response = self.send(request).await?;
        read_envelope(response).await
    }

    pub async fn list_keys(
        &self,
        access_token: &str,
        name: &str,
    ) -> Result<Vec<ApiKeyRecord>, String> {
        let request = self
            .http
            .get(self.url("/api/v1/keys"))
            .query(&[("search", name), ("page_size", KEY_PAGE_SIZE)])
            .bearer_auth(access_token);
        let response = self.send(request).await?;
        let page: RawPage<ApiKeyRecord> = read_envelope(response).await?;
        Ok(page.items)
    }

    pub async fn create_key(
        &self,
        access_token: &str,
        req: &CreateKeyRequest,
    ) -> Result<ApiKeyRecord, String> {
        let request = self
            .http
            .post(self.url("/api/v1/keys"))
            .bearer_auth(access_token)
            .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
            .json(req);
        let response = self.send(request).await?;
        read_envelope(response).await
    }

    /// `/v1/models` 走网关而非管理 API：没有 `/api` 前缀、用 API Key 鉴权、且不套信封。
    pub async fn models(&self, api_key: &str) -> Result<Vec<String>, String> {
        let request = self.http.get(self.url("/v1/models")).bearer_auth(api_key);
        let response = self.send(request).await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(normalize_reason(status.as_u16(), None));
        }
        let list: RawModelList =
            serde_json::from_str(&body).map_err(|_| network_error_code().to_string())?;
        Ok(list
            .data
            .into_iter()
            .map(|model| model.id)
            .filter(|id| !id.trim().is_empty())
            .collect())
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        // reqwest 的错误 Display 会带上完整 URL 与查询串，一律折叠成稳定码。
        request
            .send()
            .await
            .map_err(|_| network_error_code().to_string())
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn token_pair(raw: &RawAuthResponse) -> TokenPair {
    TokenPair {
        access_token: raw.access_token.clone(),
        refresh_token: raw.refresh_token.clone(),
        expires_in: raw.expires_in,
    }
}

fn profile(raw: RawUser) -> AccountProfile {
    AccountProfile {
        id: raw.id,
        email: raw.email,
        balance: raw.balance,
        status: raw.status,
    }
}

/// 2FA 挑战是 HTTP 200 的成功响应，只能从 `requires_2fa` 判断，不能靠状态码。
async fn read_auth_outcome(response: reqwest::Response) -> Result<AuthOutcome, String> {
    let data: serde_json::Value = read_envelope(response).await?;
    let challenge: RawTwoFactorChallenge =
        serde_json::from_value(data.clone()).map_err(|_| network_error_code().to_string())?;
    if challenge.requires_2fa {
        return Ok(AuthOutcome::TwoFactorRequired {
            temp_token: challenge.temp_token,
            masked_email: challenge.user_email_masked,
        });
    }
    let raw: RawAuthResponse =
        serde_json::from_value(data).map_err(|_| network_error_code().to_string())?;
    let tokens = token_pair(&raw);
    let profile = raw.user.map(profile).unwrap_or(AccountProfile {
        id: 0,
        email: String::new(),
        balance: 0.0,
        status: String::new(),
    });
    Ok(AuthOutcome::Tokens { tokens, profile })
}

async fn read_envelope<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let reason = serde_json::from_str::<Envelope<serde_json::Value>>(&body)
            .ok()
            .and_then(|envelope| envelope.reason);
        return Err(normalize_reason(status.as_u16(), reason.as_deref()));
    }
    let envelope: Envelope<T> =
        serde_json::from_str(&body).map_err(|_| network_error_code().to_string())?;
    if envelope.code != 0 {
        return Err(normalize_reason(
            status.as_u16(),
            envelope.reason.as_deref(),
        ));
    }
    envelope
        .data
        .ok_or_else(|| network_error_code().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "code": 0, "message": "success", "data": data })
    }

    fn failure(code: u16, reason: &str, message: &str) -> serde_json::Value {
        serde_json::json!({ "code": code, "message": message, "reason": reason })
    }

    #[tokio::test]
    async fn public_settings_reads_the_registration_rules() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/settings/public"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "registration_enabled": true,
                    "email_verify_enabled": true,
                    "registration_email_suffix_whitelist": ["@example.com"],
                    "password_reset_enabled": true,
                    "login_agreement_enabled": true,
                    "login_agreement_revision": "abc123",
                    "login_agreement_documents": [
                        { "id": "terms", "title": "服务条款", "content_md": "# 条款" }
                    ],
                    "ccswitch_default_model_openai": "gpt-example",
                    "site_name": "Lumio"
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let settings = client.public_settings().await.unwrap();

        assert!(settings.registration_enabled);
        assert!(settings.email_verify_enabled);
        assert_eq!(
            settings.email_suffix_whitelist,
            vec!["@example.com".to_string()]
        );
        assert_eq!(settings.agreement_revision, "abc123");
        assert_eq!(settings.agreement_documents.len(), 1);
        assert_eq!(settings.agreement_documents[0].id, "terms");
        assert_eq!(settings.default_model.as_deref(), Some("gpt-example"));
    }

    #[tokio::test]
    async fn missing_optional_settings_fields_fall_back_safely() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/settings/public"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "registration_enabled": false
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let settings = client.public_settings().await.unwrap();

        assert!(!settings.registration_enabled);
        assert!(settings.email_suffix_whitelist.is_empty());
        assert!(settings.agreement_documents.is_empty());
        assert_eq!(settings.default_model, None);
    }

    #[tokio::test]
    async fn login_returns_tokens_and_profile() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .and(body_json(serde_json::json!({
                "email": "user@example.com",
                "password": "supersecret"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(
                serde_json::json!({
                    "access_token": "header.payload.signature",
                    "refresh_token": "rt_abc",
                    "expires_in": 3600,
                    "token_type": "Bearer",
                    "user": { "id": 7, "email": "user@example.com", "balance": 12.5, "status": "active" }
                }),
            )))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client
            .login("user@example.com", "supersecret")
            .await
            .unwrap();

        match outcome {
            AuthOutcome::Tokens { tokens, profile } => {
                assert_eq!(tokens.access_token, "header.payload.signature");
                assert_eq!(tokens.refresh_token, "rt_abc");
                assert_eq!(profile.email, "user@example.com");
                assert_eq!(profile.balance, 12.5);
            }
            other => panic!("expected tokens, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn login_surfaces_the_two_factor_challenge_as_a_success_variant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "requires_2fa": true,
                    "temp_token": "tmp_123",
                    "user_email_masked": "u***@example.com"
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client
            .login("user@example.com", "supersecret")
            .await
            .unwrap();

        match outcome {
            AuthOutcome::TwoFactorRequired {
                temp_token,
                masked_email,
            } => {
                assert_eq!(temp_token, "tmp_123");
                assert_eq!(masked_email, "u***@example.com");
            }
            other => panic!("expected a two-factor challenge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bad_credentials_become_a_normalized_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(ResponseTemplate::new(401).set_body_json(failure(
                401,
                "INVALID_CREDENTIALS",
                "invalid email or password",
            )))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let error = client.login("user@example.com", "nope").await.unwrap_err();

        assert_eq!(error, "AUTH_INVALID_CREDENTIALS");
    }

    #[tokio::test]
    async fn the_rate_limiter_response_shape_is_handled_even_without_an_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/send-verify-code"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": "rate limit exceeded",
                "message": "Too many requests, please try again later"
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let error = client
            .send_verify_code("user@example.com")
            .await
            .unwrap_err();

        assert_eq!(error, "SERVICE_RATE_LIMITED");
    }

    #[tokio::test]
    async fn send_verify_code_returns_the_server_countdown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/send-verify-code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "message": "Verification code sent successfully",
                    "countdown": 60
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            client.send_verify_code("user@example.com").await.unwrap(),
            60
        );
    }

    #[tokio::test]
    async fn two_factor_login_sends_the_temp_token_and_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login/2fa"))
            .and(body_json(serde_json::json!({
                "temp_token": "tmp_123",
                "totp_code": "654321"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(
                serde_json::json!({
                    "access_token": "header.payload.signature",
                    "refresh_token": "rt_abc",
                    "token_type": "Bearer",
                    "user": { "id": 7, "email": "user@example.com", "balance": 0.0, "status": "active" }
                }),
            )))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client.login_two_factor("tmp_123", "654321").await.unwrap();

        assert!(matches!(outcome, AuthOutcome::Tokens { .. }));
    }

    #[tokio::test]
    async fn authenticated_requests_carry_the_bearer_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .and(header("authorization", "Bearer access-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "id": 7,
                    "email": "user@example.com",
                    "balance": 3.25,
                    "status": "active"
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let profile = client.me("access-token").await.unwrap();

        assert_eq!(profile.balance, 3.25);
    }

    #[tokio::test]
    async fn key_listing_filters_by_the_reserved_desktop_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .and(query_param("search", "Lumio Codex Desktop"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                    "items": [
                        {
                            "id": 1,
                            "name": "Lumio Codex Desktop",
                            "key": "sk-existing",
                            "status": "active",
                            "group_id": 3,
                            "created_at": "2026-01-01T00:00:00Z"
                        }
                    ],
                    "total": 1,
                    "page": 1,
                    "page_size": 20,
                    "pages": 1
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let keys = client
            .list_keys("access-token", "Lumio Codex Desktop")
            .await
            .unwrap();

        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "Lumio Codex Desktop");
        assert_eq!(keys[0].status, "active");
    }

    #[tokio::test]
    async fn key_creation_always_sends_an_idempotency_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .and(header_exists("idempotency-key"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(envelope(serde_json::json!({
                    "id": 2,
                    "name": "Lumio Codex Desktop",
                    "key": "sk-created",
                    "status": "active",
                    "group_id": 3,
                    "created_at": "2026-02-01T00:00:00Z"
                }))),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let request = CreateKeyRequest {
            name: "Lumio Codex Desktop".to_string(),
            group_id: Some(3),
        };
        let created = client.create_key("access-token", &request).await.unwrap();

        assert_eq!(created.key, "sk-created");
    }

    #[tokio::test]
    async fn a_dead_server_reports_service_unavailable_rather_than_a_transport_error() {
        let client = LumioApiClient::new("http://127.0.0.1:1").unwrap();
        assert_eq!(
            client.public_settings().await.unwrap_err(),
            "SERVICE_UNAVAILABLE"
        );
    }

    #[tokio::test]
    async fn malformed_success_bodies_do_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            client.me("access-token").await.unwrap_err(),
            "SERVICE_UNAVAILABLE"
        );
    }

    #[tokio::test]
    async fn the_model_catalog_uses_api_key_auth_and_returns_model_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-desktop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [{ "id": "gpt-example" }, { "id": "gpt-example-mini" }]
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let models = client.models("sk-desktop").await.unwrap();

        assert_eq!(
            models,
            vec!["gpt-example".to_string(), "gpt-example-mini".to_string()]
        );
    }
}
