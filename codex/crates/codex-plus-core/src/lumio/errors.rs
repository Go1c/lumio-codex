/// Lumio 面向 UI 的稳定错误码。服务端 reason 与网络故障都先归一化到这里，
/// 原始服务端字符串永不越过这一层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LumioError {
    pub code: String,
    pub stage: &'static str,
}

impl LumioError {
    pub fn new(code: impl Into<String>, stage: &'static str) -> Self {
        Self {
            code: code.into(),
            stage,
        }
    }
}

pub fn network_error_code() -> &'static str {
    "SERVICE_UNAVAILABLE"
}

pub fn normalize_reason(http_status: u16, reason: Option<&str>) -> String {
    let mapped = match reason.map(str::trim).filter(|value| !value.is_empty()) {
        Some("INVALID_CREDENTIALS" | "INVALID_USER") => "AUTH_INVALID_CREDENTIALS",
        Some("USER_NOT_ACTIVE") => "AUTH_ACCOUNT_DISABLED",
        // 余额不足是用户可操作的账户态：提示充值即可解决，绝不能伪装成服务故障让用户空等重试。
        Some("INSUFFICIENT_BALANCE") => "ACCOUNT_INSUFFICIENT_BALANCE",
        Some("INVALID_VERIFY_CODE" | "VERIFY_CODE_MAX_ATTEMPTS") => "AUTH_CODE_INVALID",
        Some("VERIFY_CODE_TOO_FREQUENT") => "AUTH_CODE_RATE_LIMITED",
        Some("EMAIL_VERIFY_REQUIRED" | "VERIFY_CODE_REQUIRED") => "AUTH_CODE_REQUIRED",
        Some("REGISTRATION_DISABLED") => "AUTH_REGISTRATION_CLOSED",
        Some("EMAIL_SUFFIX_NOT_ALLOWED" | "EMAIL_RESERVED") => "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
        Some("EMAIL_EXISTS") => "AUTH_EMAIL_ALREADY_REGISTERED",
        Some("INVALID_EMAIL") => "AUTH_EMAIL_INVALID",
        // 站点要求邀请码但用户没填 —— 可操作，绝不能伪装成服务故障让用户空转重试。
        Some("INVITATION_CODE_REQUIRED") => "AUTH_INVITATION_CODE_REQUIRED",
        // 填了但无效 / 已被使用；`DISABLED` 是邀请码功能整体关闭，用户填的码同样不会被接受，
        // 归到「无效」而非「需要填」，否则会催用户去填一个根本用不上的码。
        Some("INVITATION_CODE_INVALID" | "INVITATION_CODE_DISABLED") => {
            "AUTH_INVITATION_CODE_INVALID"
        }
        Some("TOTP_INVALID_CODE" | "TOTP_TOO_MANY_ATTEMPTS") => "AUTH_2FA_INVALID",
        Some(
            "TOTP_NOT_SETUP"
            | "TOTP_NOT_ENABLED"
            | "TOTP_SETUP_EXPIRED"
            | "TOTP_VERIFY_ERROR"
            | "EMAIL_VERIFY_NOT_ENABLED",
        ) => "AUTH_2FA_UNAVAILABLE",
        Some(
            "INVALID_TOKEN"
            | "TOKEN_EXPIRED"
            | "ACCESS_TOKEN_EXPIRED"
            | "TOKEN_REVOKED"
            | "TOKEN_TOO_LARGE"
            | "REFRESH_TOKEN_INVALID"
            | "REFRESH_TOKEN_EXPIRED"
            | "REFRESH_TOKEN_REUSED"
            | "SESSION_BINDING_MISMATCH",
        ) => "AUTH_SESSION_EXPIRED",
        Some(
            "GROUP_NOT_ALLOWED"
            | "API_KEY_EXISTS"
            | "API_KEY_RATE_LIMITED"
            | "API_KEY_TOO_SHORT"
            | "API_KEY_INVALID_CHARS"
            | "INVALID_IP_PATTERN"
            | "IDEMPOTENCY_KEY_REQUIRED"
            | "IDEMPOTENCY_KEY_CONFLICT"
            | "IDEMPOTENCY_IN_PROGRESS"
            | "IDEMPOTENCY_STORE_UNAVAILABLE",
        ) => "KEY_PROVISION_FAILED",
        _ => "",
    };

    if !mapped.is_empty() {
        return mapped.to_string();
    }

    if http_status == 429 {
        return "SERVICE_RATE_LIMITED".to_string();
    }
    if http_status == 401 {
        return "AUTH_SESSION_EXPIRED".to_string();
    }
    network_error_code().to_string()
}

/// 在任何字符串越过 IPC 边界或进入日志前调用。覆盖 Bearer 令牌、`sk-` 类
/// Key、`rt_` 刷新令牌、JWT 与邮箱。
pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for token in input.split_inclusive(char::is_whitespace) {
        let (word, trailing) = split_trailing_whitespace(token);
        if is_secret_like(word) {
            out.push_str("[redacted]");
        } else {
            out.push_str(word);
        }
        out.push_str(trailing);
    }
    out
}

fn split_trailing_whitespace(token: &str) -> (&str, &str) {
    let end = token.trim_end().len();
    token.split_at(end)
}

/// 一个词里可能藏着多个密钥（`{"access_token":"…","refresh_token":"…"}` 整体不含空白，
/// 是一个词）。因此按分隔符切成候选片段**逐段**检查，任一段命中就整词脱敏。
fn is_secret_like(word: &str) -> bool {
    word.split(['=', ':', ',']).any(candidate_is_secret_like)
}

fn candidate_is_secret_like(raw: &str) -> bool {
    // JSON 的引号、括号、结尾的分号等会挡在前缀锚点前面，先剥掉首尾的非字母数字字符；
    // `-` `_` `.` 是密钥自身的组成部分，必须保留。
    let candidate =
        raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.');
    if candidate.is_empty() {
        return false;
    }
    if candidate.contains('@') && candidate.contains('.') {
        return true;
    }
    if candidate.starts_with("sk-") || candidate.starts_with("rt_") {
        return true;
    }
    // JWT: 三段 base64url，用 `.` 分隔。header 段恒以 `eyJ`（`{"` 的 base64url 编码）开头，
    // 以此为锚点，才不会把 `production.kubernetes.local` 这类点分标识符当成令牌。
    // 签名段长度不做要求——截断后的签名仍是密文。
    let segments: Vec<&str> = candidate.split('.').collect();
    if segments.len() == 3
        && segments[0].starts_with("eyJ")
        && segments[0].len() >= 8
        && segments[1].len() >= 8
        && !segments[2].is_empty()
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_reasons_collapse_to_one_code_to_avoid_account_enumeration() {
        assert_eq!(
            normalize_reason(401, Some("INVALID_CREDENTIALS")),
            "AUTH_INVALID_CREDENTIALS"
        );
        assert_eq!(
            normalize_reason(401, Some("INVALID_USER")),
            "AUTH_INVALID_CREDENTIALS"
        );
    }

    #[test]
    fn verification_code_reasons_map_to_their_ux_codes() {
        assert_eq!(
            normalize_reason(400, Some("INVALID_VERIFY_CODE")),
            "AUTH_CODE_INVALID"
        );
        assert_eq!(
            normalize_reason(429, Some("VERIFY_CODE_MAX_ATTEMPTS")),
            "AUTH_CODE_INVALID"
        );
        assert_eq!(
            normalize_reason(429, Some("VERIFY_CODE_TOO_FREQUENT")),
            "AUTH_CODE_RATE_LIMITED"
        );
        assert_eq!(
            normalize_reason(400, Some("EMAIL_VERIFY_REQUIRED")),
            "AUTH_CODE_REQUIRED"
        );
        assert_eq!(
            normalize_reason(400, Some("VERIFY_CODE_REQUIRED")),
            "AUTH_CODE_REQUIRED"
        );
    }

    #[test]
    fn registration_reasons_map_to_their_ux_codes() {
        assert_eq!(
            normalize_reason(403, Some("REGISTRATION_DISABLED")),
            "AUTH_REGISTRATION_CLOSED"
        );
        assert_eq!(
            normalize_reason(400, Some("EMAIL_SUFFIX_NOT_ALLOWED")),
            "AUTH_EMAIL_DOMAIN_NOT_ALLOWED"
        );
        assert_eq!(
            normalize_reason(400, Some("EMAIL_RESERVED")),
            "AUTH_EMAIL_DOMAIN_NOT_ALLOWED"
        );
        assert_eq!(
            normalize_reason(409, Some("EMAIL_EXISTS")),
            "AUTH_EMAIL_ALREADY_REGISTERED"
        );
    }

    #[test]
    fn invitation_code_reasons_stay_actionable_instead_of_reading_as_an_outage() {
        assert_eq!(
            normalize_reason(400, Some("INVITATION_CODE_REQUIRED")),
            "AUTH_INVITATION_CODE_REQUIRED"
        );
        assert_eq!(
            normalize_reason(400, Some("INVITATION_CODE_INVALID")),
            "AUTH_INVITATION_CODE_INVALID"
        );
        // 服务端把这个 reason 放在 200 响应体里，归一化不能依赖状态码。
        assert_eq!(
            normalize_reason(200, Some("INVITATION_CODE_DISABLED")),
            "AUTH_INVITATION_CODE_INVALID"
        );
    }

    #[test]
    fn a_malformed_email_is_not_reported_as_an_unsupported_domain() {
        assert_eq!(
            normalize_reason(400, Some("INVALID_EMAIL")),
            "AUTH_EMAIL_INVALID"
        );
    }

    #[test]
    fn two_factor_reasons_map_to_their_ux_codes() {
        assert_eq!(
            normalize_reason(400, Some("TOTP_INVALID_CODE")),
            "AUTH_2FA_INVALID"
        );
        assert_eq!(
            normalize_reason(429, Some("TOTP_TOO_MANY_ATTEMPTS")),
            "AUTH_2FA_INVALID"
        );
        assert_eq!(
            normalize_reason(400, Some("TOTP_NOT_SETUP")),
            "AUTH_2FA_UNAVAILABLE"
        );
        assert_eq!(
            normalize_reason(400, Some("TOTP_NOT_ENABLED")),
            "AUTH_2FA_UNAVAILABLE"
        );
        assert_eq!(
            normalize_reason(500, Some("TOTP_VERIFY_ERROR")),
            "AUTH_2FA_UNAVAILABLE"
        );
        assert_eq!(
            normalize_reason(400, Some("EMAIL_VERIFY_NOT_ENABLED")),
            "AUTH_2FA_UNAVAILABLE"
        );
    }

    #[test]
    fn every_token_failure_becomes_a_single_session_expiry_code() {
        for reason in [
            "INVALID_TOKEN",
            "TOKEN_EXPIRED",
            "ACCESS_TOKEN_EXPIRED",
            "TOKEN_REVOKED",
            "REFRESH_TOKEN_INVALID",
            "REFRESH_TOKEN_EXPIRED",
            "REFRESH_TOKEN_REUSED",
            "SESSION_BINDING_MISMATCH",
        ] {
            assert_eq!(
                normalize_reason(401, Some(reason)),
                "AUTH_SESSION_EXPIRED",
                "{reason}"
            );
        }
    }

    #[test]
    fn disabled_accounts_are_distinguishable_from_bad_passwords() {
        assert_eq!(
            normalize_reason(403, Some("USER_NOT_ACTIVE")),
            "AUTH_ACCOUNT_DISABLED"
        );
    }

    #[test]
    fn key_provisioning_reasons_collapse_into_the_key_domain() {
        for reason in [
            "GROUP_NOT_ALLOWED",
            "API_KEY_EXISTS",
            "API_KEY_RATE_LIMITED",
            "IDEMPOTENCY_KEY_CONFLICT",
            "IDEMPOTENCY_IN_PROGRESS",
        ] {
            assert_eq!(
                normalize_reason(409, Some(reason)),
                "KEY_PROVISION_FAILED",
                "{reason}"
            );
        }
    }

    #[test]
    fn insufficient_balance_is_an_account_state_not_an_outage() {
        assert_eq!(
            normalize_reason(403, Some("INSUFFICIENT_BALANCE")),
            "ACCOUNT_INSUFFICIENT_BALANCE"
        );
        // 映射只认 reason、不看状态码：网关把这个码挂在别的状态码上也不许退化成宕机。
        assert_eq!(
            normalize_reason(402, Some("INSUFFICIENT_BALANCE")),
            "ACCOUNT_INSUFFICIENT_BALANCE"
        );
    }

    #[test]
    fn a_bodyless_401_still_reads_as_an_expired_session() {
        assert_eq!(normalize_reason(401, None), "AUTH_SESSION_EXPIRED");
    }

    #[test]
    fn backend_mode_and_server_faults_read_as_service_unavailable() {
        assert_eq!(
            normalize_reason(403, Some("BACKEND_MODE_ADMIN_ONLY")),
            "SERVICE_UNAVAILABLE"
        );
        assert_eq!(
            normalize_reason(503, Some("SERVICE_UNAVAILABLE")),
            "SERVICE_UNAVAILABLE"
        );
        assert_eq!(normalize_reason(500, None), "SERVICE_UNAVAILABLE");
        assert_eq!(normalize_reason(502, None), "SERVICE_UNAVAILABLE");
    }

    #[test]
    fn a_rate_limited_response_without_a_reason_still_gets_a_stable_code() {
        assert_eq!(normalize_reason(429, None), "SERVICE_RATE_LIMITED");
    }

    #[test]
    fn unrecognized_reasons_do_not_leak_the_server_string() {
        let code = normalize_reason(418, Some("SOME_BRAND_NEW_SERVER_REASON"));
        assert_eq!(code, "SERVICE_UNAVAILABLE");
    }

    #[test]
    fn an_unrecognized_reason_on_401_still_reads_as_an_expired_session() {
        assert_eq!(
            normalize_reason(401, Some("SOME_UNRECOGNIZED_REASON")),
            "AUTH_SESSION_EXPIRED"
        );
    }

    #[test]
    fn redaction_removes_bearer_tokens_keys_and_emails() {
        let dirty = "Authorization: Bearer eyJhbGciOi.JIUzI1NiJ9.sig key=sk-abcdef0123456789 user@example.com rt_0123456789abcdef";
        let clean = redact(dirty);

        assert!(!clean.contains("eyJhbGciOi"));
        assert!(!clean.contains("sk-abcdef0123456789"));
        assert!(!clean.contains("user@example.com"));
        assert!(!clean.contains("rt_0123456789abcdef"));
        assert!(clean.contains("[redacted]"));
    }

    #[test]
    fn redaction_leaves_ordinary_diagnostics_readable() {
        assert_eq!(
            redact("stage=prepare-connection code=KEY_PROVISION_FAILED"),
            "stage=prepare-connection code=KEY_PROVISION_FAILED"
        );
    }

    #[test]
    fn redaction_survives_json_quoting_around_a_refresh_token() {
        let clean = redact(r#"{"refresh_token":"rt_0123456789abcdef"}"#);
        assert!(!clean.contains("rt_0123456789abcdef"), "{clean}");
        assert!(clean.contains("[redacted]"), "{clean}");
    }

    #[test]
    fn redaction_reaches_every_field_of_a_multi_field_json_body() {
        let clean = redact(
            r#"{"access_token":"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.Sfl","refresh_token":"rt_0123456789abcdef"}"#,
        );
        assert!(
            !clean.contains("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.Sfl"),
            "{clean}"
        );
        assert!(!clean.contains("rt_0123456789abcdef"), "{clean}");
    }

    #[test]
    fn redaction_sees_through_surrounding_punctuation() {
        let clean = redact("(sk-abcdef0123456789)");
        assert!(!clean.contains("sk-abcdef0123456789"), "{clean}");
        assert!(clean.contains("[redacted]"), "{clean}");
    }

    #[test]
    fn dotted_identifiers_are_not_mistaken_for_jwts() {
        for readable in [
            "host=production.kubernetes.local",
            "codex-plus-core.integration.smoke",
            "stage=prepare-connection.provision.step",
        ] {
            assert_eq!(redact(readable), readable);
        }
    }
}
