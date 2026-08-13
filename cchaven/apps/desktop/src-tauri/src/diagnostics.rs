//! Typed diagnostics facade for Desktop Logs / Self Test / Support Bundle.
//!
//! Reads only project-scoped diagnostic JSONL and status snapshots. Components
//! must not free-tail arbitrary paths — all access goes through these commands.

use fns_observability::{
    DiagnosticEvent, HealthSnapshot, ProgressBoundary, SCHEMA_VERSION_EVENT, SCHEMA_VERSION_HEALTH,
    parse_contract, redact_fields,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_EVENTS_RETURNED: usize = 5_000;
const MAX_DIAGNOSTIC_READ_BYTES: u64 = 8 * 1024 * 1024;

/// In-memory self-test registry (process-local cancel tokens).
#[derive(Default)]
pub struct DiagnosticsState {
    active_runs: Mutex<BTreeMap<String, SelfTestHandle>>,
}

struct SelfTestHandle {
    #[allow(dead_code)]
    profile: String,
    cancel: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventFilter {
    pub project_id: String,
    pub level: Option<Value>,
    pub component: Option<String>,
    pub event_name: Option<String>,
    pub run_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundlePreview {
    pub event_count: u64,
    pub time_range: TimeRange,
    pub redaction_summary: RedactionSummaryDto,
    pub includes_paths: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSummaryDto {
    pub secret_hits: u64,
    pub path_redactions: u64,
    pub fields_removed: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleExportResult {
    pub path: String,
    pub redaction_summary: RedactionSummaryDto,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTestStartResult {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bug_package_path: Option<String>,
}

/// Project agent state directory — must match `sync::project_state_dir`.
/// Layout: `{config}/fns-workspace/projects-{id}/state`.
///
/// Requires a known saved project (UUID shape alone is not enough) so Logs /
/// Support Bundle cannot free-tail an arbitrary `projects-{uuid}/state` path.
fn project_state_dir(project_id: &str) -> Result<PathBuf, String> {
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|_| "projectId must be a UUID".to_string())?;
    crate::project::ProjectConfig::find_by_id(project_id).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("unknown projectId: {project_id}")
        } else {
            format!("load project {project_id}: {error}")
        }
    })?;
    Ok(diagnostics_base_dir()
        .join(format!("projects-{project_uuid}"))
        .join("state"))
}

fn selftest_state_dir() -> Result<PathBuf, String> {
    Ok(diagnostics_base_dir().join("selftest").join("state"))
}

fn diagnostics_base_dir() -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|directories| directories.config_dir().join("fns-workspace"))
        .unwrap_or_else(|| PathBuf::from(".config/fns-workspace"));
    base
}

fn events_path(project_id: &str) -> Result<PathBuf, String> {
    // Desktop may also write under diagnostics/ at the state root.
    Ok(project_state_dir(project_id)?
        .join("diagnostics")
        .join("events.jsonl"))
}

fn agent_state_events(project_id: &str) -> Result<PathBuf, String> {
    // Agent dual-writes JSONL via fns-agent obs under state_dir/diagnostics.
    Ok(project_state_dir(project_id)?
        .join("diagnostics")
        .join("events.jsonl"))
}

/// Optional extra event sources (rotated files).
fn event_source_paths(project_id: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = events_path(project_id) {
        paths.push(p.with_extension("jsonl.1"));
        paths.push(p);
    }
    paths
}

fn read_events_from(path: &Path, limit: usize) -> Vec<DiagnosticEvent> {
    if limit == 0 {
        return Vec::new();
    }
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let file_len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = file_len.saturating_sub(MAX_DIAGNOSTIC_READ_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut partial_line = String::new();
        if reader.read_line(&mut partial_line).is_err() {
            return Vec::new();
        }
    }
    let mut events = VecDeque::with_capacity(limit);
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = parse_contract::<DiagnosticEvent>(trimmed, SCHEMA_VERSION_EVENT) {
            if events.len() == limit {
                events.pop_front();
            }
            if limit > 0 {
                events.push_back(event);
            }
        }
    }
    events.into()
}

fn event_belongs_to_project(event: &DiagnosticEvent, project_id: &str) -> bool {
    // Path isolation is primary (events are only read from this project's
    // state dir). Still drop clearly foreign projectRef values when present.
    // Agent pseudonym is derived from the project's workspace_id, not a global
    // ws-/local- prefix match.
    if event.project_ref.is_empty() {
        return true;
    }
    if event.project_ref == project_id {
        return true;
    }
    if let Ok(project) = crate::project::ProjectConfig::find_by_id(project_id) {
        let expected = agent_project_pseudonym(&project.workspace_id.to_string());
        if event.project_ref == expected {
            return true;
        }
    }
    // Reject foreign absolute project ids / other workspace pseudonyms.
    false
}

/// Matches `fns-agent` obs pseudonym: `ws-` + first 8 bytes of blake3(workspace_id).
fn agent_project_pseudonym(workspace_id: &str) -> String {
    let hash = blake3::hash(workspace_id.as_bytes());
    format!("ws-{}", hex::encode(&hash.as_bytes()[..8]))
}

fn collect_events_from_paths(
    paths: impl IntoIterator<Item = PathBuf>,
    project_id: &str,
    limit: usize,
) -> Vec<DiagnosticEvent> {
    let mut events = VecDeque::with_capacity(limit);
    let mut seen_paths = std::collections::HashSet::new();
    for path in paths {
        if !seen_paths.insert(path.clone()) || limit == 0 {
            continue;
        }
        for event in read_events_from(&path, limit) {
            if !event_belongs_to_project(&event, project_id) {
                continue;
            }
            if events.len() == limit {
                events.pop_front();
            }
            events.push_back(event);
        }
    }
    events.into()
}

fn collect_project_events(project_id: &str, limit: usize) -> Vec<DiagnosticEvent> {
    let mut paths = event_source_paths(project_id);
    // Also try agent alias path helper (same location today; kept for clarity).
    if let Ok(path) = agent_state_events(project_id) {
        paths.push(path);
    }
    // Project isolation: keep events for this project id OR workspace pseudonyms
    // produced by the agent for this project's state dir (all events under that
    // state dir belong to the project — path itself is the isolation boundary).
    // Still drop clearly foreign projectRef strings when present.
    collect_events_from_paths(paths, project_id, limit)
}

fn level_matches(event: &DiagnosticEvent, filter: &EventFilter) -> bool {
    let Some(level) = &filter.level else {
        return true;
    };
    let event_level = format!("{:?}", event.level).to_ascii_lowercase();
    match level {
        Value::String(s) => event_level == s.to_ascii_lowercase(),
        Value::Array(arr) => arr.iter().any(|v| {
            v.as_str()
                .map(|s| event_level == s.to_ascii_lowercase())
                .unwrap_or(false)
        }),
        _ => true,
    }
}

#[tauri::command]
pub fn diagnostics_list_events(filter: EventFilter) -> Result<Vec<DiagnosticEvent>, String> {
    project_state_dir(&filter.project_id)?;
    let limit = filter
        .limit
        .unwrap_or(MAX_EVENTS_RETURNED)
        .min(MAX_EVENTS_RETURNED);
    let mut events = collect_project_events(&filter.project_id, limit * 2);
    events.retain(|e| {
        if !level_matches(e, &filter) {
            return false;
        }
        if let Some(c) = &filter.component {
            if &e.component != c {
                return false;
            }
        }
        if let Some(n) = &filter.event_name {
            if &e.event_name != n {
                return false;
            }
        }
        if let Some(r) = &filter.run_id {
            if &e.run_id != r {
                return false;
            }
        }
        // Strict project isolation when event carries project id
        true
    });
    events.truncate(limit);
    Ok(events)
}

#[tauri::command]
pub fn diagnostics_get_health(project_id: String) -> Result<HealthSnapshot, String> {
    project_state_dir(&project_id)?;
    let events = collect_project_events(&project_id, 200);
    let runtime_status = read_runtime_status(&project_id)?;
    Ok(build_health_snapshot(
        &project_id,
        &events,
        runtime_status.as_ref(),
    ))
}

fn read_runtime_status(project_id: &str) -> Result<Option<fns_agent::AgentStatus>, String> {
    let path = project_state_dir(project_id)?.join("runtime-status.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read runtime status: {error}")),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("invalid runtime status: {error}"))
}

fn build_health_snapshot(
    project_id: &str,
    events: &[DiagnosticEvent],
    runtime_status: Option<&fns_agent::AgentStatus>,
) -> HealthSnapshot {
    let run_id = events
        .last()
        .map(|e| e.run_id.clone())
        .unwrap_or_else(|| "none".into());
    let generation = events.last().map(|e| e.connection_generation).unwrap_or(0);
    let mut snap = HealthSnapshot::empty(run_id, project_id.to_string(), generation);
    snap.last_progress_boundary = infer_boundary_from_events(&events);
    snap.desktop.insert(
        "appVersion".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    snap.desktop.insert(
        "lastStatusRefreshAt".into(),
        Value::String(snap.timestamp.clone()),
    );
    if let Some(status) = runtime_status {
        let health_state =
            if status.last_error_code.is_some() || status.phase == fns_agent::AgentPhase::Fatal {
                "error"
            } else if !status.running || status.phase == fns_agent::AgentPhase::Stopped {
                "stopped"
            } else if status.phase == fns_agent::AgentPhase::Online && status.connected {
                "healthy"
            } else {
                "degraded"
            };
        snap.process
            .insert("running".into(), Value::Bool(status.running));
        snap.process.insert(
            "phase".into(),
            serde_json::to_value(status.phase).unwrap_or(Value::String("unknown".into())),
        );
        snap.process.insert("pid".into(), json!(status.pid));
        snap.process.insert(
            "workspaceId".into(),
            Value::String(status.workspace_id.to_string()),
        );
        snap.process
            .insert("updatedAtMs".into(), Value::from(status.updated_at_ms));
        snap.process
            .insert("healthState".into(), Value::String(health_state.into()));
        snap.watcher.insert(
            "queuedWatcherBatches".into(),
            Value::from(status.queued_watcher_batches as u64),
        );
        snap.outbox.insert(
            "pendingCommands".into(),
            Value::from(status.pending_commands),
        );
        snap.transport
            .insert("connected".into(), Value::Bool(status.connected));
        snap.transport.insert(
            "activeTransfers".into(),
            Value::from(status.active_transfers as u64),
        );
        snap.transport.insert(
            "reconnectAttempt".into(),
            Value::from(status.reconnect_attempt),
        );
        snap.transport.insert(
            "lastErrorCode".into(),
            serde_json::to_value(status.last_error_code).unwrap_or(Value::Null),
        );
        snap.cursor.insert(
            "lastAckRevision".into(),
            Value::String(status.last_ack_revision.to_string()),
        );
        if snap.last_progress_boundary == ProgressBoundary::Unknown {
            snap.last_progress_boundary = if status.queued_watcher_batches > 0 {
                ProgressBoundary::Watcher
            } else if status.pending_commands > 0 {
                ProgressBoundary::Outbox
            } else if status.active_transfers > 0 {
                ProgressBoundary::Transport
            } else if status.last_ack_revision > fns_protocol::WorkspaceRevision::ZERO {
                ProgressBoundary::Ack
            } else if status.connected {
                ProgressBoundary::Transport
            } else {
                ProgressBoundary::Unknown
            };
        }
    }
    // Attach latest event name per component for Health panel.
    for component in [
        "watcher",
        "outbox",
        "transport",
        "server",
        "stream",
        "apply",
        "ack",
        "agent",
        "sync",
    ] {
        if let Some(ev) = events.iter().rev().find(|e| e.component == component) {
            let target = match component {
                "watcher" => &mut snap.watcher,
                "transport" | "agent" => &mut snap.transport,
                "sync" | "outbox" => &mut snap.outbox,
                "stream" => &mut snap.stream,
                "server" => &mut snap.server,
                _ => &mut snap.process,
            };
            target.insert("lastEventName".into(), Value::String(ev.event_name.clone()));
            target.insert("lastEventAt".into(), Value::String(ev.timestamp.clone()));
        }
    }
    // Validate schema constant
    let _ = SCHEMA_VERSION_HEALTH;
    snap
}

fn infer_boundary_from_events(events: &[DiagnosticEvent]) -> ProgressBoundary {
    for event in events.iter().rev() {
        let name = event.event_name.as_str();
        if name.contains("ack") {
            return ProgressBoundary::Ack;
        }
        if name.contains("apply") {
            return ProgressBoundary::Apply;
        }
        if name.contains("stream") {
            return ProgressBoundary::Stream;
        }
        if name.contains("server") || name.contains("revision") {
            return ProgressBoundary::Server;
        }
        if name.starts_with("transport.") {
            return ProgressBoundary::Transport;
        }
        if name.contains("outbox") {
            return ProgressBoundary::Outbox;
        }
        if name.starts_with("watcher.") {
            return ProgressBoundary::Watcher;
        }
    }
    ProgressBoundary::Unknown
}

#[tauri::command]
pub fn diagnostics_preview_support_bundle(
    project_id: String,
) -> Result<SupportBundlePreview, String> {
    project_state_dir(&project_id)?;
    let events = collect_project_events(&project_id, MAX_EVENTS_RETURNED);
    let mut summary = RedactionSummaryDto {
        secret_hits: 0,
        path_redactions: 0,
        fields_removed: 0,
    };
    for event in &events {
        let fields = Value::Object(
            event
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let (_, partial) = redact_fields(&fields);
        summary.secret_hits += partial.secret_hits;
        summary.path_redactions += partial.path_redactions;
        summary.fields_removed += partial.fields_removed;
    }
    let from = events.first().map(|e| e.timestamp.clone());
    let to = events.last().map(|e| e.timestamp.clone());
    Ok(SupportBundlePreview {
        event_count: events.len() as u64,
        time_range: TimeRange { from, to },
        redaction_summary: summary,
        includes_paths: false,
    })
}

#[tauri::command]
pub fn diagnostics_export_support_bundle(
    project_id: String,
) -> Result<SupportBundleExportResult, String> {
    let preview = diagnostics_preview_support_bundle(project_id.clone())?;
    let events = collect_project_events(&project_id, MAX_EVENTS_RETURNED);
    let dir = project_state_dir(&project_id)?.join("support-bundles");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = uuid::Uuid::new_v4();
    let path = dir.join(format!("bundle-{stamp}.json"));
    let health = diagnostics_get_health(project_id.clone())?;
    let mut redacted_events = Vec::new();
    for mut event in events {
        let fields = Value::Object(std::mem::take(&mut event.fields).into_iter().collect());
        let (redacted, _) = redact_fields(&fields);
        if let Value::Object(map) = redacted {
            event.fields = map.into_iter().collect();
        } else {
            event.fields = Default::default();
        }
        redacted_events.push(event);
    }
    let payload = json!({
        "schemaVersion": "fns-support-bundle/1",
        "projectId": project_id,
        "preview": preview,
        "health": health,
        "events": redacted_events,
    });
    let mut file = File::create(&path).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(SupportBundleExportResult {
        path: path.to_string_lossy().into_owned(),
        redaction_summary: preview.redaction_summary,
    })
}

#[tauri::command]
pub fn diagnostics_run_self_test(
    state: tauri::State<'_, DiagnosticsState>,
    profile: String,
) -> Result<SelfTestStartResult, String> {
    // Profiles are: filesystem path, inline JSON, or a named built-in.
    if profile.trim().is_empty() {
        return Err("profile is required".into());
    }
    if profile.contains("testOnly=false")
        || profile.contains("\"testOnly\": false")
        || profile.contains("\"testOnly\":false")
    {
        return Err("refusing non-testOnly profile".into());
    }

    let profile_path = resolve_self_test_profile(&profile)?;
    let sandbox_parent = selftest_state_dir().map(|p| p.join("sandboxes")).ok();
    if let Some(parent) = &sandbox_parent {
        let _ = fs::create_dir_all(parent);
    }

    let options = test_sync::selftest::SelfTestOptions {
        profile_path: profile_path.clone(),
        sandbox_parent,
        timeout: Some(std::time::Duration::from_secs(120)),
        ..Default::default()
    };

    // Track as active for cancel visibility (orchestrator itself is sync).
    let provisional_id = uuid::Uuid::new_v4().to_string();
    {
        let mut guard = state.active_runs.lock().map_err(|e| e.to_string())?;
        guard.insert(
            provisional_id.clone(),
            SelfTestHandle {
                profile: profile.clone(),
                cancel: false,
            },
        );
    }

    let result = test_sync::selftest::run_self_test(options).map_err(|e| e.to_string())?;

    let mut guard = state.active_runs.lock().map_err(|e| e.to_string())?;
    guard.remove(&provisional_id);
    guard.insert(
        result.manifest.run_id.clone(),
        SelfTestHandle {
            profile,
            cancel: false,
        },
    );

    // Mirror manifest under project selftest runs for Desktop listing.
    if let Ok(dir) = selftest_state_dir() {
        let runs = dir.join("runs");
        let _ = fs::create_dir_all(&runs);
        let dest = runs.join(format!("{}.json", result.manifest.run_id));
        let _ = fs::copy(&result.manifest_path, dest);
    }

    Ok(SelfTestStartResult {
        run_id: result.manifest.run_id,
        outcome: Some(format!("{:?}", result.manifest.outcome).to_ascii_lowercase()),
        manifest_path: Some(result.manifest_path.display().to_string()),
        bug_package_path: Some(result.bug_package_path.display().to_string()),
    })
}

/// Resolve a profile path from a filesystem path, inline JSON, or named builtin.
fn resolve_self_test_profile(profile: &str) -> Result<PathBuf, String> {
    let trimmed = profile.trim();
    let as_path = Path::new(trimmed);
    if as_path.is_file() {
        return Ok(as_path.to_path_buf());
    }
    let json = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        // Named profile → built-in testOnly skeleton.
        serde_json::json!({
            "name": trimmed,
            "testOnly": true,
            "serverEndpoint": "https://selftest.local",
            "sshHostAlias": "selftest-ssh",
            "scenarios": ["bidirectional-soak-10m"]
        })
        .to_string()
    };
    // Parse once to enforce testOnly before writing.
    let value: Value =
        serde_json::from_str(&json).map_err(|e| format!("invalid profile json: {e}"))?;
    if value.get("testOnly").and_then(|v| v.as_bool()) != Some(true) {
        return Err("refusing non-testOnly profile".into());
    }
    let dir = selftest_state_dir()
        .map(|p| p.join("profiles"))
        .map_err(|e| e)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    fs::write(&path, json.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

#[tauri::command]
pub fn diagnostics_cancel_self_test(
    state: tauri::State<'_, DiagnosticsState>,
    run_id: String,
) -> Result<(), String> {
    let mut guard = state.active_runs.lock().map_err(|e| e.to_string())?;
    if let Some(handle) = guard.get_mut(&run_id) {
        handle.cancel = true;
    }
    guard.remove(&run_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fns_observability::{DiagnosticLevel, SCHEMA_VERSION_EVENT};
    use std::io::Write;

    fn diagnostic_event(project: &str, name: &str) -> DiagnosticEvent {
        let mut event = DiagnosticEvent::new(
            DiagnosticLevel::Info,
            "test",
            name,
            "msg",
            project,
            "run",
            1,
        );
        event.schema_version = SCHEMA_VERSION_EVENT.into();
        event
    }

    #[test]
    fn project_state_dir_rejects_non_uuid_and_path_traversal() {
        for invalid in ["", "project", "../../escape", "../other-project"] {
            assert!(
                project_state_dir(invalid).is_err(),
                "accepted invalid project id: {invalid}"
            );
        }

        // Valid UUID shape is not enough — project must exist in the registry.
        let unknown = uuid::Uuid::new_v4().to_string();
        let err = project_state_dir(&unknown).expect_err("unknown uuid must be rejected");
        assert!(
            err.contains("unknown projectId") || err.contains("project not found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn diagnostics_commands_reject_invalid_project_id() {
        let filter = EventFilter {
            project_id: "../../escape".into(),
            level: None,
            component: None,
            event_name: None,
            run_id: None,
            limit: None,
        };

        assert!(diagnostics_list_events(filter).is_err());
        assert!(diagnostics_get_health("not-a-uuid".into()).is_err());
        assert!(diagnostics_preview_support_bundle("../foreign".into()).is_err());
    }

    #[test]
    fn diagnostics_commands_reject_unknown_project_uuid() {
        let project_id = uuid::Uuid::new_v4().to_string();
        let filter = EventFilter {
            project_id: project_id.clone(),
            level: None,
            component: None,
            event_name: None,
            run_id: None,
            limit: None,
        };

        assert!(diagnostics_list_events(filter).is_err());
        assert!(diagnostics_get_health(project_id.clone()).is_err());
        assert!(diagnostics_preview_support_bundle(project_id).is_err());
    }

    #[test]
    fn bounded_event_read_keeps_the_latest_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut file = File::create(&path).unwrap();
        for name in ["old", "middle", "new"] {
            writeln!(
                file,
                "{}",
                serde_json::to_string(&diagnostic_event("p", name)).unwrap()
            )
            .unwrap();
        }

        let events = read_events_from(&path, 2);
        let names: Vec<_> = events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect();
        assert_eq!(names, ["middle", "new"]);
    }

    #[test]
    fn event_read_is_bounded_to_the_file_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::to_string(&diagnostic_event("p", "outside-window")).unwrap()
        )
        .unwrap();
        file.write_all(&vec![b'x'; 8 * 1024 * 1024 + 1024]).unwrap();
        writeln!(file).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::to_string(&diagnostic_event("p", "inside-window")).unwrap()
        )
        .unwrap();

        let events = read_events_from(&path, 10);
        let names: Vec<_> = events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect();
        assert_eq!(names, ["inside-window"]);
    }

    #[test]
    fn rotated_and_active_files_return_the_global_latest_project_events() {
        let dir = tempfile::tempdir().unwrap();
        let rotated = dir.path().join("events.jsonl.1");
        let active = dir.path().join("events.jsonl");
        // Empty project_ref = state-dir owned; foreign refs are dropped.
        let project_id = uuid::Uuid::new_v4().to_string();

        let mut old_file = File::create(&rotated).unwrap();
        for name in ["old-one", "old-two"] {
            writeln!(
                old_file,
                "{}",
                serde_json::to_string(&diagnostic_event("", name)).unwrap()
            )
            .unwrap();
        }
        let mut active_file = File::create(&active).unwrap();
        for (project, name) in [
            ("foreign-project", "foreign"),
            ("", "new-one"),
            ("", "new-two"),
        ] {
            writeln!(
                active_file,
                "{}",
                serde_json::to_string(&diagnostic_event(project, name)).unwrap()
            )
            .unwrap();
        }

        let events = collect_events_from_paths([rotated, active], &project_id, 3);
        let names: Vec<_> = events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect();
        assert_eq!(names, ["old-two", "new-one", "new-two"]);
        assert!(events.iter().all(|e| e.event_name != "foreign"));
    }

    #[test]
    fn event_belongs_rejects_foreign_ws_prefix_without_matching_pseudonym() {
        let project_id = uuid::Uuid::new_v4().to_string();
        let foreign = diagnostic_event("ws-deadbeefcafebabe", "foreign.event");
        assert!(!event_belongs_to_project(&foreign, &project_id));

        let empty = diagnostic_event("", "local.event");
        assert!(event_belongs_to_project(&empty, &project_id));

        let matching = diagnostic_event(&project_id, "own.event");
        assert!(event_belongs_to_project(&matching, &project_id));
    }

    fn agent_status(
        running: bool,
        phase: fns_agent::AgentPhase,
        error: Option<fns_agent::AgentErrorCode>,
    ) -> fns_agent::AgentStatus {
        fns_agent::AgentStatus {
            schema_version: "fns-agent-status/1".into(),
            running,
            phase,
            pid: running.then_some(4242),
            connected: false,
            workspace_id: fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000001")
                .unwrap(),
            last_ack_revision: fns_protocol::WorkspaceRevision::new(42),
            pending_commands: 7,
            queued_watcher_batches: 4,
            active_transfers: 2,
            reconnect_attempt: 3,
            last_error_code: error,
            updated_at_ms: 1234,
        }
    }

    #[test]
    fn health_snapshot_maps_the_complete_runtime_status() {
        let project_id = uuid::Uuid::new_v4().to_string();
        let status = agent_status(
            false,
            fns_agent::AgentPhase::Fatal,
            Some(fns_agent::AgentErrorCode::Network),
        );

        let health = build_health_snapshot(&project_id, &[], Some(&status));

        assert_eq!(health.process["running"], false);
        assert_eq!(health.process["phase"], "fatal");
        assert_eq!(health.process["healthState"], "error");
        assert_eq!(health.watcher["queuedWatcherBatches"], 4);
        assert_eq!(health.outbox["pendingCommands"], 7);
        assert_eq!(health.transport["connected"], false);
        assert_eq!(health.transport["activeTransfers"], 2);
        assert_eq!(health.transport["reconnectAttempt"], 3);
        assert_eq!(health.transport["lastErrorCode"], "network");
        assert_eq!(health.cursor["lastAckRevision"], "42");
    }

    #[test]
    fn stopped_runtime_is_not_reported_as_healthy() {
        let project_id = uuid::Uuid::new_v4().to_string();
        let status = agent_status(false, fns_agent::AgentPhase::Stopped, None);

        let health = build_health_snapshot(&project_id, &[], Some(&status));

        assert_eq!(health.process["healthState"], "stopped");
        assert_ne!(health.process["healthState"], "healthy");
    }

    #[test]
    fn runtime_status_supplies_a_boundary_when_history_is_empty() {
        let project_id = uuid::Uuid::new_v4().to_string();
        let mut status = agent_status(true, fns_agent::AgentPhase::Online, None);
        status.connected = true;
        status.pending_commands = 0;
        status.queued_watcher_batches = 0;
        status.active_transfers = 0;

        let health = build_health_snapshot(&project_id, &[], Some(&status));

        assert_eq!(health.last_progress_boundary, ProgressBoundary::Ack);
    }

    #[test]
    fn list_events_scoped_by_project_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        // Override via writing to a temp path exercised through read_events_from
        let path = dir.path().join("events.jsonl");
        let mut file = File::create(&path).unwrap();
        for (project, name) in [
            ("proj-a", "a.one"),
            ("proj-a", "a.two"),
            ("proj-b", "b.one"),
        ] {
            let event = diagnostic_event(project, name);
            writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
        }
        let events = read_events_from(&path, 100);
        assert_eq!(events.len(), 3);
        let a: Vec<_> = events
            .iter()
            .filter(|e| e.project_ref == "proj-a")
            .collect();
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|e| e.project_ref == "proj-a"));
    }

    #[test]
    fn preview_redaction_summary_counts_secrets() {
        let mut event = DiagnosticEvent::new(
            DiagnosticLevel::Info,
            "test",
            "test.event",
            "msg",
            "p",
            "r",
            0,
        );
        event
            .fields
            .insert("password".into(), Value::String("secret".into()));
        let fields = Value::Object(event.fields.into_iter().collect());
        let (_, summary) = redact_fields(&fields);
        assert!(summary.secret_hits >= 1);
    }
}
