use std::path::PathBuf;

pub const PRODUCT_NAME: &str = "Lumio Codex";
pub const BUNDLE_IDENTIFIER: &str = "games.lumio.codex";
pub const API_BASE_URL: &str = "https://api.lumio.games/";
pub const DESKTOP_KEY_NAME: &str = "Lumio Codex Desktop";

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
