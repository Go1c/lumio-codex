//! CC 控制面薄客户端：用已登录的 Sub2API Bearer 问谁有权用 Claude。
//!
//! 控制面成功体是 `{"data": …}`；测试和部分 mock 也会给裸 `{status}`。

use serde::Deserialize;
use std::time::Duration;

pub const CLAUDE_CONTROL_API: &str = "https://api.cc.bestcodex.app";
const SESSION_EXPIRED: &str = "AUTH_SESSION_EXPIRED";
const SERVICE_UNAVAILABLE: &str = "SERVICE_UNAVAILABLE";
const ACCOUNT_INSUFFICIENT_BALANCE: &str = "ACCOUNT_INSUFFICIENT_BALANCE";

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

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())
}

async fn read_body(response: reqwest::Response) -> Result<(u16, String), String> {
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    Ok((status, body))
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

pub fn new_idempotency_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeBillingOrder {
    pub order_no: String,
    pub amount_cents: i64,
    pub currency: String,
    pub channel: String,
    pub status: String,
    pub paid_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePayEntitlement {
    pub status: String,
    pub expires_at: Option<String>,
    pub days_left: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePayWithBalanceSnapshot {
    pub order: ClaudeBillingOrder,
    pub entitlement: ClaudePayEntitlement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePlanSnapshot {
    pub amount_cents: i64,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct RawOrder {
    order_no: String,
    amount_cents: i64,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    status: String,
    #[serde(default)]
    paid_at: Option<String>,
    created_at: String,
}

#[derive(Deserialize)]
struct RawEntitlement {
    status: String,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    days_left: Option<i64>,
}

#[derive(Deserialize)]
struct PayEnvelope {
    #[serde(default)]
    data: Option<PayInner>,
    #[serde(default)]
    order: Option<RawOrder>,
    #[serde(default)]
    entitlement: Option<RawEntitlement>,
}

#[derive(Deserialize)]
struct PayInner {
    order: RawOrder,
    entitlement: RawEntitlement,
}

#[derive(Deserialize)]
struct OrdersEnvelope {
    #[serde(default)]
    data: Option<OrdersInner>,
    #[serde(default)]
    items: Option<Vec<RawOrder>>,
}

#[derive(Deserialize)]
struct OrdersInner {
    #[serde(default)]
    items: Vec<RawOrder>,
}

#[derive(Deserialize)]
struct PlanEnvelope {
    #[serde(default)]
    data: Option<RawPlan>,
    #[serde(default)]
    amount_cents: Option<i64>,
}

#[derive(Deserialize)]
struct RawPlan {
    amount_cents: i64,
}

fn from_raw_order(order: RawOrder) -> ClaudeBillingOrder {
    ClaudeBillingOrder {
        order_no: order.order_no,
        amount_cents: order.amount_cents,
        currency: order.currency.unwrap_or_default(),
        channel: order.channel.unwrap_or_default(),
        status: order.status,
        paid_at: order.paid_at,
        created_at: order.created_at,
    }
}

fn from_raw_entitlement(entitlement: RawEntitlement) -> ClaudePayEntitlement {
    ClaudePayEntitlement {
        status: entitlement.status,
        expires_at: entitlement.expires_at,
        days_left: entitlement.days_left,
    }
}

fn is_insufficient_balance_code(code: &str) -> bool {
    code.eq_ignore_ascii_case("insufficient_balance")
}

pub fn map_billing_error(status: u16, body: &str) -> String {
    if status == 401 {
        return SESSION_EXPIRED.to_string();
    }
    if let Ok(parsed) = serde_json::from_str::<ErrorEnvelope>(body) {
        if parsed
            .error
            .code
            .as_deref()
            .is_some_and(is_insufficient_balance_code)
        {
            return ACCOUNT_INSUFFICIENT_BALANCE.to_string();
        }
    }
    SERVICE_UNAVAILABLE.to_string()
}

pub fn parse_pay_with_balance_json(body: &str) -> Result<ClaudePayWithBalanceSnapshot, String> {
    let parsed: PayEnvelope =
        serde_json::from_str(body).map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let (order, entitlement) = if let Some(inner) = parsed.data {
        (inner.order, inner.entitlement)
    } else if let (Some(order), Some(entitlement)) = (parsed.order, parsed.entitlement) {
        (order, entitlement)
    } else {
        return Err(SERVICE_UNAVAILABLE.to_string());
    };
    Ok(ClaudePayWithBalanceSnapshot {
        order: from_raw_order(order),
        entitlement: from_raw_entitlement(entitlement),
    })
}

pub fn parse_orders_json(body: &str) -> Result<Vec<ClaudeBillingOrder>, String> {
    let parsed: OrdersEnvelope =
        serde_json::from_str(body).map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let items = parsed
        .items
        .or_else(|| parsed.data.map(|inner| inner.items))
        .unwrap_or_default();
    Ok(items.into_iter().map(from_raw_order).collect())
}

pub fn parse_plan_json(body: &str) -> Result<ClaudePlanSnapshot, String> {
    let parsed: PlanEnvelope =
        serde_json::from_str(body).map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let amount_cents = parsed
        .amount_cents
        .or_else(|| parsed.data.map(|inner| inner.amount_cents))
        .unwrap_or(0);
    if amount_cents <= 0 {
        return Err(SERVICE_UNAVAILABLE.to_string());
    }
    Ok(ClaudePlanSnapshot { amount_cents })
}

pub async fn pay_with_balance(
    access_token: &str,
    idempotency_key: &str,
) -> Result<ClaudePayWithBalanceSnapshot, String> {
    let client = http_client()?;
    let response = client
        .post(format!(
            "{CLAUDE_CONTROL_API}/api/v1/billing/pay-with-balance"
        ))
        .bearer_auth(access_token)
        .header("Idempotency-Key", idempotency_key)
        .send()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let (status, body) = read_body(response).await?;
    if !is_success(status) {
        return Err(map_billing_error(status, &body));
    }
    parse_pay_with_balance_json(&body)
}

pub async fn list_orders(access_token: &str) -> Result<Vec<ClaudeBillingOrder>, String> {
    let client = http_client()?;
    let response = client
        .get(format!("{CLAUDE_CONTROL_API}/api/v1/billing/orders"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let (status, body) = read_body(response).await?;
    if !is_success(status) {
        return Err(map_billing_error(status, &body));
    }
    parse_orders_json(&body)
}

pub async fn fetch_plan() -> Result<ClaudePlanSnapshot, String> {
    let client = http_client()?;
    let response = client
        .get(format!("{CLAUDE_CONTROL_API}/api/v1/billing/plan"))
        .send()
        .await
        .map_err(|_| SERVICE_UNAVAILABLE.to_string())?;
    let (status, body) = read_body(response).await?;
    if !is_success(status) {
        return Err(SERVICE_UNAVAILABLE.to_string());
    }
    parse_plan_json(&body)
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

    #[test]
    fn pay_unwraps_data_envelope() {
        let body = r#"{"data":{"order":{"order_no":"BC202608190001","amount_cents":1990,"currency":"CNY","channel":"balance","status":"paid","paid_at":"2026-08-19T00:00:00Z","created_at":"2026-08-19T00:00:00Z"},"entitlement":{"status":"active","expires_at":"2026-09-18T00:00:00Z","days_left":30}}}"#;
        let snap = parse_pay_with_balance_json(body).unwrap();
        assert_eq!(snap.order.order_no, "BC202608190001");
        assert_eq!(snap.order.amount_cents, 1990);
        assert_eq!(snap.order.status, "paid");
        assert_eq!(snap.entitlement.status, "active");
        assert_eq!(snap.entitlement.days_left, Some(30));
    }

    #[test]
    fn pay_maps_insufficient_balance() {
        let body = r#"{"error":{"code":"insufficient_balance","message":"余额不足","details":{"purchase_url":"https://example.com/purchase"}}}"#;
        assert_eq!(map_billing_error(403, body), "ACCOUNT_INSUFFICIENT_BALANCE");
    }

    #[test]
    fn pay_maps_uppercase_insufficient_balance() {
        let body = r#"{"error":{"code":"INSUFFICIENT_BALANCE"}}"#;
        assert_eq!(map_billing_error(403, body), "ACCOUNT_INSUFFICIENT_BALANCE");
    }

    #[test]
    fn pay_maps_401_to_session_expired() {
        assert_eq!(map_billing_error(401, "{}"), "AUTH_SESSION_EXPIRED");
    }

    #[test]
    fn orders_unwraps_items() {
        let body = r#"{"data":{"items":[{"order_no":"BC1","amount_cents":1990,"currency":"CNY","channel":"balance","status":"paid","created_at":"2026-08-19T00:00:00Z"}]}}"#;
        let items = parse_orders_json(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].order_no, "BC1");
        assert_eq!(items[0].amount_cents, 1990);
    }

    #[test]
    fn plan_unwraps_amount_cents() {
        let body = r#"{"data":{"amount_cents":1990,"currency":"CNY"}}"#;
        assert_eq!(parse_plan_json(body).unwrap().amount_cents, 1990);
    }
}
