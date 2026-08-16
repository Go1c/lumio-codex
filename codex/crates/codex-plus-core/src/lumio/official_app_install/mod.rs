use std::path::{Path, PathBuf};

mod download;
mod install_path;
mod macos;
mod plan;
mod progress;
mod sources;
mod verify;
mod windows;
mod windows_store;

pub use download::{DownloadProgress, download_to_cache, redact_url};
pub use macos::{
    choose_macos_dest, install_macos_from_dmg, interpret_codesign_output, user_applications,
    verify_macos_team_id,
};
pub use plan::plan_official_app;
pub use progress::{current_status, phase_kebab, request_cancel};
pub use sources::{
    MIRROR_CHECKSUMS_URL, MIRROR_MAC_ARM64, MIRROR_MAC_X64, MIRROR_MANIFEST_URL, MIRROR_WIN_ARM64,
    MIRROR_WIN_X64, MirrorPayload, OFFICIAL_MAC_ARM64, OFFICIAL_MAC_X64, OPENAI_MAC_TEAM_ID,
    OPENAI_MARKETPLACE_SUBJECT, PORTABLE_REL, STORE_PRODUCT_ID, parse_mirror_manifest,
    planned_sources,
};
pub use verify::verify_sha256;
pub use windows::{
    AuthenticodeVerdict, CapabilityCheck, WinCapabilityReport, authenticode_pin_ok,
    authenticode_route_decision, authenticode_verdict, install_windows_portable,
    install_windows_sideload, parse_appx_application_executable, probe_windows_sideload,
    probe_windows_sideload_with,
};
pub use windows_store::resolve_store_msix_url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Idle,
    Planning,
    Downloading,
    Verifying,
    Installing,
    Detecting,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStatus {
    pub phase: InstallPhase,
    /// 仅内部/测试用：download / verify / install。前端映射成人话，不直接上屏内部词。
    pub stage: Option<&'static str>,
    pub bytes_downloaded: Option<u64>,
    pub bytes_total: Option<u64>,
    pub error_code: Option<String>,
    pub installed_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Windows,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostArch {
    X64,
    Arm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallRoute {
    WindowsSideload,
    WindowsPortable,
    MacosCopyApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Mirror,
    Official,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSource {
    pub kind: SourceKind,
    /// 决定缓存文件扩展名（.msix / .dmg）——裸文件名会让 Windows 的签名与
    /// 部署工具认不出包格式（D-21）。
    pub platform: HostPlatform,
    pub url: String,
    pub sha256: Option<String>,
    /// 镜像 v5 撤掉 SHA256SUMS 后唯一的完整性线索；已知则下载后核对尺寸。
    pub expected_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallDecision {
    AlreadyInstalled {
        path: PathBuf,
    },
    NeedsNetwork,
    Ready {
        platform: HostPlatform,
        arch: HostArch,
        route: InstallRoute,
        sources: Vec<PackageSource>,
    },
}

pub struct PlanInput<'a> {
    pub platform: HostPlatform,
    pub arch: HostArch,
    pub detected_app: Option<&'a Path>,
    pub online: bool,
    pub windows_sideload_ok: Option<bool>,
    /// 用户选择的安装目录：Windows 上强制便携路线（MSIX 装哪由系统管，选目录只能
    /// 兑现到便携解压），macOS 上作为 .app 拷贝目标由安装层消费。
    pub destination: Option<&'a Path>,
}

const DOWNLOAD_FAILED: &str = "CODEX_APP_DOWNLOAD_FAILED";
const VERIFY_FAILED: &str = "CODEX_APP_VERIFY_FAILED";
const INSTALL_FAILED: &str = "CODEX_APP_INSTALL_FAILED";

pub struct OfficialAppInstallRequest<'a> {
    pub plan: PlanInput<'a>,
    pub detect: &'a dyn Fn() -> Option<PathBuf>,
    pub download: &'a mut dyn FnMut(&PackageSource) -> Result<PathBuf, String>,
    /// 带路由：Windows 预检不可用时只有侧载路线可以放行给系统部署验证（D-21）。
    pub verify: &'a dyn Fn(&Path, &PackageSource, InstallRoute) -> Result<(), String>,
    pub install: &'a mut dyn FnMut(&Path, InstallRoute) -> Result<PathBuf, String>,
    /// 成功上报前的收尾钩子（当前用途：持久化自选目录，D-24）。拿到的是最终
    /// 安装路径；取消 / 半失败不进这里。
    pub write_config: &'a mut dyn FnMut(&Path),
}

/// Injectable install pipeline. Production [`start_official_app_install`] wires the live adapters.
pub fn run_official_app_install(request: OfficialAppInstallRequest<'_>) -> Result<PathBuf, String> {
    progress::set_planning();
    if cancelled_download() {
        return Err(DOWNLOAD_FAILED.to_string());
    }

    let from_detect = (request.detect)();
    let from_plan = request.plan.detected_app.map(Path::to_path_buf);
    let detected = from_detect.or(from_plan);
    let plan = PlanInput {
        platform: request.plan.platform,
        arch: request.plan.arch,
        detected_app: detected.as_deref(),
        online: request.plan.online,
        windows_sideload_ok: request.plan.windows_sideload_ok,
        destination: request.plan.destination,
    };

    match plan_official_app(plan)? {
        InstallDecision::AlreadyInstalled { path } => {
            progress::set_succeeded(path.clone());
            Ok(path)
        }
        InstallDecision::NeedsNetwork => fail_download(),
        InstallDecision::Ready { route, sources, .. } => {
            if cancelled_download() {
                return Err(DOWNLOAD_FAILED.to_string());
            }
            progress::set_phase(InstallPhase::Downloading, Some("download"));
            let mut last_error = DOWNLOAD_FAILED.to_string();
            for source in &sources {
                if cancelled_download() {
                    return Err(DOWNLOAD_FAILED.to_string());
                }
                match (request.download)(source) {
                    Ok(package) => {
                        progress::set_phase(InstallPhase::Verifying, Some("verify"));
                        if let Err(err) = (request.verify)(&package, source, route) {
                            if err == VERIFY_FAILED || cancelled_download() {
                                return fail_with(err);
                            }
                            last_error = err;
                            continue;
                        }
                        if cancelled_download() {
                            return Err(DOWNLOAD_FAILED.to_string());
                        }
                        progress::set_phase(InstallPhase::Installing, Some("install"));
                        match (request.install)(&package, route) {
                            Ok(installed) => {
                                progress::set_phase(InstallPhase::Detecting, Some("detect"));
                                let path = (request.detect)().unwrap_or(installed);
                                (request.write_config)(&path);
                                // 安装包装完即删；失败路径不进这里，包留给重试。
                                let _ = std::fs::remove_file(&package);
                                progress::set_succeeded(path.clone());
                                return Ok(path);
                            }
                            Err(err) => return fail_with(err),
                        }
                    }
                    Err(err) if err == VERIFY_FAILED => return fail_with(err),
                    Err(err) => {
                        if cancelled_download() {
                            return Err(err);
                        }
                        last_error = err;
                    }
                }
            }
            if cancelled_download() {
                return Err(last_error);
            }
            fail_with(last_error)
        }
    }
}

pub async fn start_official_app_install() -> Result<PathBuf, String> {
    start_official_app_install_with(None, None).await
}

pub async fn start_official_app_install_with(
    session_app: Option<PathBuf>,
    destination: Option<PathBuf>,
) -> Result<PathBuf, String> {
    let detected = detect_existing_app(session_app.as_deref());
    let platform = current_host_platform()?;
    let arch = current_host_arch();
    let windows_sideload_ok = match platform {
        HostPlatform::Windows => Some(probe_windows_sideload()),
        HostPlatform::Macos => None,
    };
    if let Some(path) = detected.clone() {
        progress::set_succeeded(path.clone());
        return Ok(path);
    }

    let cache = crate::lumio::product::cache_dir().ok_or_else(|| DOWNLOAD_FAILED.to_string())?;
    std::fs::create_dir_all(&cache).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    std::fs::create_dir_all(cache.join("official-app")).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    prepare_destination(destination.as_deref())?;

    let mirror_payload = fetch_mirror_payload(platform, arch).await;
    let session_for_detect = session_app.clone();

    run_official_app_install(OfficialAppInstallRequest {
        plan: PlanInput {
            platform,
            arch,
            detected_app: detected.as_deref(),
            online: true,
            windows_sideload_ok,
            destination: destination.as_deref(),
        },
        detect: &|| detect_existing_app(session_for_detect.as_deref()),
        download: &mut |source| live_download(source, arch, mirror_payload.as_ref(), &cache),
        verify: &|path, source, route| live_verify(path, source, platform, route),
        install: &mut |path, route| live_install(path, route, destination.as_deref()),
        write_config: &mut |path| {
            // 用户选了目录的安装在这里记住落点（最终安装路径，平台无关），重启后
            // detect 才找得到。必须在 succeeded 上报之前完成：前端一看到成功就
            // 发起启动，晚于此的落盘会让启动撞上「还没记住」的窗口（D-24）。
            if destination.is_some() {
                if let Some(state) = crate::lumio::product::state_dir() {
                    let _ = install_path::save_install_path(&state, path);
                }
            }
        },
    })
}

/// Start the install on the current Tokio runtime and return immediately.
pub fn begin_background_install(
    session_app: Option<PathBuf>,
    destination: Option<PathBuf>,
) -> Result<(), String> {
    if !progress::try_begin_job() {
        return Ok(());
    }
    progress::prepare_new_job();
    progress::set_planning();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        progress::end_job();
        return Err(INSTALL_FAILED.to_string());
    };
    handle.spawn(async move {
        struct JobGuard;
        impl Drop for JobGuard {
            fn drop(&mut self) {
                progress::end_job();
            }
        }
        let _guard = JobGuard;
        let result = start_official_app_install_with(session_app, destination).await;
        if let Err(code) = result {
            note_background_failure(code);
        }
    });
    Ok(())
}

/// 主管线外的提前失败（缓存目录不可写、平台不支持）不会经过 `fail_with`，
/// 状态会停在 planning——按钮永远忙、失败无反馈（D-20）。只升级仍停在
/// planning 的状态，绝不覆盖主管线已写下的 failed / cancelled。
fn note_background_failure(code: String) {
    progress::update_status(|status| {
        if status.phase == InstallPhase::Planning {
            status.phase = InstallPhase::Failed;
            status.error_code = Some(code);
        }
    });
}

pub fn detect_existing_app(session_app: Option<&Path>) -> Option<PathBuf> {
    detect_existing_app_with(session_app, crate::lumio::product::state_dir().as_deref())
}

/// `state_dir` 注入仅为测试隔离；生产 wrapper 传真实状态目录。
pub fn detect_existing_app_with(
    session_app: Option<&Path>,
    state_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = session_app {
        if let Some(valid) = valid_manual_app(path) {
            return Some(valid);
        }
    }
    // 用户自选目录优先于自动扫描；保存的是安装时的最终路径，失效（卸载/移动）则
    // 原样回落自动探测，绝不比不保存时更糟（D-23）。
    if let Some(saved) = state_dir.and_then(install_path::saved_install_path) {
        if let Some(valid) = valid_manual_app(&saved) {
            return Some(valid);
        }
    }
    crate::app_paths::resolve_codex_app_dir(None)
        .or_else(crate::app_paths::find_standalone_codex_app_dir)
}

pub fn manual_path_still_valid(path: &Path) -> bool {
    valid_manual_app(path).is_some()
}

pub fn note_already_installed(path: PathBuf) {
    progress::set_succeeded(path);
}

fn valid_manual_app(path: &Path) -> Option<PathBuf> {
    let normalized = crate::app_paths::normalize_codex_app_path(path)?;
    normalized.exists().then_some(normalized)
}

fn current_host_platform() -> Result<HostPlatform, String> {
    match std::env::consts::OS {
        "windows" => Ok(HostPlatform::Windows),
        "macos" => Ok(HostPlatform::Macos),
        _ => Err(INSTALL_FAILED.to_string()),
    }
}

fn current_host_arch() -> HostArch {
    match std::env::consts::ARCH {
        "aarch64" => HostArch::Arm64,
        _ => HostArch::X64,
    }
}

fn live_download(
    source: &PackageSource,
    arch: HostArch,
    mirror_payload: Option<&MirrorPayload>,
    cache: &Path,
) -> Result<PathBuf, String> {
    let mut source = source.clone();
    if source.kind == SourceKind::Mirror {
        if let Some(payload) = mirror_payload {
            if source.sha256.is_none() {
                source.sha256 = payload.sha256.clone();
            }
            if source.expected_bytes.is_none() {
                source.expected_bytes = payload.content_length;
            }
        }
    }
    if source.url.starts_with("store:") {
        let resolved = resolve_official_store_url(arch)?;
        if !store_resolved_url_allowed(&resolved) {
            return Err(DOWNLOAD_FAILED.to_string());
        }
        source.url = resolved;
    } else if !source.url.starts_with("https://") {
        return Err(DOWNLOAD_FAILED.to_string());
    }

    let cancel = progress::cancel_flag();
    let mut on_progress = |progress_update: DownloadProgress| {
        progress::update_status(|status| {
            status.phase = InstallPhase::Downloading;
            status.stage = Some("download");
            status.bytes_downloaded = Some(progress_update.bytes_downloaded);
            status.bytes_total = progress_update.bytes_total;
        });
    };

    let handle = tokio::runtime::Handle::try_current().map_err(|_| DOWNLOAD_FAILED.to_string())?;
    tokio::task::block_in_place(|| {
        handle.block_on(download_to_cache(&source, cache, cancel, &mut on_progress))
    })
}

fn resolve_official_store_url(arch: HostArch) -> Result<String, String> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle.block_on(async move {
                tokio::task::spawn_blocking(move || resolve_store_msix_url(STORE_PRODUCT_ID, arch))
                    .await
                    .map_err(|_| DOWNLOAD_FAILED.to_string())?
            })
        }),
        Err(_) => resolve_store_msix_url(STORE_PRODUCT_ID, arch),
    }
}

/// FE3 只签发 `http://` 投递链接（https 重放 403），除微软投递域外仍必须 https。
fn store_resolved_url_allowed(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() == "https" {
        return parsed.host_str().is_some();
    }
    parsed.scheme() == "http" && windows_store::is_microsoft_delivery_host(&parsed)
}

fn live_verify(
    path: &Path,
    source: &PackageSource,
    platform: HostPlatform,
    route: InstallRoute,
) -> Result<(), String> {
    if let Some(expected) = source.sha256.as_deref() {
        verify_sha256(path, expected)?;
    }
    match platform {
        HostPlatform::Windows => {
            // 预检三态：钉选通过→过；确凿不匹配→拒；跑不出结果→侧载放行给
            // Add-AppxPackage 的系统级签名验证（装后解析只认 OpenAI.Codex 包族），
            // 便携路线没有系统兜底必须拒（D-21）。
            let verdict = windows::authenticode_verdict(path);
            windows::authenticode_route_decision(route, &verdict)
        }
        // Team ID is enforced inside `install_macos_from_dmg`.
        HostPlatform::Macos => Ok(()),
    }
}

fn live_install(
    path: &Path,
    route: InstallRoute,
    destination: Option<&Path>,
) -> Result<PathBuf, String> {
    match route {
        InstallRoute::WindowsSideload => install_windows_sideload(path),
        InstallRoute::WindowsPortable => {
            let dest = match destination {
                Some(dest) => dest.to_path_buf(),
                None => windows_portable_dest()?,
            };
            install_windows_portable(path, &dest)
        }
        InstallRoute::MacosCopyApp => install_macos_from_dmg(path, destination),
    }
}

fn windows_portable_dest() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| INSTALL_FAILED.to_string())?;
    Ok(PathBuf::from(local).join(PORTABLE_REL))
}

/// 用户自选的安装目录必须在下载开始前就建好并确认可写（Windows 便携解压与
/// macOS .app 拷贝都到安装阶段才消费它）；失败在下载前暴露，不让 745MB 白下。
fn prepare_destination(destination: Option<&Path>) -> Result<(), String> {
    let Some(root) = destination else {
        return Ok(());
    };
    if std::fs::create_dir_all(root).is_err() {
        return Err(INSTALL_FAILED.to_string());
    }
    if !dir_is_writable(root) {
        return Err(INSTALL_FAILED.to_string());
    }
    Ok(())
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".lumio-write-probe-{}", std::process::id()));
    // 上次异常退出可能残留同名探针：先清再试，残留不等于不可写。
    let _ = std::fs::remove_file(&probe);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

async fn fetch_mirror_payload(platform: HostPlatform, arch: HostArch) -> Option<MirrorPayload> {
    let manifest = fetch_https_text(MIRROR_MANIFEST_URL).await?;
    let checksums = fetch_https_text(MIRROR_CHECKSUMS_URL).await;
    parse_mirror_manifest(&manifest, platform, arch, checksums.as_deref()).ok()
}

async fn fetch_https_text(url: &str) -> Option<String> {
    if !url.starts_with("https://") {
        return None;
    }
    let client = reqwest::Client::builder()
        .user_agent(format!("LumioCodex/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    response.status().is_success().then_some(())?;
    response.text().await.ok()
}

fn cancelled_download() -> bool {
    if progress::cancel_requested() {
        progress::set_cancelled();
        true
    } else {
        false
    }
}

fn fail_download() -> Result<PathBuf, String> {
    fail_with(DOWNLOAD_FAILED.to_string())
}

fn fail_with(error: String) -> Result<PathBuf, String> {
    if progress::cancel_requested() {
        progress::set_cancelled();
    } else {
        progress::set_failed(&error);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn ready_request(detected: Option<&Path>) -> PlanInput<'_> {
        PlanInput {
            platform: HostPlatform::Macos,
            arch: HostArch::Arm64,
            detected_app: detected,
            online: true,
            windows_sideload_ok: None,
            destination: None,
        }
    }

    #[test]
    fn store_resolved_urls_must_be_https_or_microsoft_delivery() {
        assert!(store_resolved_url_allowed(
            "https://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/abc?P1=1"
        ));
        assert!(store_resolved_url_allowed(
            "http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/abc?P1=1"
        ));
        assert!(!store_resolved_url_allowed("http://example.com/file"));
        assert!(!store_resolved_url_allowed(
            "http://tlu.dl.delivery.mp.microsoft.com.evil.com/file"
        ));
        assert!(!store_resolved_url_allowed("store:9PLM9XGG6VKS"));
    }

    /// 自选目录必须在 745MB 下载开始前就建好并确认可写：Windows 便携解压与
    /// macOS .app 拷贝都在下载完成后才消费它，坏目录晚失败等于白下一遍。
    #[test]
    fn a_custom_destination_is_created_before_download() {
        let home = tempfile::tempdir().unwrap();
        let dest = home.path().join("MyApps").join("Codex");

        prepare_destination(Some(&dest)).expect("缺失的多级目录要先创建");
        assert!(dest.is_dir());

        assert!(
            prepare_destination(None).is_ok(),
            "默认路线没有自选目录，直接放行"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_custom_destination_fails_before_download() {
        let home = tempfile::tempdir().unwrap();
        let locked = home.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms).unwrap();

        assert_eq!(
            prepare_destination(Some(&locked)),
            Err(INSTALL_FAILED.to_string())
        );
    }

    #[test]
    fn already_installed_skips_download() {
        let _guard = progress::reset_status_for_tests();
        let existing = PathBuf::from("/Applications/Codex.app");
        let downloaded = Cell::new(false);
        let installed = Cell::new(false);

        let path = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| Some(existing.clone()),
            download: &mut |_source| {
                downloaded.set(true);
                Ok(PathBuf::from("/tmp/pkg"))
            },
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| {
                installed.set(true);
                Ok(PathBuf::from("/tmp/app"))
            },
            write_config: &mut |_path| panic!("must not write config.toml"),
        })
        .expect("already-installed must succeed");

        assert_eq!(path, existing);
        assert!(
            !downloaded.get(),
            "must not download when an app is already present"
        );
        assert!(
            !installed.get(),
            "must not install when an app is already present"
        );
        assert_eq!(current_status().phase, InstallPhase::Succeeded);
        assert_eq!(
            current_status().installed_path.as_deref(),
            Some(existing.as_path())
        );
    }

    #[test]
    fn verify_failure_does_not_call_install() {
        let _guard = progress::reset_status_for_tests();
        let installed = Cell::new(false);
        let mut download_kinds = Vec::new();

        let err = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |source| {
                download_kinds.push(source.kind);
                Ok(PathBuf::from("/tmp/pkg"))
            },
            verify: &|_path, _source, _route| Err("CODEX_APP_VERIFY_FAILED".to_string()),
            install: &mut |_path, _route| {
                installed.set(true);
                Ok(PathBuf::from("/tmp/app"))
            },
            write_config: &mut |_path| panic!("must not write config.toml"),
        })
        .expect_err("verify failure must abort the pipeline");

        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
        assert!(!installed.get(), "verify failure must not call install");
        assert_eq!(
            download_kinds,
            vec![SourceKind::Mirror],
            "VERIFY_FAILED is not a network miss and must not try the next source"
        );
        assert_eq!(current_status().phase, InstallPhase::Failed);
        assert_eq!(
            current_status().error_code.as_deref(),
            Some("CODEX_APP_VERIFY_FAILED")
        );
    }

    #[test]
    fn mirror_failure_falls_back_to_official() {
        let _guard = progress::reset_status_for_tests();
        let official_pkg = PathBuf::from("/tmp/official-pkg");
        let mut download_kinds = Vec::new();
        let installed_from = Cell::new(None);

        let path = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |source| {
                download_kinds.push(source.kind);
                match source.kind {
                    SourceKind::Mirror => Err("CODEX_APP_DOWNLOAD_FAILED".to_string()),
                    SourceKind::Official => Ok(official_pkg.clone()),
                }
            },
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |path, _route| {
                installed_from.set(Some(path.to_path_buf()));
                Ok(PathBuf::from("/Applications/Codex.app"))
            },
            write_config: &mut |_path| {},
        })
        .expect("official source must be tried after a mirror download miss");

        assert_eq!(
            download_kinds,
            vec![SourceKind::Mirror, SourceKind::Official]
        );
        assert_eq!(
            installed_from.take().as_deref(),
            Some(official_pkg.as_path())
        );
        assert_eq!(path, PathBuf::from("/Applications/Codex.app"));
    }

    #[test]
    fn cancel_or_half_failure_does_not_write_config_toml() {
        let _guard = progress::reset_status_for_tests();
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(&config, "model = \"keep-me\"\n").unwrap();
        let wrote_config = Cell::new(false);

        let err = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |_source| {
                request_cancel();
                Err("CODEX_APP_DOWNLOAD_FAILED".to_string())
            },
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Ok(PathBuf::from("/tmp/app")),
            write_config: &mut |_path| {
                wrote_config.set(true);
                std::fs::write(&config, "model = \"overwritten\"\n").unwrap();
            },
        })
        .expect_err("cancel / half-failure must not succeed");

        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");
        assert!(!wrote_config.get(), "cancel must not write config.toml");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "model = \"keep-me\"\n"
        );
        assert_eq!(current_status().phase, InstallPhase::Cancelled);
    }

    #[test]
    fn a_successful_install_deletes_the_downloaded_package() {
        // 745MB 安装包装完即删（失败保留供重试）：C 盘峰值是下载瞬时，不常驻。
        let _guard = progress::reset_status_for_tests();
        let pkg = tempfile::tempdir().unwrap();
        let package = pkg.path().join("win-x64.msix");
        std::fs::write(&package, b"pkg").unwrap();

        run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |_source| Ok(package.clone()),
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Ok(PathBuf::from("/tmp/app")),
            write_config: &mut |_path| {},
        })
        .expect("install must succeed");

        assert!(
            !package.exists(),
            "the package must be removed after success"
        );
    }

    #[test]
    fn a_failed_install_keeps_the_package_for_retry() {
        let _guard = progress::reset_status_for_tests();
        let pkg = tempfile::tempdir().unwrap();
        let package = pkg.path().join("win-x64.msix");
        std::fs::write(&package, b"pkg").unwrap();

        let _ = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |_source| Ok(package.clone()),
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Err("CODEX_APP_INSTALL_FAILED".to_string()),
            write_config: &mut |_path| {},
        })
        .expect_err("install must fail");

        assert!(
            package.exists(),
            "a failed install must keep the package for retry"
        );
    }

    /// 自选目录装的官方应用只有这里能被再次找到：保存的最终路径优先于自动扫描，
    /// 失效（卸载/移动）则原样回落，绝不比不保存时更糟。
    #[test]
    fn detection_prefers_a_saved_destination_and_falls_back_when_stale() {
        let state = tempfile::tempdir().unwrap();
        let saved_app = state.path().join("MyApps").join("Codex.app");
        std::fs::create_dir_all(&saved_app).unwrap();
        install_path::save_install_path(state.path(), &saved_app).unwrap();

        assert_eq!(
            detect_existing_app_with(None, Some(state.path())),
            Some(saved_app)
        );

        let stale = state.path().join("gone");
        install_path::save_install_path(state.path(), &stale).unwrap();
        assert_eq!(
            detect_existing_app_with(None, Some(state.path())),
            detect_existing_app_with(None, None),
            "an invalid saved destination must fall back to automatic detection"
        );
    }

    /// D-24：持久化自选目录必须在 succeeded 上报之前完成——前端轮询到成功即发起
    /// 启动，晚于 set_succeeded 的落盘会让启动撞上「还没记住」的窗口。
    #[test]
    fn the_post_install_hook_sees_the_final_path_before_success_is_reported() {
        let _guard = progress::reset_status_for_tests();
        let seen = std::cell::RefCell::new(None);
        let still_detecting_at_hook = std::cell::Cell::new(false);

        let path = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |_source| Ok(PathBuf::from("/tmp/pkg")),
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Ok(PathBuf::from("/tmp/app")),
            write_config: &mut |path: &Path| {
                still_detecting_at_hook
                    .set(matches!(current_status().phase, InstallPhase::Detecting));
                *seen.borrow_mut() = Some(path.to_path_buf());
            },
        })
        .expect("install must succeed");

        assert_eq!(seen.into_inner().as_deref(), Some(path.as_path()));
        assert!(
            still_detecting_at_hook.get(),
            "the hook must run before the succeeded phase is reported"
        );
        assert_eq!(current_status().phase, InstallPhase::Succeeded);
    }

    /// D-20：后台任务在进入主管线前提前出错（缓存目录不可写等）时，
    /// 状态会停在 planning、按钮永远忙。守卫必须把它升级为 failed，
    /// 且不得覆盖主管线已经写下的失败码。
    #[test]
    fn early_background_failure_upgrades_a_stuck_planning_phase_only() {
        let _guard = progress::reset_status_for_tests();
        progress::set_planning();

        note_background_failure("CODEX_APP_DOWNLOAD_FAILED".to_string());
        let status = current_status();
        assert_eq!(status.phase, InstallPhase::Failed);
        assert_eq!(
            status.error_code.as_deref(),
            Some("CODEX_APP_DOWNLOAD_FAILED")
        );

        progress::set_phase(InstallPhase::Failed, Some("verify"));
        note_background_failure("LATER_CODE".to_string());
        let status = current_status();
        assert_eq!(
            status.error_code.as_deref(),
            Some("CODEX_APP_DOWNLOAD_FAILED"),
            "不得覆盖主管线已记录的失败码"
        );
    }
}
