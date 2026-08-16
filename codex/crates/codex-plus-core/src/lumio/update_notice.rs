//! 更新弹窗的本地偏好（`state_dir()/update-notice.json`）。
//!
//! 弹窗是提示不是打扰：用户点「稍后」即忽略**该版本**（绿标常驻，弹窗静默，
//! 出现更新的版本才恢复）；没表达忽略时同一天最多弹一次（UTC 天，零依赖取舍，
//! 跨时区用户在日期边界±数小时内多/少弹一次，可接受）。

use std::path::Path;

use serde::{Deserialize, Serialize};

const PREFS_FILE: &str = "update-notice.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNoticePrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_notice_day: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeDecision {
    /// 可以弹：这个版本没被忽略过，今天也没弹过。
    Show,
    /// 用户忽略过这个版本：绿标照常，弹窗静默，更新的版本出现才恢复。
    MutedByVersion,
    /// 今天已经弹过一次。
    MutedForToday,
}

pub fn notice_decision(
    latest_version: &str,
    prefs: &UpdateNoticePrefs,
    today_day: u64,
) -> NoticeDecision {
    if prefs.dismissed_version.as_deref() == Some(latest_version) {
        return NoticeDecision::MutedByVersion;
    }
    if prefs.last_notice_day == Some(today_day) {
        return NoticeDecision::MutedForToday;
    }
    NoticeDecision::Show
}

/// epoch 天（UTC）：`std::time` 无时区概念，零依赖下的「每天一次」粒度。
pub fn today_day(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0)
}

fn prefs_path(dir: &Path) -> std::path::PathBuf {
    dir.join(PREFS_FILE)
}

pub fn read_prefs(dir: &Path) -> UpdateNoticePrefs {
    let Ok(text) = std::fs::read_to_string(prefs_path(dir)) else {
        return UpdateNoticePrefs::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_prefs(dir: &Path, prefs: &UpdateNoticePrefs) -> bool {
    let Ok(text) = serde_json::to_string(prefs) else {
        return false;
    };
    std::fs::create_dir_all(dir).is_ok() && std::fs::write(prefs_path(dir), text).is_ok()
}

/// 弹窗上的「稍后」：忽略这个版本，下一个版本再提示。
pub fn dismiss_version(dir: &Path, version: &str) -> bool {
    let mut prefs = read_prefs(dir);
    prefs.dismissed_version = Some(version.to_string());
    write_prefs(dir, &prefs)
}

/// 弹窗真正渲染时记录当天已弹过一次。
pub fn mark_notice_shown(dir: &Path, day: u64) -> bool {
    let mut prefs = read_prefs(dir);
    prefs.last_notice_day = Some(day);
    write_prefs(dir, &prefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_version_shows_the_notice() {
        let prefs = UpdateNoticePrefs::default();
        assert_eq!(notice_decision("v1.3.0", &prefs, 100), NoticeDecision::Show);
    }

    #[test]
    fn a_dismissed_version_stays_silent_until_a_newer_one_arrives() {
        let prefs = UpdateNoticePrefs {
            dismissed_version: Some("v1.3.0".into()),
            last_notice_day: None,
        };
        assert_eq!(
            notice_decision("v1.3.0", &prefs, 100),
            NoticeDecision::MutedByVersion,
            "忽略过的版本不再弹"
        );
        assert_eq!(
            notice_decision("v1.4.0", &prefs, 100),
            NoticeDecision::Show,
            "下一个版本恢复提示"
        );
    }

    #[test]
    fn the_notice_shows_at_most_once_per_day() {
        let shown_today = UpdateNoticePrefs {
            dismissed_version: None,
            last_notice_day: Some(100),
        };
        assert_eq!(
            notice_decision("v1.3.0", &shown_today, 100),
            NoticeDecision::MutedForToday
        );
        assert_eq!(
            notice_decision("v1.3.0", &shown_today, 101),
            NoticeDecision::Show,
            "第二天可以再提示（用户还没表达忽略）"
        );
    }

    #[test]
    fn version_dismissal_outranks_the_daily_gate() {
        // 同一天弹过后用户点了「稍后」：即使第二天也保持静默。
        let prefs = UpdateNoticePrefs {
            dismissed_version: Some("v1.3.0".into()),
            last_notice_day: Some(100),
        };
        assert_eq!(
            notice_decision("v1.3.0", &prefs, 101),
            NoticeDecision::MutedByVersion
        );
    }

    #[test]
    fn prefs_round_trip_and_corrupt_files_read_as_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_prefs(dir.path()), UpdateNoticePrefs::default());

        assert!(dismiss_version(dir.path(), "v1.3.0"));
        assert_eq!(
            read_prefs(dir.path()).dismissed_version.as_deref(),
            Some("v1.3.0")
        );

        assert!(mark_notice_shown(dir.path(), 100));
        let prefs = read_prefs(dir.path());
        assert_eq!(prefs.dismissed_version.as_deref(), Some("v1.3.0"));
        assert_eq!(prefs.last_notice_day, Some(100));

        std::fs::write(dir.path().join(PREFS_FILE), "not json").unwrap();
        assert_eq!(read_prefs(dir.path()), UpdateNoticePrefs::default());
    }

    #[test]
    fn today_day_counts_epoch_days() {
        let day = 86_400 * 7 + 100;
        assert_eq!(
            today_day(std::time::UNIX_EPOCH + std::time::Duration::from_secs(day)),
            7
        );
        assert_eq!(today_day(std::time::UNIX_EPOCH), 0);
    }
}
