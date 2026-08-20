pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 页脚 / 设置里给用户看的版本。CI 会把 `LUMIO_PACKAGE_VERSION` 打成
/// `1.2.46-internal-95` 这类与官网下载卡一致的标签；本地开发没有该环境变量
/// 时退回 Cargo 版本。
pub fn resolve_display_version<'a>(
    package_version: Option<&'a str>,
    cargo_version: &'a str,
) -> &'a str {
    match package_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value,
        None => cargo_version,
    }
}

#[cfg(test)]
mod tests {
    use super::{VERSION, resolve_display_version};

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
}
