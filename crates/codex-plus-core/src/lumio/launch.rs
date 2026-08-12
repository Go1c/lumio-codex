//! 启动官方 Codex。这条路径与 `launcher::launch_and_inject` 完全独立：
//! 不开远程调试端口、不注入、不起 watchdog——Lumio 只是把官方应用原样拉起来。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(not(target_os = "macos"))]
use crate::app_paths::build_codex_executable;
use crate::app_paths::normalize_codex_app_path;

const APP_INVALID: &str = "CODEX_APP_INVALID";
const LAUNCH_FAILED: &str = "CODEX_LAUNCH_FAILED";

pub fn build_launch_command(app_dir: &Path) -> Result<(String, Vec<String>), String> {
    #[cfg(target_os = "macos")]
    {
        Ok((
            "open".to_string(),
            vec!["-a".to_string(), app_dir.to_string_lossy().into_owned()],
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let executable = build_codex_executable(app_dir);
        if !executable.is_file() {
            return Err(APP_INVALID.to_string());
        }
        Ok((executable.to_string_lossy().into_owned(), Vec::new()))
    }
}

pub fn launch_official_codex(app_dir: &Path) -> Result<(), String> {
    let (program, args) = build_launch_command(app_dir)?;
    spawn_detached(&program, &args)
}

pub fn validate_selected_app(path: &Path) -> Result<PathBuf, String> {
    // `normalize_codex_app_path` 只看路径形状（任何 `*.app` 都通过），
    // 用户手选的路径还得真实存在才算数。
    let normalized = normalize_codex_app_path(path).ok_or_else(|| APP_INVALID.to_string())?;
    if !normalized.exists() {
        return Err(APP_INVALID.to_string());
    }
    Ok(normalized)
}

pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(APP_INVALID.to_string());
    }

    #[cfg(target_os = "macos")]
    {
        spawn_detached("open", &[url.to_string()])
    }

    #[cfg(windows)]
    {
        spawn_detached(
            "cmd",
            &[
                "/C".to_string(),
                "start".to_string(),
                String::new(),
                url.to_string(),
            ],
        )
    }

    #[cfg(all(not(target_os = "macos"), not(windows)))]
    {
        spawn_detached("xdg-open", &[url.to_string()])
    }
}

fn spawn_detached(program: &str, args: &[String]) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::windows_integration::CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| LAUNCH_FAILED.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launches_the_bundle_through_open_without_debugging_flags() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Codex.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();

        let (program, args) = build_launch_command(&app).unwrap();

        assert_eq!(program, "open");
        assert_eq!(
            args,
            vec!["-a".to_string(), app.to_string_lossy().into_owned()]
        );
        assert!(!args.iter().any(|arg| arg.contains("remote-debugging-port")));
        assert!(!args.iter().any(|arg| arg.contains("remote-allow-origins")));
    }

    #[test]
    fn the_launch_command_never_carries_injection_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Codex.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();

        if let Ok((_, args)) = build_launch_command(&app) {
            for arg in &args {
                assert!(
                    !arg.contains("remote-debugging"),
                    "injection flag leaked: {arg}"
                );
                assert!(!arg.contains("inspect"), "injection flag leaked: {arg}");
            }
        }
    }

    #[test]
    fn a_nonexistent_path_is_rejected_as_an_invalid_app() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Nope.app");

        assert_eq!(
            validate_selected_app(&missing).unwrap_err(),
            "CODEX_APP_INVALID"
        );
    }

    #[test]
    fn a_plain_directory_is_rejected_as_an_invalid_app() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("just-a-folder");
        std::fs::create_dir_all(&plain).unwrap();

        assert_eq!(
            validate_selected_app(&plain).unwrap_err(),
            "CODEX_APP_INVALID"
        );
    }

    #[test]
    fn only_http_and_https_urls_may_be_opened() {
        for rejected in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "",
        ] {
            assert_eq!(
                open_in_browser(rejected).unwrap_err(),
                "CODEX_APP_INVALID",
                "{rejected}"
            );
        }
    }
}
