use std::path::Path;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 页脚 / 设置里给用户看的版本。CI 会把 `LUMIO_PACKAGE_VERSION` 打成
/// `1.2.46-internal-95` 这类与官网下载卡一致的标签；本地开发没有该环境变量
/// 时退回 Cargo 版本。
pub fn resolve_display_version<'a>(
    package_version: Option<&'a str>,
    cargo_version: &'a str,
) -> &'a str {
    resolve_display_version_with_bundle(package_version, cargo_version, None)
}

/// 已安装的 `.app` 的 `CFBundleShortVersionString` 比 Cargo 版本更接近用户看到的
/// 安装包；CI 标签仍优先，保证与官网下载卡一致。
pub fn resolve_display_version_with_bundle<'a>(
    package_version: Option<&'a str>,
    cargo_version: &'a str,
    bundle_version: Option<&'a str>,
) -> &'a str {
    nonempty(package_version)
        .or_else(|| nonempty(bundle_version))
        .unwrap_or(cargo_version)
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// 当前进程若在我们自己的 macOS `.app` 里，读 Info.plist 的短版本。
pub fn running_bundle_short_version() -> Option<String> {
    macos_bundle_short_version(&std::env::current_exe().ok()?)
}

pub fn macos_bundle_short_version(exe: &Path) -> Option<String> {
    let mut current = exe.parent()?;
    loop {
        if current.extension().and_then(|ext| ext.to_str()) == Some("app") {
            return macos_app_short_version(current);
        }
        current = current.parent()?;
    }
}

fn macos_app_short_version(app_dir: &Path) -> Option<String> {
    let plist = std::fs::read_to_string(app_dir.join("Contents").join("Info.plist")).ok()?;
    let identifier = plist_string_value(&plist, "CFBundleIdentifier")?;
    if identifier != crate::lumio::product::BUNDLE_IDENTIFIER {
        return None;
    }
    plist_string_value(&plist, "CFBundleShortVersionString")
}

fn plist_string_value(plist: &str, key: &str) -> Option<String> {
    let (_, after_key) = plist.split_once(&format!("<key>{key}</key>"))?;
    let (_, after_string_open) = after_key.split_once("<string>")?;
    let (value, _) = after_string_open.split_once("</string>")?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        VERSION, macos_bundle_short_version, resolve_display_version,
        resolve_display_version_with_bundle,
    };

    #[test]
    fn exposes_workspace_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn display_version_prefers_the_ci_package_label() {
        assert_eq!(
            resolve_display_version(Some("1.2.46-internal-95"), "1.2.46"),
            "1.2.46-internal-95"
        );
    }

    #[test]
    fn display_version_falls_back_to_cargo_when_the_package_label_is_missing() {
        assert_eq!(resolve_display_version(None, "1.2.46"), "1.2.46");
        assert_eq!(resolve_display_version(Some(""), "1.2.46"), "1.2.46");
        assert_eq!(resolve_display_version(Some("   "), "1.2.46"), "1.2.46");
    }

    #[test]
    fn display_version_uses_the_installed_bundle_when_the_ci_label_is_missing() {
        // 内测包 Info.plist 是 `0.0.0-internal-38`，未打 LUMIO_PACKAGE_VERSION 时
        // 页脚不能掉回 Cargo 的 1.2.46。
        assert_eq!(
            resolve_display_version_with_bundle(None, "1.2.46", Some("0.0.0-internal-38")),
            "0.0.0-internal-38"
        );
        assert_eq!(
            resolve_display_version_with_bundle(Some("   "), "1.2.46", Some("0.0.0-internal-38")),
            "0.0.0-internal-38"
        );
    }

    #[test]
    fn display_version_still_prefers_the_ci_package_label_over_the_bundle() {
        assert_eq!(
            resolve_display_version_with_bundle(
                Some("1.2.46-internal-95"),
                "1.2.46",
                Some("1.2.46"),
            ),
            "1.2.46-internal-95"
        );
    }

    #[test]
    fn macos_bundle_short_version_reads_our_app_plist() {
        let root = tempfile::tempdir().unwrap();
        let macos = root
            .path()
            .join("BestCodex.app")
            .join("Contents")
            .join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("lumio-codex");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(
            root.path().join("BestCodex.app").join("Contents").join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>games.lumio.codex</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.0-internal-38</string>
</dict>
</plist>
"#,
        )
        .unwrap();

        assert_eq!(
            macos_bundle_short_version(&exe).as_deref(),
            Some("0.0.0-internal-38")
        );
    }

    #[test]
    fn macos_bundle_short_version_ignores_a_foreign_app() {
        let root = tempfile::tempdir().unwrap();
        let macos = root.path().join("Codex.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("Codex");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(
            root.path()
                .join("Codex.app")
                .join("Contents")
                .join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.openai.codex</string>
  <key>CFBundleShortVersionString</key>
  <string>1.2.3</string>
</dict>
</plist>
"#,
        )
        .unwrap();

        assert_eq!(macos_bundle_short_version(&exe), None);
    }
}
