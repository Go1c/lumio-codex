use super::HostArch;

const DOWNLOAD_FAILED: &str = "CODEX_APP_DOWNLOAD_FAILED";
const DISPLAYCATALOG_BASE: &str = "https://displaycatalog.mp.microsoft.com/v7.0/products/";
const FE3_URL: &str = "https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx";
const FE3_SECURED_URL: &str =
    "https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured";

const GET_COOKIE_TEMPLATE: &str = include_str!("windows_store/templates/GetCookie.xml");
const SYNC_UPDATES_TEMPLATE: &str = include_str!("windows_store/templates/SyncUpdates.xml");
const GET_EXTENDED_UPDATE_INFO2_TEMPLATE: &str =
    include_str!("windows_store/templates/GetExtendedUpdateInfo2.xml");

/// Anonymous MSA device ticket used only as a SOAP request-body placeholder.
/// Never implement Debug/Display; do not log the raw value.
struct AnonymousMsaToken(&'static str);

impl AnonymousMsaToken {
    fn as_request_body(&self) -> &'static str {
        self.0
    }
}

const ANONYMOUS_MSA_TOKEN: AnonymousMsaToken = AnonymousMsaToken(include_str!(
    "windows_store/templates/anonymous_msa_ticket.txt"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePackageCandidate {
    pub moniker: String,
    pub update_id: String,
    pub revision_id: String,
    pub architecture: String,
}

pub fn resolve_store_msix_url(product_id: &str, arch: HostArch) -> Result<String, String> {
    let runtime = tokio::runtime::Handle::try_current();
    match runtime {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle.block_on(resolve_store_msix_url_async(product_id, arch))
        }),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| DOWNLOAD_FAILED.to_string())?;
            rt.block_on(resolve_store_msix_url_async(product_id, arch))
        }
    }
}

async fn resolve_store_msix_url_async(product_id: &str, arch: HostArch) -> Result<String, String> {
    if product_id.trim().is_empty() {
        return Err(DOWNLOAD_FAILED.to_string());
    }
    let client = store_http_client()?;
    let category_id = fetch_wu_category_id(&client, product_id).await?;
    let cookie = get_cookie(&client).await?;
    let sync_xml = sync_updates(&client, cookie.as_str(), &category_id).await?;
    let candidates = parse_sync_update_candidates(&sync_xml)?;
    let chosen =
        pick_msix_url_for_arch(&candidates, arch).ok_or_else(|| DOWNLOAD_FAILED.to_string())?;
    let urls = get_file_urls(&client, &chosen.update_id, &chosen.revision_id).await?;
    pick_store_msix_url(&urls).ok_or_else(|| DOWNLOAD_FAILED.to_string())
}

pub fn resolve_store_msix_from_fixtures(
    arch: HostArch,
    sync_xml: &str,
    file_xml: &str,
) -> Result<String, String> {
    let candidates = parse_sync_update_candidates(sync_xml)?;
    pick_msix_url_for_arch(&candidates, arch).ok_or_else(|| DOWNLOAD_FAILED.to_string())?;
    pick_store_msix_url(&extract_file_urls(file_xml)).ok_or_else(|| DOWNLOAD_FAILED.to_string())
}

fn store_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Windows-Update-Agent/10.0.10011.16384 Client-Protocol/1.40")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| DOWNLOAD_FAILED.to_string())
}

async fn fetch_wu_category_id(
    client: &reqwest::Client,
    product_id: &str,
) -> Result<String, String> {
    let url = format!("{DISPLAYCATALOG_BASE}{product_id}?market=US&languages=en-US");
    let body: serde_json::Value = client
        .get(url)
        .send()
        .await
        .map_err(|_| DOWNLOAD_FAILED.to_string())?
        .error_for_status()
        .map_err(|_| DOWNLOAD_FAILED.to_string())?
        .json()
        .await
        .map_err(|_| DOWNLOAD_FAILED.to_string())?;
    let skus = body
        .pointer("/Product/DisplaySkuAvailabilities")
        .and_then(|value| value.as_array())
        .ok_or_else(|| DOWNLOAD_FAILED.to_string())?;
    for sku in skus {
        let Some(fd) = sku.pointer("/Sku/Properties/FulfillmentData") else {
            continue;
        };
        let inner: serde_json::Value = match fd {
            serde_json::Value::String(raw) => {
                serde_json::from_str(raw).map_err(|_| DOWNLOAD_FAILED.to_string())?
            }
            other => other.clone(),
        };
        if let Some(id) = inner.get("WuCategoryId").and_then(|value| value.as_str()) {
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }
    Err(DOWNLOAD_FAILED.to_string())
}

async fn get_cookie(client: &reqwest::Client) -> Result<String, String> {
    let xml = post_soap(client, FE3_URL, GET_COOKIE_TEMPLATE).await?;
    extract_element_text(&xml, "EncryptedData").ok_or_else(|| DOWNLOAD_FAILED.to_string())
}

async fn sync_updates(
    client: &reqwest::Client,
    cookie: &str,
    category_id: &str,
) -> Result<String, String> {
    let body = SYNC_UPDATES_TEMPLATE
        .replacen("{0}", cookie, 1)
        .replacen("{1}", category_id, 1)
        .replacen("{2}", ANONYMOUS_MSA_TOKEN.as_request_body().trim(), 1);
    let xml = post_soap(client, FE3_URL, &body).await?;
    Ok(html_decode(&xml))
}

async fn get_file_urls(
    client: &reqwest::Client,
    update_id: &str,
    revision_id: &str,
) -> Result<Vec<String>, String> {
    let body = GET_EXTENDED_UPDATE_INFO2_TEMPLATE
        .replacen("{0}", update_id, 1)
        .replacen("{1}", revision_id, 1)
        .replacen("{2}", ANONYMOUS_MSA_TOKEN.as_request_body().trim(), 1);
    let xml = post_soap(client, FE3_SECURED_URL, &body).await?;
    Ok(extract_file_urls(&xml))
}

async fn post_soap(client: &reqwest::Client, url: &str, body: &str) -> Result<String, String> {
    client
        .post(url)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .body(body.to_string())
        .send()
        .await
        .map_err(|_| DOWNLOAD_FAILED.to_string())?
        .error_for_status()
        .map_err(|_| DOWNLOAD_FAILED.to_string())?
        .text()
        .await
        .map_err(|_| DOWNLOAD_FAILED.to_string())
}

pub fn parse_sync_update_candidates(xml: &str) -> Result<Vec<StorePackageCandidate>, String> {
    let decoded = html_decode(xml);
    let mut out = Vec::new();
    let mut rest = decoded.as_str();
    while let Some(start) = find_open_tag(rest, "UpdateInfo") {
        let after_start = &rest[start..];
        let Some(end_rel) = find_close_tag(after_start, "UpdateInfo") else {
            break;
        };
        let block = &after_start[..end_rel];
        if let Some(candidate) = candidate_from_update_info(block) {
            out.push(candidate);
        }
        rest = &after_start[end_rel..];
    }
    Ok(out)
}

fn candidate_from_update_info(block: &str) -> Option<StorePackageCandidate> {
    if !contains_tag(block, "SecuredFragment") {
        return None;
    }
    let identity = first_open_tag(block, "UpdateIdentity")?;
    let update_id = xml_attr(identity, "UpdateID")?;
    let revision_id = xml_attr(identity, "RevisionNumber")?;
    let appx = first_open_tag(block, "AppxMetadata")?;
    let moniker = xml_attr(appx, "PackageMoniker")?;
    let architecture = moniker_arch(&moniker).unwrap_or_default();
    Some(StorePackageCandidate {
        moniker,
        update_id,
        revision_id,
        architecture,
    })
}

pub fn pick_msix_url_for_arch(
    candidates: &[StorePackageCandidate],
    arch: HostArch,
) -> Option<&StorePackageCandidate> {
    let wanted = match arch {
        HostArch::X64 => "x64",
        HostArch::Arm64 => "arm64",
    };
    candidates
        .iter()
        .filter(|candidate| {
            candidate.architecture.eq_ignore_ascii_case(wanted)
                && candidate.moniker.starts_with("OpenAI.Codex_")
        })
        .max_by(|left, right| {
            parse_version(moniker_version(&left.moniker).unwrap_or("")).cmp(&parse_version(
                moniker_version(&right.moniker).unwrap_or(""),
            ))
        })
}

pub fn pick_store_msix_url(urls: &[String]) -> Option<String> {
    urls.iter()
        .filter(|url| url.starts_with("http"))
        .filter(|url| !url.contains("ms-windows-store:"))
        .filter(|url| url.len() != 99)
        .cloned()
        .max_by_key(|url| url.len())
}

fn extract_file_urls(xml: &str) -> Vec<String> {
    let decoded = html_decode(xml);
    let mut urls = Vec::new();
    let mut rest = decoded.as_str();
    while let Some(start) = find_open_tag(rest, "FileLocation") {
        let after_start = &rest[start..];
        let Some(end_rel) = find_close_tag(after_start, "FileLocation") else {
            break;
        };
        let block = &after_start[..end_rel];
        if let Some(url) = extract_element_text(block, "Url") {
            if !url.is_empty() {
                urls.push(url);
            }
        }
        rest = &after_start[end_rel..];
    }
    urls
}

fn extract_element_text(xml: &str, tag: &str) -> Option<String> {
    let at = find_open_tag(xml, tag)?;
    let rest = &xml[at..];
    let gt = rest.find('>')?;
    if rest[..gt].trim_end().ends_with('/') {
        return None;
    }
    let after = &rest[gt + 1..];
    let close_at = find_matching_close(after, tag)?;
    let value = after[..close_at].trim();
    if value.is_empty() {
        None
    } else {
        Some(html_decode(value))
    }
}

fn find_matching_close(after: &str, tag: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(rel) = after[search..].find("</") {
        let start = search + rel;
        let rest = &after[start + 2..];
        let name = rest
            .split(|ch: char| ch == '>' || ch.is_whitespace())
            .next()?;
        let local = name.rsplit(':').next()?;
        if local == tag {
            return Some(start);
        }
        search = start + 2;
    }
    None
}

fn first_open_tag<'a>(xml: &'a str, local_name: &str) -> Option<&'a str> {
    let at = find_open_tag(xml, local_name)?;
    let rest = &xml[at..];
    let end = rest.find('>')?;
    Some(&rest[..=end])
}

fn find_open_tag(xml: &str, local_name: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = xml[search_from..].find(local_name) {
        let at = search_from + rel;
        let before = &xml[..at];
        let after = &xml[at + local_name.len()..];
        let starts_tag = before.ends_with('<')
            || (before.ends_with(':') && before[..before.len().saturating_sub(1)].ends_with('<'));
        if starts_tag && after.starts_with(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
        {
            let tag_start = before.rfind('<')?;
            return Some(tag_start);
        }
        search_from = at + local_name.len();
    }
    None
}

fn find_close_tag(xml: &str, local_name: &str) -> Option<usize> {
    let patterns = [format!("</{local_name}>"), format!("</{local_name} ")];
    let mut best = None;
    for pattern in patterns {
        if let Some(at) = xml.find(&pattern) {
            best = Some(best.map_or(at, |current: usize| current.min(at)));
        }
    }
    if best.is_none() {
        let needle = format!("/{local_name}>");
        if let Some(at) = xml.find(&needle) {
            return Some(at + needle.len());
        }
    }
    best.map(|at| at + format!("</{local_name}>").len())
}

fn contains_tag(xml: &str, local_name: &str) -> bool {
    find_open_tag(xml, local_name).is_some()
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        if let Some((_, rest)) = tag.split_once(&key) {
            if let Some((value, _)) = rest.split_once(quote) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(html_decode(trimmed));
                }
            }
        }
    }
    None
}

fn html_decode(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn moniker_version(moniker: &str) -> Option<&str> {
    moniker.split('_').nth(1)
}

fn moniker_arch(moniker: &str) -> Option<String> {
    moniker.split('_').nth(2).map(str::to_string)
}

fn parse_version(value: &str) -> Vec<u64> {
    value
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ANONYMOUS_MSA_TOKEN, GET_COOKIE_TEMPLATE, GET_EXTENDED_UPDATE_INFO2_TEMPLATE,
        SYNC_UPDATES_TEMPLATE, parse_sync_update_candidates, pick_msix_url_for_arch,
        pick_store_msix_url, resolve_store_msix_from_fixtures,
    };
    use crate::lumio::official_app_install::HostArch;

    const SYNC_FIXTURE: &str = r#"
<SyncUpdatesResult>
  <UpdateInfo>
    <UpdateIdentity UpdateID="upd-x64" RevisionNumber="11"/>
    <Properties>
      <SecuredFragment>FileUrl</SecuredFragment>
    </Properties>
    <ApplicabilityRules>
      <AppxMetadata PackageMoniker="OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0"/>
    </ApplicabilityRules>
    <Relationships>
      <UpdateIdentity UpdateID="prerequisite" RevisionNumber="1"/>
    </Relationships>
  </UpdateInfo>
  <UpdateInfo>
    <UpdateIdentity UpdateID="upd-arm64" RevisionNumber="22"/>
    <Properties>
      <SecuredFragment>FileUrl</SecuredFragment>
    </Properties>
    <ApplicabilityRules>
      <AppxMetadata PackageMoniker="OpenAI.Codex_26.602.3474.0_arm64__2p2nqsd0c76g0"/>
    </ApplicabilityRules>
  </UpdateInfo>
  <UpdateInfo>
    <UpdateIdentity UpdateID="upd-old-x64" RevisionNumber="9"/>
    <Properties>
      <SecuredFragment>FileUrl</SecuredFragment>
    </Properties>
    <ApplicabilityRules>
      <AppxMetadata PackageMoniker="OpenAI.Codex_25.0.0.0_x64__2p2nqsd0c76g0"/>
    </ApplicabilityRules>
  </UpdateInfo>
</SyncUpdatesResult>
"#;

    const FILE_X64: &str = r#"
<ExtendedUpdateInfo>
  <FileLocation>
    <Url>http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/blockmap</Url>
  </FileLocation>
  <FileLocation>
    <Url>https://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/OpenAI.Codex_x64.msix?P1=1&amp;P2=sig</Url>
  </FileLocation>
</ExtendedUpdateInfo>
"#;

    const FILE_ARM64: &str = r#"
<ExtendedUpdateInfo>
  <FileLocation>
    <Url>http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/blockmap</Url>
  </FileLocation>
  <FileLocation>
    <Url>https://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/OpenAI.Codex_arm64.msix?P1=1&amp;P2=sig</Url>
  </FileLocation>
</ExtendedUpdateInfo>
"#;

    #[test]
    fn store_fixture_picks_arch_matched_msix_url() {
        let candidates = parse_sync_update_candidates(SYNC_FIXTURE).unwrap();
        let x64 = pick_msix_url_for_arch(&candidates, HostArch::X64).unwrap();
        let arm = pick_msix_url_for_arch(&candidates, HostArch::Arm64).unwrap();
        assert_eq!(x64.moniker, "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0");
        assert_eq!(
            arm.moniker,
            "OpenAI.Codex_26.602.3474.0_arm64__2p2nqsd0c76g0"
        );

        let x64_url =
            resolve_store_msix_from_fixtures(HostArch::X64, SYNC_FIXTURE, FILE_X64).unwrap();
        let arm_url =
            resolve_store_msix_from_fixtures(HostArch::Arm64, SYNC_FIXTURE, FILE_ARM64).unwrap();
        assert!(x64_url.contains("OpenAI.Codex_x64.msix"));
        assert!(arm_url.contains("OpenAI.Codex_arm64.msix"));
        assert!(x64_url.starts_with("https://"));
        assert!(arm_url.starts_with("https://"));
        assert!(!x64_url.contains("ms-windows-store:"));
        assert!(!arm_url.contains("ms-windows-store:"));
        assert!(
            !pick_store_msix_url(&[x64_url, arm_url])
                .unwrap()
                .contains("ms-windows-store:")
        );
    }

    #[test]
    fn soap_templates_are_store_envelopes_not_store_ui() {
        assert!(GET_COOKIE_TEMPLATE.contains("GetCookie"));
        assert!(SYNC_UPDATES_TEMPLATE.contains("SyncUpdates"));
        assert!(GET_EXTENDED_UPDATE_INFO2_TEMPLATE.contains("GetExtendedUpdateInfo2"));
        assert!(SYNC_UPDATES_TEMPLATE.contains("{0}"));
        assert!(SYNC_UPDATES_TEMPLATE.contains("{1}"));
        assert!(SYNC_UPDATES_TEMPLATE.contains("{2}"));
        assert!(!GET_COOKIE_TEMPLATE.contains("ms-windows-store:"));
        assert!(!SYNC_UPDATES_TEMPLATE.contains("ms-windows-store:"));
        assert!(!GET_EXTENDED_UPDATE_INFO2_TEMPLATE.contains("ms-windows-store:"));
    }

    #[test]
    fn anonymous_msa_token_is_request_body_only() {
        let production = include_str!("windows_store.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(
            !production.contains("impl std::fmt::Debug for AnonymousMsaToken"),
            "MSA token must not implement Debug"
        );
        assert!(
            !production.contains("impl std::fmt::Display for AnonymousMsaToken"),
            "MSA token must not implement Display"
        );
        assert!(
            !production.contains("#[derive(Debug)]\nstruct AnonymousMsaToken")
                && !production.contains("#[derive(Debug, Clone)]\nstruct AnonymousMsaToken"),
            "MSA token must not derive Debug"
        );
        let _ = ANONYMOUS_MSA_TOKEN.as_request_body();
    }
}
