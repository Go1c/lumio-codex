use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCHEMA_VERSION_EVENT: &str = "fns-diagnostic-event/1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub schema_version: String,
    pub timestamp: String,
    pub level: DiagnosticLevel,
    pub component: String,
    pub event_name: String,
    pub message: String,
    pub project_ref: String,
    pub run_id: String,
    pub trace_id: String,
    pub connection_generation: u64,
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
    pub stream_id: Option<String>,
    pub error_code: Option<String>,
    pub retryable: bool,
    pub fields: BTreeMap<String, Value>,
}

impl DiagnosticEvent {
    pub fn new(
        level: DiagnosticLevel,
        component: impl Into<String>,
        event_name: impl Into<String>,
        message: impl Into<String>,
        project_ref: impl Into<String>,
        run_id: impl Into<String>,
        connection_generation: u64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_EVENT.to_string(),
            timestamp: chrono_like_now(),
            level,
            component: component.into(),
            event_name: event_name.into(),
            message: message.into(),
            project_ref: project_ref.into(),
            run_id: run_id.into(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            connection_generation,
            request_id: None,
            operation_id: None,
            stream_id: None,
            error_code: None,
            retryable: false,
            fields: BTreeMap::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }

    pub fn with_error(mut self, code: impl Into<String>, retryable: bool) -> Self {
        self.error_code = Some(code.into());
        self.retryable = retryable;
        self
    }
}

/// Minimal RFC3339 timestamp without pulling chrono as a dependency.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    // Sufficient for diagnostics; tests use fixture timestamps.
    format_rfc3339(secs, millis)
}

fn format_rfc3339(secs: u64, millis: u32) -> String {
    // 1970-01-01 + secs; simple UTC formatter.
    let days = secs / 86_400;
    let day_secs = secs % 86_400;
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}Z")
}

// Howard Hinnant civil_from_days algorithm (public domain).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_camel_case_schema() {
        let event = DiagnosticEvent::new(
            DiagnosticLevel::Info,
            "transport",
            "workspace.ack.confirmed",
            "ack advanced",
            "proj",
            "run-1",
            1,
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["schemaVersion"], SCHEMA_VERSION_EVENT);
        assert_eq!(json["eventName"], "workspace.ack.confirmed");
        assert_eq!(json["connectionGeneration"], 1);
    }
}
