use std::path::PathBuf;

pub const PRODUCT_NAME: &str = "Lumio Codex";
pub const BUNDLE_IDENTIFIER: &str = "games.lumio.codex";
pub const API_BASE_URL: &str = "https://api.lumio.games/";
/// 官网与下载引导站点。账户相关网页（重置密码等）仍走 `API_BASE_URL`。
pub const SITE_BASE_URL: &str = "https://lumio.games";
/// 充值页路径：相对官网同源绝对路径，禁止跨域。
pub const PAYMENT_PATH: &str = "/payment";
pub const RELEASES_PAGE_URL: &str = "https://github.com/Go1c/lumio-codex/releases";
pub const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Go1c/lumio-codex/releases/latest";
pub const DESKTOP_KEY_NAME: &str = "Lumio Codex Desktop";

pub fn payment_url() -> String {
    format!(
        "{}{}",
        SITE_BASE_URL.trim_end_matches('/'),
        if PAYMENT_PATH.starts_with('/') {
            PAYMENT_PATH
        } else {
            "/payment"
        }
    )
}

pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("games", "Lumio", PRODUCT_NAME)
}

pub fn state_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_local_dir().join("state"))
}

pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.cache_dir().to_path_buf())
}

pub fn log_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_local_dir().join("logs"))
}
