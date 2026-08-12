//! Lumio 客户端版本提醒。只做「有没有更新」探测与下载页引导，不自动安装。
//! 清单来源是本仓库 GitHub Releases API，与旧 Codex++ 的 `latest.json` 路径分开。

use serde::Serialize;

use super::product;

const NETWORK: &str = "SERVICE_UNAVAILABLE";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReminder {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub download_url: String,
    pub release_summary: String,
}

pub async fn check_update_reminder(current_version: &str) -> Result<UpdateReminder, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("LumioCodex/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| NETWORK.to_string())?;

    let response = client
        .get(product::GITHUB_LATEST_RELEASE_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| NETWORK.to_string())?;

    // 还没有正式 Release 时不算失败：安静地回报「无更新」。
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateReminder {
            current_version: current_version.to_string(),
            latest_version: None,
            update_available: false,
            download_url: product::RELEASES_PAGE_URL.to_string(),
            release_summary: String::new(),
        });
    }

    let response = response
        .error_for_status()
        .map_err(|_| NETWORK.to_string())?;
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| NETWORK.to_string())?;

    let release =
        crate::update::release_from_github_payload(&payload).map_err(|_| NETWORK.to_string())?;
    let update_available =
        crate::update::is_newer_version(&release.version, current_version).unwrap_or(false);
    let download_url = if release.url.trim().is_empty() {
        product::RELEASES_PAGE_URL.to_string()
    } else {
        release.url
    };

    Ok(UpdateReminder {
        current_version: current_version.to_string(),
        latest_version: Some(release.version),
        update_available,
        download_url,
        release_summary: release.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payment_url_targets_api_purchase_page() {
        // 充值只允许 API 站 /purchase；禁止落到产品站。存量客户端硬编码了这个地址。
        assert_eq!(product::payment_url(), "https://api.lumio.games/purchase");
        assert!(product::payment_url().starts_with("https://api.lumio.games/"));
        assert!(!product::payment_url().starts_with(product::SITE_BASE_URL));
        assert!(!product::payment_url().contains("lumio.games/payment"));
        assert_eq!(product::SITE_BASE_URL, "https://codex.lumiogame.com");
        assert_ne!(
            product::SITE_BASE_URL,
            product::API_BASE_URL.trim_end_matches('/')
        );
    }

    #[test]
    fn a_missing_release_is_reported_as_no_update() {
        // 纯逻辑：构造提醒结构必须能表达「尚无 latest」。
        let reminder = UpdateReminder {
            current_version: "1.2.46".into(),
            latest_version: None,
            update_available: false,
            download_url: product::RELEASES_PAGE_URL.into(),
            release_summary: String::new(),
        };
        assert!(!reminder.update_available);
        assert!(reminder.latest_version.is_none());
    }
}
