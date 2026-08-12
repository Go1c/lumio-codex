//! Progress-boundary classifier for sync stall diagnosis (Wave 0 / M4).
//!
//! Labels the last proven stage of the sync chain from a structured
//! [`ChainSnapshot`]. Unit tests inject per-boundary faults without a live
//! remote; the same snapshot shape can later be filled from harness evidence.

use serde::{Deserialize, Serialize};

/// Ordered progress stages of the bidirectional sync chain, plus the UI
/// false-online special case. String values match `fns-diagnostic-run/1` and
/// `fns-health-snapshot/1` boundary fields.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressBoundary {
    Watcher,
    Outbox,
    Transport,
    Server,
    Stream,
    Apply,
    Ack,
    #[serde(rename = "ui-false-online")]
    UiFalseOnline,
}

impl ProgressBoundary {
    /// Contract string used by diagnostic-run / health-snapshot fixtures.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watcher => "watcher",
            Self::Outbox => "outbox",
            Self::Transport => "transport",
            Self::Server => "server",
            Self::Stream => "stream",
            Self::Apply => "apply",
            Self::Ack => "ack",
            Self::UiFalseOnline => "ui-false-online",
        }
    }

    /// Chain stages in pipeline order (excludes [`ProgressBoundary::UiFalseOnline`]).
    pub fn chain() -> &'static [ProgressBoundary] {
        &[
            Self::Watcher,
            Self::Outbox,
            Self::Transport,
            Self::Server,
            Self::Stream,
            Self::Apply,
            Self::Ack,
        ]
    }

    fn predecessor(self) -> Option<ProgressBoundary> {
        match self {
            Self::Watcher | Self::UiFalseOnline => None,
            Self::Outbox => Some(Self::Watcher),
            Self::Transport => Some(Self::Outbox),
            Self::Server => Some(Self::Transport),
            Self::Stream => Some(Self::Server),
            Self::Apply => Some(Self::Stream),
            Self::Ack => Some(Self::Apply),
        }
    }
}

/// Structured counters/timestamps for each stage of the sync chain.
///
/// Tests construct this directly to inject faults. A live harness can later
/// populate the same fields from watcher/outbox/transport/server/stream/
/// journal/cursor/runtime samples.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainSnapshot {
    /// Wall-clock or fake-clock observation time (milliseconds).
    pub observed_at_ms: u64,

    /// Local filesystem mutation that should eventually sync.
    pub local_file_mutations: u64,
    pub watcher_events: u64,
    pub watcher_rescans: u64,

    pub outbox_entries: u64,
    pub local_intents: u64,

    pub transport_requests_sent: u64,
    /// True when a connection exists (or should) but no request progress is observed.
    pub transport_connection_idle: bool,

    pub server_operations: u64,
    pub server_revision: u64,

    pub peer_stream_items: u64,
    pub peer_stream_revision: u64,

    /// Ready stream revision items waiting for local apply.
    pub stream_items_ready: u64,
    pub apply_journal_entries: u64,
    pub applied_revision: u64,

    pub pending_ack_revision: Option<u64>,
    pub last_ack_revision: u64,

    /// Agent runtime is degraded/stopped (not online).
    pub runtime_degraded: bool,
    /// UI still presents the session as running/online.
    pub ui_shows_running: bool,
}

impl ChainSnapshot {
    /// True when residual work should still advance some stage.
    ///
    /// A completed end-to-end mutation (fully acked, empty queues) is not
    /// pending even if historical mutation counters remain non-zero.
    pub fn has_pending_work(&self) -> bool {
        if self.fully_quiescent() {
            return false;
        }
        self.local_file_mutations > 0
            || self.outbox_entries > 0
            || self.local_intents > 0
            || self.stream_items_ready > 0
            || self.apply_journal_entries > 0
            || self.pending_ack_revision.is_some()
            || (self.server_revision > 0 && self.peer_stream_revision < self.server_revision)
            || (self.applied_revision > 0 && self.last_ack_revision < self.applied_revision)
            || self.transport_backlog()
    }

    fn fully_quiescent(&self) -> bool {
        self.outbox_entries == 0
            && self.local_intents == 0
            && self.stream_items_ready == 0
            && self.apply_journal_entries == 0
            && self.pending_ack_revision.is_none()
            && !self.transport_connection_idle
            && self.server_revision > 0
            && self.peer_stream_revision >= self.server_revision
            && self.applied_revision >= self.peer_stream_revision
            && self.last_ack_revision >= self.applied_revision
    }

    fn transport_backlog(&self) -> bool {
        (self.outbox_entries > 0 || self.local_intents > 0)
            && (self.transport_requests_sent == 0 || self.transport_connection_idle)
    }

    /// Direct evidence that the watcher stage observed a change.
    fn watcher_observed(&self) -> bool {
        self.watcher_events > 0 || self.watcher_rescans > 0
    }

    /// Direct evidence of outbox/intent materialization (may later drain).
    fn outbox_present(&self) -> bool {
        self.outbox_entries > 0 || self.local_intents > 0
    }

    /// Direct evidence that transport sent work and is not idle.
    fn transport_sent(&self) -> bool {
        self.transport_requests_sent > 0 && !self.transport_connection_idle
    }

    fn server_observed(&self) -> bool {
        self.server_operations > 0 || self.server_revision > 0
    }

    fn stream_caught_up(&self) -> bool {
        self.server_observed()
            && self.peer_stream_revision >= self.server_revision
            && (self.peer_stream_items > 0
                || self.stream_items_ready > 0
                || self.applied_revision > 0
                || self.peer_stream_revision > 0)
    }

    fn apply_caught_up(&self) -> bool {
        self.applied_revision > 0
            && self.applied_revision >= self.peer_stream_revision
            && self.apply_journal_entries == 0
            && self.stream_items_ready == 0
    }

    fn ack_caught_up(&self) -> bool {
        self.pending_ack_revision.is_none()
            && self.last_ack_revision > 0
            && self.last_ack_revision >= self.applied_revision
    }

    /// Whether work demonstrably transited this stage (including drained queues
    /// inferred from later-stage evidence).
    fn stage_passed(&self, stage: ProgressBoundary) -> bool {
        match stage {
            ProgressBoundary::Watcher => {
                self.watcher_observed()
                    || self.outbox_present()
                    || self.transport_sent()
                    || self.server_observed()
            }
            ProgressBoundary::Outbox => {
                // Outbox may drain after transport; later evidence still proves transit.
                self.outbox_present() || self.transport_sent() || self.server_observed()
            }
            ProgressBoundary::Transport => self.transport_sent() || self.server_observed(),
            ProgressBoundary::Server => self.server_observed(),
            ProgressBoundary::Stream => self.stream_caught_up(),
            ProgressBoundary::Apply => self.apply_caught_up(),
            ProgressBoundary::Ack => self.ack_caught_up(),
            ProgressBoundary::UiFalseOnline => false,
        }
    }

    /// Whether this stage is the next expected hop given residual pending work.
    fn stage_blocked(&self, stage: ProgressBoundary) -> bool {
        if !self.has_pending_work() && stage != ProgressBoundary::UiFalseOnline {
            return false;
        }
        match stage {
            ProgressBoundary::Watcher => {
                self.local_file_mutations > 0
                    && !self.watcher_observed()
                    && !self.stage_passed(ProgressBoundary::Outbox)
            }
            ProgressBoundary::Outbox => {
                self.watcher_observed()
                    && !self.outbox_present()
                    && !self.transport_sent()
                    && !self.server_observed()
            }
            ProgressBoundary::Transport => {
                self.outbox_present()
                    && (self.transport_requests_sent == 0 || self.transport_connection_idle)
                    && !self.server_observed()
            }
            ProgressBoundary::Server => self.transport_sent() && !self.server_observed(),
            ProgressBoundary::Stream => {
                self.server_observed()
                    && (self.peer_stream_revision < self.server_revision
                        || (self.peer_stream_items == 0
                            && self.stream_items_ready == 0
                            && self.applied_revision < self.server_revision))
            }
            ProgressBoundary::Apply => {
                (self.stream_items_ready > 0 || self.apply_journal_entries > 0)
                    || (self.stream_caught_up()
                        && self.applied_revision < self.peer_stream_revision)
            }
            ProgressBoundary::Ack => {
                self.apply_caught_up()
                    && (self.pending_ack_revision.is_some()
                        || self.last_ack_revision < self.applied_revision)
            }
            ProgressBoundary::UiFalseOnline => false,
        }
    }
}

/// Result of classifying a stalled or quiescent chain snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryClassification {
    /// Deepest stage that demonstrably advanced.
    pub last_passed: Option<ProgressBoundary>,
    /// First stage that should have advanced but did not (or UI false-online).
    pub first_failed: Option<ProgressBoundary>,
}

impl BoundaryClassification {
    pub fn last_passed_str(self) -> Option<&'static str> {
        self.last_passed.map(ProgressBoundary::as_str)
    }

    pub fn first_failed_str(self) -> Option<&'static str> {
        self.first_failed.map(ProgressBoundary::as_str)
    }
}

/// Classify the last proven boundary from a single structured chain snapshot.
///
/// Rules (first blocked stage wins while walking watcher → ack):
/// - **ui-false-online**: runtime degraded/stopped but UI still shows running
/// - **watcher stopped**: file changed but watcher event/rescan not growing
/// - **outbox stopped**: watcher received but outbox/intent not generated
/// - **transport stopped**: outbox exists but request not sent / connection idle
/// - **server stopped**: request sent, no operation/revision
/// - **stream stopped**: server revision grew, peer stream not advancing
/// - **apply stopped**: stream item ready, journal/fs/db not advancing
/// - **ack stopped**: apply done, pending ack/lastAck not advancing
///
/// When every expected stage advanced (or there is no pending work and the
/// chain is fully acked), `first_failed` is `None` and `last_passed` is `Ack`
/// if ack evidence is present.
pub fn classify_last_passed_boundary(snapshot: &ChainSnapshot) -> BoundaryClassification {
    if snapshot.runtime_degraded && snapshot.ui_shows_running {
        let chain = classify_chain(snapshot);
        return BoundaryClassification {
            last_passed: chain.last_passed,
            first_failed: Some(ProgressBoundary::UiFalseOnline),
        };
    }
    classify_chain(snapshot)
}

fn classify_chain(snapshot: &ChainSnapshot) -> BoundaryClassification {
    let mut last_passed = None;
    for &stage in ProgressBoundary::chain() {
        if snapshot.stage_blocked(stage) {
            // Prefer explicit predecessor when the blocked stage has one; else
            // the deepest stage that still proves transit.
            let last_passed = stage
                .predecessor()
                .filter(|pred| snapshot.stage_passed(*pred))
                .or(last_passed);
            return BoundaryClassification {
                last_passed,
                first_failed: Some(stage),
            };
        }
        if snapshot.stage_passed(stage) {
            last_passed = Some(stage);
        }
    }

    BoundaryClassification {
        last_passed,
        first_failed: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ChainSnapshot {
        ChainSnapshot {
            observed_at_ms: 1_000,
            ..ChainSnapshot::default()
        }
    }

    /// Healthy chain that completed one mutation end-to-end.
    fn fully_acked() -> ChainSnapshot {
        ChainSnapshot {
            observed_at_ms: 5_000,
            local_file_mutations: 1,
            watcher_events: 2,
            watcher_rescans: 0,
            outbox_entries: 0,
            local_intents: 0,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 1,
            server_revision: 7,
            peer_stream_items: 1,
            peer_stream_revision: 7,
            stream_items_ready: 0,
            apply_journal_entries: 0,
            applied_revision: 7,
            pending_ack_revision: None,
            last_ack_revision: 7,
            runtime_degraded: false,
            ui_shows_running: true,
        }
    }

    #[test]
    fn progress_boundary_contract_strings_match_diagnostic_run() {
        assert_eq!(ProgressBoundary::Watcher.as_str(), "watcher");
        assert_eq!(ProgressBoundary::Outbox.as_str(), "outbox");
        assert_eq!(ProgressBoundary::Transport.as_str(), "transport");
        assert_eq!(ProgressBoundary::Server.as_str(), "server");
        assert_eq!(ProgressBoundary::Stream.as_str(), "stream");
        assert_eq!(ProgressBoundary::Apply.as_str(), "apply");
        assert_eq!(ProgressBoundary::Ack.as_str(), "ack");
        assert_eq!(ProgressBoundary::UiFalseOnline.as_str(), "ui-false-online");
    }

    #[test]
    fn injected_watcher_fault_classifies_watcher_as_first_failed() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 0,
            watcher_rescans: 0,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.first_failed, Some(ProgressBoundary::Watcher));
        assert_eq!(result.last_passed, None);
        assert_eq!(result.first_failed_str(), Some("watcher"));
        assert_eq!(result.last_passed_str(), None);
    }

    #[test]
    fn injected_outbox_fault_classifies_after_watcher() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 3,
            outbox_entries: 0,
            local_intents: 0,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Watcher));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Outbox));
    }

    #[test]
    fn injected_transport_fault_classifies_after_outbox() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 2,
            local_intents: 1,
            transport_requests_sent: 0,
            transport_connection_idle: true,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Outbox));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Transport));
    }

    #[test]
    fn injected_server_fault_classifies_after_transport() {
        // Fixture shape matches diagnostic-run-v1: lastPassed=transport, firstFailed=server.
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 1,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 0,
            server_revision: 0,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Transport));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Server));
        assert_eq!(result.last_passed_str(), Some("transport"));
        assert_eq!(result.first_failed_str(), Some("server"));
    }

    #[test]
    fn injected_stream_fault_classifies_after_server() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 1,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 1,
            server_revision: 9,
            peer_stream_revision: 4,
            peer_stream_items: 0,
            stream_items_ready: 0,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Server));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Stream));
    }

    #[test]
    fn injected_apply_fault_classifies_after_stream() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 0,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 1,
            server_revision: 5,
            peer_stream_items: 1,
            peer_stream_revision: 5,
            stream_items_ready: 1,
            apply_journal_entries: 1,
            applied_revision: 4,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Stream));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Apply));
    }

    #[test]
    fn injected_ack_fault_classifies_after_apply() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 0,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 1,
            server_revision: 5,
            peer_stream_items: 1,
            peer_stream_revision: 5,
            stream_items_ready: 0,
            apply_journal_entries: 0,
            applied_revision: 5,
            pending_ack_revision: Some(5),
            last_ack_revision: 4,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Apply));
        assert_eq!(result.first_failed, Some(ProgressBoundary::Ack));
    }

    #[test]
    fn injected_ui_false_online_classifies_special_boundary() {
        let snapshot = ChainSnapshot {
            local_file_mutations: 0,
            runtime_degraded: true,
            ui_shows_running: true,
            ..base()
        };
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.first_failed, Some(ProgressBoundary::UiFalseOnline));
        assert_eq!(result.first_failed_str(), Some("ui-false-online"));
    }

    #[test]
    fn ui_false_online_preserves_chain_last_passed_when_present() {
        let mut snapshot = fully_acked();
        snapshot.runtime_degraded = true;
        snapshot.ui_shows_running = true;
        // Fully acked has no residual outbox/intent, but mutations > 0 is ok.
        // Clear pending-work signals that would re-open earlier stages.
        snapshot.local_file_mutations = 0;
        let result = classify_last_passed_boundary(&snapshot);
        assert_eq!(result.first_failed, Some(ProgressBoundary::UiFalseOnline));
        assert_eq!(result.last_passed, Some(ProgressBoundary::Ack));
    }

    #[test]
    fn fully_acked_chain_has_no_failure() {
        let result = classify_last_passed_boundary(&fully_acked());
        assert_eq!(result.first_failed, None);
        assert_eq!(result.last_passed, Some(ProgressBoundary::Ack));
    }

    #[test]
    fn every_chain_boundary_is_reachable_via_injected_fault() {
        let cases: [(ProgressBoundary, ChainSnapshot); 7] = [
            (
                ProgressBoundary::Watcher,
                ChainSnapshot {
                    local_file_mutations: 1,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Outbox,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Transport,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    outbox_entries: 1,
                    transport_connection_idle: true,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Server,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    outbox_entries: 1,
                    transport_requests_sent: 1,
                    transport_connection_idle: false,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Stream,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    outbox_entries: 1,
                    transport_requests_sent: 1,
                    transport_connection_idle: false,
                    server_operations: 1,
                    server_revision: 3,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Apply,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    transport_requests_sent: 1,
                    transport_connection_idle: false,
                    server_operations: 1,
                    server_revision: 3,
                    peer_stream_revision: 3,
                    peer_stream_items: 1,
                    stream_items_ready: 1,
                    apply_journal_entries: 1,
                    applied_revision: 2,
                    ..base()
                },
            ),
            (
                ProgressBoundary::Ack,
                ChainSnapshot {
                    local_file_mutations: 1,
                    watcher_events: 1,
                    transport_requests_sent: 1,
                    transport_connection_idle: false,
                    server_operations: 1,
                    server_revision: 3,
                    peer_stream_revision: 3,
                    peer_stream_items: 1,
                    applied_revision: 3,
                    pending_ack_revision: Some(3),
                    last_ack_revision: 2,
                    ..base()
                },
            ),
        ];
        for (expected_failed, snapshot) in cases {
            let result = classify_last_passed_boundary(&snapshot);
            assert_eq!(
                result.first_failed,
                Some(expected_failed),
                "expected first_failed={expected_failed:?} for {snapshot:?}, got {result:?}"
            );
            assert_eq!(
                result.last_passed,
                expected_failed.predecessor(),
                "last_passed predecessor for {expected_failed:?}"
            );
        }
    }
}
