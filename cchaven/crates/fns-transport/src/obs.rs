//! Transport observability: phase, reconnect, request, and transfer events.
//!
//! Instrumentation is best-effort. Sink failures never fail the transport path.

use fns_observability::{DiagnosticLevel, ProgressBoundary, RuntimeDiagnostics, fields_from};
use serde_json::Value;
use std::sync::Arc;

/// Optional diagnostics handle attached to a transport session.
#[derive(Clone, Default)]
pub struct TransportDiagnostics {
    inner: Option<Arc<RuntimeDiagnostics>>,
}

impl TransportDiagnostics {
    pub fn none() -> Self {
        Self { inner: None }
    }

    pub fn new(diag: Arc<RuntimeDiagnostics>) -> Self {
        Self { inner: Some(diag) }
    }

    pub fn as_runtime(&self) -> Option<&RuntimeDiagnostics> {
        self.inner.as_deref()
    }

    pub fn on_phase(&self, phase: &str, message: &str) {
        let Some(diag) = self.as_runtime() else {
            return;
        };
        diag.note_boundary(ProgressBoundary::Transport);
        diag.emit(
            DiagnosticLevel::Info,
            "transport",
            &format!("transport.phase.{phase}"),
            message,
            fields_from([("phase", Value::String(phase.into()))]),
        );
    }

    pub fn on_reconnect(&self, attempt: u32, reason: &str) {
        let Some(diag) = self.as_runtime() else {
            return;
        };
        let generation = diag.bump_connection_generation();
        diag.note_boundary(ProgressBoundary::Transport);
        diag.emit(
            DiagnosticLevel::Warn,
            "transport",
            "transport.reconnect.scheduled",
            "reconnect scheduled",
            fields_from([
                ("attempt", Value::from(attempt)),
                ("reason", Value::String(reason.into())),
                ("connectionGeneration", Value::from(generation)),
            ]),
        );
    }

    pub fn on_request_sent(&self, request_kind: &str) {
        let Some(diag) = self.as_runtime() else {
            return;
        };
        diag.note_boundary(ProgressBoundary::Transport);
        diag.emit_info(
            "transport",
            "transport.request.sent",
            "request sent",
            fields_from([("requestKind", Value::String(request_kind.into()))]),
        );
    }

    pub fn on_transfer(&self, direction: &str, active: usize) {
        let Some(diag) = self.as_runtime() else {
            return;
        };
        diag.emit_info(
            "transport",
            "transport.transfer.progress",
            "transfer progress",
            fields_from([
                ("direction", Value::String(direction.into())),
                ("activeTransfers", Value::from(active as u64)),
            ]),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fns_observability::MemorySink;

    #[test]
    fn reconnect_bumps_connection_generation() {
        let sink = Arc::new(MemorySink::new());
        let runtime = Arc::new(RuntimeDiagnostics::new("run-t", "proj", sink.clone()));
        runtime.set_connection_generation(1);
        let td = TransportDiagnostics::new(runtime.clone());
        td.on_reconnect(2, "socket_closed");
        td.on_phase("online", "connected");
        let events = sink.events();
        assert!(
            events
                .iter()
                .any(|e| e.event_name == "transport.reconnect.scheduled")
        );
        assert!(
            events
                .iter()
                .any(|e| e.event_name == "transport.phase.online")
        );
        assert!(events.iter().all(|e| e.run_id == "run-t"));
        // generation bumped on reconnect
        assert_eq!(runtime.connection_generation(), 2);
    }

    #[test]
    fn missing_diagnostics_is_noop() {
        let td = TransportDiagnostics::none();
        td.on_phase("online", "ok");
        td.on_reconnect(1, "x");
    }
}
