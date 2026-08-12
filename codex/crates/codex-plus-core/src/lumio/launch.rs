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
        let (program, args) = windows_browser_command(url);
        spawn_detached(&program, &args)
    }

    #[cfg(all(not(target_os = "macos"), not(windows)))]
    {
        spawn_detached("xdg-open", &[url.to_string()])
    }
}

/// Windows 上打开 URL 不经 `cmd`：`cmd /C start "" <url>` 会把 URL 里的 `&` `|` `^`
/// 解释成命令分隔符，而 Rust std 只按 CommandLineToArgvW 的规则加引号、不按 cmd 的规则
/// 转义。改用 `rundll32 url.dll,FileProtocolHandler`——由 CreateProcess 直接拉起，
/// 整条路径上没有 shell，也就不需要维护一份元字符黑名单。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_browser_command(url: &str) -> (String, Vec<String>) {
    (
        "rundll32".to_string(),
        vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()],
    )
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

    /// `cmd /C start "" <url>` 把 URL 里的 `&` `|` `^` 当成命令分隔符，而 Rust std
    /// 不按 cmd 的解析规则转义参数——`lumio_open_browser` 在 IPC 白名单上，这个洞
    /// 不该留。这条断言与平台无关：所有平台都必须构造出一条不经 shell 的命令。
    #[test]
    fn the_windows_browser_command_does_not_go_through_cmd() {
        let url = "https://lumio.games/pay?session=1&plan=pro|tier^2";
        let (program, args) = windows_browser_command(url);

        assert_eq!(program, "rundll32");
        assert_eq!(
            args,
            vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()]
        );
        for token in std::iter::once(&program).chain(args.iter()) {
            assert!(
                !token.eq_ignore_ascii_case("cmd") && !token.eq_ignore_ascii_case("cmd.exe"),
                "the url still reaches a command interpreter: {token}"
            );
            assert_ne!(token, "start");
        }
        // URL 必须整体作为一个参数传下去，不能被拆开或与别的 token 拼接。
        assert_eq!(args.iter().filter(|arg| arg.contains(url)).count(), 1);
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
