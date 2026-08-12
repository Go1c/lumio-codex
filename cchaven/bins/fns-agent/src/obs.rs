//! Agent-side observability adapter: dual-writes lifecycle events to durable JSONL
//! while keeping `runtime-status.json` as the cheap current snapshot.

use fns_observability::{
    DiagnosticLevel, MemorySink, ProgressBoundary, RollingJsonlSink, RuntimeDiagnostics,
    fields_from,
};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Open a rolling diagnostics sink under `state_dir/diagnostics/events.jsonl`.
///
/// `runId` is created once by the caller per Agent process. The current id is
/// atomically published at `diagnostics/run-id`; historical JSONL events keep
/// their original id across process restarts.
pub fn open_agent_diagnostics(
    state_dir: &Path,
    workspace_id: &str,
    run_id: Option<String>,
) -> RuntimeDiagnostics {
    let run_id = run_id.unwrap_or_else(|| create_run_id(state_dir));
    let project_ref = pseudonym(workspace_id);
    let path = state_dir.join("diagnostics").join("events.jsonl");
    let sink: Arc<dyn fns_observability::DiagnosticSink> =
        match RollingJsonlSink::open(&path, 100 * 1024 * 1024) {
            Ok(s) => Arc::new(s),
            Err(_) => Arc::new(MemorySink::new()),
        };
    RuntimeDiagnostics::new(run_id, project_ref, sink)
}

fn create_run_id(state_dir: &Path) -> String {
    let dir = state_dir.join("diagnostics");
    let path = dir.join("run-id");
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(&dir);
    let temporary = dir.join(format!(".run-id-{}.tmp", uuid::Uuid::new_v4()));
    if let Ok(mut file) = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        if file.write_all(id.as_bytes()).is_ok() && file.sync_all().is_ok() {
            let _ = std::fs::rename(&temporary, &path);
        }
    }
    let _ = std::fs::remove_file(&temporary);
    id
}

fn pseudonym(workspace_id: &str) -> String {
    let hash = blake3::hash(workspace_id.as_bytes());
    format!("ws-{}", hex::encode(&hash.as_bytes()[..8]))
}

/// Emit lifecycle / phase events without blocking the agent loop.
pub fn emit_lifecycle(
    diag: &RuntimeDiagnostics,
    phase: &str,
    message: &str,
    extra: Vec<(&str, Value)>,
) {
    let mut fields = fields_from(extra);
    fields.insert("phase".into(), Value::String(phase.into()));
    diag.emit(
        DiagnosticLevel::Info,
        "agent",
        &format!("agent.lifecycle.{phase}"),
        message,
        fields,
    );
}

pub fn emit_watcher(
    diag: &RuntimeDiagnostics,
    event_name: &str,
    message: &str,
    queued_batches: usize,
) {
    diag.note_boundary(ProgressBoundary::Watcher);
    diag.emit_info(
        "watcher",
        event_name,
        message,
        fields_from([("queuedBatches", Value::from(queued_batches as u64))]),
    );
}

pub fn emit_status_snapshot(
    diag: &RuntimeDiagnostics,
    phase: &str,
    connected: bool,
    pending_commands: u64,
    queued_watcher_batches: usize,
    active_transfers: usize,
    last_ack: &str,
    reconnect_attempt: u32,
    last_error_code: Option<&str>,
) {
    if connected {
        diag.note_boundary(ProgressBoundary::Transport);
    }
    diag.emit_info(
        "agent",
        "agent.status.published",
        "runtime status published",
        fields_from([
            ("phase", Value::String(phase.into())),
            ("connected", Value::Bool(connected)),
            ("pendingCommands", Value::from(pending_commands)),
            (
                "queuedWatcherBatches",
                Value::from(queued_watcher_batches as u64),
            ),
            ("activeTransfers", Value::from(active_transfers as u64)),
            ("lastAckRevision", Value::String(last_ack.into())),
            ("reconnectAttempt", Value::from(reconnect_attempt)),
            (
                "lastErrorCode",
                last_error_code
                    .map(|code| Value::String(code.into()))
                    .unwrap_or(Value::Null),
            ),
            (
                "connectionGeneration",
                Value::from(diag.connection_generation()),
            ),
        ]),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fns_observability::MemorySink;

    #[test]
    fn open_agent_diagnostics_emits_correlated_events() {
        let dir = tempfile::tempdir().unwrap();
        let diag = open_agent_diagnostics(dir.path(), "workspace-uuid-1", Some("run-1".into()));
        assert_eq!(diag.run_id(), "run-1");
        emit_lifecycle(&diag, "started", "agent started", vec![]);
        emit_watcher(&diag, "watcher.batch.queued", "batch", 2);
        // Events should land in JSONL under diagnostics/
        let path = dir.path().join("diagnostics").join("events.jsonl");
        assert!(
            path.exists(),
            "expected durable events at {}",
            path.display()
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("run-1"));
        assert!(content.contains("agent.lifecycle.started"));
        assert!(content.contains("watcher.batch.queued"));
        assert!(!content.contains("workspace-uuid-1"));
    }

    #[test]
    fn default_run_id_is_new_for_each_agent_process() {
        let dir = tempfile::tempdir().unwrap();
        let first = open_agent_diagnostics(dir.path(), "workspace-uuid-1", None);
        let second = open_agent_diagnostics(dir.path(), "workspace-uuid-1", None);

        assert_ne!(first.run_id(), second.run_id());
        let current =
            std::fs::read_to_string(dir.path().join("diagnostics").join("run-id")).unwrap();
        assert_eq!(current.trim(), second.run_id());
    }

    #[test]
    fn sink_failure_does_not_surface_to_caller() {
        let sink = Arc::new(MemorySink::new());
        sink.force_fail_next();
        let diag = RuntimeDiagnostics::new("r", "p", sink);
        // Must not panic
        emit_lifecycle(&diag, "online", "up", vec![]);
        emit_status_snapshot(&diag, "online", true, 0, 0, 0, "24", 0, None);
    }
}
