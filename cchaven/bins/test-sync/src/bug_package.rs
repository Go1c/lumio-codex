//! Redacted bug-package summary for Agent handoff after a self-test run.

use crate::selftest::{DiagnosticRunManifest, RunOutcome};
use serde::{Deserialize, Serialize};

pub const BUG_PACKAGE_SCHEMA: &str = "fns-bug-package-summary/1";

/// Compact, redacted handoff object for Agent investigation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BugPackageSummary {
    pub schema_version: String,
    pub run_id: String,
    pub profile: String,
    pub outcome: RunOutcome,
    pub last_passed_boundary: Option<String>,
    pub first_failed_boundary: Option<String>,
    pub scenario_ids: Vec<String>,
    pub event_ids: Vec<String>,
    pub artifact_paths: Vec<String>,
    /// One-line human/agent summary; never contains secrets.
    pub summary: String,
    pub redaction_summary: crate::selftest::RedactionSummary,
}

impl BugPackageSummary {
    pub fn from_manifest(manifest: &DiagnosticRunManifest) -> Self {
        let summary = build_summary(manifest);
        Self {
            schema_version: BUG_PACKAGE_SCHEMA.to_owned(),
            run_id: manifest.run_id.clone(),
            profile: manifest.profile.clone(),
            outcome: manifest.outcome,
            last_passed_boundary: manifest.last_passed_boundary.clone(),
            first_failed_boundary: manifest.first_failed_boundary.clone(),
            scenario_ids: manifest.scenario_ids.clone(),
            event_ids: manifest.event_ids.clone(),
            artifact_paths: manifest.artifact_paths.clone(),
            summary,
            redaction_summary: manifest.redaction_summary.clone(),
        }
    }
}

fn build_summary(manifest: &DiagnosticRunManifest) -> String {
    let last = manifest.last_passed_boundary.as_deref().unwrap_or("(none)");
    let first_fail = manifest
        .first_failed_boundary
        .as_deref()
        .unwrap_or("(none)");
    format!(
        "self-test profile={} outcome={:?} lastPassedBoundary={} firstFailedBoundary={} scenarios={} events={}",
        manifest.profile,
        manifest.outcome,
        last,
        first_fail,
        manifest.scenario_ids.len(),
        manifest.event_ids.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::{RedactionSummary, SCHEMA_VERSION_RUN};

    #[test]
    fn summary_is_derived_from_manifest_without_secrets() {
        let manifest = DiagnosticRunManifest {
            schema_version: SCHEMA_VERSION_RUN.to_owned(),
            run_id: "run-1".into(),
            started_at: "2026-08-10T10:00:00.000Z".into(),
            finished_at: Some("2026-08-10T10:01:00.000Z".into()),
            profile: "ci-isolation".into(),
            outcome: RunOutcome::Failed,
            last_passed_boundary: Some("transport".into()),
            first_failed_boundary: Some("server".into()),
            scenario_ids: vec!["bidirectional-soak-10m".into()],
            event_ids: vec!["e1".into()],
            artifact_paths: vec!["evidence/process.jsonl".into()],
            redaction_summary: RedactionSummary {
                secret_hits: 0,
                path_redactions: 1,
                fields_removed: 0,
            },
        };
        let package = BugPackageSummary::from_manifest(&manifest);
        assert_eq!(package.schema_version, BUG_PACKAGE_SCHEMA);
        assert!(package.summary.contains("transport"));
        assert!(package.summary.contains("server"));
        assert!(!package.summary.to_ascii_lowercase().contains("password"));
        assert!(!package.summary.to_ascii_lowercase().contains("jwt"));
    }
}
