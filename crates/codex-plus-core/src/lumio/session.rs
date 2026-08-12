//! 会话令牌状态与访问令牌自动续期。access token 的寿命由服务端
//! `jwt.access_token_expire_minutes` 决定（通常一两小时），而 refresh token 默认 30 天：
//! 过期的 access token 应该在后端就地续期，而不是把用户踢回登录页。
//!
//! 令牌明文只在这里与 [`CredentialStore`] 之间流转，不进 payload、不进日志。

use std::sync::Mutex;

use super::api::LumioApiClient;
use super::credentials::{CredentialStatus, CredentialStore, StoredCredentials};

const SESSION_EXPIRED: &str = "AUTH_SESSION_EXPIRED";

/// 进程内的登录态。[`AuthSession::with_access_token`] 是唯一应该用来发起
/// 需要鉴权的调用的入口：它在会话过期时自动续期并重试一次。
pub struct AuthSession {
    store: CredentialStore,
    state: Mutex<Option<StoredCredentials>>,
    /// 续期是轮转式的（旧 refresh token 立即失效），同一时刻只能有一个刷新在飞。
    renewal: tokio::sync::Mutex<()>,
}

impl AuthSession {
    pub fn new(store: CredentialStore) -> Self {
        let restored = store.load();
        Self {
            store,
            state: Mutex::new(restored),
            renewal: tokio::sync::Mutex::new(()),
        }
    }

    pub fn credential_status(&self) -> CredentialStatus {
        self.store.status()
    }

    pub fn access_token(&self) -> Result<String, String> {
        self.with_state(|state| state.access_token.clone())
            .ok_or_else(|| SESSION_EXPIRED.to_string())
    }

    pub fn refresh_token(&self) -> Option<String> {
        self.with_state(|state| state.refresh_token.clone())
    }

    pub fn email(&self) -> Option<String> {
        self.with_state(|state| state.email.clone())
    }

    pub fn api_key(&self) -> Option<String> {
        self.with_state(|state| state.api_key.clone()).flatten()
    }

    /// 登录 / 注册 / 2FA 成功后落地令牌对。已有的桌面 Key 保留：换令牌不该把它冲掉。
    ///
    /// 落盘失败时**不会**把新令牌留在内存里：Sub2API 的 refresh 是轮转式的，服务端一旦
    /// 接受新 refresh，旧的立刻作废——若进程里假装续期成功、磁盘上却仍是旧 token，重启后
    /// 每次续期都会注定失败。失败时清空两边，逼用户重新登录。
    pub fn adopt_tokens(
        &self,
        access_token: String,
        refresh_token: String,
        email: String,
    ) -> Result<(), String> {
        let credentials = StoredCredentials {
            access_token,
            refresh_token,
            api_key: self.api_key(),
            email,
        };
        *lock(&self.state) = Some(credentials);
        if let Err(error) = self.persist() {
            *lock(&self.state) = None;
            let _ = self.store.clear();
            return Err(error);
        }
        Ok(())
    }

    pub fn set_api_key(&self, api_key: String) -> Result<(), String> {
        {
            let mut state = lock(&self.state);
            let Some(state) = state.as_mut() else {
                return Err(SESSION_EXPIRED.to_string());
            };
            state.api_key = Some(api_key);
        }
        self.persist()
    }

    /// 退出登录：先忘掉进程内令牌，再删掉磁盘上的凭据。
    pub fn clear(&self) -> Result<(), String> {
        *lock(&self.state) = None;
        self.store.clear()
    }

    /// 用当前 access token 执行 `call`。若结果是 `AUTH_SESSION_EXPIRED`，先用 refresh token
    /// 换一对新令牌，然后**重试一次**；续期本身失败才把 `AUTH_SESSION_EXPIRED` 交给上层。
    pub async fn with_access_token<T, F, Fut>(
        &self,
        client: &LumioApiClient,
        call: F,
    ) -> Result<T, String>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        let access_token = self.access_token()?;
        let code = match call(access_token.clone()).await {
            Ok(value) => return Ok(value),
            Err(code) => code,
        };
        if code != SESSION_EXPIRED {
            return Err(code);
        }
        let renewed = self.renew(client, &access_token).await?;
        // 只重试一次：这一次的结果无论成败都直接上抛，绝不再续期。
        call(renewed).await
    }

    async fn renew(
        &self,
        client: &LumioApiClient,
        stale_access_token: &str,
    ) -> Result<String, String> {
        let _guard = self.renewal.lock().await;
        // 门内再读一次：另一个调用可能刚刚续期完成。轮转后旧 refresh token 已失效，
        // 再刷一次只会把双方都踢下线，所以直接复用它换来的 access token。
        let current = self.access_token()?;
        if current != stale_access_token {
            return Ok(current);
        }

        let refresh_token = self
            .refresh_token()
            .ok_or_else(|| SESSION_EXPIRED.to_string())?;
        // 续期失败的原因（refresh 也过期 / 服务不可用）对 UI 是同一件事：会话到此为止。
        let renewed = client
            .refresh(&refresh_token)
            .await
            .map_err(|_| SESSION_EXPIRED.to_string())?;
        let email = self.email().unwrap_or_default();
        // 轮转式刷新：新的 access 与新的 refresh 必须一起落盘，
        // 否则下一次续期会拿一个已经失效的 refresh token 去换。
        self.adopt_tokens(renewed.access_token.clone(), renewed.refresh_token, email)?;
        Ok(renewed.access_token)
    }

    fn with_state<T>(&self, read: impl FnOnce(&StoredCredentials) -> T) -> Option<T> {
        lock(&self.state).as_ref().map(read)
    }

    fn persist(&self) -> Result<(), String> {
        let credentials = lock(&self.state).clone();
        match credentials {
            Some(credentials) => self.store.save(&credentials),
            None => Ok(()),
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const STALE: &str = "stale-access";
    const FRESH: &str = "fresh-access";

    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "code": 0, "message": "success", "data": data })
    }

    fn failure(code: u16, reason: &str) -> serde_json::Value {
        serde_json::json!({ "code": code, "message": "failed", "reason": reason })
    }

    fn signed_in(root: &std::path::Path) -> AuthSession {
        let store = CredentialStore::new(root);
        store
            .save(&StoredCredentials {
                access_token: STALE.to_string(),
                refresh_token: "rt_old".to_string(),
                api_key: Some("sk-desktop".to_string()),
                email: "user@example.com".to_string(),
            })
            .unwrap();
        AuthSession::new(CredentialStore::new(root))
    }

    async fn mock_me(server: &MockServer, token: &str, response: ResponseTemplate, calls: u64) {
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(response)
            .expect(calls)
            .mount(server)
            .await;
    }

    async fn mock_refresh(server: &MockServer, response: ResponseTemplate, calls: u64) {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .and(body_json(serde_json::json!({ "refresh_token": "rt_old" })))
            .respond_with(response)
            .expect(calls)
            .mount(server)
            .await;
    }

    fn profile_body() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
            "id": 7,
            "email": "user@example.com",
            "balance": 4.5,
            "status": "active"
        })))
    }

    fn renewed_body() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
            "access_token": FRESH,
            "refresh_token": "rt_new",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
    }

    fn expired_body() -> ResponseTemplate {
        ResponseTemplate::new(401).set_body_json(failure(401, "TOKEN_EXPIRED"))
    }

    async fn calls_to(server: &MockServer, suffix: &str) -> usize {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path().ends_with(suffix))
            .count()
    }

    #[tokio::test]
    async fn an_expired_access_token_is_renewed_and_the_call_retried_once() {
        let server = MockServer::start().await;
        mock_me(&server, STALE, expired_body(), 1).await;
        mock_me(&server, FRESH, profile_body(), 1).await;
        mock_refresh(&server, renewed_body(), 1).await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;

        let profile = session
            .with_access_token(api, move |token| async move { api.me(&token).await })
            .await
            .unwrap();

        assert_eq!(profile.balance, 4.5);
        assert_eq!(session.access_token().unwrap(), FRESH);
        // 轮转式刷新：落盘的 refresh token 必须是新签发的那一个。
        let persisted = CredentialStore::new(dir.path()).load().unwrap();
        assert_eq!(persisted.refresh_token, "rt_new");
        assert_eq!(persisted.access_token, FRESH);
        assert_eq!(persisted.api_key.as_deref(), Some("sk-desktop"));
    }

    #[tokio::test]
    async fn a_failed_renewal_reports_the_session_as_expired_without_retrying() {
        let server = MockServer::start().await;
        mock_me(&server, STALE, expired_body(), 1).await;
        mock_refresh(
            &server,
            ResponseTemplate::new(401).set_body_json(failure(401, "REFRESH_TOKEN_INVALID")),
            1,
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;

        let error = session
            .with_access_token(api, move |token| async move { api.me(&token).await })
            .await
            .unwrap_err();

        assert_eq!(error, SESSION_EXPIRED);
        assert_eq!(calls_to(&server, "/auth/me").await, 1);
        assert_eq!(
            CredentialStore::new(dir.path())
                .load()
                .unwrap()
                .refresh_token,
            "rt_old"
        );
    }

    #[tokio::test]
    async fn a_non_session_failure_never_triggers_a_renewal() {
        let server = MockServer::start().await;
        mock_me(
            &server,
            STALE,
            ResponseTemplate::new(403).set_body_json(failure(403, "USER_NOT_ACTIVE")),
            1,
        )
        .await;
        mock_refresh(&server, renewed_body(), 0).await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;

        let error = session
            .with_access_token(api, move |token| async move { api.me(&token).await })
            .await
            .unwrap_err();

        assert_eq!(error, "AUTH_ACCOUNT_DISABLED");
        assert_eq!(calls_to(&server, "/auth/refresh").await, 0);
    }

    #[tokio::test]
    async fn a_still_expired_retry_gives_up_instead_of_looping() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .respond_with(expired_body())
            .expect(2)
            .mount(&server)
            .await;
        mock_refresh(&server, renewed_body(), 1).await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;

        let error = session
            .with_access_token(api, move |token| async move { api.me(&token).await })
            .await
            .unwrap_err();

        assert_eq!(error, SESSION_EXPIRED);
        assert_eq!(calls_to(&server, "/auth/me").await, 2);
        assert_eq!(calls_to(&server, "/auth/refresh").await, 1);
    }

    #[tokio::test]
    async fn a_persist_failure_after_token_rotation_does_not_pretend_renewal_succeeded() {
        let server = MockServer::start().await;
        mock_me(&server, STALE, expired_body(), 1).await;
        mock_refresh(&server, renewed_body(), 1).await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        // 把凭据路径换成目录，迫使后续 save 失败——模拟磁盘满 / 权限被夺。
        let path = CredentialStore::new(dir.path()).path();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;
        let error = session
            .with_access_token(api, move |token| async move { api.me(&token).await })
            .await
            .unwrap_err();

        assert_eq!(error, "KEY_STORAGE_UNAVAILABLE");
        // 不能用轮转后的新 access token 去重试：那会假装续期成功。
        assert_eq!(calls_to(&server, "/auth/me").await, 1);
        assert!(session.access_token().is_err());
        assert_eq!(session.credential_status(), CredentialStatus::Invalid);
    }

    #[tokio::test]
    async fn two_calls_expiring_together_share_a_single_renewal() {
        let server = MockServer::start().await;
        mock_me(&server, STALE, expired_body(), 2).await;
        mock_me(&server, FRESH, profile_body(), 2).await;
        // 慢响应让第二个调用一定会在门外等：轮转后它手里的 refresh token 已经失效，
        // 只能复用第一个调用换来的 access token。
        mock_refresh(
            &server,
            renewed_body().set_delay(std::time::Duration::from_millis(80)),
            1,
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let session = signed_in(dir.path());
        let client = LumioApiClient::new(&server.uri()).unwrap();
        let api = &client;

        let (first, second) = tokio::join!(
            session.with_access_token(api, move |token| async move { api.me(&token).await }),
            session.with_access_token(api, move |token| async move { api.me(&token).await }),
        );

        assert!(first.is_ok(), "{first:?}");
        assert!(second.is_ok(), "{second:?}");
        assert_eq!(calls_to(&server, "/auth/refresh").await, 1);
    }
}
