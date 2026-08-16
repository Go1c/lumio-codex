//! 桌面 API Key 的编排：复用保留名下已有的可用 Key，没有才创建。
//! Key 明文只在进程内流转，绝不进日志、也不跨 IPC。

use super::api::{ApiKeyRecord, CreateKeyRequest, GroupSummary, LumioApiClient};
use super::product::DESKTOP_KEY_NAME;

const PROVISION_FAILED: &str = "KEY_PROVISION_FAILED";

pub async fn ensure_desktop_key(
    client: &LumioApiClient,
    access_token: &str,
) -> Result<String, String> {
    let groups = client.available_groups(access_token).await?;
    let allowed: Vec<i64> = groups.iter().map(|group| group.id).collect();

    let mut candidates: Vec<ApiKeyRecord> = client
        .list_keys(access_token, DESKTOP_KEY_NAME)
        .await?
        .into_iter()
        .filter(|record| is_reusable(record, &allowed))
        .collect();
    // 最早创建的优先：并发的两台设备各自建过 Key 时，双方最终会收敛到同一把。
    candidates.sort_by(|left, right| left.created_at.cmp(&right.created_at));

    if let Some(existing) = candidates.first() {
        return usable_key(&existing.key);
    }

    let created = client
        .create_key(
            access_token,
            &CreateKeyRequest {
                name: DESKTOP_KEY_NAME.to_string(),
                group_id: select_group(&groups),
            },
        )
        .await?;
    usable_key(&created.key)
}

/// 服务端返回的顺序即优先级；空表示账户没有分组限制。
pub fn select_group(groups: &[GroupSummary]) -> Option<i64> {
    groups.first().map(|group| group.id)
}

pub fn is_reusable(key: &ApiKeyRecord, allowed_group_ids: &[i64]) -> bool {
    if key.name != DESKTOP_KEY_NAME || key.status != "active" {
        return false;
    }
    if key
        .expires_at
        .as_deref()
        .map(str::trim)
        .is_some_and(|expiry| !expiry.is_empty() && is_expired(expiry))
    {
        return false;
    }
    match key.group_id {
        Some(group_id) => allowed_group_ids.is_empty() || allowed_group_ids.contains(&group_id),
        None => true,
    }
}

fn usable_key(key: &str) -> Result<String, String> {
    if key.trim().is_empty() {
        return Err(PROVISION_FAILED.to_string());
    }
    Ok(key.to_string())
}

/// 有效期判断只认服务端的 RFC 3339 UTC 时间戳；解析不出来的一律当作过期，
/// 宁可多建一把 Key，也不把一把可能已失效的 Key 写进官方配置。
fn is_expired(expires_at: &str) -> bool {
    match parse_rfc3339_seconds(expires_at) {
        Some(expiry) => expiry <= now_unix_seconds(),
        None => true,
    }
}

fn parse_rfc3339_seconds(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ' {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Howard Hinnant 的 civil-from-days 逆运算，避免为一个日期比较引入新依赖。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lumio::api::{ApiKeyRecord, GroupSummary, LumioApiClient};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn key(name: &str, status: &str, group_id: Option<i64>, created_at: &str) -> ApiKeyRecord {
        ApiKeyRecord {
            id: 1,
            name: name.to_string(),
            key: "sk-x".to_string(),
            status: status.to_string(),
            group_id,
            expires_at: None,
            created_at: created_at.to_string(),
        }
    }

    #[test]
    fn only_active_keys_in_an_allowed_group_are_reusable() {
        assert!(is_reusable(
            &key("BestCodex Desktop", "active", Some(3), "2026-01-01"),
            &[3]
        ));
        assert!(!is_reusable(
            &key("BestCodex Desktop", "disabled", Some(3), "2026-01-01"),
            &[3]
        ));
        assert!(!is_reusable(
            &key("BestCodex Desktop", "active", Some(9), "2026-01-01"),
            &[3]
        ));
        assert!(!is_reusable(
            &key("Other Key", "active", Some(3), "2026-01-01"),
            &[3]
        ));
    }

    #[test]
    fn a_key_with_no_group_is_reusable_when_the_account_has_no_group_restriction() {
        assert!(is_reusable(
            &key("BestCodex Desktop", "active", None, "2026-01-01"),
            &[]
        ));
    }

    #[test]
    fn expiry_decides_reuse_and_an_unreadable_expiry_counts_as_expired() {
        let base = key("BestCodex Desktop", "active", Some(3), "2026-01-01");
        for (expiry, reusable) in [
            ("2999-01-01T00:00:00Z", true),
            ("2020-01-01T00:00:00Z", false),
            ("whenever", false),
        ] {
            let record = ApiKeyRecord {
                expires_at: Some(expiry.to_string()),
                ..base.clone()
            };
            assert_eq!(is_reusable(&record, &[3]), reusable, "{expiry}");
        }
    }

    #[test]
    fn group_selection_prefers_the_first_available_group() {
        let groups = vec![
            GroupSummary {
                id: 5,
                name: "beta".to_string(),
            },
            GroupSummary {
                id: 3,
                name: "default".to_string(),
            },
        ];
        assert_eq!(select_group(&groups), Some(5));
        assert_eq!(select_group(&[]), None);
    }

    #[tokio::test]
    async fn an_existing_active_key_is_reused_without_creating_another() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [{
                    "id": 1, "name": "BestCodex Desktop", "key": "sk-existing",
                    "status": "active", "group_id": 3, "created_at": "2026-01-01T00:00:00Z"
                }], "total": 1, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        // 未 mount POST /api/v1/keys —— 若实现试图创建，wiremock 会以 404 让测试失败。

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let resolved = ensure_desktop_key(&client, "access-token").await.unwrap();

        assert_eq!(resolved, "sk-existing");
    }

    #[tokio::test]
    async fn the_oldest_reusable_key_wins_when_several_share_the_reserved_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [
                    { "id": 2, "name": "BestCodex Desktop", "key": "sk-newer",
                      "status": "active", "group_id": 3, "created_at": "2026-05-01T00:00:00Z" },
                    { "id": 1, "name": "BestCodex Desktop", "key": "sk-older",
                      "status": "active", "group_id": 3, "created_at": "2026-01-01T00:00:00Z" }
                ], "total": 2, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            ensure_desktop_key(&client, "access-token").await.unwrap(),
            "sk-older"
        );
    }

    #[tokio::test]
    async fn a_key_is_created_when_none_is_reusable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [{
                    "id": 1, "name": "BestCodex Desktop", "key": "sk-dead",
                    "status": "disabled", "group_id": 3, "created_at": "2026-01-01T00:00:00Z"
                }], "total": 1, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "id": 9, "name": "BestCodex Desktop", "key": "sk-fresh",
                          "status": "active", "group_id": 3, "created_at": "2026-08-01T00:00:00Z" }
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            ensure_desktop_key(&client, "access-token").await.unwrap(),
            "sk-fresh"
        );
    }

    #[tokio::test]
    async fn a_rejected_creation_surfaces_the_key_domain_error_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [], "total": 0, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "code": 403, "message": "group not allowed", "reason": "GROUP_NOT_ALLOWED"
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            ensure_desktop_key(&client, "access-token")
                .await
                .unwrap_err(),
            "KEY_PROVISION_FAILED"
        );
    }
}
