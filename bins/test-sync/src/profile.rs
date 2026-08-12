//! Self-test profile loading and test-only gate.
//!
//! Ordinary (non-test) projects must never enter the self-test orchestrator.

use crate::{io_error, HarnessError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Named self-test profile. Only profiles with `test_only = true` are accepted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTestProfile {
    pub name: String,
    /// Hard gate: must be present and `true`. Missing or `false` is rejected.
    #[serde(default)]
    pub test_only: bool,
    pub server_endpoint: String,
    pub ssh_host_alias: String,
    #[serde(default)]
    pub scenarios: Vec<String>,
}

/// Canonical valid self-test server endpoint used by fixtures.
#[cfg(test)]
const VALID_SELFTEST_ENDPOINT: &str = "ws://127.0.0.1:9000/api/user/workspace-sync/v2";

impl SelfTestProfile {
    /// Reject profiles that are not explicitly marked test-only, and enforce
    /// the self-test endpoint contract (loopback IP + explicit port + workspace path).
    pub fn ensure_test_only(&self) -> Result<()> {
        if !self.test_only {
            return Err(HarnessError::ProfileRejected(
                "profile is not marked testOnly=true; ordinary projects are refused".into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(HarnessError::InvalidConfiguration(
                "self-test profile name must not be empty",
            ));
        }
        validate_server_endpoint(&self.server_endpoint)?;
        if self.ssh_host_alias.trim().is_empty() {
            return Err(HarnessError::InvalidConfiguration(
                "self-test profile sshHostAlias must not be empty",
            ));
        }
        Ok(())
    }
}

/// PRECHECK: `serverEndpoint` must be a parseable workspace-sync URL on a
/// loopback IP literal with an explicit port (matches `fns-transport::WorkspaceEndpoint`).
///
/// Hard-rejects non-loopback hosts (e.g. public VPS IPs) and URLs without an
/// explicit port so self-test cannot report Passed against an illegal endpoint.
fn validate_server_endpoint(endpoint: &str) -> Result<()> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(HarnessError::InvalidConfiguration(
            "self-test profile serverEndpoint must not be empty",
        ));
    }
    // Reuse transport crate rules: scheme ws, loopback IP literal (127.0.0.1 / ::1),
    // explicit port, exact workspace-sync v2 path. Hostnames like "localhost" are rejected.
    fns_transport::config::WorkspaceEndpoint::parse(endpoint).map_err(|_| {
        HarnessError::InvalidConfiguration(
            "self-test serverEndpoint must be ws://127.0.0.1|::1:<explicit-port>/api/user/workspace-sync/v2 (loopback IP literal required)",
        )
    })?;
    Ok(())
}

/// Load a profile from JSON or TOML based on file extension.
pub fn load_profile(path: &Path) -> Result<SelfTestProfile> {
    let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| HarnessError::InvalidConfiguration("self-test profile must be valid UTF-8"))?;
    parse_profile_text(path, text)
}

fn parse_profile_text(path: &Path, text: &str) -> Result<SelfTestProfile> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let profile = match extension.as_str() {
        "json" => serde_json::from_str(text)?,
        "toml" => parse_toml_profile(text)?,
        "" => detect_and_parse(text)?,
        _ => {
            // Prefer JSON; fall back to TOML for unknown extensions.
            detect_and_parse(text)?
        }
    };
    profile.ensure_test_only()?;
    Ok(profile)
}

fn detect_and_parse(text: &str) -> Result<SelfTestProfile> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        Ok(serde_json::from_str(text)?)
    } else {
        parse_toml_profile(text)
    }
}

/// Minimal TOML parser for the fixed self-test profile schema.
///
/// Avoids a new crate dependency: only the documented keys are supported.
fn parse_toml_profile(text: &str) -> Result<SelfTestProfile> {
    let mut name = None;
    let mut test_only = false;
    let mut server_endpoint = None;
    let mut ssh_host_alias = None;
    let mut scenarios = Vec::new();
    let mut in_scenarios = false;

    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line == "scenarios" || line.starts_with("scenarios") && line.contains('[') {
            // scenarios = ["a", "b"]  or  scenarios = [
            if let Some(bracket) = line.find('[') {
                let after = &line[bracket + 1..];
                if let Some(end) = after.find(']') {
                    scenarios = parse_toml_string_list(&after[..end])?;
                    in_scenarios = false;
                } else {
                    in_scenarios = true;
                    scenarios.extend(parse_toml_string_list(after)?);
                }
            }
            continue;
        }
        if in_scenarios {
            if line.starts_with(']') {
                in_scenarios = false;
                continue;
            }
            scenarios.extend(parse_toml_string_list(line)?);
            if line.contains(']') {
                in_scenarios = false;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => name = Some(parse_toml_string(value)?),
            "testOnly" | "test_only" => test_only = parse_toml_bool(value)?,
            "serverEndpoint" | "server_endpoint" => {
                server_endpoint = Some(parse_toml_string(value)?)
            }
            "sshHostAlias" | "ssh_host_alias" => ssh_host_alias = Some(parse_toml_string(value)?),
            "scenarios" => {
                if let Some(bracket) = value.find('[') {
                    let after = &value[bracket + 1..];
                    if let Some(end) = after.find(']') {
                        scenarios = parse_toml_string_list(&after[..end])?;
                    } else {
                        in_scenarios = true;
                        scenarios.extend(parse_toml_string_list(after)?);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(SelfTestProfile {
        name: name.ok_or(HarnessError::InvalidConfiguration(
            "self-test profile is missing name",
        ))?,
        test_only,
        server_endpoint: server_endpoint.ok_or(HarnessError::InvalidConfiguration(
            "self-test profile is missing serverEndpoint",
        ))?,
        ssh_host_alias: ssh_host_alias.ok_or(HarnessError::InvalidConfiguration(
            "self-test profile is missing sshHostAlias",
        ))?,
        scenarios,
    })
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' if !in_string => in_string = true,
            b'"' if in_string => in_string = false,
            b'#' if !in_string => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

fn parse_toml_string(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return Ok(value[1..value.len() - 1].to_owned());
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Ok(value[1..value.len() - 1].to_owned());
    }
    Err(HarnessError::InvalidConfiguration(
        "self-test TOML string values must be quoted",
    ))
}

fn parse_toml_bool(value: &str) -> Result<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(HarnessError::InvalidConfiguration(
            "self-test TOML boolean must be true or false",
        )),
    }
}

fn parse_toml_string_list(fragment: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for part in fragment.split(',') {
        let part = part.trim().trim_end_matches(']').trim();
        if part.is_empty() {
            continue;
        }
        items.push(parse_toml_string(part)?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_missing_test_only() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = temporary.path().join("ordinary.json");
        let mut file = fs::File::create(&path).expect("create");
        write!(
            file,
            r#"{{
              "name": "prod-like",
              "serverEndpoint": "{}",
              "sshHostAlias": "prod-ssh",
              "scenarios": ["bidirectional-soak-10m"]
            }}"#,
            VALID_SELFTEST_ENDPOINT
        )
        .expect("write");
        let error = load_profile(&path).expect_err("must reject");
        assert!(
            matches!(error, HarnessError::ProfileRejected(_)),
            "expected ProfileRejected, got {error:?}"
        );
    }

    #[test]
    fn rejects_explicit_false_test_only() {
        let profile = SelfTestProfile {
            name: "x".into(),
            test_only: false,
            server_endpoint: VALID_SELFTEST_ENDPOINT.into(),
            ssh_host_alias: "test-ssh".into(),
            scenarios: vec![],
        };
        assert!(matches!(
            profile.ensure_test_only(),
            Err(HarnessError::ProfileRejected(_))
        ));
    }

    #[test]
    fn rejects_non_loopback_server_endpoint() {
        let profile = SelfTestProfile {
            name: "remote-vps".into(),
            test_only: true,
            server_endpoint: "ws://108.80.81.15:9000/api/user/workspace-sync/v2".into(),
            ssh_host_alias: "test-ssh".into(),
            scenarios: vec![],
        };
        let error = profile.ensure_test_only().expect_err("must reject non-loopback");
        assert!(
            matches!(error, HarnessError::InvalidConfiguration(_)),
            "expected InvalidConfiguration, got {error:?}"
        );
        assert!(
            error.to_string().contains("loopback") || error.to_string().contains("serverEndpoint"),
            "error should mention endpoint contract, got {error}"
        );
    }

    #[test]
    fn rejects_missing_explicit_port() {
        let profile = SelfTestProfile {
            name: "no-port".into(),
            test_only: true,
            server_endpoint: "ws://127.0.0.1/api/user/workspace-sync/v2".into(),
            ssh_host_alias: "test-ssh".into(),
            scenarios: vec![],
        };
        let error = profile.ensure_test_only().expect_err("must reject missing port");
        assert!(
            matches!(error, HarnessError::InvalidConfiguration(_)),
            "expected InvalidConfiguration, got {error:?}"
        );
    }

    #[test]
    fn rejects_localhost_hostname() {
        // Match transport: only IP literals 127.0.0.1 / ::1, not "localhost".
        let profile = SelfTestProfile {
            name: "hostname".into(),
            test_only: true,
            server_endpoint: "ws://localhost:9000/api/user/workspace-sync/v2".into(),
            ssh_host_alias: "test-ssh".into(),
            scenarios: vec![],
        };
        assert!(matches!(
            profile.ensure_test_only(),
            Err(HarnessError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn accepts_valid_loopback_explicit_port_endpoint() {
        validate_server_endpoint(VALID_SELFTEST_ENDPOINT).expect("valid endpoint");
        let profile = SelfTestProfile {
            name: "ci-isolation".into(),
            test_only: true,
            server_endpoint: VALID_SELFTEST_ENDPOINT.into(),
            ssh_host_alias: "test-ssh".into(),
            scenarios: vec!["bidirectional-soak-10m".into()],
        };
        profile.ensure_test_only().expect("must accept");
    }

    #[test]
    fn accepts_json_test_only_profile() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = temporary.path().join("ci-isolation.json");
        fs::write(
            &path,
            format!(
                r#"{{
              "name": "ci-isolation",
              "testOnly": true,
              "serverEndpoint": "{VALID_SELFTEST_ENDPOINT}",
              "sshHostAlias": "test-ssh",
              "scenarios": ["bidirectional-soak-10m"]
            }}"#
            ),
        )
        .expect("write");
        let profile = load_profile(&path).expect("accept");
        assert_eq!(profile.name, "ci-isolation");
        assert!(profile.test_only);
        assert_eq!(profile.server_endpoint, VALID_SELFTEST_ENDPOINT);
        assert_eq!(profile.scenarios, vec!["bidirectional-soak-10m"]);
    }

    #[test]
    fn accepts_toml_test_only_profile() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = temporary.path().join("ci-isolation.toml");
        fs::write(
            &path,
            format!(
                r#"
name = "ci-isolation"
testOnly = true
serverEndpoint = "{VALID_SELFTEST_ENDPOINT}"
sshHostAlias = "test-ssh"
scenarios = ["bidirectional-soak-10m", "watcher-stall"]
"#
            ),
        )
        .expect("write");
        let profile = load_profile(&path).expect("accept");
        assert_eq!(profile.name, "ci-isolation");
        assert_eq!(
            profile.scenarios,
            vec!["bidirectional-soak-10m", "watcher-stall"]
        );
    }
}
