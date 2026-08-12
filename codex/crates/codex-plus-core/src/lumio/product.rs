use std::path::PathBuf;

pub const PRODUCT_NAME: &str = "Lumio Codex";
pub const BUNDLE_IDENTIFIER: &str = "games.lumio.codex";
pub const API_BASE_URL: &str = "https://api.lumio.games/";
/// 产品站与下载引导站点（营销 / 下载）。充值、支持、重置密码不走这里。
///
/// 统一门户是 `lumiogame.com`，Codex 的产品站挂在它的子域上；账号与充值都在
/// Sub2API（`API_BASE_URL`），改这个常量不影响存量客户端的接口地址。
pub const SITE_BASE_URL: &str = "https://codex.lumiogame.com";
/// 充值页路径：相对 `API_BASE_URL`（`https://api.lumio.games/purchase`），禁止挂到官网。
pub const PAYMENT_PATH: &str = "/purchase";
pub const RELEASES_PAGE_URL: &str = "https://github.com/Go1c/lumio-codex/releases";
pub const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Go1c/lumio-codex/releases/latest";
pub const DESKTOP_KEY_NAME: &str = "Lumio Codex Desktop";

pub fn payment_url() -> String {
    format!(
        "{}{}",
        API_BASE_URL.trim_end_matches('/'),
        if PAYMENT_PATH.starts_with('/') {
            PAYMENT_PATH
        } else {
            "/purchase"
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
