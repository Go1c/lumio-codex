//! Sync-core observability: outbox, stream, journal, and cursor snapshots.
//! Best-effort only — never fails the sync hot path.

use fns_observability::{ProgressBoundary, RuntimeDiagnostics, fields_from};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct SyncDiagnostics {
    inner: Option<Arc<RuntimeDiagnostics>>,
}

impl SyncDiagnostics {
    pub fn none() -> Self {
        Self { inner: None }
    }

    pub fn new(diag: Arc<RuntimeDiagnostics>) -> Self {
        Self { inner: Some(diag) }
    }

    pub fn on_outbox_snapshot(
        &self,
        queued: u64,
        dispatched: u64,
        awaiting_blob: u64,
        blocked_conflict: u64,
    ) {
        let Some(diag) = self.inner.as_deref() else {
            return;
        };
        if queued > 0 || dispatched > 0 {
            diag.note_boundary(ProgressBoundary::Outbox);
        }
        diag.emit_info(
            "sync",
            "sync.outbox.snapshot",
            "outbox snapshot",
            fields_from([
                ("queued", Value::from(queued)),
                ("dispatched", Value::from(dispatched)),
                ("awaitingBlob", Value::from(awaiting_blob)),
                ("blockedConflict", Value::from(blocked_conflict)),
            ]),
        );
    }

    pub fn on_stream_advance(
        &self,
        mode: &str,
        from_revision: &str,
        received: u64,
        end_received: bool,
    ) {
        let Some(diag) = self.inner.as_deref() else {
            return;
        };
        diag.note_boundary(ProgressBoundary::Stream);
        diag.emit_info(
            "sync",
            "sync.stream.advanced",
            "stream advanced",
            fields_from([
                ("mode", Value::String(mode.into())),
                ("fromRevision", Value::String(from_revision.into())),
                ("received", Value::from(received)),
                ("endReceived", Value::Bool(end_received)),
            ]),
        );
    }

    pub fn on_apply_progress(&self, applied: u64, journal_depth: u64) {
        let Some(diag) = self.inner.as_deref() else {
            return;
        };
        diag.note_boundary(ProgressBoundary::Apply);
        diag.emit_info(
            "sync",
            "sync.apply.progress",
            "apply progress",
            fields_from([
                ("applied", Value::from(applied)),
                ("journalDepth", Value::from(journal_depth)),
            ]),
        );
    }

    pub fn on_cursor(
        &self,
        last_ack: &str,
        last_applied: &str,
        pending_ack: u64,
        pending_segment_ack: u64,
    ) {
        let Some(diag) = self.inner.as_deref() else {
            return;
        };
        if pending_ack == 0 && last_ack == last_applied {
            diag.note_boundary(ProgressBoundary::Ack);
        }
        diag.emit_info(
            "sync",
            "sync.cursor.snapshot",
            "cursor snapshot",
            fields_from([
                ("lastAck", Value::String(last_ack.into())),
                ("lastApplied", Value::String(last_applied.into())),
                ("pendingAck", Value::from(pending_ack)),
                ("pendingSegmentAck", Value::from(pending_segment_ack)),
            ]),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fns_observability::{MemorySink, RuntimeDiagnostics};

    #[test]
    fn outbox_stream_cursor_share_run_id() {
        let sink = Arc::new(MemorySink::new());
        let runtime = Arc::new(RuntimeDiagnostics::new("run-sync", "proj", sink.clone()));
        runtime.set_connection_generation(7);
        let sd = SyncDiagnostics::new(runtime);
        sd.on_outbox_snapshot(1, 0, 0, 0);
        sd.on_stream_advance("live", "20", 2, false);
        sd.on_apply_progress(2, 1);
        sd.on_cursor("24", "24", 0, 0);
        let events = sink.events();
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|e| e.run_id == "run-sync"));
        assert!(events.iter().all(|e| e.connection_generation == 7));
        assert!(
            events
                .iter()
                .any(|e| e.event_name == "sync.outbox.snapshot")
        );
        assert!(
            events
                .iter()
                .any(|e| e.event_name == "sync.cursor.snapshot")
        );
    }

    #[test]
    fn sink_failure_does_not_error_sync_path() {
        let sink = Arc::new(MemorySink::new());
        sink.force_fail_next();
        let runtime = Arc::new(RuntimeDiagnostics::new("r", "p", sink));
        let sd = SyncDiagnostics::new(runtime);
        // Must not panic
        sd.on_outbox_snapshot(1, 0, 0, 0);
        sd.on_cursor("1", "1", 0, 0);
    }
}
