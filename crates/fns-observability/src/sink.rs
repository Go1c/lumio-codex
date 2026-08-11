use crate::event::DiagnosticEvent;
use crate::redact::{RedactionSummary, redact_fields};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SinkError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sink closed")]
    Closed,
}

/// Non-blocking diagnostic sink. Implementations must never panic callers
/// when disk is full or permissions fail — emit APIs return Result and
/// callers treat errors as best-effort.
pub trait DiagnosticSink: Send + Sync {
    fn emit(&self, event: &DiagnosticEvent) -> Result<(), SinkError>;
    fn flush(&self) -> Result<(), SinkError> {
        Ok(())
    }
}

/// In-memory sink for tests and Desktop facade mocks.
#[derive(Clone, Default)]
pub struct MemorySink {
    events: Arc<Mutex<Vec<DiagnosticEvent>>>,
    fail_next: Arc<Mutex<bool>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_fail_next(&self) {
        *self.fail_next.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    pub fn events(&self) -> Vec<DiagnosticEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn len(&self) -> usize {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl DiagnosticSink for MemorySink {
    fn emit(&self, event: &DiagnosticEvent) -> Result<(), SinkError> {
        let mut fail = self.fail_next.lock().unwrap_or_else(|e| e.into_inner());
        if *fail {
            *fail = false;
            return Err(SinkError::Io(io::Error::other("forced sink failure")));
        }
        drop(fail);
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
        Ok(())
    }
}

/// Rolling JSONL sink. Caps by max_bytes (best-effort rotate).
pub struct RollingJsonlSink {
    path: PathBuf,
    max_bytes: u64,
    file: Mutex<Option<File>>,
    last_redaction: Mutex<RedactionSummary>,
}

impl RollingJsonlSink {
    pub fn open(path: impl AsRef<Path>, max_bytes: u64) -> Result<Self, SinkError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(Self {
            path,
            max_bytes,
            file: Mutex::new(Some(file)),
            last_redaction: Mutex::new(RedactionSummary::default()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn last_redaction(&self) -> RedactionSummary {
        self.last_redaction
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn rotate_if_needed(&self, file: &mut Option<File>) -> Result<(), SinkError> {
        let meta = fs::metadata(&self.path)?;
        if meta.len() < self.max_bytes {
            return Ok(());
        }
        // Close current, rename, open new.
        *file = None;
        let rotated = self.path.with_extension("jsonl.1");
        let _ = fs::remove_file(&rotated);
        fs::rename(&self.path, &rotated)?;
        let new_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        *file = Some(new_file);
        Ok(())
    }
}

impl DiagnosticSink for RollingJsonlSink {
    fn emit(&self, event: &DiagnosticEvent) -> Result<(), SinkError> {
        // Redact fields before durable write.
        let mut owned = event.clone();
        let fields_value = Value::Object(owned.fields.into_iter().collect());
        let (redacted, summary) = redact_fields(&fields_value);
        if let Value::Object(map) = redacted {
            owned.fields = map.into_iter().collect();
        } else {
            owned.fields = Default::default();
        }
        *self
            .last_redaction
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = summary;

        let line = serde_json::to_string(&owned)?;
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        self.rotate_if_needed(&mut guard)?;
        let file = guard.as_mut().ok_or(SinkError::Closed)?;
        writeln!(file, "{line}")?;
        file.flush()?;
        Ok(())
    }

    fn flush(&self) -> Result<(), SinkError> {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = guard.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

/// Best-effort emit that never propagates sink errors to the hot path.
pub fn emit_lossy(sink: &dyn DiagnosticSink, event: &DiagnosticEvent) {
    let _ = sink.emit(event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{DiagnosticEvent, DiagnosticLevel};

    #[test]
    fn memory_sink_force_fail_does_not_store() {
        let sink = MemorySink::new();
        sink.force_fail_next();
        let event = DiagnosticEvent::new(
            DiagnosticLevel::Info,
            "test",
            "test.event",
            "hi",
            "p",
            "r",
            0,
        );
        assert!(sink.emit(&event).is_err());
        assert!(sink.is_empty());
        assert!(sink.emit(&event).is_ok());
        assert_eq!(sink.len(), 1);
    }

    #[test]
    fn rolling_jsonl_writes_redacted_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let sink = RollingJsonlSink::open(&path, 1024 * 1024).unwrap();
        let mut event = DiagnosticEvent::new(
            DiagnosticLevel::Info,
            "test",
            "test.secret",
            "msg",
            "p",
            "r",
            1,
        );
        event.fields.insert(
            "password".into(),
            Value::String("should-not-persist".into()),
        );
        sink.emit(&event).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[REDACTED]"));
        assert!(!content.contains("should-not-persist"));
        assert!(content.contains("fns-diagnostic-event/1"));
    }
}
