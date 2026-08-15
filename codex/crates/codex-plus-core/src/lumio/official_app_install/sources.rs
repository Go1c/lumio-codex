use std::collections::BTreeMap;

use serde::Deserialize;

use super::{HostArch, HostPlatform, PackageSource, SourceKind};

pub const MIRROR_MANIFEST_URL: &str = "https://codexapp.agentsmirror.com/latest/manifest";
pub const MIRROR_CHECKSUMS_URL: &str = "https://codexapp.agentsmirror.com/latest/SHA256SUMS";
pub const MIRROR_WIN_X64: &str = "https://codexapp.agentsmirror.com/latest/win-x64";
pub const MIRROR_WIN_ARM64: &str = "https://codexapp.agentsmirror.com/latest/win-arm64";
pub const MIRROR_MAC_ARM64: &str = "https://codexapp.agentsmirror.com/latest/mac-arm64";
pub const MIRROR_MAC_X64: &str = "https://codexapp.agentsmirror.com/latest/mac-intel";
pub const OFFICIAL_MAC_ARM64: &str = "https://persistent.oaistatic.com/codex-app-prod/Codex.dmg";
pub const OFFICIAL_MAC_X64: &str =
    "https://persistent.oaistatic.com/codex-app-prod/Codex-latest-x64.dmg";
pub const STORE_PRODUCT_ID: &str = "9PLM9XGG6VKS";
pub const OPENAI_MAC_TEAM_ID: &str = "2DC432GLL2";
pub const OPENAI_MARKETPLACE_SUBJECT: &str = "cn=50bdfd77-8903-4850-9ffe-6e8522f64d5b";
pub const PORTABLE_REL: &str = "Programs/Codex";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorPayload {
    pub platform: HostPlatform,
    pub arch: HostArch,
    pub package_moniker: Option<String>,
    pub sha256: Option<String>,
    /// v5 起按架构携带的载荷尺寸，sha 缺位时的完整性防线（D-21）。
    pub content_length: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MirrorManifest {
    schema_version: u64,
    sources: ManifestSources,
    #[serde(default)]
    manager: Option<ManagerSection>,
}

#[derive(Debug, Deserialize)]
struct ManifestSources {
    windows: Option<WindowsSource>,
    macos: Option<MacosSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsSource {
    package_moniker: Option<String>,
    #[serde(default)]
    architectures: BTreeMap<String, WindowsArchitectureSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsArchitectureSource {
    package_moniker: Option<String>,
    #[serde(default)]
    downloadable: Option<bool>,
    #[serde(default)]
    content_length: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct MacosSource {
    arm64: Option<MacosArchSource>,
    x64: Option<MacosArchSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MacosArchSource {
    sha256: Option<String>,
    #[serde(default)]
    content_length: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct ManagerSection {
    #[serde(default)]
    payloads: BTreeMap<String, ManagerPayload>,
}

#[derive(Debug, Deserialize)]
struct ManagerPayload {
    sha256: Option<String>,
}

pub fn official_windows_url() -> String {
    format!("store:{STORE_PRODUCT_ID}")
}

pub fn planned_sources(platform: HostPlatform, arch: HostArch) -> Vec<PackageSource> {
    let (mirror, official) = match (platform, arch) {
        (HostPlatform::Windows, HostArch::X64) => (MIRROR_WIN_X64, official_windows_url()),
        (HostPlatform::Windows, HostArch::Arm64) => (MIRROR_WIN_ARM64, official_windows_url()),
        (HostPlatform::Macos, HostArch::Arm64) => {
            (MIRROR_MAC_ARM64, OFFICIAL_MAC_ARM64.to_string())
        }
        (HostPlatform::Macos, HostArch::X64) => (MIRROR_MAC_X64, OFFICIAL_MAC_X64.to_string()),
    };
    vec![
        PackageSource {
            kind: SourceKind::Mirror,
            platform,
            url: mirror.to_string(),
            sha256: None,
            expected_bytes: None,
        },
        PackageSource {
            kind: SourceKind::Official,
            platform,
            url: official.to_string(),
            sha256: None,
            expected_bytes: None,
        },
    ]
}

pub fn parse_mirror_manifest(
    json: &str,
    platform: HostPlatform,
    arch: HostArch,
    checksums: Option<&str>,
) -> Result<MirrorPayload, String> {
    let manifest: MirrorManifest =
        serde_json::from_str(json).map_err(|err| format!("invalid mirror manifest: {err}"))?;
    if manifest.schema_version < 2 {
        return Err(format!(
            "unsupported schemaVersion {}",
            manifest.schema_version
        ));
    }

    match platform {
        HostPlatform::Windows => parse_windows_payload(&manifest, arch, checksums),
        HostPlatform::Macos => parse_macos_payload(&manifest, arch),
    }
}

fn parse_windows_payload(
    manifest: &MirrorManifest,
    arch: HostArch,
    checksums: Option<&str>,
) -> Result<MirrorPayload, String> {
    let windows = manifest
        .sources
        .windows
        .as_ref()
        .ok_or_else(|| "missing Windows source".to_string())?;
    let requested = arch_key(arch);
    let selected = select_windows_arch(&windows.architectures, requested)?;
    let package_moniker = selected
        .and_then(|(_, source)| source.package_moniker.clone())
        .or_else(|| windows.package_moniker.clone())
        .ok_or_else(|| "missing Windows packageMoniker".to_string())?;

    let mut sha256 = payload_sha(manifest, HostPlatform::Windows, arch);
    if sha256.is_none() {
        if let Some(text) = checksums {
            sha256 = sha_from_checksums(text, &package_moniker);
        }
    }

    Ok(MirrorPayload {
        platform: HostPlatform::Windows,
        arch,
        package_moniker: Some(package_moniker),
        sha256,
        content_length: selected.and_then(|(_, source)| source.content_length),
    })
}

fn parse_macos_payload(manifest: &MirrorManifest, arch: HostArch) -> Result<MirrorPayload, String> {
    let macos = manifest
        .sources
        .macos
        .as_ref()
        .ok_or_else(|| "missing macOS source".to_string())?;
    let source = match arch {
        HostArch::Arm64 => macos.arm64.as_ref(),
        HostArch::X64 => macos.x64.as_ref(),
    }
    .ok_or_else(|| format!("missing macOS {} source", arch_key(arch)))?;

    Ok(MirrorPayload {
        platform: HostPlatform::Macos,
        arch,
        package_moniker: None,
        sha256: nonempty_sha(source.sha256.as_deref())
            .or_else(|| payload_sha(manifest, HostPlatform::Macos, arch)),
        content_length: source.content_length,
    })
}

fn select_windows_arch<'a>(
    architectures: &'a BTreeMap<String, WindowsArchitectureSource>,
    requested: &'a str,
) -> Result<Option<(&'a str, &'a WindowsArchitectureSource)>, String> {
    if architectures.is_empty() {
        return Ok(None);
    }
    let matching = architectures
        .iter()
        .find(|(key, _)| normalize_architecture(key).as_deref() == Some(requested));
    match matching {
        Some((_, source)) if source.downloadable.unwrap_or(true) => Ok(Some((requested, source))),
        Some((_, _)) => Err(format!(
            "Windows {requested} package is not available in the current mirror manifest"
        )),
        None if requested == "arm64" => {
            Err("Windows arm64 package is not available in the current mirror manifest".to_string())
        }
        None => Ok(None),
    }
}

fn payload_sha(
    manifest: &MirrorManifest,
    platform: HostPlatform,
    arch: HostArch,
) -> Option<String> {
    let key = match (platform, arch) {
        (HostPlatform::Windows, HostArch::X64) => "windowsX64Msix",
        (HostPlatform::Windows, HostArch::Arm64) => "windowsArm64Msix",
        (HostPlatform::Macos, HostArch::Arm64) => "macosArm64Dmg",
        (HostPlatform::Macos, HostArch::X64) => "macosIntelDmg",
    };
    manifest
        .manager
        .as_ref()
        .and_then(|manager| manager.payloads.get(key))
        .and_then(|payload| nonempty_sha(payload.sha256.as_deref()))
}

fn sha_from_checksums(text: &str, package_moniker: &str) -> Option<String> {
    let mut matches = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(file_name) = parts.next() else {
            continue;
        };
        if !is_sha256_hex(hash) {
            continue;
        }
        let file_name = file_name.trim_start_matches('*');
        if checksum_name_matches(file_name, package_moniker) {
            matches.push(hash.to_ascii_lowercase());
        }
    }
    match matches.as_slice() {
        [sha] => Some(sha.clone()),
        _ => None,
    }
}

fn checksum_name_matches(file_name: &str, package_moniker: &str) -> bool {
    let base = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_name)
        .trim();
    let stem = base
        .strip_suffix(".msix")
        .or_else(|| base.strip_suffix(".Msix"))
        .or_else(|| base.strip_suffix(".MSIX"))
        .unwrap_or(base);
    stem.eq_ignore_ascii_case(package_moniker)
}

fn nonempty_sha(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn arch_key(arch: HostArch) -> &'static str {
    match arch {
        HostArch::X64 => "x64",
        HostArch::Arm64 => "arm64",
    }
}

fn normalize_architecture(architecture: &str) -> Option<String> {
    match architecture.trim().to_ascii_lowercase().as_str() {
        "x64" | "x86_64" | "amd64" => Some("x64".to_string()),
        "arm64" | "aarch64" => Some("arm64".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HostArch, HostPlatform};
    use super::parse_mirror_manifest;
    use std::fs;
    use std::path::{Path, PathBuf};

    const PAYLOAD_X64_SHA: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PAYLOAD_ARM64_SHA: &str =
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const MAC_ARM64_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const MAC_X64_SHA: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const X64_MONIKER: &str = "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0";
    const ARM64_MONIKER: &str = "OpenAI.Codex_26.602.3474.0_arm64__2p2nqsd0c76g0";

    fn v2_fixture() -> String {
        format!(
            r#"{{
              "schemaVersion": 2,
              "sources": {{
                "windows": {{
                  "version": "26.602.3474.0",
                  "packageMoniker": "{X64_MONIKER}",
                  "architectures": {{
                    "x64": {{
                      "architecture": "x64",
                      "status": "downloadable",
                      "downloadable": true,
                      "version": "26.602.3474.0",
                      "packageMoniker": "{X64_MONIKER}"
                    }},
                    "arm64": {{
                      "architecture": "arm64",
                      "status": "downloadable",
                      "downloadable": true,
                      "version": "26.602.3474.0",
                      "packageMoniker": "{ARM64_MONIKER}"
                    }}
                  }}
                }},
                "macos": {{
                  "arm64": {{ "sha256": "{MAC_ARM64_SHA}" }},
                  "x64": {{ "sha256": "{MAC_X64_SHA}" }}
                }}
              }},
              "manager": {{
                "payloads": {{
                  "windowsX64Msix": {{
                    "url": "https://example.invalid/latest/win-x64",
                    "sha256": "{PAYLOAD_X64_SHA}"
                  }},
                  "windowsArm64Msix": {{
                    "url": "https://example.invalid/latest/win-arm64",
                    "sha256": "{PAYLOAD_ARM64_SHA}"
                  }}
                }}
              }}
            }}"#
        )
    }

    #[test]
    fn parses_v2_manifest_arch_and_optional_payload_sha() {
        let json = v2_fixture();

        let x64 = parse_mirror_manifest(&json, HostPlatform::Windows, HostArch::X64, None).unwrap();
        assert_eq!(x64.package_moniker.as_deref(), Some(X64_MONIKER));
        assert_eq!(x64.sha256.as_deref(), Some(PAYLOAD_X64_SHA));
        assert_eq!(x64.content_length, None, "v4 fixture 没有按架构尺寸");

        let arm =
            parse_mirror_manifest(&json, HostPlatform::Windows, HostArch::Arm64, None).unwrap();
        assert_eq!(arm.package_moniker.as_deref(), Some(ARM64_MONIKER));
        assert_eq!(arm.sha256.as_deref(), Some(PAYLOAD_ARM64_SHA));

        let mac = parse_mirror_manifest(&json, HostPlatform::Macos, HostArch::Arm64, None).unwrap();
        assert_eq!(mac.sha256.as_deref(), Some(MAC_ARM64_SHA));

        let without_payload_sha = json
            .replace(
                &format!("\"sha256\": \"{PAYLOAD_X64_SHA}\""),
                "\"sha256\": \"\"",
            )
            .replace(
                &format!("\"sha256\": \"{PAYLOAD_ARM64_SHA}\""),
                "\"sha256\": \"\"",
            );
        let checksums = format!("{PAYLOAD_X64_SHA}  {X64_MONIKER}.Msix\n");
        let from_sums = parse_mirror_manifest(
            &without_payload_sha,
            HostPlatform::Windows,
            HostArch::X64,
            Some(&checksums),
        )
        .unwrap();
        assert_eq!(from_sums.sha256.as_deref(), Some(PAYLOAD_X64_SHA));
    }

    /// D-21：镜像升到 v5 后 manager 段与 SHA256SUMS 都没了，唯一能带的
    /// 完整性线索是按架构的 contentLength。
    #[test]
    fn v5_manifests_without_payload_shas_carry_the_content_length() {
        let json = r#"{
          "schemaVersion": 5,
          "generatedAt": "2026-08-15T10:14:57Z",
          "sources": {
            "windows": {
              "productId": "9PLM9XGG6VKS",
              "architecture": "x64",
              "version": "26.810.7004.0",
              "packageMoniker": "OpenAI.Codex_26.810.7004.0_x64__2p2nqsd0c76g0",
              "architectures": {
                "x64": {
                  "architecture": "x64",
                  "status": "downloadable",
                  "downloadable": true,
                  "version": "26.810.7004.0",
                  "packageMoniker": "OpenAI.Codex_26.810.7004.0_x64__2p2nqsd0c76g0",
                  "contentLength": 745309050
                }
              }
            },
            "macos": {
              "arm64": { "contentLength": 639631488 },
              "x64": {}
            }
          }
        }"#;

        let x64 = parse_mirror_manifest(json, HostPlatform::Windows, HostArch::X64, None).unwrap();
        assert_eq!(x64.sha256, None, "v5 没有载荷 sha");
        assert_eq!(x64.content_length, Some(745309050));

        let mac = parse_mirror_manifest(json, HostPlatform::Macos, HostArch::Arm64, None).unwrap();
        assert_eq!(mac.sha256, None);
        assert_eq!(mac.content_length, Some(639631488));

        let mac_x64 =
            parse_mirror_manifest(json, HostPlatform::Macos, HostArch::X64, None).unwrap();
        assert_eq!(mac_x64.content_length, None);
    }

    #[test]
    fn third_party_hosts_are_not_hardcoded_outside_sources_rs() {
        let lumio_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lumio");
        let forbidden = ["agentsmirror", "oaistatic", "displaycatalog"];
        let allowed = ["sources.rs", "windows_store.rs"];
        let mut offenders = Vec::new();
        let mut files = Vec::new();
        collect_rs_files(&lumio_root, &mut files);
        for path in files {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if allowed.iter().any(|name| *name == file_name) {
                continue;
            }
            let lower = text.to_ascii_lowercase();
            for host in forbidden {
                if lower.contains(host) {
                    offenders.push(format!("{}: {host}", path.display()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "third-party hosts leaked outside sources.rs / windows_store.rs: {offenders:?}"
        );
    }

    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
}
