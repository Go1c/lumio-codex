use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCHEMA_VERSION_HEALTH: &str = "fns-health-snapshot/1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    Unknown,
}

impl ProgressBoundary {
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
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub schema_version: String,
    pub timestamp: String,
    pub run_id: String,
    pub project_ref: String,
    pub connection_generation: u64,
    pub last_progress_boundary: ProgressBoundary,
    pub desktop: BTreeMap<String, Value>,
    pub process: BTreeMap<String, Value>,
    pub watcher: BTreeMap<String, Value>,
    pub outbox: BTreeMap<String, Value>,
    pub transport: BTreeMap<String, Value>,
    pub stream: BTreeMap<String, Value>,
    pub cursor: BTreeMap<String, Value>,
    pub server: BTreeMap<String, Value>,
}

impl HealthSnapshot {
    pub fn empty(
        run_id: impl Into<String>,
        project_ref: impl Into<String>,
        connection_generation: u64,
    ) -> Self {
        let run_id = run_id.into();
        let project_ref = project_ref.into();
        // Borrow timestamp formatter via a throwaway event.
        let timestamp = crate::event::DiagnosticEvent::new(
            crate::event::DiagnosticLevel::Info,
            "health",
            "health.snapshot",
            "empty",
            project_ref.clone(),
            run_id.clone(),
            connection_generation,
        )
        .timestamp;
        Self {
            schema_version: SCHEMA_VERSION_HEALTH.to_string(),
            timestamp,
            run_id,
            project_ref,
            connection_generation,
            last_progress_boundary: ProgressBoundary::Unknown,
            desktop: BTreeMap::new(),
            process: BTreeMap::new(),
            watcher: BTreeMap::new(),
            outbox: BTreeMap::new(),
            transport: BTreeMap::new(),
            stream: BTreeMap::new(),
            cursor: BTreeMap::new(),
            server: BTreeMap::new(),
        }
    }
}
