use super::sources::planned_sources;
use super::{HostPlatform, InstallDecision, InstallRoute, PlanInput};

pub fn plan_official_app(input: PlanInput<'_>) -> Result<InstallDecision, String> {
    if let Some(path) = input.detected_app {
        return Ok(InstallDecision::AlreadyInstalled {
            path: path.to_path_buf(),
        });
    }
    if !input.online {
        return Ok(InstallDecision::NeedsNetwork);
    }

    let route = match input.platform {
        HostPlatform::Windows if input.windows_sideload_ok == Some(false) => {
            InstallRoute::WindowsPortable
        }
        HostPlatform::Windows => InstallRoute::WindowsSideload,
        HostPlatform::Macos => InstallRoute::MacosCopyApp,
    };

    Ok(InstallDecision::Ready {
        platform: input.platform,
        arch: input.arch,
        route,
        sources: planned_sources(input.platform, input.arch),
    })
}

#[cfg(test)]
mod tests {
    use super::super::{
        HostArch, HostPlatform, InstallDecision, InstallRoute, PlanInput, SourceKind,
    };
    use super::plan_official_app;
    use std::path::Path;

    #[test]
    fn already_installed_does_not_schedule_a_download() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::X64,
            detected_app: Some(Path::new(r"C:\Apps\ChatGPT.exe")),
            online: true,
            windows_sideload_ok: Some(true),
        })
        .unwrap();
        assert!(matches!(decision, InstallDecision::AlreadyInstalled { .. }));
    }

    #[test]
    fn offline_without_an_app_needs_network() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::X64,
            detected_app: None,
            online: false,
            windows_sideload_ok: None,
        })
        .unwrap();
        assert_eq!(decision, InstallDecision::NeedsNetwork);
    }

    #[test]
    fn mirror_then_official_for_windows_x64() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::X64,
            detected_app: None,
            online: true,
            windows_sideload_ok: Some(true),
        })
        .unwrap();
        let InstallDecision::Ready { sources, route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::WindowsSideload);
        assert_eq!(sources[0].kind, SourceKind::Mirror);
        assert!(sources[0].url.ends_with("/latest/win-x64"));
        assert_eq!(sources[1].kind, SourceKind::Official);
        assert_eq!(sources[1].url, "store:9PLM9XGG6VKS"); // 官方备用占位符，真实 FE3 URL 执行时再解析
    }

    #[test]
    fn capability_failure_selects_portable_without_changing_sources() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::Arm64,
            detected_app: None,
            online: true,
            windows_sideload_ok: Some(false),
        })
        .unwrap();
        let InstallDecision::Ready { route, sources, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::WindowsPortable);
        assert!(sources[0].url.ends_with("/latest/win-arm64"));
    }

    #[test]
    fn macos_arm64_copies_app_from_mirror_then_official() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Macos,
            arch: HostArch::Arm64,
            detected_app: None,
            online: true,
            windows_sideload_ok: None,
        })
        .unwrap();
        let InstallDecision::Ready { sources, route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::MacosCopyApp);
        assert_eq!(sources[0].kind, SourceKind::Mirror);
        assert!(sources[0].url.ends_with("/latest/mac-arm64"));
        assert_eq!(sources[1].kind, SourceKind::Official);
        assert!(sources[1].url.ends_with("/codex-app-prod/Codex.dmg"));
    }

    #[test]
    fn macos_x64_copies_app_from_mirror_then_official() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Macos,
            arch: HostArch::X64,
            detected_app: None,
            online: true,
            windows_sideload_ok: None,
        })
        .unwrap();
        let InstallDecision::Ready { sources, route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::MacosCopyApp);
        assert_eq!(sources[0].kind, SourceKind::Mirror);
        assert!(sources[0].url.ends_with("/latest/mac-intel"));
        assert_eq!(sources[1].kind, SourceKind::Official);
        assert!(
            sources[1]
                .url
                .ends_with("/codex-app-prod/Codex-latest-x64.dmg")
        );
    }

    #[test]
    fn windows_without_a_sideload_probe_still_prefers_sideload() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::X64,
            detected_app: None,
            online: true,
            windows_sideload_ok: None,
        })
        .unwrap();
        let InstallDecision::Ready { route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::WindowsSideload);
    }
}
