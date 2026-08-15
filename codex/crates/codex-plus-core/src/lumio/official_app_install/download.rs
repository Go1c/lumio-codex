use super::PackageSource;
use super::verify::verify_sha256;
use futures_util::StreamExt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DOWNLOAD_FAILED: &str = "CODEX_APP_DOWNLOAD_FAILED";
const VERIFY_FAILED: &str = "CODEX_APP_VERIFY_FAILED";

pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub bytes_total: Option<u64>,
}

pub fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or_default();
            format!("{}://{}{}", parsed.scheme(), host, parsed.path())
        }
        Err(_) => url
            .split_once(['?', '#'])
            .map(|(head, _)| head.to_string())
            .unwrap_or_else(|| url.to_string()),
    }
}

/// Download `source` into `dest`.
///
/// Production callers pass [`crate::lumio::product::cache_dir`]; a directory dest
/// becomes `dest/official-app/<last-path-segment>`. Tests may pass an explicit file Path.
pub async fn download_to_cache(
    source: &PackageSource,
    dest: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<PathBuf, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(DOWNLOAD_FAILED.to_string());
    }
    if !url_is_allowed(&source.url) {
        return Err(DOWNLOAD_FAILED.to_string());
    }
    if let Some(expected) = source.sha256.as_deref() {
        if !is_sha256_hex(expected) {
            return Err(VERIFY_FAILED.to_string());
        }
    }

    let ready = ready_path(dest, source);
    let part = part_path(&ready);
    if let Some(parent) = ready.parent() {
        std::fs::create_dir_all(parent).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    }

    let client = http_client()?;
    let response = client
        .get(&source.url)
        .send()
        .await
        .map_err(|_| DOWNLOAD_FAILED.to_string())?;
    if !response.status().is_success() {
        return Err(DOWNLOAD_FAILED.to_string());
    }

    let bytes_total = response.content_length();
    let mut file = std::fs::File::create(&part).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    let mut stream = response.bytes_stream();
    let mut bytes_downloaded = 0_u64;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            return Err(DOWNLOAD_FAILED.to_string());
        }
        let chunk = chunk.map_err(|_| DOWNLOAD_FAILED.to_string())?;
        file.write_all(&chunk)
            .map_err(|_| DOWNLOAD_FAILED.to_string())?;
        bytes_downloaded = bytes_downloaded.saturating_add(chunk.len() as u64);
        on_progress(DownloadProgress {
            bytes_downloaded,
            bytes_total,
        });
        if cancel.load(Ordering::SeqCst) {
            let _ = file.sync_all();
            return Err(DOWNLOAD_FAILED.to_string());
        }
    }
    file.sync_all().map_err(|_| DOWNLOAD_FAILED.to_string())?;
    drop(file);

    if let Some(expected) = source.sha256.as_deref() {
        if let Err(err) = verify_sha256(&part, expected) {
            // Leave `.part`; never promote it, and do not delete a previous ready file.
            return Err(err);
        }
    }

    // 镜像 v5 撤掉 SHA256SUMS 后的尺寸防线：已知期望尺寸时不许静默落盘
    // 与清单不符的载荷（D-21）。
    if let Some(expected) = source.expected_bytes {
        let actual = std::fs::metadata(&part)
            .map_err(|_| DOWNLOAD_FAILED.to_string())?
            .len();
        if actual != expected {
            return Err(DOWNLOAD_FAILED.to_string());
        }
    }

    if ready.exists() {
        std::fs::remove_file(&ready).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    }
    std::fs::rename(&part, &ready).map_err(|_| DOWNLOAD_FAILED.to_string())?;
    Ok(ready)
}

fn http_client() -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .user_agent(format!("LumioCodex/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        // 官方包 600–750MB，慢链路也要装得上。取消只在 chunk 边界生效，
        // body 停滞时由该总超时兜底（D-17 遗留：读空闲超时待补）。
        .timeout(Duration::from_secs(3600))
        // 镜像 302 到自己的 CDN；跟随但每一跳必须仍是 https（测试里 wiremock 只会说 http）。
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let https = url.scheme() == "https" && url.host_str().is_some();
            let test_local = cfg!(test)
                && url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
            if (https || test_local) && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }));
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().map_err(|_| DOWNLOAD_FAILED.to_string())
}

fn url_is_allowed(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() == "https" {
        return parsed.host_str().is_some();
    }
    if parsed.scheme() == "http" && super::windows_store::is_microsoft_delivery_host(&parsed) {
        return true;
    }
    // wiremock speaks HTTP only; production builds still reject every other http URL.
    cfg!(test)
        && parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

fn is_sha256_hex(value: &str) -> bool {
    let value = value.trim();
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ready_path(dest: &Path, source: &PackageSource) -> PathBuf {
    // Production dest is `product::cache_dir()`; tests may pass an explicit file Path.
    if dest.is_dir() {
        dest.join("official-app").join(format!(
            "{}{}",
            package_name(source),
            payload_ext(source.platform)
        ))
    } else {
        dest.to_path_buf()
    }
}

/// 缓存文件必须带平台扩展名：Windows 的签名与部署工具靠它认出包格式，
/// 裸文件名是「校验未通过」的头号嫌疑（D-21）。
fn payload_ext(platform: super::HostPlatform) -> &'static str {
    match platform {
        super::HostPlatform::Windows => ".msix",
        super::HostPlatform::Macos => ".dmg",
    }
}

#[cfg(test)]
async fn download_first_available(
    sources: &[PackageSource],
    dest: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<PathBuf, String> {
    let mut last_error = DOWNLOAD_FAILED.to_string();
    for source in sources {
        match download_to_cache(source, dest, cancel, on_progress).await {
            Ok(path) => return Ok(path),
            Err(err) if err == VERIFY_FAILED => return Err(err),
            Err(err) => last_error = err,
        }
    }
    Err(last_error)
}

fn package_name(source: &PackageSource) -> String {
    reqwest::Url::parse(&source.url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|segments| segments.filter(|segment| !segment.is_empty()).last())
                .map(str::to_string)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "package".to_string())
}

fn part_path(ready: &Path) -> PathBuf {
    let mut name = ready.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    ready.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lumio::official_app_install::verify::verify_sha256;
    use crate::lumio::official_app_install::{HostPlatform, SourceKind};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::Ordering;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BODY: &[u8] = b"official-codex-app-fixture\n";

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn source(url: impl Into<String>, sha256: Option<String>) -> PackageSource {
        PackageSource {
            kind: SourceKind::Mirror,
            platform: HostPlatform::Macos,
            url: url.into(),
            sha256,
            expected_bytes: None,
        }
    }

    /// D-21：镜像 v5 撤掉 SHA256SUMS 期间至少要有尺寸防线；缓存文件必须带
    /// 平台扩展名——裸文件名会让 Get-AuthenticodeSignature 认不出 Appx 签名
    /// （真机「校验未通过」的头号嫌疑）。
    #[tokio::test]
    async fn windows_payloads_cache_with_the_msix_extension() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let mut win = source(format!("{}/latest/win-x64", server.uri()), None);
        win.platform = HostPlatform::Windows;
        let path = download_to_cache(&win, dest.path(), &AtomicBool::new(false), &mut |_| {})
            .await
            .unwrap();

        assert!(
            path.to_string_lossy().ends_with("win-x64.msix"),
            "cache file must carry the msix extension: {}",
            path.display()
        );
    }

    #[tokio::test]
    async fn a_wrong_sized_payload_is_rejected_even_without_a_hash() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let mut win = source(format!("{}/latest/win-x64", server.uri()), None);
        win.platform = HostPlatform::Windows;
        win.expected_bytes = Some(BODY.len() as u64 + 1);

        let err = download_to_cache(&win, dest.path(), &AtomicBool::new(false), &mut |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");
    }

    #[tokio::test]
    async fn an_expected_size_equal_to_the_payload_passes() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let mut win = source(format!("{}/latest/win-x64", server.uri()), None);
        win.platform = HostPlatform::Windows;
        win.expected_bytes = Some(BODY.len() as u64);

        download_to_cache(&win, dest.path(), &AtomicBool::new(false), &mut |_| {})
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn https_download_writes_to_cache_and_hashes() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let expected = sha256_hex(BODY);
        let cancel = AtomicBool::new(false);
        let path = download_to_cache(
            &source(
                format!("{}/latest/win-x64", server.uri()),
                Some(expected.clone()),
            ),
            dest.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap();

        // helper 默认 macOS 平台：扩展名取 .dmg（平台由字段决定，不看 URL 字样）。
        assert_eq!(path, dest.path().join("official-app").join("win-x64.dmg"));
        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        assert!(!path.with_file_name("win-x64.dmg.part").exists());
        verify_sha256(&path, &expected).unwrap();
    }

    #[tokio::test]
    async fn http_url_is_rejected_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let err = download_to_cache(
            &PackageSource {
                kind: SourceKind::Mirror,
                platform: HostPlatform::Macos,
                url: "http://example.com/x".into(),
                sha256: None,
                expected_bytes: None,
            },
            dir.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");
        assert!(!dir.path().join("official-app").exists());
    }

    #[tokio::test]
    async fn sha_mismatch_is_verify_failed_and_does_not_keep_a_ready_file() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let err = download_to_cache(
            &source(
                format!("{}/latest/win-x64", server.uri()),
                Some("0".repeat(64)),
            ),
            dest.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap_err();

        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
        let ready = dest.path().join("official-app").join("win-x64.dmg");
        assert!(!ready.exists(), "hash mismatch must not keep a ready file");
    }

    #[tokio::test]
    async fn cancel_leaves_partial_and_does_not_touch_codex_home_config() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "model = \"keep-me\"\n").unwrap();
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pkg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 256 * 1024]))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let mut first = true;
        let err = download_to_cache(
            &source(format!("{}/pkg", server.uri()), None),
            dest.path(),
            &cancel,
            &mut |_| {
                if first {
                    first = false;
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");

        let ready = dest.path().join("official-app").join("pkg.dmg");
        let part = dest.path().join("official-app").join("pkg.dmg.part");
        assert!(!ready.exists());
        assert!(part.exists(), "cancel must keep the .part file");
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            "model = \"keep-me\"\n"
        );
    }

    #[tokio::test]
    async fn first_source_failure_lets_caller_try_the_next() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/mirror"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/official"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let sources = [
            PackageSource {
                kind: SourceKind::Mirror,
                platform: HostPlatform::Macos,
                url: format!("{}/mirror", server.uri()),
                sha256: None,
                expected_bytes: None,
            },
            PackageSource {
                kind: SourceKind::Official,
                platform: HostPlatform::Macos,
                url: format!("{}/official", server.uri()),
                sha256: None,
                expected_bytes: None,
            },
        ];

        let path = download_first_available(&sources, dest.path(), &cancel, &mut |_| {})
            .await
            .expect("caller should succeed on the next source");
        assert_eq!(std::fs::read(path).unwrap(), BODY);
    }

    #[tokio::test]
    async fn explicit_dest_file_is_honored() {
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("custom-name");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pkg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let path = download_to_cache(
            &source(format!("{}/pkg", server.uri()), Some(sha256_hex(BODY))),
            &dest,
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap();

        assert_eq!(path, dest);
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert!(!dest.with_file_name("custom-name.part").exists());
    }

    #[tokio::test]
    async fn network_failure_is_download_failed() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let err = download_to_cache(
            &source(format!("{}/missing", server.uri()), None),
            dest.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");
    }

    #[test]
    fn only_https_or_microsoft_delivery_http_passes_the_gate() {
        assert!(url_is_allowed("https://mirror.example.org/latest/win-x64"));
        assert!(url_is_allowed(
            "http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/x?P1=1"
        ));
        assert!(!url_is_allowed("http://example.com/x"));
        assert!(!url_is_allowed(
            "http://dl.delivery.mp.microsoft.com.evil.com/x"
        ));
        assert!(!url_is_allowed("ftp://dl.delivery.mp.microsoft.com/x"));
        assert!(url_is_allowed("http://127.0.0.1:1234/x"));
    }

    #[tokio::test]
    async fn mirror_redirect_chain_is_followed_to_the_final_200() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/real/win-x64"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/real/win-x64"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let path = download_to_cache(
            &source(format!("{}/latest/win-x64", server.uri()), None),
            dest.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .expect("302 到同源 https/测试本机目标必须跟随到最终 200");

        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        assert!(!path.with_file_name("win-x64.dmg.part").exists());
    }

    #[tokio::test]
    async fn redirect_to_a_foreign_http_host_is_not_followed() {
        let dest = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest/win-x64"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://example.com/pkg"),
            )
            .mount(&server)
            .await;

        let cancel = AtomicBool::new(false);
        let err = download_to_cache(
            &source(format!("{}/latest/win-x64", server.uri()), None),
            dest.path(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err, "CODEX_APP_DOWNLOAD_FAILED");
        assert!(!dest.path().join("official-app").join("win-x64").exists());
    }

    #[test]
    fn redact_url_keeps_scheme_host_path_only() {
        let redacted = redact_url("https://cdn.example/file?token=secret");
        assert_eq!(redacted, "https://cdn.example/file");
        assert!(!redacted.contains("token=secret"));
        assert!(!redacted.contains('?'));
    }
}
