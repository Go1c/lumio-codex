//! CC 控制面薄客户端：用已登录的 Sub2API Bearer 问谁有权用 Claude。
//!
//! 控制面成功体是 `{"data": …}`；测试和部分 mock 也会给裸 `{status}`。

use serde::Deserialize;
use std::time::Duration;

pub const CLAUDE_CONTROL_API: &str = "https://api.cc.bestcodex.app";
const SESSION_EXPIRED: &str = "AUTH_SESSION_EXPIRED";
const SERVICE_UNAVAILABLE: &str = "SERVICE_UNAVAILABLE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeEntitlementSnapshot {
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct EntitlementBody {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    data: Option<EntitlementInner>,
}

#[derive(Debug, Deserialize)]
struct EntitlementInner {
    #[serde(default)]
    status: Option<String>,
}

pub fn parse_entitlement_json(body: &str) -> Result<ClaudeEntitlementSnapshot, String> {
    let parsed: EntitlementBody =
        serde_json::from_str(body).map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let status = parsed
        .status
        .or_else(|| parsed.data.and_then(|inner| inner.status))
        .unwrap_or_default();
    match status.as_str() {
        "active" | "trialing" | "none" | "expired" => Ok(ClaudeEntitlementSnapshot { status }),
        _ => Err(SERVICE_UNAVAILABLE.to_string()),
    }
}

pub async fn fetch_entitlement(access_token: &str) -> Result<ClaudeEntitlementSnapshot, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let response = client
        .get(format!("{CLAUDE_CONTROL_API}/api/v1/me/entitlement"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(SESSION_EXPIRED.to_string());
    }
    if !status.is_success() {
        return Err(SERVICE_UNAVAILABLE.to_string());
    }
    let body = response
        .text()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    parse_entitlement_json(&body)
}

pub async fn heartbeat(
    access_token: &str,
    device_id: &str,
    app_version: &str,
    os_version: &str,
    arch: &str,
) -> Result<ClaudeEntitlementSnapshot, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let response = client
        .post(format!("{CLAUDE_CONTROL_API}/api/v1/app/heartbeat"))
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "device_id": device_id,
            "app_version": app_version,
            "os_version": os_version,
            "arch": arch,
        }))
        .send()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(SESSION_EXPIRED.to_string());
    }
    if !status.is_success() {
        return Err(SERVICE_UNAVAILABLE.to_string());
    }
    let body = response
        .text()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    // Heartbeat is `{data:{entitlement:{status}}}` or `{entitlement:{status}}`.
    parse_heartbeat_json(&body)
}

fn parse_heartbeat_json(body: &str) -> Result<ClaudeEntitlementSnapshot, String> {
    #[derive(Deserialize)]
    struct HeartbeatBody {
        #[serde(default)]
        entitlement: Option<EntitlementInner>,
        #[serde(default)]
        data: Option<HeartbeatInner>,
    }
    #[derive(Deserialize)]
    struct HeartbeatInner {
        #[serde(default)]
        entitlement: Option<EntitlementInner>,
    }
    if let Ok(snapshot) = parse_entitlement_json(body) {
        return Ok(snapshot);
    }
    let parsed: HeartbeatBody =
        serde_json::from_str(body).map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let status = parsed
        .entitlement
        .and_then(|inner| inner.status)
        .or_else(|| {
            parsed
                .data
                .and_then(|inner| inner.entitlement)
                .and_then(|inner| inner.status)
        })
        .unwrap_or_default();
    match status.as_str() {
        "active" | "trialing" | "none" | "expired" => Ok(ClaudeEntitlementSnapshot { status }),
        _ => Err(SERVICE_UNAVAILABLE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_data_envelope() {
        let body = r#"{"data":{"status":"active","days_left":12}}"#;
        assert_eq!(parse_entitlement_json(body).unwrap().status, "active");
    }

    #[test]
    fn accepts_bare_status_for_tests() {
        let body = r#"{"status":"none"}"#;
        assert_eq!(parse_entitlement_json(body).unwrap().status, "none");
    }

    #[test]
    fn heartbeat_envelope_reads_entitlement_status() {
        let body = r#"{"data":{"entitlement":{"status":"trialing","days_left":2},"notices":[]}}"#;
        assert_eq!(parse_heartbeat_json(body).unwrap().status, "trialing");
    }
}
