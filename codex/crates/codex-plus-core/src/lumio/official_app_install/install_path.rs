//! 用户自选安装目录的持久化。默认路线（MSIX / /Applications）由自动探测覆盖，不写
//! 这个文件；只有「用户选了目录」这种自动探测找不到的安装才需要记住落点，否则
//! 重启后会误判未安装并触发重复安装（D-23，与 D-3 手选丢失同族）。

use std::path::{Path, PathBuf};

const INSTALL_PATH_FILE: &str = "official-app-path.json";

pub fn saved_install_path(state_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(state_dir.join(INSTALL_PATH_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = value.get("installPath")?.as_str()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

pub fn save_install_path(state_dir: &Path, path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|_| super::INSTALL_FAILED.to_string())?;
    let payload = serde_json::json!({ "installPath": path.to_string_lossy() });
    let target = state_dir.join(INSTALL_PATH_FILE);
    crate::settings::atomic_write(&target, payload.to_string().as_bytes())
        .map_err(|_| super::INSTALL_FAILED.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips_the_path() {
        let root = tempfile::tempdir().unwrap();
        save_install_path(root.path(), Path::new(r"D:\MyApps\Codex")).unwrap();
        assert_eq!(
            saved_install_path(root.path()),
            Some(PathBuf::from(r"D:\MyApps\Codex"))
        );
    }

    #[test]
    fn blank_or_corrupt_or_missing_records_read_as_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(saved_install_path(root.path()), None);
        std::fs::write(root.path().join("official-app-path.json"), "not json").unwrap();
        assert_eq!(saved_install_path(root.path()), None);
        std::fs::write(
            root.path().join("official-app-path.json"),
            r#"{"installPath":"   "}"#,
        )
        .unwrap();
        assert_eq!(saved_install_path(root.path()), None);
    }
}
