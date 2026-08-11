//! Bidirectional soak scenario skeleton (Wave 0 / M4-A).
//!
//! Describes a 10-minute alternating A/B progress plan, wait-for-progress
//! conditions (revision + peer hash + durable ack), stall detection with a
//! full-chain snapshot, and a minimal bug-package summary compatible with
//! `fns-diagnostic-run/1` boundary strings.
//!
//! Live remote execution is intentionally not required: unit tests drive the
//! wait loop with fake clocks and injectable progress samplers.

use crate::boundary::{
    classify_last_passed_boundary, BoundaryClassification, ChainSnapshot, ProgressBoundary,
};
use crate::scenario::Endpoint;
use crate::{HarnessError, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default soak length: 10 minutes of bidirectional progress.
pub const DEFAULT_SOAK_DURATION: Duration = Duration::from_secs(10 * 60);
/// Issue the next A/B mutation every 15 seconds while the soak is healthy.
pub const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_secs(15);
/// Pending work that does not advance for 30 seconds fails the soak.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BidirectionalSoakConfig {
    /// Total soak length in milliseconds (default 10 minutes).
    pub duration_ms: u64,
    /// Issue the next A/B mutation every N milliseconds (default 15s).
    pub progress_interval_ms: u64,
    /// Pending work without progress for N milliseconds fails (default 30s).
    pub stall_timeout_ms: u64,
    /// When true, the plan inserts reconnect effects between progress windows.
    pub inject_reconnect: bool,
    /// When true, the plan inserts agent/app restart effects between windows.
    pub inject_restart: bool,
}

impl Default for BidirectionalSoakConfig {
    fn default() -> Self {
        Self {
            duration_ms: u64::try_from(DEFAULT_SOAK_DURATION.as_millis()).unwrap_or(600_000),
            progress_interval_ms: u64::try_from(DEFAULT_PROGRESS_INTERVAL.as_millis())
                .unwrap_or(15_000),
            stall_timeout_ms: u64::try_from(DEFAULT_STALL_TIMEOUT.as_millis()).unwrap_or(30_000),
            inject_reconnect: false,
            inject_restart: false,
        }
    }
}

impl BidirectionalSoakConfig {
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms)
    }

    pub fn progress_interval(&self) -> Duration {
        Duration::from_millis(self.progress_interval_ms)
    }

    pub fn stall_timeout(&self) -> Duration {
        Duration::from_millis(self.stall_timeout_ms)
    }

    pub fn validate(&self) -> Result<()> {
        if self.duration_ms == 0 {
            return Err(HarnessError::InvalidConfiguration(
                "soak duration must be positive",
            ));
        }
        if self.progress_interval_ms == 0 {
            return Err(HarnessError::InvalidConfiguration(
                "soak progress interval must be positive",
            ));
        }
        if self.stall_timeout_ms == 0 {
            return Err(HarnessError::InvalidConfiguration(
                "soak stall timeout must be positive",
            ));
        }
        if self.stall_timeout_ms < self.progress_interval_ms {
            return Err(HarnessError::InvalidConfiguration(
                "soak stall timeout must be at least the progress interval",
            ));
        }
        Ok(())
    }
}

/// Alternating create/modify + rename/delete/empty/binary/dir work items.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SoakMutation {
    CreateText {
        endpoint: Endpoint,
        path: String,
    },
    Modify {
        endpoint: Endpoint,
        path: String,
    },
    Rename {
        endpoint: Endpoint,
        from: String,
        to: String,
    },
    Delete {
        endpoint: Endpoint,
        path: String,
    },
    CreateEmpty {
        endpoint: Endpoint,
        path: String,
    },
    CreateBinary {
        endpoint: Endpoint,
        path: String,
    },
    CreateDirectory {
        endpoint: Endpoint,
        path: String,
    },
    InjectReconnect,
    InjectRestart {
        endpoint: Endpoint,
    },
}

/// One scheduled step of the soak plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoakStep {
    pub index: u64,
    pub due_at_ms: u64,
    pub mutation: SoakMutation,
}

/// Bidirectional soak scenario: config + deterministic alternating plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BidirectionalSoakScenario {
    pub scenario_id: String,
    pub config: BidirectionalSoakConfig,
    pub steps: Vec<SoakStep>,
}

impl BidirectionalSoakScenario {
    pub fn plan(config: BidirectionalSoakConfig) -> Result<Self> {
        config.validate()?;
        let interval_ms = config.progress_interval_ms;
        let duration_ms = config.duration_ms;

        let mut steps = Vec::new();
        let mut due_at_ms = 0_u64;
        let mut index = 0_u64;
        let mut cycle = 0_u64;
        while due_at_ms < duration_ms {
            let endpoint = if cycle % 2 == 0 {
                Endpoint::A
            } else {
                Endpoint::B
            };
            let path_base = format!("soak/{}/c{cycle}", endpoint_label(endpoint));
            for mutation in cycle_mutations(endpoint, &path_base, cycle) {
                steps.push(SoakStep {
                    index,
                    due_at_ms,
                    mutation,
                });
                index = index.saturating_add(1);
            }
            if config.inject_reconnect && cycle > 0 && cycle % 4 == 0 {
                steps.push(SoakStep {
                    index,
                    due_at_ms,
                    mutation: SoakMutation::InjectReconnect,
                });
                index = index.saturating_add(1);
            }
            if config.inject_restart && cycle > 0 && cycle % 6 == 0 {
                steps.push(SoakStep {
                    index,
                    due_at_ms,
                    mutation: SoakMutation::InjectRestart { endpoint },
                });
                index = index.saturating_add(1);
            }
            due_at_ms = due_at_ms.saturating_add(interval_ms);
            cycle = cycle.saturating_add(1);
        }

        Ok(Self {
            scenario_id: "bidirectional-soak-10m".to_owned(),
            config,
            steps,
        })
    }
}

fn endpoint_label(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::A => "a",
        Endpoint::B => "b",
    }
}

fn cycle_mutations(endpoint: Endpoint, path_base: &str, cycle: u64) -> Vec<SoakMutation> {
    // Rotate create/modify + rename/delete/empty/binary/dir so each cycle
    // exercises a different shape while still alternating endpoints.
    let kind = cycle % 6;
    match kind {
        0 => vec![
            SoakMutation::CreateDirectory {
                endpoint,
                path: format!("{path_base}/dir"),
            },
            SoakMutation::CreateText {
                endpoint,
                path: format!("{path_base}/dir/note.txt"),
            },
        ],
        1 => vec![SoakMutation::Modify {
            endpoint,
            path: format!("{path_base}/dir/note.txt"),
        }],
        2 => vec![
            SoakMutation::CreateEmpty {
                endpoint,
                path: format!("{path_base}/empty.dat"),
            },
            SoakMutation::CreateBinary {
                endpoint,
                path: format!("{path_base}/blob.bin"),
            },
        ],
        3 => vec![SoakMutation::Rename {
            endpoint,
            from: format!("{path_base}/dir/note.txt"),
            to: format!("{path_base}/dir/renamed.txt"),
        }],
        4 => vec![SoakMutation::Delete {
            endpoint,
            path: format!("{path_base}/empty.dat"),
        }],
        _ => vec![
            SoakMutation::CreateText {
                endpoint,
                path: format!("{path_base}/alt.txt"),
            },
            SoakMutation::Modify {
                endpoint,
                path: format!("{path_base}/alt.txt"),
            },
        ],
    }
}

/// Evidence required for a durable progress wait condition.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvidence {
    pub revision: u64,
    pub peer_manifest_digest: String,
    pub durable_ack_revision: u64,
    pub chain: ChainSnapshot,
}

impl ProgressEvidence {
    /// True when revision, peer hash, and durable ack all agree.
    pub fn satisfies(&self, required_revision: u64, required_peer_digest: &str) -> bool {
        self.revision >= required_revision
            && self.durable_ack_revision >= required_revision
            && self.durable_ack_revision == self.revision
            && !required_peer_digest.is_empty()
            && self.peer_manifest_digest == required_peer_digest
            && self.chain.pending_ack_revision.is_none()
    }
}

/// Fake-clock / live-clock abstraction for unit-testable waits.
pub trait ProgressClock {
    fn now_ms(&self) -> u64;
}

/// Samples current progress evidence (live harness or injected fake).
pub trait ProgressSampler {
    fn sample(&mut self) -> ProgressEvidence;
}

/// Stall failure carrying the full chain snapshot and boundary classification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StallFailure {
    pub waited_ms: u64,
    pub stall_timeout_ms: u64,
    pub required_revision: u64,
    pub required_peer_digest: String,
    pub last_evidence: ProgressEvidence,
    pub classification: BoundaryClassification,
}

/// Pure wait-step evaluation: no sleeps, no threads.
///
/// Callers (live harness or unit tests) sample evidence, advance a clock, and
/// invoke this until [`WaitStatus::Ready`] or [`WaitStatus::Stalled`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WaitStatus {
    Ready(ProgressEvidence),
    Pending { waited_ms: u64 },
    Stalled(StallFailure),
}

/// Evaluate one progress sample against the durable wait condition.
///
/// Stall rule: if `now_ms - started_ms >= stall_timeout` and the sample still
/// does not satisfy revision + peer hash + durable ack, fail with a full-chain
/// boundary classification. Pending work is not required to declare a stall —
/// missing expected progress past the timeout is enough.
pub fn evaluate_wait_progress(
    started_ms: u64,
    now_ms: u64,
    evidence: ProgressEvidence,
    required_revision: u64,
    required_peer_digest: &str,
    stall_timeout: Duration,
) -> WaitStatus {
    let stall_timeout_ms = u64::try_from(stall_timeout.as_millis()).unwrap_or(u64::MAX);
    if evidence.satisfies(required_revision, required_peer_digest) {
        return WaitStatus::Ready(evidence);
    }
    let waited_ms = now_ms.saturating_sub(started_ms);
    if waited_ms >= stall_timeout_ms {
        return WaitStatus::Stalled(stall_failure(
            waited_ms,
            stall_timeout_ms,
            required_revision,
            required_peer_digest,
            evidence,
        ));
    }
    WaitStatus::Pending { waited_ms }
}

/// Wait until revision + peer hash + durable ack converge, or fail on stall.
///
/// Does **not** use a fixed sleep: the caller supplies a clock and sampler.
/// The clock must advance across poll iterations (tests use [`FakeClock`] with
/// interior mutability via [`FakeClock::advance`]). When no satisfying sample
/// arrives before `stall_timeout`, returns [`StallFailure`] with a full-chain
/// classification.
pub fn wait_for_revision_peer_hash_and_ack<C, S>(
    clock: &C,
    sampler: &mut S,
    required_revision: u64,
    required_peer_digest: &str,
    stall_timeout: Duration,
) -> std::result::Result<ProgressEvidence, StallFailure>
where
    C: ProgressClock + ?Sized,
    S: ProgressSampler + ?Sized,
{
    let started_ms = clock.now_ms();
    // Bound iterations so a stuck clock cannot hang unit tests forever.
    for _ in 0..256 {
        let evidence = sampler.sample();
        match evaluate_wait_progress(
            started_ms,
            clock.now_ms(),
            evidence,
            required_revision,
            required_peer_digest,
            stall_timeout,
        ) {
            WaitStatus::Ready(evidence) => return Ok(evidence),
            WaitStatus::Stalled(failure) => return Err(failure),
            WaitStatus::Pending { .. } => continue,
        }
    }
    let evidence = sampler.sample();
    Err(stall_failure(
        clock.now_ms().saturating_sub(started_ms),
        u64::try_from(stall_timeout.as_millis()).unwrap_or(u64::MAX),
        required_revision,
        required_peer_digest,
        evidence,
    ))
}

fn stall_failure(
    waited_ms: u64,
    stall_timeout_ms: u64,
    required_revision: u64,
    required_peer_digest: &str,
    last_evidence: ProgressEvidence,
) -> StallFailure {
    let classification = classify_last_passed_boundary(&last_evidence.chain);
    StallFailure {
        waited_ms,
        stall_timeout_ms,
        required_revision,
        required_peer_digest: required_peer_digest.to_owned(),
        last_evidence,
        classification,
    }
}

/// Minimal stall bug-package summary with diagnostic-run-compatible boundary strings.
///
/// Distinct from [`crate::bug_package::BugPackageSummary`] (self-test handoff):
/// this shape carries the full [`ChainSnapshot`] used for injected-fault
/// classification, while remaining field-compatible on boundary strings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StallBugPackage {
    pub schema_version: String,
    pub scenario_id: String,
    pub outcome: String,
    /// Compatible with `fns-diagnostic-run/1` `lastPassedBoundary`.
    pub last_passed_boundary: Option<String>,
    /// Compatible with `fns-diagnostic-run/1` `firstFailedBoundary`.
    pub first_failed_boundary: Option<String>,
    pub chain: ChainSnapshot,
    pub notes: String,
}

pub const STALL_BUG_PACKAGE_SCHEMA: &str = "test-sync-stall-bug-package/1";

/// Build a minimal stall bug package from a classification + chain snapshot.
pub fn build_stall_bug_package(
    scenario_id: &str,
    chain: &ChainSnapshot,
    classification: BoundaryClassification,
    notes: impl Into<String>,
) -> StallBugPackage {
    StallBugPackage {
        schema_version: STALL_BUG_PACKAGE_SCHEMA.to_owned(),
        scenario_id: scenario_id.to_owned(),
        outcome: if classification.first_failed.is_some() {
            "failed".to_owned()
        } else {
            "passed".to_owned()
        },
        last_passed_boundary: classification
            .last_passed
            .map(ProgressBoundary::as_str)
            .map(str::to_owned),
        first_failed_boundary: classification
            .first_failed
            .map(ProgressBoundary::as_str)
            .map(str::to_owned),
        chain: chain.clone(),
        notes: notes.into(),
    }
}

impl StallBugPackage {
    pub fn from_stall(scenario_id: &str, stall: &StallFailure) -> Self {
        build_stall_bug_package(
            scenario_id,
            &stall.last_evidence.chain,
            stall.classification,
            format!(
                "stall after {}ms (timeout {}ms) waiting for revision {} peer_digest={}",
                stall.waited_ms,
                stall.stall_timeout_ms,
                stall.required_revision,
                stall.required_peer_digest
            ),
        )
    }
}

/// Simple integer clock for unit tests (shared interior mutability via `Rc`).
#[derive(Clone, Debug, Default)]
pub struct FakeClock {
    now_ms: std::rc::Rc<std::cell::Cell<u64>>,
}

impl ProgressClock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.get()
    }
}

impl FakeClock {
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms: std::rc::Rc::new(std::cell::Cell::new(now_ms)),
        }
    }

    pub fn advance(&self, delta_ms: u64) {
        self.now_ms.set(self.now_ms.get().saturating_add(delta_ms));
    }

    pub fn set(&self, now_ms: u64) {
        self.now_ms.set(now_ms);
    }
}

/// Sampler that returns a preloaded sequence of evidence snapshots.
///
/// Optional `advance_clock_ms_per_sample` advances a linked [`FakeClock`] on
/// each sample so stall timeouts can be exercised without real sleeps.
#[derive(Clone, Debug, Default)]
pub struct ScriptedSampler {
    pub samples: Vec<ProgressEvidence>,
    pub index: usize,
    pub advance_clock_ms_per_sample: u64,
    clock: Option<FakeClock>,
}

impl ScriptedSampler {
    pub fn new(samples: Vec<ProgressEvidence>) -> Self {
        Self {
            samples,
            index: 0,
            advance_clock_ms_per_sample: 0,
            clock: None,
        }
    }

    pub fn with_clock(mut self, clock: FakeClock, advance_ms_per_sample: u64) -> Self {
        self.clock = Some(clock);
        self.advance_clock_ms_per_sample = advance_ms_per_sample;
        self
    }
}

impl ProgressSampler for ScriptedSampler {
    fn sample(&mut self) -> ProgressEvidence {
        if let Some(clock) = &self.clock {
            if self.advance_clock_ms_per_sample > 0 {
                clock.advance(self.advance_clock_ms_per_sample);
            }
        }
        if self.samples.is_empty() {
            return ProgressEvidence::default();
        }
        let sample = self.samples[self.index.min(self.samples.len() - 1)].clone();
        if self.index + 1 < self.samples.len() {
            self.index += 1;
        }
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::ProgressBoundary;

    #[test]
    fn default_config_matches_design_intervals() {
        let config = BidirectionalSoakConfig::default();
        assert_eq!(config.duration(), Duration::from_secs(600));
        assert_eq!(config.progress_interval(), Duration::from_secs(15));
        assert_eq!(config.stall_timeout(), Duration::from_secs(30));
        config.validate().expect("default config");
    }

    #[test]
    fn soak_plan_alternates_endpoints_and_mutation_shapes() {
        let config = BidirectionalSoakConfig {
            duration_ms: 60_000,
            progress_interval_ms: 15_000,
            stall_timeout_ms: 30_000,
            inject_reconnect: false,
            inject_restart: false,
        };
        let scenario = BidirectionalSoakScenario::plan(config).expect("plan");
        assert_eq!(scenario.scenario_id, "bidirectional-soak-10m");
        assert!(!scenario.steps.is_empty());

        let mut saw_a = false;
        let mut saw_b = false;
        let mut kinds = std::collections::HashSet::new();
        for step in &scenario.steps {
            match &step.mutation {
                SoakMutation::CreateText { endpoint, .. }
                | SoakMutation::Modify { endpoint, .. }
                | SoakMutation::Rename { endpoint, .. }
                | SoakMutation::Delete { endpoint, .. }
                | SoakMutation::CreateEmpty { endpoint, .. }
                | SoakMutation::CreateBinary { endpoint, .. }
                | SoakMutation::CreateDirectory { endpoint, .. } => match endpoint {
                    Endpoint::A => saw_a = true,
                    Endpoint::B => saw_b = true,
                },
                SoakMutation::InjectReconnect | SoakMutation::InjectRestart { .. } => {}
            }
            kinds.insert(std::mem::discriminant(&step.mutation));
        }
        assert!(saw_a && saw_b, "plan must alternate both endpoints");
        assert!(
            kinds.len() >= 4,
            "plan should exercise multiple mutation shapes, got {}",
            kinds.len()
        );
    }

    #[test]
    fn inject_flags_emit_reconnect_and_restart_steps() {
        let config = BidirectionalSoakConfig {
            duration_ms: 15_000 * 12,
            progress_interval_ms: 15_000,
            stall_timeout_ms: 30_000,
            inject_reconnect: true,
            inject_restart: true,
        };
        let scenario = BidirectionalSoakScenario::plan(config).expect("plan");
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step.mutation, SoakMutation::InjectReconnect)));
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step.mutation, SoakMutation::InjectRestart { .. })));
    }

    #[test]
    fn wait_condition_succeeds_when_revision_peer_hash_and_ack_agree() {
        let clock = FakeClock::new(0);
        let evidence = ProgressEvidence {
            revision: 9,
            peer_manifest_digest: "digest-abc".to_owned(),
            durable_ack_revision: 9,
            chain: ChainSnapshot {
                observed_at_ms: 0,
                server_revision: 9,
                peer_stream_revision: 9,
                applied_revision: 9,
                last_ack_revision: 9,
                pending_ack_revision: None,
                ..ChainSnapshot::default()
            },
        };
        let mut sampler = ScriptedSampler::new(vec![evidence.clone()]);
        let got = wait_for_revision_peer_hash_and_ack(
            &clock,
            &mut sampler,
            9,
            "digest-abc",
            Duration::from_secs(30),
        )
        .expect("progress");
        assert_eq!(got, evidence);
    }

    #[test]
    fn evaluate_wait_progress_is_pure_and_stalls_after_timeout() {
        let stalled_chain = ChainSnapshot {
            observed_at_ms: 30_000,
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 1,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 0,
            server_revision: 0,
            ..ChainSnapshot::default()
        };
        let evidence = ProgressEvidence {
            revision: 1,
            peer_manifest_digest: "stale".to_owned(),
            durable_ack_revision: 1,
            chain: stalled_chain.clone(),
        };
        assert!(matches!(
            evaluate_wait_progress(
                0,
                10_000,
                evidence.clone(),
                2,
                "expected",
                Duration::from_secs(30)
            ),
            WaitStatus::Pending { waited_ms: 10_000 }
        ));
        match evaluate_wait_progress(
            0,
            30_000,
            evidence,
            2,
            "expected-digest",
            Duration::from_secs(30),
        ) {
            WaitStatus::Stalled(err) => {
                assert_eq!(
                    err.classification.last_passed,
                    Some(ProgressBoundary::Transport)
                );
                assert_eq!(
                    err.classification.first_failed,
                    Some(ProgressBoundary::Server)
                );
                assert_eq!(err.last_evidence.chain, stalled_chain);
            }
            other => panic!("expected stalled, got {other:?}"),
        }
    }

    #[test]
    fn wait_condition_fails_after_stall_timeout_with_chain_classification() {
        let clock = FakeClock::new(0);
        let stalled_chain = ChainSnapshot {
            observed_at_ms: 30_000,
            local_file_mutations: 1,
            watcher_events: 1,
            outbox_entries: 1,
            transport_requests_sent: 1,
            transport_connection_idle: false,
            server_operations: 0,
            server_revision: 0,
            ..ChainSnapshot::default()
        };
        let mut sampler = ScriptedSampler::new(vec![ProgressEvidence {
            revision: 1,
            peer_manifest_digest: "stale".to_owned(),
            durable_ack_revision: 1,
            chain: stalled_chain.clone(),
        }])
        .with_clock(clock.clone(), 15_000);
        let err = wait_for_revision_peer_hash_and_ack(
            &clock,
            &mut sampler,
            2,
            "expected-digest",
            Duration::from_secs(30),
        )
        .expect_err("must stall");
        assert!(err.waited_ms >= 30_000);
        assert_eq!(
            err.classification.last_passed,
            Some(ProgressBoundary::Transport)
        );
        assert_eq!(
            err.classification.first_failed,
            Some(ProgressBoundary::Server)
        );
        assert_eq!(err.last_evidence.chain, stalled_chain);

        let package = StallBugPackage::from_stall("bidirectional-soak-10m", &err);
        assert_eq!(package.last_passed_boundary.as_deref(), Some("transport"));
        assert_eq!(package.first_failed_boundary.as_deref(), Some("server"));
        assert_eq!(package.outcome, "failed");
        assert_eq!(package.scenario_id, "bidirectional-soak-10m");
    }

    #[test]
    fn bug_package_builder_emits_diagnostic_run_compatible_boundary_strings() {
        let chain = ChainSnapshot {
            local_file_mutations: 1,
            watcher_events: 0,
            ..ChainSnapshot::default()
        };
        let classification = classify_last_passed_boundary(&chain);
        let package = build_stall_bug_package(
            "bidirectional-soak-10m",
            &chain,
            classification,
            "injected watcher fault",
        );
        assert_eq!(package.schema_version, STALL_BUG_PACKAGE_SCHEMA);
        assert_eq!(package.last_passed_boundary, None);
        assert_eq!(package.first_failed_boundary.as_deref(), Some("watcher"));
        assert_eq!(package.outcome, "failed");
    }

    #[test]
    fn progress_evidence_requires_matching_peer_digest_and_ack() {
        let evidence = ProgressEvidence {
            revision: 3,
            peer_manifest_digest: "left".to_owned(),
            durable_ack_revision: 3,
            chain: ChainSnapshot::default(),
        };
        assert!(!evidence.satisfies(3, "right"));
        assert!(!evidence.satisfies(4, "left"));
        let mut pending = evidence.clone();
        pending.chain.pending_ack_revision = Some(3);
        assert!(!pending.satisfies(3, "left"));
        assert!(evidence.satisfies(3, "left"));
    }
}
