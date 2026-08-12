//! Self-test orchestrator: profile gate, sandbox lifecycle, diagnostic-run/1
//! manifest, and fail-path cleanup.
//!
//! Lifecycle:
//! PRECHECK → PROVISION_SANDBOX → BASELINE → RUN_SCENARIOS →
//! COLLECT_EVIDENCE → CLASSIFY_BOUNDARY → CREATE_BUG_PACKAGE
//!
//! On any failure the orchestrator still collects evidence and runs cleanup.

use crate::bug_package::BugPackageSummary;
use crate::cleanup::{CleanupGuard, MockProcessKiller, ProcessKiller};
use crate::profile::{load_profile, SelfTestProfile};
use crate::{io_error, HarnessError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION_RUN: &str = "fns-diagnostic-run/1";

/// Ordered lifecycle stages for boundary classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleStage {
    Precheck,
    ProvisionSandbox,
    Baseline,
    RunScenarios,
    CollectEvidence,
    ClassifyBoundary,
    CreateBugPackage,
}

impl LifecycleStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Precheck => "PRECHECK",
            Self::ProvisionSandbox => "PROVISION_SANDBOX",
            Self::Baseline => "BASELINE",
            Self::RunScenarios => "RUN_SCENARIOS",
            Self::CollectEvidence => "COLLECT_EVIDENCE",
            Self::ClassifyBoundary => "CLASSIFY_BOUNDARY",
            Self::CreateBugPackage => "CREATE_BUG_PACKAGE",
        }
    }

    pub fn all() -> &'static [LifecycleStage] {
        &[
            Self::Precheck,
            Self::ProvisionSandbox,
            Self::Baseline,
            Self::RunScenarios,
            Self::CollectEvidence,
            Self::ClassifyBoundary,
            Self::CreateBugPackage,
        ]
    }
}

/// Terminal outcome for a diagnostic run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunOutcome {
    Passed,
    Failed,
    Cancelled,
    Timeout,
    Crashed,
}

/// Redaction counters included in the diagnostic-run/1 manifest.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSummary {
    pub secret_hits: u64,
    pub path_redactions: u64,
    pub fields_removed: u64,
}

/// Normalized `fns-diagnostic-run/1` manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRunManifest {
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
    pub redaction_summary: RedactionSummary,
}

impl DiagnosticRunManifest {
    pub fn validate_schema_version(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION_RUN {
            return Err(HarnessError::InvalidConfiguration(
                "diagnostic run schemaVersion must be fns-diagnostic-run/1",
            ));
        }
        Ok(())
    }

    pub fn write_json(&self, path: &Path) -> Result<()> {
        self.validate_schema_version()?;
        let mut encoded = serde_json::to_vec_pretty(self)?;
        encoded.push(b'\n');
        fs::write(path, encoded).map_err(|error| io_error(path, error))
    }

    pub fn read_json(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
        let manifest: Self = serde_json::from_slice(&bytes)?;
        manifest.validate_schema_version()?;
        Ok(manifest)
    }
}

/// Sandbox directories provisioned for a single run.
#[derive(Clone, Debug)]
pub struct Sandbox {
    pub run_id: String,
    pub root: PathBuf,
    pub workspace_a: PathBuf,
    pub workspace_b: PathBuf,
    pub state_a: PathBuf,
    pub state_b: PathBuf,
    pub evidence_root: PathBuf,
    pub temp_root: PathBuf,
}

/// Options controlling a self-test orchestration pass.
#[derive(Clone, Debug)]
pub struct SelfTestOptions {
    pub profile_path: PathBuf,
    /// Parent directory for sandboxes. Defaults to a system temp dir when `None`.
    pub sandbox_parent: Option<PathBuf>,
    /// Optional wall-clock budget for the whole run.
    pub timeout: Option<Duration>,
    /// When true, simulate a crash mid-run (test helper).
    pub simulate_crash: bool,
    /// When true, treat the run as cancelled before scenarios complete.
    pub simulate_cancel: bool,
    /// When true, treat the run as timed out before scenarios complete.
    pub simulate_timeout: bool,
    /// Optional mock PIDs to track (unit tests).
    pub mock_pids: Vec<i32>,
    /// Optional pre-seeded plaintext credential files under state dirs.
    pub seed_plaintext_creds: bool,
}

impl Default for SelfTestOptions {
    fn default() -> Self {
        Self {
            profile_path: PathBuf::new(),
            sandbox_parent: None,
            timeout: None,
            simulate_crash: false,
            simulate_cancel: false,
            simulate_timeout: false,
            mock_pids: Vec::new(),
            seed_plaintext_creds: false,
        }
    }
}

/// Result of a completed (or failed) self-test orchestration.
#[derive(Debug)]
pub struct SelfTestResult {
    pub profile: SelfTestProfile,
    pub sandbox: Sandbox,
    pub manifest: DiagnosticRunManifest,
    pub bug_package: BugPackageSummary,
    pub manifest_path: PathBuf,
    pub bug_package_path: PathBuf,
    pub cleanup: crate::cleanup::CleanupReport,
    /// Whether the sandbox root was removed by cleanup.
    pub sandbox_removed: bool,
}

/// Run the self-test orchestrator end-to-end with a mockable process killer.
pub fn run_self_test_with_killer(
    options: SelfTestOptions,
    killer: Box<dyn ProcessKiller>,
) -> Result<SelfTestResult> {
    let started_at = rfc3339_now();
    let run_id = new_run_id();

    // PRECHECK: load profile and enforce testOnly gate.
    let profile = match load_profile(&options.profile_path) {
        Ok(profile) => profile,
        Err(error) => return Err(error),
    };

    let mut cleanup = CleanupGuard::with_killer(killer);
    // PRECHECK succeeded (profile loaded + testOnly gate); provision overwrites.
    let mut last_passed: Option<LifecycleStage>;
    let mut first_failed: Option<LifecycleStage> = None;
    let mut outcome = RunOutcome::Passed;
    let mut event_ids = Vec::new();
    let mut artifact_paths = Vec::new();
    let redaction = RedactionSummary::default();
    let mut stage_error: Option<String> = None;

    // PROVISION_SANDBOX
    let sandbox = match provision_sandbox(&run_id, &options) {
        Ok(sandbox) => {
            last_passed = Some(LifecycleStage::ProvisionSandbox);
            for pid in &options.mock_pids {
                cleanup.track_pid(*pid);
            }
            // Tear down mutable test surfaces; keep evidence_root for the
            // diagnostic-run manifest and bug package handoff.
            cleanup.track_workspace(&sandbox.workspace_a);
            cleanup.track_workspace(&sandbox.workspace_b);
            cleanup.track_workspace(&sandbox.temp_root);
            cleanup.track_state_dir(&sandbox.state_a);
            cleanup.track_state_dir(&sandbox.state_b);
            if options.seed_plaintext_creds {
                seed_credentials(&sandbox)?;
            }
            sandbox
        }
        Err(error) => {
            let _ = cleanup.cleanup();
            return Err(error);
        }
    };

    // BASELINE
    match run_stage(LifecycleStage::Baseline, || {
        write_baseline(&sandbox, &profile)
    }) {
        Ok(events) => {
            last_passed = Some(LifecycleStage::Baseline);
            event_ids.extend(events);
            artifact_paths.push(relative_artifact(&sandbox, "baseline.json"));
        }
        Err(error) => {
            first_failed = Some(LifecycleStage::Baseline);
            outcome = RunOutcome::Failed;
            stage_error = Some(error.to_string());
        }
    }

    // RUN_SCENARIOS (unless already failed, or simulated fault)
    if first_failed.is_none() {
        if options.simulate_cancel {
            first_failed = Some(LifecycleStage::RunScenarios);
            outcome = RunOutcome::Cancelled;
            stage_error = Some("self-test cancelled".into());
        } else if options.simulate_timeout {
            first_failed = Some(LifecycleStage::RunScenarios);
            outcome = RunOutcome::Timeout;
            stage_error = Some("self-test timed out".into());
        } else if options.simulate_crash {
            first_failed = Some(LifecycleStage::RunScenarios);
            outcome = RunOutcome::Crashed;
            stage_error = Some("self-test crashed".into());
        } else if let Some(budget) = options.timeout {
            if budget == Duration::ZERO {
                first_failed = Some(LifecycleStage::RunScenarios);
                outcome = RunOutcome::Timeout;
                stage_error = Some("self-test timed out (zero budget)".into());
            } else {
                match run_scenarios(&sandbox, &profile) {
                    Ok(events) => {
                        last_passed = Some(LifecycleStage::RunScenarios);
                        event_ids.extend(events);
                        for scenario in &profile.scenarios {
                            artifact_paths.push(relative_artifact(
                                &sandbox,
                                &format!("scenario-{scenario}.json"),
                            ));
                        }
                    }
                    Err(error) => {
                        first_failed = Some(LifecycleStage::RunScenarios);
                        outcome = RunOutcome::Failed;
                        stage_error = Some(error.to_string());
                    }
                }
            }
        } else {
            match run_scenarios(&sandbox, &profile) {
                Ok(events) => {
                    last_passed = Some(LifecycleStage::RunScenarios);
                    event_ids.extend(events);
                    for scenario in &profile.scenarios {
                        artifact_paths.push(relative_artifact(
                            &sandbox,
                            &format!("scenario-{scenario}.json"),
                        ));
                    }
                }
                Err(error) => {
                    first_failed = Some(LifecycleStage::RunScenarios);
                    outcome = RunOutcome::Failed;
                    stage_error = Some(error.to_string());
                }
            }
        }
    }

    // COLLECT_EVIDENCE — always (even after cancel/timeout/crash/failure).
    match collect_evidence(&sandbox, stage_error.as_deref()) {
        Ok(paths) => {
            if first_failed.is_none() {
                last_passed = Some(LifecycleStage::CollectEvidence);
            }
            artifact_paths.extend(paths);
        }
        Err(error) => {
            if first_failed.is_none() {
                first_failed = Some(LifecycleStage::CollectEvidence);
                outcome = RunOutcome::Failed;
            }
            let _ = error;
        }
    }

    // CLASSIFY_BOUNDARY
    if first_failed.is_none() {
        last_passed = Some(LifecycleStage::ClassifyBoundary);
    }
    let last_passed_boundary = last_passed.map(|s| s.as_str().to_owned());
    let first_failed_boundary = first_failed.map(|s| s.as_str().to_owned());
    artifact_paths.push(relative_artifact(&sandbox, "classification.json"));
    let _ = write_classification(
        &sandbox,
        last_passed_boundary.as_deref(),
        first_failed_boundary.as_deref(),
        outcome,
    );

    // CREATE_BUG_PACKAGE (always emit summary for Agent handoff)
    let finished_at = rfc3339_now();
    let mut manifest = DiagnosticRunManifest {
        schema_version: SCHEMA_VERSION_RUN.to_owned(),
        run_id: run_id.clone(),
        started_at,
        finished_at: Some(finished_at),
        profile: profile.name.clone(),
        outcome,
        last_passed_boundary: last_passed_boundary.clone(),
        first_failed_boundary: first_failed_boundary.clone(),
        scenario_ids: profile.scenarios.clone(),
        event_ids,
        artifact_paths,
        redaction_summary: redaction,
    };

    let manifest_path = sandbox.evidence_root.join("diagnostic-run.json");
    let bug_package_path = sandbox.evidence_root.join("bug-package.json");
    if manifest.write_json(&manifest_path).is_err() {
        if first_failed.is_none() {
            outcome = RunOutcome::Failed;
            manifest.outcome = outcome;
            manifest.first_failed_boundary =
                Some(LifecycleStage::CreateBugPackage.as_str().to_owned());
        }
    } else if first_failed.is_none() {
        manifest.last_passed_boundary = Some(LifecycleStage::CreateBugPackage.as_str().to_owned());
    }
    manifest
        .artifact_paths
        .push(relative_artifact(&sandbox, "diagnostic-run.json"));
    manifest
        .artifact_paths
        .push(relative_artifact(&sandbox, "bug-package.json"));
    let _ = manifest.write_json(&manifest_path);

    let bug_package = BugPackageSummary::from_manifest(&manifest);
    let _ = write_json_private(&bug_package_path, &bug_package);

    // Always cleanup processes / temp workspaces / plaintext creds.
    let cleanup_report = cleanup.cleanup();
    let workspaces_removed = !sandbox.workspace_a.exists()
        && !sandbox.workspace_b.exists()
        && !sandbox.temp_root.exists()
        && !sandbox.state_a.exists()
        && !sandbox.state_b.exists();

    Ok(SelfTestResult {
        profile,
        sandbox,
        manifest,
        bug_package,
        manifest_path,
        bug_package_path,
        cleanup: cleanup_report,
        sandbox_removed: workspaces_removed,
    })
}

/// Convenience entry: OS process killer.
pub fn run_self_test(options: SelfTestOptions) -> Result<SelfTestResult> {
    run_self_test_with_killer(options, Box::new(crate::cleanup::OsProcessKiller))
}

/// Provision unique runId + temp roots + state dirs.
pub fn provision_sandbox(run_id: &str, options: &SelfTestOptions) -> Result<Sandbox> {
    validate_run_id(run_id)?;
    let parent = match &options.sandbox_parent {
        Some(path) => path.clone(),
        None => std::env::temp_dir(),
    };
    if !parent.is_absolute() {
        return Err(HarnessError::InvalidConfiguration(
            "sandbox parent must be an absolute path",
        ));
    }
    fs::create_dir_all(&parent).map_err(|error| io_error(&parent, error))?;
    let root = parent.join(format!("selftest-{run_id}"));
    if root.exists() {
        return Err(HarnessError::InvalidConfiguration(
            "self-test sandbox root already exists",
        ));
    }
    create_private_dir(&root)?;
    let temp_root = root.join("tmp");
    let workspace_a = root.join("workspace-a");
    let workspace_b = root.join("workspace-b");
    let state_a = root.join("state-a");
    let state_b = root.join("state-b");
    let evidence_root = root.join("evidence");
    for dir in [
        &temp_root,
        &workspace_a,
        &workspace_b,
        &state_a,
        &state_b,
        &evidence_root,
    ] {
        create_private_dir(dir)?;
    }
    Ok(Sandbox {
        run_id: run_id.to_owned(),
        root,
        workspace_a,
        workspace_b,
        state_a,
        state_b,
        evidence_root,
        temp_root,
    })
}

fn run_stage<F>(stage: LifecycleStage, body: F) -> Result<Vec<String>>
where
    F: FnOnce() -> Result<Vec<String>>,
{
    let _ = stage;
    body()
}

fn write_baseline(sandbox: &Sandbox, profile: &SelfTestProfile) -> Result<Vec<String>> {
    let path = sandbox.evidence_root.join("baseline.json");
    let payload = serde_json::json!({
        "stage": LifecycleStage::Baseline.as_str(),
        "profile": profile.name,
        "serverEndpoint": profile.server_endpoint,
        "workspaceA": sandbox.workspace_a.display().to_string(),
        "workspaceB": sandbox.workspace_b.display().to_string(),
    });
    write_json_private(&path, &payload)?;
    Ok(vec![format!("baseline-{}", sandbox.run_id)])
}

fn run_scenarios(sandbox: &Sandbox, profile: &SelfTestProfile) -> Result<Vec<String>> {
    let mut events = Vec::new();
    if profile.scenarios.is_empty() {
        // Empty scenario list is a valid dry-run of the orchestrator.
        let path = sandbox.evidence_root.join("scenario-empty.json");
        write_json_private(
            &path,
            &serde_json::json!({
                "stage": LifecycleStage::RunScenarios.as_str(),
                "scenarios": [],
                "status": "skipped-empty",
            }),
        )?;
        events.push(format!("scenario-empty-{}", sandbox.run_id));
        return Ok(events);
    }
    for scenario in &profile.scenarios {
        let safe = sanitize_scenario_id(scenario);
        let path = sandbox.evidence_root.join(format!("scenario-{safe}.json"));
        // Hard gate: injected-fault classifier suite (real code path). Never claim
        // a live dual-agent soak Passed without the controlled_ssh harness.
        if matches!(
            scenario.as_str(),
            "bidirectional-soak-10m"
                | "bidirectional_soak_10m"
                | "boundary-classifier-suite"
                | "injected-fault-classifier"
        ) {
            events.extend(run_soak_classifier_suite(sandbox, &path, scenario)?);
            continue;
        }
        write_json_private(
            &path,
            &serde_json::json!({
                "stage": LifecycleStage::RunScenarios.as_str(),
                "scenarioId": scenario,
                "status": "unsupported",
                "error": "scenario requires live dual-agent harness or is unknown",
            }),
        )?;
        return Err(HarnessError::InvalidConfiguration(
            "self-test scenario is not executable without the live dual-agent harness; refuse false Passed",
        ));
    }
    Ok(events)
}

/// Run the M4 injected-fault classifier suite against the real classifier.
///
/// This is the hard gate for soak scenarios. Live 10m dual-agent execution is
/// evidence-only (`controlled_ssh_e2e` / official e2e) and must not be faked.
fn run_soak_classifier_suite(
    sandbox: &Sandbox,
    path: &Path,
    scenario: &str,
) -> Result<Vec<String>> {
    use crate::boundary::{classify_last_passed_boundary, ChainSnapshot, ProgressBoundary};
    use crate::soak::{BidirectionalSoakConfig, BidirectionalSoakScenario};

    let config = BidirectionalSoakConfig::default();
    let scenario_plan = BidirectionalSoakScenario::plan(config.clone())?;
    let healthy = ChainSnapshot {
        observed_at_ms: 60_000,
        local_file_mutations: 4,
        watcher_events: 4,
        watcher_rescans: 1,
        outbox_entries: 0,
        local_intents: 0,
        transport_requests_sent: 4,
        transport_connection_idle: false,
        server_operations: 4,
        server_revision: 24,
        peer_stream_items: 4,
        peer_stream_revision: 24,
        stream_items_ready: 0,
        apply_journal_entries: 0,
        applied_revision: 24,
        pending_ack_revision: None,
        last_ack_revision: 24,
        runtime_degraded: false,
        ui_shows_running: true,
    };
    let healthy_class = classify_last_passed_boundary(&healthy);
    if healthy_class.last_passed != Some(ProgressBoundary::Ack)
        || healthy_class.first_failed.is_some()
    {
        return Err(HarnessError::InvalidConfiguration(
            "healthy chain must classify lastPassed=ack with no firstFailed",
        ));
    }

    // Drive every public injected-fault unit path that the classifier exposes.
    let suite_checks: [(&str, ChainSnapshot, Option<ProgressBoundary>); 8] = [
        ("healthy", healthy.clone(), None),
        (
            "watcher-starved",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 0,
                watcher_rescans: 0,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Watcher),
        ),
        (
            "outbox-starved",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 2,
                outbox_entries: 0,
                local_intents: 0,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Outbox),
        ),
        (
            "transport-idle",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 2,
                outbox_entries: 1,
                transport_requests_sent: 0,
                transport_connection_idle: true,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Transport),
        ),
        (
            "server-no-revision",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 2,
                outbox_entries: 0,
                transport_requests_sent: 1,
                server_operations: 0,
                server_revision: 0,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Server),
        ),
        (
            "apply-lag",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 2,
                transport_requests_sent: 1,
                server_operations: 1,
                server_revision: 7,
                peer_stream_items: 1,
                peer_stream_revision: 7,
                stream_items_ready: 1,
                apply_journal_entries: 1,
                applied_revision: 3,
                last_ack_revision: 3,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Apply),
        ),
        (
            "ack-pending",
            ChainSnapshot {
                local_file_mutations: 1,
                watcher_events: 2,
                transport_requests_sent: 1,
                server_operations: 1,
                server_revision: 7,
                peer_stream_items: 1,
                peer_stream_revision: 7,
                applied_revision: 7,
                pending_ack_revision: Some(7),
                last_ack_revision: 3,
                ..ChainSnapshot::default()
            },
            Some(ProgressBoundary::Ack),
        ),
        (
            "ui-false-online",
            ChainSnapshot {
                runtime_degraded: true,
                ui_shows_running: true,
                ..healthy.clone()
            },
            Some(ProgressBoundary::UiFalseOnline),
        ),
    ];

    let mut classifications = Vec::new();
    for (name, snap, expected_failed) in suite_checks {
        let class = classify_last_passed_boundary(&snap);
        if let Some(expected) = expected_failed {
            if class.first_failed != Some(expected) {
                return Err(HarnessError::InvalidConfiguration(
                    "injected-fault classifier did not label expected boundary",
                ));
            }
        } else if class.first_failed.is_some() {
            return Err(HarnessError::InvalidConfiguration(
                "healthy case unexpectedly failed a boundary",
            ));
        }
        classifications.push(serde_json::json!({
            "case": name,
            "lastPassedBoundary": class.last_passed_str(),
            "firstFailedBoundary": class.first_failed_str(),
            "expectedFailed": expected_failed.map(ProgressBoundary::as_str),
        }));
    }

    write_json_private(
        path,
        &serde_json::json!({
            "stage": LifecycleStage::RunScenarios.as_str(),
            "scenarioId": scenario,
            "status": "classifier-suite-ok",
            "liveDualAgent": false,
            "planSteps": scenario_plan.steps.len(),
            "scenarioPlanId": scenario_plan.scenario_id,
            "intervalMs": config.progress_interval_ms,
            "stallTimeoutMs": config.stall_timeout_ms,
            "durationMs": config.duration_ms,
            "healthyClassification": {
                "lastPassedBoundary": healthy_class.last_passed_str(),
                "firstFailedBoundary": healthy_class.first_failed_str(),
            },
            "injectedFaults": classifications,
            "sandbox": {
                "workspaceA": sandbox.workspace_a.display().to_string(),
                "workspaceB": sandbox.workspace_b.display().to_string(),
            },
            "note": "Hard gate is the injected-fault classifier suite on real classify_last_passed_boundary. Full 10m dual-agent live soak is evidence-only.",
        }),
    )?;
    Ok(vec![format!(
        "scenario-{}-classifier-suite-{}",
        sanitize_scenario_id(scenario),
        sandbox.run_id
    )])
}

fn collect_evidence(sandbox: &Sandbox, error: Option<&str>) -> Result<Vec<String>> {
    let path = sandbox.evidence_root.join("evidence-summary.json");
    write_json_private(
        &path,
        &serde_json::json!({
            "stage": LifecycleStage::CollectEvidence.as_str(),
            "error": error,
            "stateAExists": sandbox.state_a.exists(),
            "stateBExists": sandbox.state_b.exists(),
        }),
    )?;
    Ok(vec![relative_artifact(sandbox, "evidence-summary.json")])
}

fn write_classification(
    sandbox: &Sandbox,
    last_passed: Option<&str>,
    first_failed: Option<&str>,
    outcome: RunOutcome,
) -> Result<()> {
    let path = sandbox.evidence_root.join("classification.json");
    write_json_private(
        &path,
        &serde_json::json!({
            "stage": LifecycleStage::ClassifyBoundary.as_str(),
            "lastPassedBoundary": last_passed,
            "firstFailedBoundary": first_failed,
            "outcome": outcome,
        }),
    )
}

fn seed_credentials(sandbox: &Sandbox) -> Result<()> {
    for state in [&sandbox.state_a, &sandbox.state_b] {
        fs::write(state.join("token"), b"plaintext-test-token")
            .map_err(|error| io_error(state.join("token"), error))?;
        fs::write(state.join("ipc-token"), b"plaintext-ipc")
            .map_err(|error| io_error(state.join("ipc-token"), error))?;
        fs::write(state.join("state.sqlite"), b"not-a-secret")
            .map_err(|error| io_error(state.join("state.sqlite"), error))?;
    }
    Ok(())
}

fn relative_artifact(_sandbox: &Sandbox, name: &str) -> String {
    format!("evidence/{name}")
}

fn sanitize_scenario_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty()
        || run_id.len() > 80
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(HarnessError::InvalidConfiguration(
            "run ID must be an ASCII slug",
        ));
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

fn write_json_private(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(value)?;
    encoded.push(b'\n');
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    fs::write(path, &encoded).map_err(|error| io_error(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

fn rfc3339_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Compact UTC timestamp without chrono/time crates.
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let (year, month, day, hour, minute, second) = civil_from_days(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert Unix seconds to UTC civil components (proleptic Gregorian).
fn civil_from_days(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let second = secs % 60;
    let minutes = secs / 60;
    let minute = minutes % 60;
    let hours = minutes / 60;
    let hour = hours % 24;
    let days = hours / 24;
    // Algorithm from civil_from_days (Howard Hinnant).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hour, minute, second)
}

/// Test helper: build a mock killer with the given live PIDs.
pub fn mock_killer_with_pids(pids: &[i32]) -> MockProcessKiller {
    let killer = MockProcessKiller::new();
    for pid in pids {
        killer.register(*pid);
    }
    killer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::remove_plaintext_credentials;
    use std::io::Write;

    fn write_profile(dir: &Path, test_only: bool, name: &str) -> PathBuf {
        let path = dir.join(format!("{name}.json"));
        let mut file = fs::File::create(&path).expect("create profile");
        // Endpoint must satisfy loopback + explicit port PRECHECK (see profile.rs).
        write!(
            file,
            r#"{{
              "name": "{name}",
              "testOnly": {test_only},
              "serverEndpoint": "ws://127.0.0.1:9000/api/user/workspace-sync/v2",
              "sshHostAlias": "test-ssh",
              "scenarios": ["bidirectional-soak-10m"]
            }}"#
        )
        .expect("write profile");
        path
    }

    #[test]
    fn non_test_only_profile_is_rejected() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = write_profile(temporary.path(), false, "ordinary");
        let options = SelfTestOptions {
            profile_path: path,
            sandbox_parent: Some(temporary.path().join("sandboxes")),
            ..SelfTestOptions::default()
        };
        let error = run_self_test(options).expect_err("must reject ordinary project");
        assert!(
            matches!(error, HarnessError::ProfileRejected(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn missing_test_only_profile_is_rejected() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = temporary.path().join("missing-flag.json");
        fs::write(
            &path,
            r#"{
              "name": "no-flag",
              "serverEndpoint": "ws://127.0.0.1:9000/api/user/workspace-sync/v2",
              "sshHostAlias": "test-ssh",
              "scenarios": []
            }"#,
        )
        .expect("write");
        let error = load_profile(&path).expect_err("missing testOnly");
        assert!(matches!(error, HarnessError::ProfileRejected(_)));
    }

    #[test]
    fn test_only_profile_accepted_and_creates_sandbox() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = write_profile(temporary.path(), true, "ci-isolation");
        let sandboxes = temporary.path().join("sandboxes");
        fs::create_dir_all(&sandboxes).expect("sandboxes");
        let options = SelfTestOptions {
            profile_path: path,
            sandbox_parent: Some(sandboxes.clone()),
            ..SelfTestOptions::default()
        };
        // Capture sandbox root before cleanup removes it by tracking via custom flow.
        // run_self_test cleans up; assert via provision_sandbox + result fields.
        let killer = mock_killer_with_pids(&[]);
        let result =
            run_self_test_with_killer(options, Box::new(killer)).expect("self-test should pass");
        assert_eq!(result.profile.name, "ci-isolation");
        assert!(result.profile.test_only);
        assert_eq!(result.manifest.schema_version, SCHEMA_VERSION_RUN);
        assert_eq!(result.manifest.outcome, RunOutcome::Passed);
        assert_eq!(result.manifest.profile, "ci-isolation");
        assert_eq!(
            result.manifest.scenario_ids,
            vec!["bidirectional-soak-10m".to_owned()]
        );
        // Workspaces/state were cleaned; evidence root is retained for handoff.
        assert!(result.sandbox_removed);
        assert!(result.manifest_path.exists());
        assert!(result.bug_package_path.exists());
        assert!(!result.sandbox.workspace_a.exists());
        assert!(!result.sandbox.state_a.exists());
    }

    #[test]
    fn manifest_schema_version_is_diagnostic_run_v1() {
        let temporary = tempfile::tempdir().expect("temp");
        let path = write_profile(temporary.path(), true, "ci-isolation");
        let sandboxes = temporary.path().join("sandboxes");
        fs::create_dir_all(&sandboxes).expect("sandboxes");
        let result = run_self_test_with_killer(
            SelfTestOptions {
                profile_path: path,
                sandbox_parent: Some(sandboxes),
                ..SelfTestOptions::default()
            },
            Box::new(MockProcessKiller::new()),
        )
        .expect("run");
        result
            .manifest
            .validate_schema_version()
            .expect("schemaVersion");
        assert_eq!(result.manifest.schema_version, "fns-diagnostic-run/1");
        // Serialize round-trip uses camelCase keys expected by the contract.
        let encoded = serde_json::to_value(&result.manifest).expect("json");
        assert_eq!(
            encoded.get("schemaVersion").and_then(|v| v.as_str()),
            Some("fns-diagnostic-run/1")
        );
        assert!(encoded.get("runId").is_some());
        assert!(encoded.get("redactionSummary").is_some());
        assert!(encoded.get("lastPassedBoundary").is_some());
        assert!(encoded.get("firstFailedBoundary").is_some());
        assert!(encoded.get("scenarioIds").is_some());
        assert!(encoded.get("eventIds").is_some());
        assert!(encoded.get("artifactPaths").is_some());
    }

    #[test]
    fn cancel_cleanup_leaves_no_orphan_mock_processes_or_plaintext_creds() {
        fault_cleanup_leaves_clean(RunOutcome::Cancelled, |opts| {
            opts.simulate_cancel = true;
        });
    }

    #[test]
    fn timeout_cleanup_leaves_no_orphan_mock_processes_or_plaintext_creds() {
        fault_cleanup_leaves_clean(RunOutcome::Timeout, |opts| {
            opts.simulate_timeout = true;
        });
    }

    #[test]
    fn crash_cleanup_leaves_no_orphan_mock_processes_or_plaintext_creds() {
        fault_cleanup_leaves_clean(RunOutcome::Crashed, |opts| {
            opts.simulate_crash = true;
        });
    }

    fn fault_cleanup_leaves_clean(
        expected: RunOutcome,
        configure: impl FnOnce(&mut SelfTestOptions),
    ) {
        let temporary = tempfile::tempdir().expect("temp");
        let path = write_profile(temporary.path(), true, "ci-isolation");
        let sandboxes = temporary.path().join("sandboxes");
        fs::create_dir_all(&sandboxes).expect("sandboxes");
        let mock_pids = vec![4242, 4343];
        let killer = mock_killer_with_pids(&mock_pids);
        let mut options = SelfTestOptions {
            profile_path: path,
            sandbox_parent: Some(sandboxes.clone()),
            mock_pids: mock_pids.clone(),
            seed_plaintext_creds: true,
            ..SelfTestOptions::default()
        };
        configure(&mut options);
        let result = run_self_test_with_killer(options, Box::new(killer.clone())).expect("run");
        assert_eq!(result.manifest.outcome, expected);
        assert!(
            killer.alive_pids().is_empty(),
            "orphan mock processes remain: {:?}",
            killer.alive_pids()
        );
        assert!(result.sandbox_removed, "sandbox root must be removed");
        // No plaintext credential files remain under the sandboxes parent.
        assert_no_plaintext_under(&sandboxes);
        // Cleanup report recorded credential scrub before dir removal.
        assert!(
            !result.cleanup.removed_credential_files.is_empty()
                || result.cleanup.removed_state_dirs.len() >= 2,
            "expected credential or state-dir cleanup, got {:?}",
            result.cleanup
        );
    }

    fn assert_no_plaintext_under(root: &Path) {
        if !root.exists() {
            return;
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                assert!(
                    !name.contains("token")
                        && !name.contains("password")
                        && !name.contains("secret")
                        && !name.contains("credential"),
                    "plaintext credential leaked: {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn provision_sandbox_creates_unique_layout() {
        let temporary = tempfile::tempdir().expect("temp");
        let options = SelfTestOptions {
            sandbox_parent: Some(temporary.path().to_path_buf()),
            ..SelfTestOptions::default()
        };
        let sandbox = provision_sandbox("run-abc_1", &options).expect("provision");
        assert!(sandbox.root.exists());
        assert!(sandbox.workspace_a.exists());
        assert!(sandbox.workspace_b.exists());
        assert!(sandbox.state_a.exists());
        assert!(sandbox.state_b.exists());
        assert!(sandbox.evidence_root.exists());
        assert!(sandbox.temp_root.exists());
        assert_eq!(sandbox.run_id, "run-abc_1");
        // Keep root for inspection; drop via remove.
        fs::remove_dir_all(&sandbox.root).expect("cleanup");
    }

    #[test]
    fn scrub_helper_removes_seeded_tokens() {
        let temporary = tempfile::tempdir().expect("temp");
        let state = temporary.path().join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(state.join("token"), b"secret").expect("token");
        let removed = remove_plaintext_credentials(&state).expect("scrub");
        assert!(!state.join("token").exists());
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn lifecycle_stages_cover_required_sequence() {
        let names: Vec<_> = LifecycleStage::all().iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "PRECHECK",
                "PROVISION_SANDBOX",
                "BASELINE",
                "RUN_SCENARIOS",
                "COLLECT_EVIDENCE",
                "CLASSIFY_BOUNDARY",
                "CREATE_BUG_PACKAGE",
            ]
        );
    }
}
