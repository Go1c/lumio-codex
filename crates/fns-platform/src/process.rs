//! Stale-safe process locking and atomic private JSON writes.
//!
//! Linux locks use `/proc/sys/kernel/random/boot_id` plus field 22 of
//! `/proc/<pid>/stat` to detect live vs stale lock holders. Non-Linux targets
//! return `UnsupportedPlatform` at runtime.

use crate::error::{PlatformError, PlatformErrorCode};

use std::fmt;
use std::path::{Path, PathBuf};

/// On-disk record of who holds a process lock.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessLockRecord {
    pub pid: u32,
    pub boot_id: String,
    pub start_ticks: u64,
    pub nonce: uuid::Uuid,
}

/// A process lock acquired via `create_new` on Linux. Dropping it removes the
/// lock file only if the on-disk nonce still matches its own.
pub struct ProcessLock {
    #[allow(dead_code)]
    path: PathBuf,
    #[allow(dead_code)]
    nonce: uuid::Uuid,
    #[allow(dead_code)]
    released: bool,
}

impl fmt::Debug for ProcessLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessLock").finish()
    }
}

#[cfg(target_os = "linux")]
impl ProcessLock {
    /// Acquire an exclusive lock. If an existing lock is stale (PID gone, boot_id
    /// mismatch, or start_ticks mismatch), it is renamed atomically and a new one is created.
    pub fn acquire_linux(path: &Path) -> Result<Self, PlatformError> {
        use std::fs;
        use std::io::Read;

        let boot_id = read_boot_id()?;
        let pid = std::process::id();
        let start_ticks = read_start_ticks(pid)?;
        let nonce = uuid::Uuid::new_v4();

        // Check for existing lock.
        if let Ok(record) = Self::probe_linux(path)? {
            // Is it live?
            if is_live(&record, &boot_id) {
                return Err(PlatformError::new(PlatformErrorCode::AlreadyRunning));
            }
            // Stale — rename it atomically.
            let stale_name = format!(".fns-stale-lock-{}", record.nonce);
            let stale_path = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&stale_name);
            let _ = fs::rename(path, &stale_path);
            let _ = fs::remove_file(&stale_path);
        }

        // Create new lock file.
        let record = ProcessLockRecord {
            pid,
            boot_id: boot_id.clone(),
            start_ticks,
            nonce,
        };
        let json =
            serde_json::to_vec(&record).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        use std::io::Write;
        file.write_all(&json)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

        Ok(Self {
            path: path.to_path_buf(),
            nonce,
            released: false,
        })
    }

    /// Probe a lock file without acquiring. Returns `None` if the file does not exist.
    pub fn probe_linux(path: &Path) -> Result<Option<ProcessLockRecord>, PlatformError> {
        use std::fs;
        let mut buf = Vec::new();
        let exists = match fs::File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf)
                    .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
                if buf.len() > 4096 {
                    return Err(PlatformError::new(PlatformErrorCode::CorruptLock));
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
        };
        if !exists {
            return Ok(None);
        }
        let record: ProcessLockRecord = serde_json::from_slice(&buf)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CorruptLock))?;
        Ok(Some(record))
    }
}

#[cfg(not(target_os = "linux"))]
impl ProcessLock {
    pub fn acquire_linux(_path: &Path) -> Result<Self, PlatformError> {
        Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
    }

    pub fn probe_linux(_path: &Path) -> Result<Option<ProcessLockRecord>, PlatformError> {
        Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        // On non-Linux the lock can never be acquired, so there is nothing to clean up.
        #[cfg(target_os = "linux")]
        {
            if self.released {
                return;
            }
            // Only remove if the on-disk nonce matches our own.
            if let Ok(Some(record)) = Self::probe_linux(&self.path) {
                if record.nonce == self.nonce {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
    }
}

/// Write JSON atomically to a private (0600) file via a temp sibling + rename.
#[cfg(target_os = "linux")]
pub fn atomic_write_private_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), PlatformError> {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let json = serde_json::to_vec(value).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(".fns-tmp-{}", uuid::Uuid::new_v4());
    let tmp_path = parent.join(&tmp_name);

    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        file.write_all(&json)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        file.sync_all()
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    }

    fs::rename(&tmp_path, path).map_err(|_| {
        // Cleanup our temp file on failure.
        let _ = fs::remove_file(&tmp_path);
        PlatformError::new(PlatformErrorCode::Io)
    })?;

    // Sync parent directory where supported.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn atomic_write_private_json<T: serde::Serialize>(
    _path: &Path,
    _value: &T,
) -> Result<(), PlatformError> {
    Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
}

#[cfg(target_os = "linux")]
fn read_boot_id() -> Result<String, PlatformError> {
    use std::io::Read;
    let mut buf = String::new();
    std::fs::File::open("/proc/sys/kernel/random/boot_id")
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?
        .read_to_string(&mut buf)
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    Ok(buf.trim().to_string())
}

#[cfg(target_os = "linux")]
fn read_start_ticks(pid: u32) -> Result<u64, PlatformError> {
    use std::io::Read;
    let stat_path = format!("/proc/{}/stat", pid);
    let mut content = String::new();
    std::fs::File::open(&stat_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?
        .read_to_string(&mut content)
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

    // The comm field may contain spaces/parentheses; locate the last ')' first.
    let close_paren = content
        .rfind(')')
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::Io))?;
    let after = &content[close_paren + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // Field 2 (in the after-paren slice) is state; field 20 (index 19) is starttime.
    // In the full stat, field 20 is start_ticks. After removing pid and comm,
    // it's at index 19 (0-based) in the whitespace-split remainder after ')'.
    if fields.len() < 20 {
        return Err(PlatformError::new(PlatformErrorCode::Io));
    }
    fields[19]
        .parse::<u64>()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))
}

#[cfg(target_os = "linux")]
fn is_live(record: &ProcessLockRecord, current_boot_id: &str) -> bool {
    if record.boot_id != current_boot_id {
        return false;
    }
    // Check if the PID is still alive with matching start_ticks.
    let stat_path = format!("/proc/{}/stat", record.pid);
    let content = match std::fs::read_to_string(&stat_path) {
        Ok(c) => c,
        Err(_) => return false, // PID doesn't exist
    };
    let close_paren = match content.rfind(')') {
        Some(i) => i,
        None => return false,
    };
    let after = &content[close_paren + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    if fields.len() < 20 {
        return false;
    }
    let current_ticks = match fields[19].parse::<u64>() {
        Ok(t) => t,
        Err(_) => return false,
    };
    current_ticks == record.start_ticks
}
