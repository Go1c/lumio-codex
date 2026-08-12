//! Cross-language diagnostic contracts, redaction, and non-blocking sinks.
//!
//! Contract ownership lives in repo-root `contracts/diagnostics/`. This crate
//! is the Rust adapter: typed DTOs, fail-closed schema validation, redaction,
//! and a rolling JSONL sink that never blocks the sync hot path.

mod event;
mod health;
mod redact;
mod run;
mod runtime;
mod sink;

pub use event::{DiagnosticEvent, DiagnosticLevel, SCHEMA_VERSION_EVENT};
pub use health::{HealthSnapshot, ProgressBoundary, SCHEMA_VERSION_HEALTH};
pub use redact::{RedactionSummary, SECRET_KEYS, path_fingerprint, redact_fields, redact_string};
pub use run::{DiagnosticRun, RedactionSummaryDto, RunOutcome, SCHEMA_VERSION_RUN};
pub use runtime::{RuntimeDiagnostics, fields_from};
pub use sink::{DiagnosticSink, MemorySink, RollingJsonlSink, SinkError, emit_lossy};

use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

/// Errors when parsing diagnostic contracts.
#[derive(Debug, Error)]
pub enum ContractError {
    #[error("unknown or unsupported schemaVersion: {0}")]
    UnknownSchemaVersion(String),
    #[error("missing schemaVersion field")]
    MissingSchemaVersion,
    #[error("json decode failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema validation failed: {0}")]
    Invalid(String),
}

/// Parse a diagnostic document fail-closed against an expected schema version.
pub fn parse_contract<T>(raw: &str, expected_schema: &str) -> Result<T, ContractError>
where
    T: DeserializeOwned,
{
    let value: Value = serde_json::from_str(raw)?;
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_str)
        .ok_or(ContractError::MissingSchemaVersion)?;
    if version != expected_schema {
        return Err(ContractError::UnknownSchemaVersion(version.to_string()));
    }
    Ok(serde_json::from_value(value)?)
}

/// Round-trip helper used by contract tests.
pub fn round_trip_json<T>(value: &T) -> Result<T, ContractError>
where
    T: serde::Serialize + DeserializeOwned,
{
    let encoded = serde_json::to_string(value)?;
    Ok(serde_json::from_str(&encoded)?)
}

/// Locate contracts/ relative to this crate (client/crates/fns-observability → repo root).
pub fn contracts_dir() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // client/crates/fns-observability → repo root
    manifest
        .join("../../..")
        .join("contracts")
        .join("diagnostics")
        .canonicalize()
        .unwrap_or_else(|_| {
            manifest
                .join("../../..")
                .join("contracts")
                .join("diagnostics")
        })
}

pub fn fixture_path(name: &str) -> std::path::PathBuf {
    contracts_dir().join("fixtures").join(name)
}

pub fn read_fixture(name: &str) -> Result<String, std::io::Error> {
    std::fs::read_to_string(fixture_path(name))
}
