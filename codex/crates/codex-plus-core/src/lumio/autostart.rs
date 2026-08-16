//! 开机启动（Lumio 壳自身，不是官方 Codex）。默认开启（opt-out）：从未表达过
//! 偏好的用户在 bootstrap 时自动注册一次；用户关闭后偏好落盘，此后不再自动开启。
//!
//! 机制零新依赖：macOS 写用户级 LaunchAgent plist（launchd 拉起，无授权弹窗），
//! Windows 经 `reg.exe` 写 HKCU Run 值（与仓库既有平台集成一致，全程无 shell
//! 解析风险——参数走 `Command::args`）。开发直跑（cargo target 目录 / 非 .app
//! bundle）不支持注册，如实报 `PREFERENCE_LAUNCH_AT_LOGIN_UNSUPPORTED`。
//!
//! 应用被移动/重装到新路径后，残留注册仍指向旧路径（登录时拉起失效目标）；
//! bootstrap 对「偏好开着但注册失配」的机器重写注册指向当前 exe，注册被用户
//! 从系统侧移除的则保持移除（系统现状权威，不自动恢复）。

use std::path::{Path, PathBuf};

use super::product::{self, BUNDLE_IDENTIFIER};

pub const LAUNCH_AT_LOGIN_FAILED: &str = "PREFERENCE_LAUNCH_AT_LOGIN_FAILED";
pub const LAUNCH_AT_LOGIN_UNSUPPORTED: &str = "PREFERENCE_LAUNCH_AT_LOGIN_UNSUPPORTED";
const PREFERENCE_FILE: &str = "launch-at-login.json";
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn plist_content(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>Label</key><string>{BUNDLE_IDENTIFIER}</string>\n\
    <key>ProgramArguments</key>\n\
    <array><string>{exe}</string></array>\n\
    <key>RunAtLoad</key><true/>\n\
</dict>\n\
</plist>\n",
        exe = xml_escape(&exe.to_string_lossy()),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn registry_value_data(exe: &Path) -> String {
    format!("\"{}\"", exe.to_string_lossy())
}

/// `reg.exe` 参数构造（query/add/delete 三态）。参数全部经 `Command::args`
/// 传递，不经 shell 解析；`/d` 的值带引号整条传递，reg 自行去引号。
pub fn registry_args(mode: &str, exe: &Path) -> Vec<String> {
    let mut args = vec![mode.to_string(), RUN_SUBKEY.to_string()];
    match mode {
        "query" => args.extend(["/v".to_string(), BUNDLE_IDENTIFIER.to_string()]),
        "add" => args.extend([
            "/v".to_string(),
            BUNDLE_IDENTIFIER.to_string(),
            "/t".to_string(),
            "REG_SZ".to_string(),
            "/d".to_string(),
            registry_value_data(exe),
            "/f".to_string(),
        ]),
        "delete" => args.extend([
            "/v".to_string(),
            BUNDLE_IDENTIFIER.to_string(),
            "/f".to_string(),
        ]),
        _ => {}
    }
    args
}

pub fn macos_exe_is_bundled(exe: &Path) -> bool {
    exe.components()
        .any(|component| component.as_os_str().to_string_lossy().ends_with(".app"))
}

pub fn windows_exe_is_installed(exe: &Path) -> bool {
    // Windows 路径在非 Windows 测试宿主上不是合法分量结构，按分隔符切字符串判断
    // （与 app_paths 对 Windows 路径的处理同款）。
    let normalized = exe.to_string_lossy().replace('\\', "/");
    !normalized
        .split('/')
        .any(|component| component.eq_ignore_ascii_case("target"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    /// 从未表达过偏好：注册默认开启。
    Register,
    /// 偏好开着但注册指向旧路径（应用被移动/重装）：重写指向当前 exe。
    Realign,
    /// 偏好已关 / 注册与当前 exe 一致 / 注册被用户从系统侧移除：不动。
    Leave,
}

pub fn default_action(persisted: Option<bool>, registration_stale: bool) -> DefaultAction {
    match persisted {
        None => DefaultAction::Register,
        Some(true) if registration_stale => DefaultAction::Realign,
        _ => DefaultAction::Leave,
    }
}

fn preference_path(dir: &Path) -> PathBuf {
    dir.join(PREFERENCE_FILE)
}

pub fn read_pref(dir: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(preference_path(dir)).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("enabled")?
        .as_bool()
}

pub fn write_pref(dir: &Path, enabled: bool) -> bool {
    let Ok(text) = serde_json::to_string(&serde_json::json!({ "enabled": enabled })) else {
        return false;
    };
    std::fs::create_dir_all(dir).is_ok() && std::fs::write(preference_path(dir), text).is_ok()
}

/// 当前运行方式允许注册开机启动吗。开发直跑（cargo target / 非 .app bundle）
/// 注册会指向一个随构建失效的路径，宁可不支持。
pub fn autostart_supported() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        macos_exe_is_bundled(&exe)
    }
    #[cfg(windows)]
    {
        windows_exe_is_installed(&exe)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// 系统侧现状（权威）：macOS 看 LaunchAgent plist 是否精确指向当前 exe，
/// Windows 看 HKCU Run 值是否存在。用户从系统设置里手动移除会如实反映为关。
pub fn current() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
        else {
            return false;
        };
        macos_enabled(&launch_agents_dir(&home), &exe)
    }
    #[cfg(windows)]
    {
        run_registry("query")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// bootstrap 钩子：从未表达过偏好（且当前运行方式支持）就注册一次默认开启；
/// 偏好开着但注册指向旧路径（应用被移动/重装）时重对齐到当前 exe。注册失败
/// 不落偏好，下次启动重试；dev 构建不注册也不落偏好。
pub fn ensure_default_enabled() {
    let Some(state_dir) = product::state_dir() else {
        return;
    };
    if default_action(read_pref(&state_dir), registration_is_stale()) == DefaultAction::Leave {
        return;
    }
    let _ = set(true);
}

pub fn set(enabled: bool) -> Result<bool, &'static str> {
    if !autostart_supported() {
        return Err(LAUNCH_AT_LOGIN_UNSUPPORTED);
    }
    let applied = if enabled { enable() } else { disable() };
    if !applied {
        return Err(LAUNCH_AT_LOGIN_FAILED);
    }
    // 偏好只是「默认开启」的闸门；写失败不回滚系统状态（罕见，且系统现状是权威）。
    if let Some(state_dir) = product::state_dir() {
        write_pref(&state_dir, enabled);
    }
    Ok(enabled)
}

fn enable() -> bool {
    #[cfg(target_os = "macos")]
    {
        let (Some(home), Ok(exe)) = (
            directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            std::env::current_exe(),
        ) else {
            return false;
        };
        apply_macos(&launch_agents_dir(&home), &exe, true)
    }
    #[cfg(windows)]
    {
        run_registry("add")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

fn disable() -> bool {
    #[cfg(target_os = "macos")]
    {
        let (Some(home), Ok(exe)) = (
            directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            std::env::current_exe(),
        ) else {
            return false;
        };
        apply_macos(&launch_agents_dir(&home), &exe, false)
    }
    #[cfg(windows)]
    {
        run_registry("delete")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn launch_agents_dir(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents")
}

#[cfg(target_os = "macos")]
pub fn apply_macos(agents_dir: &Path, exe: &Path, enabled: bool) -> bool {
    let plist = agents_dir.join(format!("{BUNDLE_IDENTIFIER}.plist"));
    if enabled {
        std::fs::create_dir_all(agents_dir).is_ok()
            && std::fs::write(&plist, plist_content(exe)).is_ok()
    } else {
        // 关闭对「本来就没开」也返回成功：目标状态达成即成功。
        !plist.exists() || std::fs::remove_file(&plist).is_ok()
    }
}

#[cfg(target_os = "macos")]
pub fn macos_enabled(agents_dir: &Path, exe: &Path) -> bool {
    let Ok(content) =
        std::fs::read_to_string(agents_dir.join(format!("{BUNDLE_IDENTIFIER}.plist")))
    else {
        return false;
    };
    content == plist_content(exe)
}

/// plist 存在但内容与当前 exe 不符 = 注册失配（应用被移动/重装，launchd 还在
/// 拉旧路径）；plist 不存在（用户从系统侧移除）不算失配。
#[cfg(target_os = "macos")]
pub fn macos_registration_is_stale(agents_dir: &Path, exe: &Path) -> bool {
    let Ok(content) =
        std::fs::read_to_string(agents_dir.join(format!("{BUNDLE_IDENTIFIER}.plist")))
    else {
        return false;
    };
    content != plist_content(exe)
}

fn registration_is_stale() -> bool {
    #[cfg(target_os = "macos")]
    {
        let (Some(home), Ok(exe)) = (
            directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            std::env::current_exe(),
        ) else {
            return false;
        };
        macos_registration_is_stale(&launch_agents_dir(&home), &exe)
    }
    #[cfg(windows)]
    {
        windows_registration_is_stale()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

#[cfg(windows)]
fn run_registry(mode: &str) -> bool {
    run_registry_command(mode, false).is_some()
}

/// `mode=query` 时带出 stdout（解析值数据），其余模式丢弃输出只看退出码。
#[cfg(windows)]
fn run_registry_command(mode: &str, capture: bool) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let exe = std::env::current_exe().unwrap_or_default();
    let mut command = Command::new("reg.exe");
    command
        .args(registry_args(mode, &exe))
        .creation_flags(crate::windows_create_no_window());
    if !capture {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(windows)]
fn windows_registration_is_stale() -> bool {
    let Some(output) = run_registry_command("query", true) else {
        return false;
    };
    let exe = std::env::current_exe().unwrap_or_default();
    registry_query_indicates_stale(&output, &exe)
}

/// `reg query` 的 stdout 中本值的数据与当前 exe 不符 = 注册失配（应用被移动/
/// 重装）；值不存在（退出码非 0，根本拿不到输出）不算失配。
pub fn registry_query_indicates_stale(output: &str, exe: &Path) -> bool {
    let has_value = output
        .lines()
        .any(|line| line.contains(BUNDLE_IDENTIFIER) && line.contains("REG_SZ"));
    has_value && !output.contains(&registry_value_data(exe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plist_lists_the_bundle_identifier_and_current_exe() {
        let content = plist_content(Path::new(
            "/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex",
        ));

        assert!(
            content.contains("<string>games.lumio.codex</string>"),
            "{content}"
        );
        assert!(
            content.contains(
                "<string>/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex</string>"
            ),
            "{content}"
        );
        assert!(content.contains("<key>RunAtLoad</key>"), "{content}");
        assert!(content.contains("<true/>"), "{content}");
    }

    #[test]
    fn plist_escapes_xml_sensitive_characters_in_the_exe_path() {
        let content = plist_content(Path::new("/tmp/A&B<C.app/Contents/MacOS/x"));
        assert!(
            content.contains("/tmp/A&amp;B&lt;C.app/Contents/MacOS/x"),
            "{content}"
        );
        assert!(!content.contains("A&B"), "{content}");
    }

    #[test]
    fn registry_data_quotes_the_executable_path() {
        assert_eq!(
            registry_value_data(Path::new(r"C:\Program Files\Lumio Codex\Lumio Codex.exe")),
            r#""C:\Program Files\Lumio Codex\Lumio Codex.exe""#
        );
    }

    #[test]
    fn bundled_macos_builds_are_supported_and_cargo_runs_are_not() {
        assert!(macos_exe_is_bundled(Path::new(
            "/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex"
        )));
        assert!(!macos_exe_is_bundled(Path::new(
            "/Users/dev/Sites/lumio-codex/codex/apps/codex-plus-manager/src-tauri/target/debug/codex-plus-manager"
        )));
    }

    #[test]
    fn installed_windows_builds_are_supported_and_target_runs_are_not() {
        assert!(windows_exe_is_installed(Path::new(
            r"C:\Program Files\Lumio Codex\Lumio Codex.exe"
        )));
        assert!(windows_exe_is_installed(Path::new(
            r"C:\Users\dev\AppData\Local\Programs\Lumio Codex\Lumio Codex.exe"
        )));
        assert!(!windows_exe_is_installed(Path::new(
            r"C:\dev\lumio-codex\codex\apps\codex-plus-manager\src-tauri\target\release\codex-plus-manager.exe"
        )));
    }

    #[test]
    fn default_action_registers_realigns_and_leaves() {
        assert_eq!(default_action(None, false), DefaultAction::Register);
        assert_eq!(default_action(None, true), DefaultAction::Register);
        assert_eq!(default_action(Some(false), false), DefaultAction::Leave);
        assert_eq!(
            default_action(Some(false), true),
            DefaultAction::Leave,
            "用户关过就必须保持关，注册失配也不许自动重开"
        );
        assert_eq!(
            default_action(Some(true), false),
            DefaultAction::Leave,
            "偏好与系统注册一致就不需要动"
        );
        assert_eq!(
            default_action(Some(true), true),
            DefaultAction::Realign,
            "偏好开着但注册指向旧路径，要重写指向当前 exe"
        );
    }

    #[test]
    fn preference_round_trips_through_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_pref(dir.path()), None, "无文件 = 未表达过偏好");

        assert!(write_pref(dir.path(), true));
        assert_eq!(read_pref(dir.path()), Some(true));

        assert!(write_pref(dir.path(), false));
        assert_eq!(read_pref(dir.path()), Some(false));
    }

    #[test]
    fn corrupt_preference_reads_as_unstated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PREFERENCE_FILE), "not json").unwrap();
        assert_eq!(read_pref(dir.path()), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_apply_and_query_round_trip_through_the_agents_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("LaunchAgents");
        let exe = PathBuf::from("/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex");

        assert!(!macos_enabled(&agents, &exe));
        assert!(apply_macos(&agents, &exe, true));
        assert!(macos_enabled(&agents, &exe));

        assert!(apply_macos(&agents, &exe, false));
        assert!(!macos_enabled(&agents, &exe));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_query_rejects_a_plist_pointing_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        let other = plist_content(Path::new("/Applications/Other.app/Contents/MacOS/Other"));
        std::fs::write(agents.join("games.lumio.codex.plist"), other).unwrap();

        assert!(!macos_enabled(
            &agents,
            Path::new("/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex")
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_registration_is_stale_only_when_a_foreign_plist_exists() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("LaunchAgents");
        let exe = PathBuf::from("/Applications/Lumio Codex.app/Contents/MacOS/Lumio Codex");

        assert!(
            !macos_registration_is_stale(&agents, &exe),
            "无 plist（用户从系统侧移除）失配为假，尊重现状"
        );

        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("games.lumio.codex.plist"),
            plist_content(Path::new("/Applications/Old.app/Contents/MacOS/Old")),
        )
        .unwrap();
        assert!(
            macos_registration_is_stale(&agents, &exe),
            "应用移动/重装后 plist 仍指向旧路径，需要重对齐"
        );

        std::fs::write(agents.join("games.lumio.codex.plist"), plist_content(&exe)).unwrap();
        assert!(!macos_registration_is_stale(&agents, &exe));
    }

    #[test]
    fn registry_query_output_signals_stale_only_when_the_data_differs() {
        let exe = Path::new(r"C:\Apps\Lumio Codex\Lumio Codex.exe");

        let stale = format!(
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n\
             \x20   games.lumio.codex    REG_SZ    {}\n",
            r#""C:\Old\Lumio Codex.exe""#
        );
        assert!(registry_query_indicates_stale(&stale, exe));

        let current = format!(
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n\
             \x20   games.lumio.codex    REG_SZ    {}\n",
            registry_value_data(exe)
        );
        assert!(!registry_query_indicates_stale(&current, exe));

        assert!(
            !registry_query_indicates_stale("系统找不到指定的注册表项或值。", exe),
            "值不存在（用户从系统侧移除）失配为假"
        );
    }

    #[test]
    fn registry_args_query_add_and_delete_the_owned_value() {
        let exe = Path::new(r"C:\Program Files\Lumio Codex\Lumio Codex.exe");

        assert_eq!(
            registry_args("query", exe),
            vec!["query", RUN_SUBKEY, "/v", "games.lumio.codex"]
        );
        assert_eq!(
            registry_args("add", exe),
            vec![
                "add",
                RUN_SUBKEY,
                "/v",
                "games.lumio.codex",
                "/t",
                "REG_SZ",
                "/d",
                r#""C:\Program Files\Lumio Codex\Lumio Codex.exe""#,
                "/f",
            ]
        );
        assert_eq!(
            registry_args("delete", exe),
            vec!["delete", RUN_SUBKEY, "/v", "games.lumio.codex", "/f"]
        );
    }
}
