use std::time::{SystemTime, UNIX_EPOCH};

/// Return wall-clock milliseconds for durable bookkeeping timestamps.
///
/// A clock before the Unix epoch is represented as zero; timestamps are
/// metadata and must never prevent opening otherwise recoverable state.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
