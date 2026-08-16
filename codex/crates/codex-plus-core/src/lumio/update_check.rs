//! Lumio 客户端版本提醒与手动更新下载。提醒只做「有没有更新」探测；更新
//! 由用户在提示上主动触发，应用下载平台安装包并打开安装向导，**不后台自动
//! 安装**。清单来源是本仓库 GitHub Releases API，与旧 Codex++ 的 `latest.json`
//! 路径分开。

use serde::Serialize;

use super::product;

const NETWORK: &str = "SERVICE_UNAVAILABLE";
pub const UPDATE_ASSET_UNAVAILABLE: &str = "UPDATE_ASSET_UNAVAILABLE";
pub const UPDATE_DOWNLOAD_FAILED: &str = "UPDATE_DOWNLOAD_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReminder {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    /// 弹窗静默位：该版本已被忽略或今天已弹过（绿标入口不受影响）。
    pub notice_muted: bool,
    pub download_url: String,
    pub release_summary: String,
}

pub async fn check_update_reminder(current_version: &str) -> Result<UpdateReminder, String> {
    let release = fetch_latest_release().await?;
    Ok(match release {
        Some(release) => {
            let notice_muted = notice_already_muted(&release);
            reminder_from_release(current_version, release, notice_muted)
        }
        None => no_update_reminder(current_version),
    })
}

fn notice_already_muted(release: &crate::update::Release) -> bool {
    let Some(state_dir) = product::state_dir() else {
        return false;
    };
    let prefs = super::update_notice::read_prefs(&state_dir);
    let today = super::update_notice::today_day(std::time::SystemTime::now());
    super::update_notice::notice_decision(&release.version, &prefs, today)
        != super::update_notice::NoticeDecision::Show
}

/// 用户在「有新版本」提示上点击后走这里：下载最新 Release 的平台安装包到
/// 缓存目录并打开安装向导（Windows 运行安装器 / macOS 打开 DMG），安装由
/// 用户在向导里手动完成。
pub async fn download_and_launch_update() -> Result<crate::update::UpdateInstall, String> {
    let release = fetch_latest_release()
        .await?
        .ok_or_else(|| UPDATE_ASSET_UNAVAILABLE.to_string())?;
    if !release_has_downloadable_asset(&release) {
        return Err(UPDATE_ASSET_UNAVAILABLE.to_string());
    }
    let Some(cache) = product::cache_dir() else {
        return Err(UPDATE_DOWNLOAD_FAILED.to_string());
    };
    crate::update::perform_update(&release, &cache.join("updates"))
        .await
        .map_err(|_| UPDATE_DOWNLOAD_FAILED.to_string())
}

pub fn release_has_downloadable_asset(release: &crate::update::Release) -> bool {
    release
        .asset_url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
}

fn update_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("LumioCodex/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| NETWORK.to_string())
}

/// latest API 只覆盖正式 Release；还没有正式 Release（或它 404）时回退读
/// Release 列表。两处都拿不到才返回 None（= 无可对照版本）。
async fn fetch_latest_release() -> Result<Option<crate::update::Release>, String> {
    let client = update_client()?;
    let response = client
        .get(product::GITHUB_LATEST_RELEASE_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| NETWORK.to_string())?;

    // 还没有正式 Release 时不算失败：回退读 Release 列表（含 prerelease）——
    // 内测渠道只发 prerelease，`/releases/latest` 对它恒 404（docs/ops/03-release.md
    // §3.1）。列表也为空才回报「无更新」。
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return fetch_latest_from_releases_list(&client).await;
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
    Ok(Some(release))
}

async fn fetch_latest_from_releases_list(
    client: &reqwest::Client,
) -> Result<Option<crate::update::Release>, String> {
    let response = client
        .get(product::GITHUB_RELEASES_LIST_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| NETWORK.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|_| NETWORK.to_string())?;
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| NETWORK.to_string())?;
    Ok(latest_from_releases_list(&payload))
}

/// 列表按发布时间倒序（含 prerelease）；draft 尚未发布，跳过。
pub fn latest_from_releases_list(payload: &serde_json::Value) -> Option<crate::update::Release> {
    payload
        .as_array()?
        .iter()
        .filter(|entry| {
            !entry
                .get("draft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|entry| crate::update::release_from_github_payload(entry).ok())
        .next()
}

fn reminder_from_release(
    current_version: &str,
    release: crate::update::Release,
    notice_muted: bool,
) -> UpdateReminder {
    let update_available =
        crate::update::is_newer_version(&release.version, current_version).unwrap_or(false);
    let download_url = if release.url.trim().is_empty() {
        product::RELEASES_PAGE_URL.to_string()
    } else {
        release.url
    };
    UpdateReminder {
        current_version: current_version.to_string(),
        latest_version: Some(release.version),
        update_available,
        notice_muted,
        download_url,
        release_summary: release.body,
    }
}

fn no_update_reminder(current_version: &str) -> UpdateReminder {
    UpdateReminder {
        current_version: current_version.to_string(),
        latest_version: None,
        update_available: false,
        notice_muted: true,
        download_url: product::RELEASES_PAGE_URL.to_string(),
        release_summary: String::new(),
    }
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
            notice_muted: true,
            download_url: product::RELEASES_PAGE_URL.into(),
            release_summary: String::new(),
        };
        assert!(!reminder.update_available);
        assert!(reminder.latest_version.is_none());
        assert!(reminder.notice_muted, "无更新就没有可弹的东西");
    }

    #[test]
    fn releases_list_fallback_picks_the_first_public_release_and_skips_drafts() {
        let payload = serde_json::json!([
            {
                "tag_name": "v9.9.9",
                "html_url": "https://github.com/Go1c/lumio-codex/releases/tag/v9.9.9",
                "body": "wip",
                "draft": true,
            },
            {
                "tag_name": "v1.3.0",
                "html_url": "https://github.com/Go1c/lumio-codex/releases/tag/v1.3.0",
                "prerelease": true,
                "body": "internal build",
            },
            {
                "tag_name": "v1.2.46",
                "html_url": "https://github.com/Go1c/lumio-codex/releases/tag/v1.2.46",
                "prerelease": false,
                "body": "",
            },
        ]);
        let release = latest_from_releases_list(&payload).expect("prerelease 也必须进入更新提醒");
        assert_eq!(release.version, "v1.3.0");
        assert_eq!(
            release.url,
            "https://github.com/Go1c/lumio-codex/releases/tag/v1.3.0"
        );
    }

    #[test]
    fn an_empty_releases_list_reports_no_update() {
        assert!(latest_from_releases_list(&serde_json::json!([])).is_none());
    }

    #[test]
    fn a_newer_prerelease_from_the_list_triggers_the_reminder() {
        let reminder = reminder_from_release(
            "1.2.46",
            crate::update::Release {
                version: "v1.3.0".into(),
                url: String::new(),
                body: "internal build".into(),
                asset_name: None,
                asset_url: None,
            },
            false,
        );
        assert!(reminder.update_available);
        assert!(!reminder.notice_muted);
        assert_eq!(reminder.latest_version.as_deref(), Some("v1.3.0"));
        assert_eq!(
            reminder.download_url,
            product::RELEASES_PAGE_URL,
            "空 html_url 回落到下载页"
        );

        let same_version = reminder_from_release(
            "1.2.46",
            crate::update::Release {
                version: "v1.2.46".into(),
                url: String::new(),
                body: String::new(),
                asset_name: None,
                asset_url: None,
            },
            true,
        );
        assert!(!same_version.update_available);
        assert!(same_version.notice_muted, "muted 只影响弹窗，字段原样透传");
    }

    #[test]
    fn a_release_without_a_downloadable_asset_cannot_start_an_update() {
        let bare = crate::update::Release {
            version: "v1.3.0".into(),
            url: String::new(),
            body: String::new(),
            asset_name: None,
            asset_url: None,
        };
        assert!(!release_has_downloadable_asset(&bare));

        let blank_url = crate::update::Release {
            asset_url: Some("   ".into()),
            ..bare.clone()
        };
        assert!(!release_has_downloadable_asset(&blank_url));

        let ready = crate::update::Release {
            asset_name: Some("LumioCodex-1.3.0-macos-arm64-internal-unsigned.dmg".into()),
            asset_url: Some("https://example.test/mac.dmg".into()),
            ..bare
        };
        assert!(release_has_downloadable_asset(&ready));
    }
}
