use std::io::Read;
use std::path::{Path, PathBuf};

use super::sources::{OPENAI_MARKETPLACE_SUBJECT, PORTABLE_REL};
use super::{HostPlatform, InstallRoute};

const INSTALL_FAILED: &str = "CODEX_APP_INSTALL_FAILED";
const VERIFY_FAILED: &str = "CODEX_APP_VERIFY_FAILED";
const MARKETPLACE_ISSUER_CN_PREFIX: &str = "cn=microsoft marketplace ca";
const MARKETPLACE_ISSUER_ORG: &str = "o=microsoft corporation";

/// Authenticode 预检三态（D-21）：`Unavailable` 是「跑不出结果」（PowerShell
/// 失败、无法解析），与 `Mismatch`（确凿不匹配）必须区分对待。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthenticodeVerdict {
    Pinned,
    Mismatch {
        status: String,
        subject: String,
        issuer: String,
    },
    Unavailable,
}

/// 侧载路线放行 `Unavailable`：Add-AppxPackage 部署时强制做系统级签名验证，
/// 装后解析也只认 OpenAI.Codex 包族——两道兜底都在。便携路线没有系统验证，
/// 预检不可用就等于没有密码学防线，必须拒。`Mismatch` 任何路线都拒。
pub fn authenticode_route_decision(
    route: InstallRoute,
    verdict: &AuthenticodeVerdict,
) -> Result<(), String> {
    match verdict {
        AuthenticodeVerdict::Pinned => Ok(()),
        AuthenticodeVerdict::Unavailable if route == InstallRoute::WindowsSideload => Ok(()),
        AuthenticodeVerdict::Unavailable => Err(VERIFY_FAILED.to_string()),
        AuthenticodeVerdict::Mismatch { .. } => Err(VERIFY_FAILED.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCheck {
    pub state: CapabilityState,
    pub detail: String,
}

impl CapabilityCheck {
    pub fn available(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Available,
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Unavailable,
            detail: detail.into(),
        }
    }

    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Unknown,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinCapabilityReport {
    pub add_appx_package: CapabilityCheck,
    pub appx_service: CapabilityCheck,
    pub sideload_policy: CapabilityCheck,
    pub package_manager: CapabilityCheck,
    pub sideload_ok: bool,
}

impl WinCapabilityReport {
    pub fn from_checks(
        add_appx_package: CapabilityCheck,
        appx_service: CapabilityCheck,
        sideload_policy: CapabilityCheck,
        package_manager: CapabilityCheck,
    ) -> Self {
        let sideload_ok = add_appx_package.state != CapabilityState::Unavailable
            && appx_service.state != CapabilityState::Unavailable
            && sideload_policy.state != CapabilityState::Unavailable
            && package_manager.state != CapabilityState::Unavailable;
        Self {
            add_appx_package,
            appx_service,
            sideload_policy,
            package_manager,
            sideload_ok,
        }
    }
}

pub fn probe_windows_sideload() -> bool {
    probe_windows_sideload_with(live_capability_report())
}

pub fn probe_windows_sideload_with(report: WinCapabilityReport) -> bool {
    report.sideload_ok
}

pub fn parse_appx_application_executable(xml: &str) -> Option<String> {
    let tag = open_tag_slice(xml, "Application")?;
    xml_attr(tag, "Executable").filter(|value| !value.is_empty())
}

pub fn authenticode_pin_ok(status: &str, subject: &str, issuer: &str) -> bool {
    status.eq_ignore_ascii_case("Valid") && is_pinned_marketplace_subject(subject, issuer)
}

pub fn install_windows_portable(msix: &Path, dest: &Path) -> Result<PathBuf, String> {
    extract_and_require_declared_entry(msix, dest)?;
    write_user_start_menu_shortcut(dest);
    Ok(dest.to_path_buf())
}

pub fn install_windows_sideload(msix: &Path) -> Result<PathBuf, String> {
    match run_add_appx_package(msix) {
        Ok(path) => Ok(path),
        Err(_) => {
            let dest = default_portable_dest()?;
            install_windows_portable(msix, &dest)
        }
    }
}

pub fn verify_windows_authenticode(path: &Path) -> Result<(), String> {
    let verdict = authenticode_verdict(path);
    authenticode_route_decision(InstallRoute::WindowsPortable, &verdict)
}

/// 预检三态化（D-21）：脚本跑挂 / 输出解析不出 → `Unavailable`（不再一刀切
/// 报校验失败）；解析成功 → 按钉选判定 `Pinned` / `Mismatch`。
pub fn authenticode_verdict(path: &Path) -> AuthenticodeVerdict {
    #[cfg(windows)]
    {
        match authenticode_report_from_powershell(path) {
            Ok((status, subject, issuer)) => {
                if authenticode_pin_ok(&status, &subject, &issuer) {
                    AuthenticodeVerdict::Pinned
                } else {
                    AuthenticodeVerdict::Mismatch {
                        status,
                        subject,
                        issuer,
                    }
                }
            }
            Err(_) => AuthenticodeVerdict::Unavailable,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        AuthenticodeVerdict::Unavailable
    }
}

fn live_capability_report() -> WinCapabilityReport {
    #[cfg(windows)]
    {
        WinCapabilityReport::from_checks(
            probe_add_appx_package(),
            probe_appx_service(),
            probe_sideload_policy(),
            probe_package_manager(),
        )
    }
    #[cfg(not(windows))]
    {
        WinCapabilityReport::from_checks(
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
        )
    }
}

fn default_portable_dest() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| INSTALL_FAILED.to_string())?;
    Ok(PathBuf::from(local).join(PORTABLE_REL))
}

fn extract_and_require_declared_entry(msix: &Path, dest: &Path) -> Result<(), String> {
    let xml = extract_msix(msix, dest)?;
    let Some(declared) = parse_appx_application_executable(&xml) else {
        return Err(INSTALL_FAILED.to_string());
    };
    let relative = portable_relative_path(&declared);
    if dest.join(relative).is_file() {
        Ok(())
    } else {
        Err(INSTALL_FAILED.to_string())
    }
}

fn extract_msix(msix: &Path, dest: &Path) -> Result<String, String> {
    let file = std::fs::File::open(msix).map_err(|_| INSTALL_FAILED.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| INSTALL_FAILED.to_string())?;
    let mut manifest_xml = None;
    std::fs::create_dir_all(dest).map_err(|_| INSTALL_FAILED.to_string())?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|_| INSTALL_FAILED.to_string())?;
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(&enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|_| INSTALL_FAILED.to_string())?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| INSTALL_FAILED.to_string())?;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|_| INSTALL_FAILED.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|_| INSTALL_FAILED.to_string())?;
        if enclosed
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("AppxManifest.xml"))
            && enclosed.components().count() == 1
        {
            let mut xml = String::new();
            std::fs::File::open(&out_path)
                .and_then(|mut file| file.read_to_string(&mut xml))
                .map_err(|_| INSTALL_FAILED.to_string())?;
            manifest_xml = Some(xml);
        }
    }

    manifest_xml.ok_or_else(|| INSTALL_FAILED.to_string())
}

fn portable_relative_path(declared: &str) -> PathBuf {
    declared
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn open_tag_slice<'a>(xml: &'a str, local_name: &str) -> Option<&'a str> {
    let mut search = xml;
    while let Some(at) = search.find(local_name) {
        let before = &search[..at];
        let after = &search[at + local_name.len()..];
        let starts_tag = before.ends_with('<')
            || (before.ends_with(':') && before[..before.len().saturating_sub(1)].ends_with('<'));
        if starts_tag && after.starts_with(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
        {
            let end = after.find('>')?;
            return Some(&search[at.saturating_sub(1)..=at + local_name.len() + end]);
        }
        search = &search[at + local_name.len()..];
    }
    None
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        if let Some((_, rest)) = tag.split_once(&key) {
            if let Some((value, _)) = rest.split_once(quote) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn is_pinned_marketplace_subject(subject: &str, issuer: &str) -> bool {
    subject
        .trim()
        .eq_ignore_ascii_case(OPENAI_MARKETPLACE_SUBJECT)
        && has_pinned_marketplace_issuer(issuer)
}

fn has_pinned_marketplace_issuer(issuer: &str) -> bool {
    let components: Vec<String> = issuer
        .split(',')
        .map(|component| component.trim().to_ascii_lowercase())
        .filter(|component| !component.is_empty())
        .collect();
    components
        .iter()
        .any(|component| component.starts_with(MARKETPLACE_ISSUER_CN_PREFIX))
        && components
            .iter()
            .any(|component| component == MARKETPLACE_ISSUER_ORG)
}

fn run_add_appx_package(msix: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        run_add_appx_package_windows(msix)
    }
    #[cfg(not(windows))]
    {
        let _ = msix;
        Err(INSTALL_FAILED.to_string())
    }
}

#[cfg(windows)]
fn run_add_appx_package_windows(msix: &Path) -> Result<PathBuf, String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let script = format!(
        "Add-AppxPackage -Path {}",
        ps_quote(&msix.to_string_lossy())
    );
    let mut child = Command::new(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(crate::windows_create_no_window())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| INSTALL_FAILED.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return crate::app_paths::resolve_codex_app_dir(None)
                    .ok_or_else(|| INSTALL_FAILED.to_string());
            }
            Ok(Some(_)) => return Err(INSTALL_FAILED.to_string()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(INSTALL_FAILED.to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err(INSTALL_FAILED.to_string()),
        }
    }
}

#[cfg(windows)]
fn powershell_exe() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .map(|windir| {
            windir
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn probe_add_appx_package() -> CapabilityCheck {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let output = Command::new(powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Command Add-AppxPackage -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name",
        ])
        .creation_flags(crate::windows_create_no_window())
        .output();
    match output {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains("add-appxpackage") =>
        {
            CapabilityCheck::available("Add-AppxPackage command is present")
        }
        Ok(_) => CapabilityCheck::unavailable("Add-AppxPackage command is not present"),
        Err(_) => CapabilityCheck::unknown("Add-AppxPackage probe failed"),
    }
}

#[cfg(windows)]
fn probe_appx_service() -> CapabilityCheck {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let output = Command::new(powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-Service AppXSvc -ErrorAction SilentlyContinue).Status",
        ])
        .creation_flags(crate::windows_create_no_window())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let status = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_ascii_lowercase();
            if status == "running" || status == "startpending" {
                CapabilityCheck::available("AppX service is available")
            } else if status.is_empty() {
                CapabilityCheck::unknown("AppX service status is empty")
            } else {
                CapabilityCheck::unavailable(format!("AppX service is {status}"))
            }
        }
        _ => CapabilityCheck::unknown("AppX service probe failed"),
    }
}

#[cfg(windows)]
fn probe_sideload_policy() -> CapabilityCheck {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let output = Command::new(powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"$p = Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -Name AllowAllTrustedApps -ErrorAction SilentlyContinue; if ($null -eq $p) { 'missing' } else { [string]$p.AllowAllTrustedApps }"#,
        ])
        .creation_flags(crate::windows_create_no_window())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            match value.as_str() {
                "0" => CapabilityCheck::unavailable("AllowAllTrustedApps=0"),
                "1" => CapabilityCheck::available("AllowAllTrustedApps=1"),
                _ => CapabilityCheck::unknown("AllowAllTrustedApps is not set"),
            }
        }
        _ => CapabilityCheck::unknown("sideload policy probe failed"),
    }
}

#[cfg(windows)]
fn probe_package_manager() -> CapabilityCheck {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let output = Command::new(powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "try { $null = [Windows.Management.Deployment.PackageManager,Windows.Management.Deployment,ContentType=WindowsRuntime]; 'ok' } catch { $_.Exception.HResult }",
        ])
        .creation_flags(crate::windows_create_no_window())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.eq_ignore_ascii_case("ok") {
                CapabilityCheck::available("PackageManager activates")
            } else {
                CapabilityCheck::unavailable(format!("PackageManager cannot activate ({text})"))
            }
        }
        _ => CapabilityCheck::unknown("PackageManager probe failed"),
    }
}

#[cfg(windows)]
fn authenticode_report_from_powershell(path: &Path) -> Result<(String, String, String), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$securityModule = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1'
Import-Module $securityModule -ErrorAction Stop
$sig = Get-AuthenticodeSignature -LiteralPath {path}
@{{
  status = [string]$sig.Status
  subject = if ($sig.SignerCertificate) {{ [string]$sig.SignerCertificate.Subject }} else {{ '' }}
  issuer = if ($sig.SignerCertificate) {{ [string]$sig.SignerCertificate.Issuer }} else {{ '' }}
}} | ConvertTo-Json -Compress
"#,
        path = ps_quote(&path.to_string_lossy()),
    );
    let output = Command::new(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(crate::windows_create_no_window())
        .output()
        .map_err(|_| VERIFY_FAILED.to_string())?;
    if !output.status.success() {
        return Err(VERIFY_FAILED.to_string());
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| VERIFY_FAILED.to_string())?;
    let field = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    Ok((field("status"), field("subject"), field("issuer")))
}

fn write_user_start_menu_shortcut(dest: &Path) {
    #[cfg(windows)]
    {
        write_user_start_menu_shortcut_windows(dest);
    }
    #[cfg(not(windows))]
    {
        let _ = dest;
    }
}

#[cfg(windows)]
fn write_user_start_menu_shortcut_windows(dest: &Path) {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return;
    };
    let Some(exe) = portable_entry_exe(dest) else {
        return;
    };
    let path = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Codex.lnk");
    let _ =
        crate::windows_integration::create_shortcut(&crate::windows_integration::ShortcutSpec {
            path,
            target: exe.clone(),
            arguments: String::new(),
            working_directory: exe.parent().map(Path::to_path_buf),
            description: "Codex".to_string(),
            icon: Some(exe),
            show_minimized: false,
        });
}

#[cfg(windows)]
fn portable_entry_exe(dest: &Path) -> Option<PathBuf> {
    let xml = std::fs::read_to_string(dest.join("AppxManifest.xml")).ok()?;
    let declared = parse_appx_application_executable(&xml)?;
    let exe = dest.join(portable_relative_path(&declared));
    exe.is_file().then_some(exe)
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilityCheck, WinCapabilityReport, authenticode_pin_ok, install_windows_portable,
        parse_appx_application_executable, probe_windows_sideload_with,
    };
    use std::io::Write;
    use std::path::Path;

    const MANIFEST_CHATGPT: &str = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.707.3748.0" ProcessorArchitecture="x64" />
  <Applications>
    <Application Id="App" Executable="app\ChatGPT.exe" EntryPoint="Windows.FullTrustApplication" />
  </Applications>
</Package>"#;

    fn write_msix(path: &Path, manifest: &str, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        for (name, bytes) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn portable_entry_comes_from_manifest_not_guessed_exe_name() {
        let dir = tempfile::tempdir().unwrap();
        let msix = dir.path().join("codex.msix");
        let root = dir.path().join("dest");
        write_msix(
            &msix,
            MANIFEST_CHATGPT,
            &[("app/ChatGPT.exe", b"entry"), ("app/Codex.exe", b"legacy")],
        );

        let dest = install_windows_portable(&msix, &root).unwrap();
        assert!(dest.join("app").join("ChatGPT.exe").is_file());
        assert_eq!(
            parse_appx_application_executable(MANIFEST_CHATGPT).as_deref(),
            Some(r"app\ChatGPT.exe")
        );
    }

    #[test]
    fn declared_missing_entry_is_install_failed_not_legacy_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let msix = dir.path().join("codex.msix");
        let root = dir.path().join("dest");
        write_msix(
            &msix,
            MANIFEST_CHATGPT,
            &[("app/Codex.exe", b"legacy only")],
        );

        let err = install_windows_portable(&msix, &root).unwrap_err();
        assert_eq!(err, "CODEX_APP_INSTALL_FAILED");
        assert!(
            !root.join("app").join("ChatGPT.exe").is_file(),
            "must not invent a ChatGPT.exe fallback"
        );
    }

    #[test]
    fn capability_unavailable_is_portable_route() {
        let report = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unavailable("AllowAllTrustedApps=0"),
            CapabilityCheck::available("PackageManager activates"),
        );
        assert!(!report.sideload_ok);

        let missing_cmd = WinCapabilityReport::from_checks(
            CapabilityCheck::unavailable("Add-AppxPackage missing"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unknown("policy absent"),
            CapabilityCheck::available("PackageManager activates"),
        );
        assert!(!missing_cmd.sideload_ok);

        let service_down = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::unavailable("AppX service stopped"),
            CapabilityCheck::unknown("policy absent"),
            CapabilityCheck::available("PackageManager activates"),
        );
        assert!(!service_down.sideload_ok);

        let pm_dead = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::available("AllowAllTrustedApps=1"),
            CapabilityCheck::unavailable("PackageManager cannot activate"),
        );
        assert!(!pm_dead.sideload_ok);
    }

    #[test]
    fn unknown_sideload_policy_is_not_failure() {
        let report = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unknown("policy absent"),
            CapabilityCheck::available("PackageManager activates"),
        );
        assert!(report.sideload_ok);
        assert!(probe_windows_sideload_with(report));
    }

    #[test]
    fn authenticode_requires_marketplace_subject_and_issuer() {
        assert!(authenticode_pin_ok(
            "Valid",
            "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B",
            "CN=Microsoft Marketplace CA G 028, O=Microsoft Corporation, C=US",
        ));
        assert!(!authenticode_pin_ok(
            "Valid",
            "CN=OpenAI OpCo, LLC, O=OpenAI OpCo, LLC, C=US",
            "CN=Trusted CA",
        ));
        assert!(!authenticode_pin_ok(
            "Valid",
            "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B",
            "CN=Contoso Marketplace CA, O=Contoso, C=US",
        ));
        assert!(!authenticode_pin_ok(
            "HashMismatch",
            "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B",
            "CN=Microsoft Marketplace CA G 028, O=Microsoft Corporation, C=US",
        ));
    }

    /// D-21：预检「跑不出结果」≠「签名不匹配」。侧载路线放行给
    /// Add-AppxPackage 的系统级签名验证（装后解析只认 OpenAI.Codex 包族），
    /// 便携路线没有系统验证兜底，预检不可用必须硬失败。
    #[test]
    fn an_unavailable_precheck_only_proceeds_on_the_sideload_route() {
        use super::{AuthenticodeVerdict, InstallRoute};

        let pinned = AuthenticodeVerdict::Pinned;
        let mismatch = AuthenticodeVerdict::Mismatch {
            status: "HashMismatch".to_string(),
            subject: "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B".to_string(),
            issuer: "CN=Microsoft Marketplace CA G 024, O=Microsoft Corporation, C=US".to_string(),
        };
        let unavailable = AuthenticodeVerdict::Unavailable;

        assert!(super::authenticode_route_decision(InstallRoute::WindowsSideload, &pinned).is_ok());
        assert!(super::authenticode_route_decision(InstallRoute::WindowsPortable, &pinned).is_ok());

        assert_eq!(
            super::authenticode_route_decision(InstallRoute::WindowsSideload, &mismatch)
                .unwrap_err(),
            "CODEX_APP_VERIFY_FAILED",
            "确凿的签名不匹配在任何路线都不许过"
        );
        assert_eq!(
            super::authenticode_route_decision(InstallRoute::WindowsPortable, &mismatch)
                .unwrap_err(),
            "CODEX_APP_VERIFY_FAILED"
        );

        assert!(
            super::authenticode_route_decision(InstallRoute::WindowsSideload, &unavailable).is_ok(),
            "侧载预检不可用放行给系统部署验证"
        );
        assert_eq!(
            super::authenticode_route_decision(InstallRoute::WindowsPortable, &unavailable)
                .unwrap_err(),
            "CODEX_APP_VERIFY_FAILED",
            "便携路线没有系统验证，预检不可用就是没有密码学防线"
        );
    }
}
