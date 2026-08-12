//! Runtime correlation context shared across agent / transport / sync boundaries.

use crate::event::{DiagnosticEvent, DiagnosticLevel};
use crate::health::{HealthSnapshot, ProgressBoundary};
use crate::sink::{DiagnosticSink, emit_lossy};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Shared correlation IDs for one Desktop/Agent run.
#[derive(Clone)]
pub struct RuntimeDiagnostics {
    run_id: String,
    project_ref: String,
    connection_generation: Arc<AtomicU64>,
    sink: Arc<dyn DiagnosticSink>,
    last_boundary: Arc<Mutex<ProgressBoundary>>,
}

impl RuntimeDiagnostics {
    pub fn new(
        run_id: impl Into<String>,
        project_ref: impl Into<String>,
        sink: Arc<dyn DiagnosticSink>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            project_ref: project_ref.into(),
            connection_generation: Arc::new(AtomicU64::new(0)),
            sink,
            last_boundary: Arc::new(Mutex::new(ProgressBoundary::Unknown)),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn project_ref(&self) -> &str {
        &self.project_ref
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::SeqCst)
    }

    pub fn bump_connection_generation(&self) -> u64 {
        self.connection_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn set_connection_generation(&self, generation: u64) {
        self.connection_generation
            .store(generation, Ordering::SeqCst);
    }

    pub fn note_boundary(&self, boundary: ProgressBoundary) {
        *self.last_boundary.lock().unwrap_or_else(|e| e.into_inner()) = boundary;
    }

    pub fn last_boundary(&self) -> ProgressBoundary {
        *self.last_boundary.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Emit a structured event. Sink failures are swallowed (never block hot path).
    pub fn emit(
        &self,
        level: DiagnosticLevel,
        component: &str,
        event_name: &str,
        message: &str,
        fields: BTreeMap<String, Value>,
    ) {
        let mut event = DiagnosticEvent::new(
            level,
            component,
            event_name,
            message,
            self.project_ref.clone(),
            self.run_id.clone(),
            self.connection_generation(),
        );
        event.fields = fields;
        emit_lossy(self.sink.as_ref(), &event);
    }

    pub fn emit_info(
        &self,
        component: &str,
        event_name: &str,
        message: &str,
        fields: BTreeMap<String, Value>,
    ) {
        self.emit(
            DiagnosticLevel::Info,
            component,
            event_name,
            message,
            fields,
        );
    }

    pub fn health_snapshot(&self) -> HealthSnapshot {
        let mut snap = HealthSnapshot::empty(
            &self.run_id,
            &self.project_ref,
            self.connection_generation(),
        );
        snap.last_progress_boundary = self.last_boundary();
        snap
    }

    pub fn sink(&self) -> Arc<dyn DiagnosticSink> {
        Arc::clone(&self.sink)
    }
}

/// Build field map from simple pairs.
pub fn fields_from<I, K>(pairs: I) -> BTreeMap<String, Value>
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::MemorySink;

    #[test]
    fn correlation_ids_shared_across_emits() {
        let sink = Arc::new(MemorySink::new());
        let diag = RuntimeDiagnostics::new("run-xyz", "proj-a", sink.clone());
        diag.set_connection_generation(3);
        diag.emit_info(
            "agent",
            "agent.lifecycle.started",
            "agent started",
            fields_from([("phase", Value::String("running".into()))]),
        );
        diag.bump_connection_generation();
        diag.emit_info(
            "transport",
            "transport.reconnect.succeeded",
            "reconnected",
            BTreeMap::new(),
        );
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.run_id == "run-xyz"));
        assert_eq!(events[0].connection_generation, 3);
        assert_eq!(events[1].connection_generation, 4);
        assert_eq!(events[0].component, "agent");
        assert_eq!(events[1].component, "transport");
    }

    #[test]
    fn sink_failure_does_not_panic_or_block() {
        let sink = Arc::new(MemorySink::new());
        sink.force_fail_next();
        let diag = RuntimeDiagnostics::new("run", "proj", sink.clone());
        // Must not panic
        diag.emit_info("sync", "sync.outbox.dispatched", "ok", BTreeMap::new());
        // Forced failure swallowed; second emit works
        diag.emit_info("sync", "sync.stream.advanced", "ok", BTreeMap::new());
        assert_eq!(sink.len(), 1);
    }
}
