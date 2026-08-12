//! Cross-fixture contract tests for DiagnosticEvent / HealthSnapshot / DiagnosticRun.
//!
//! Loads shared fixtures from repo-root `contracts/diagnostics/fixtures/`.

use fns_observability::{
    DiagnosticEvent, DiagnosticRun, HealthSnapshot, SCHEMA_VERSION_EVENT, SCHEMA_VERSION_HEALTH,
    SCHEMA_VERSION_RUN, parse_contract, read_fixture, redact_fields, round_trip_json,
};
use serde_json::Value;

#[test]
fn diagnostic_event_fixture_round_trips() {
    let raw = read_fixture("diagnostic-event-v1.json").expect("fixture present");
    let event: DiagnosticEvent =
        parse_contract(&raw, SCHEMA_VERSION_EVENT).expect("parse known fixture");
    assert_eq!(event.schema_version, SCHEMA_VERSION_EVENT);
    assert_eq!(event.event_name, "workspace.ack.confirmed");
    assert_eq!(event.connection_generation, 4);
    assert_eq!(event.run_id, "11111111-1111-4111-8111-111111111111");

    let again = round_trip_json(&event).expect("round-trip");
    assert_eq!(event, again);

    // Serialize back and compare key fields to fixture.
    let encoded: Value = serde_json::to_value(&event).unwrap();
    let original: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(encoded["schemaVersion"], original["schemaVersion"]);
    assert_eq!(encoded["eventName"], original["eventName"]);
    assert_eq!(encoded["fields"], original["fields"]);
    assert_eq!(
        encoded["connectionGeneration"],
        original["connectionGeneration"]
    );
}

#[test]
fn health_snapshot_fixture_round_trips() {
    let raw = read_fixture("health-snapshot-v1.json").expect("fixture present");
    let snap: HealthSnapshot =
        parse_contract(&raw, SCHEMA_VERSION_HEALTH).expect("parse known fixture");
    assert_eq!(snap.schema_version, SCHEMA_VERSION_HEALTH);
    assert_eq!(snap.last_progress_boundary.as_str(), "ack");
    let again = round_trip_json(&snap).expect("round-trip");
    assert_eq!(snap, again);
}

#[test]
fn diagnostic_run_fixture_round_trips() {
    let raw = read_fixture("diagnostic-run-v1.json").expect("fixture present");
    let run: DiagnosticRun = parse_contract(&raw, SCHEMA_VERSION_RUN).expect("parse known fixture");
    assert_eq!(run.schema_version, SCHEMA_VERSION_RUN);
    assert_eq!(run.profile, "ci-isolation");
    assert_eq!(run.redaction_summary.secret_hits, 0);
    let again = round_trip_json(&run).expect("round-trip");
    assert_eq!(run, again);
}

#[test]
fn unknown_schema_version_fails_closed() {
    let raw = read_fixture("unknown-schema-version.json").expect("fixture present");
    let err = parse_contract::<DiagnosticEvent>(&raw, SCHEMA_VERSION_EVENT)
        .expect_err("must reject unknown version");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown") || msg.contains("unsupported") || msg.contains("999"),
        "unexpected error: {msg}"
    );
}

#[test]
fn secret_corpus_redacted_output_has_zero_hits() {
    let raw = read_fixture("secret-corpus.json").expect("fixture present");
    let corpus: Value = serde_json::from_str(&raw).unwrap();
    let sample = &corpus["sample_raw"];
    let (redacted, summary) = redact_fields(sample);
    let text = serde_json::to_string(&redacted).unwrap();

    let mut hits = 0u64;
    for pattern in corpus["patterns"].as_array().unwrap() {
        let p = pattern.as_str().unwrap();
        if text.contains(p) {
            hits += 1;
            eprintln!("HIT: {p}");
        }
    }
    assert_eq!(
        hits, 0,
        "redacted output still contains secret patterns:\n{text}"
    );
    assert!(
        summary.secret_hits > 0,
        "redactor should have recorded hits"
    );
}
