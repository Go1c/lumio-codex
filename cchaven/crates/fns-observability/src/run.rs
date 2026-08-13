use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION_RUN: &str = "fns-diagnostic-run/1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunOutcome {
    Passed,
    Failed,
    Cancelled,
    Timeout,
    Crashed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSummaryDto {
    pub secret_hits: u64,
    pub path_redactions: u64,
    pub fields_removed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRun {
    pub schema_version: String,
    pub run_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub profile: String,
    pub outcome: RunOutcome,
    pub last_passed_boundary: Option<String>,
    pub first_failed_boundary: Option<String>,
    pub scenario_ids: Vec<String>,
    pub event_ids: Vec<String>,
    pub artifact_paths: Vec<String>,
    pub redaction_summary: RedactionSummaryDto,
}

impl DiagnosticRun {
    pub fn new(
        run_id: impl Into<String>,
        profile: impl Into<String>,
        started_at: impl Into<String>,
        outcome: RunOutcome,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_RUN.to_string(),
            run_id: run_id.into(),
            started_at: started_at.into(),
            finished_at: None,
            profile: profile.into(),
            outcome,
            last_passed_boundary: None,
            first_failed_boundary: None,
            scenario_ids: Vec::new(),
            event_ids: Vec::new(),
            artifact_paths: Vec::new(),
            redaction_summary: RedactionSummaryDto {
                secret_hits: 0,
                path_redactions: 0,
                fields_removed: 0,
            },
        }
    }
}
