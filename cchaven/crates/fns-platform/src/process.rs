//! Stale-safe process locking and atomic private JSON writes.
//!
//! Linux locks use `/proc/sys/kernel/random/boot_id` plus field 22 of
//! `/proc/<pid>/stat` to detect live vs stale lock holders. Atomic private JSON
//! writes are supported on Unix; process locks remain Linux-only.

use crate::error::{PlatformError, PlatformErrorCode};

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A process-scoped advisory lease for one logical state-directory path.
///
/// On Unix the authoritative lock is attached to an owner-only sibling
/// directory. It therefore survives replacement of the state-directory leaf or
/// its visible `agent.lease` PID record. The sibling anchor itself remains part
/// of the trusted owner-only filesystem area.
pub struct StateDirLease {
    #[cfg(unix)]
    anchor: File,
    #[cfg(unix)]
    _state_dir: File,
    file: File,
}

impl StateDirLease {
    pub fn probe(state_dir: &Path) -> Result<bool, PlatformError> {
        #[cfg(unix)]
        {
            probe_state_dir_lease_unix(state_dir)
        }

        #[cfg(not(unix))]
        {
            probe_visible_lease_file(state_dir)
        }
    }

    pub fn acquire(state_dir: &Path) -> Result<Self, PlatformError> {
        #[cfg(unix)]
        {
            acquire_state_dir_lease_unix(state_dir)
        }

        #[cfg(not(unix))]
        {
            acquire_state_dir_lease_fallback(state_dir)
        }
    }
}

#[cfg(unix)]
fn probe_state_dir_lease_unix(state_dir: &Path) -> Result<bool, PlatformError> {
    let anchor_path = state_dir_lease_anchor(state_dir)?;
    let anchor = match open_existing_directory_no_follow(&anchor_path)? {
        Some(anchor) => anchor,
        None => {
            validate_state_and_visible_lease_for_probe(state_dir)?;
            return probe_visible_lease_file(state_dir);
        }
    };
    validate_private_mode(&anchor, 0o700)?;

    match anchor.try_lock() {
        Ok(()) => {
            validate_open_path_identity(&anchor_path, &anchor, ExpectedFileType::Directory)?;
            validate_state_and_visible_lease_for_probe(state_dir)?;
            let _ = anchor.unlock();
            Ok(false)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(_)) => Err(PlatformError::new(PlatformErrorCode::Io)),
    }
}

#[cfg(unix)]
fn acquire_state_dir_lease_unix(state_dir: &Path) -> Result<StateDirLease, PlatformError> {
    std::fs::create_dir_all(state_dir).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    let state_dir_file = open_private_directory(state_dir, false)?;
    let anchor_path = state_dir_lease_anchor(state_dir)?;
    let anchor = open_private_directory(&anchor_path, true)?;

    anchor.try_lock().map_err(map_lock_error)?;
    validate_open_path_identity(&anchor_path, &anchor, ExpectedFileType::Directory)?;
    validate_open_path_identity(state_dir, &state_dir_file, ExpectedFileType::Directory)?;

    let mut file = open_visible_lease_file(state_dir, true)?
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::Io))?;
    file.try_lock().map_err(map_lock_error)?;
    validate_open_path_identity(
        &state_dir.join("agent.lease"),
        &file,
        ExpectedFileType::Regular,
    )?;
    validate_open_path_identity(state_dir, &state_dir_file, ExpectedFileType::Directory)?;

    write_lease_pid(&mut file)?;
    validate_open_path_identity(
        &state_dir.join("agent.lease"),
        &file,
        ExpectedFileType::Regular,
    )?;

    Ok(StateDirLease {
        anchor,
        _state_dir: state_dir_file,
        file,
    })
}

#[cfg(not(unix))]
fn acquire_state_dir_lease_fallback(state_dir: &Path) -> Result<StateDirLease, PlatformError> {
    std::fs::create_dir_all(state_dir).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    let metadata = std::fs::symlink_metadata(state_dir)
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }

    let mut file = open_visible_lease_file(state_dir, true)?
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::Io))?;
    file.try_lock().map_err(map_lock_error)?;
    write_lease_pid(&mut file)?;
    Ok(StateDirLease { file })
}

fn probe_visible_lease_file(state_dir: &Path) -> Result<bool, PlatformError> {
    let Some(file) = open_visible_lease_file(state_dir, false)? else {
        return Ok(false);
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            Ok(false)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(_)) => Err(PlatformError::new(PlatformErrorCode::Io)),
    }
}

fn open_visible_lease_file(state_dir: &Path, create: bool) -> Result<Option<File>, PlatformError> {
    let path = state_dir.join("agent.lease");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(None);
            }
        }
        Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        set_no_follow(&mut options);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) if path_is_symlink(&path) => {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
    };
    let metadata = file
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    if !metadata.is_file() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if create {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        }
        validate_open_path_identity(&path, &file, ExpectedFileType::Regular)?;
        validate_private_mode(&file, 0o600)?;
    }

    Ok(Some(file))
}

fn write_lease_pid(file: &mut File) -> Result<(), PlatformError> {
    file.set_len(0)
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    writeln!(file, "{}", std::process::id())
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    file.sync_all()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))
}

fn map_lock_error(error: std::fs::TryLockError) -> PlatformError {
    match error {
        std::fs::TryLockError::WouldBlock => PlatformError::new(PlatformErrorCode::AlreadyRunning),
        std::fs::TryLockError::Error(_) => PlatformError::new(PlatformErrorCode::Io),
    }
}

fn path_is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn validate_state_and_visible_lease_for_probe(state_dir: &Path) -> Result<(), PlatformError> {
    match std::fs::symlink_metadata(state_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        Ok(metadata) => {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
    }

    let lease_path = state_dir.join("agent.lease");
    match std::fs::symlink_metadata(lease_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(PlatformError::new(PlatformErrorCode::InvalidFileType))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlatformError::new(PlatformErrorCode::Io)),
    }
}

#[cfg(unix)]
fn state_dir_lease_anchor(state_dir: &Path) -> Result<PathBuf, PlatformError> {
    let file_name = state_dir
        .file_name()
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidFileType))?;
    let parent = state_dir
        .parent()
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidFileType))?;
    let mut anchor_name = std::ffi::OsString::from(".");
    anchor_name.push(file_name);
    anchor_name.push(".fns-agent-lease");
    Ok(parent.join(anchor_name))
}

#[cfg(unix)]
fn open_private_directory(path: &Path, create: bool) -> Result<File, PlatformError> {
    if create {
        match std::fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
        }
    }
    open_existing_directory_no_follow(path)?
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::Io))
        .and_then(|file| {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
            validate_open_path_identity(path, &file, ExpectedFileType::Directory)?;
            Ok(file)
        })
}

#[cfg(unix)]
fn open_existing_directory_no_follow(path: &Path) -> Result<Option<File>, PlatformError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(_) if path_is_symlink(path) => {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        Err(_) => return Err(PlatformError::new(PlatformErrorCode::Io)),
    };
    validate_open_path_identity(path, &file, ExpectedFileType::Directory)?;
    Ok(Some(file))
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ExpectedFileType {
    Directory,
    Regular,
}

#[cfg(unix)]
fn validate_open_path_identity(
    path: &Path,
    file: &File,
    expected: ExpectedFileType,
) -> Result<(), PlatformError> {
    use std::os::unix::fs::MetadataExt;

    let opened = file
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    let current =
        std::fs::symlink_metadata(path).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    let expected_matches = match expected {
        ExpectedFileType::Directory => opened.is_dir() && current.is_dir(),
        ExpectedFileType::Regular => opened.is_file() && current.is_file(),
    };
    if opened.file_type().is_symlink() || current.file_type().is_symlink() || !expected_matches {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }
    if opened.dev() != current.dev() || opened.ino() != current.ino() {
        return Err(PlatformError::new(PlatformErrorCode::Io));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_mode(file: &File, expected: u32) -> Result<(), PlatformError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = file
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?
        .permissions()
        .mode()
        & 0o777;
    if mode != expected {
        return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
    }
    Ok(())
}

#[cfg(unix)]
fn set_no_follow(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(flag) = no_follow_flag() {
        options.custom_flags(flag);
    }
}

#[cfg(unix)]
#[allow(unreachable_code)]
const fn no_follow_flag() -> Option<i32> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "solaris",
        target_os = "illumos"
    ))]
    {
        return Some(0x0002_0000);
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        return Some(0x0000_0100);
    }
    None
}

impl fmt::Debug for StateDirLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StateDirLease")
    }
}

impl Drop for StateDirLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        #[cfg(unix)]
        let _ = self.anchor.unlock();
    }
}

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

        let boot_id = read_boot_id()?;
        let pid = std::process::id();
        let start_ticks = read_start_ticks(pid)?;
        let nonce = uuid::Uuid::new_v4();

        // Check for existing lock.
        if let Some(record) = Self::probe_linux(path)? {
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
        use std::io::Read;
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
            if let Ok(Some(record)) = Self::probe_linux(&self.path)
                && record.nonce == self.nonce
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

/// Write JSON atomically to a private (0600) file via a temp sibling + rename.
#[cfg(target_family = "unix")]
pub fn atomic_write_private_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), PlatformError> {
    use std::fs;
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

    let dir = fs::File::open(parent).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    dir.sync_all()
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

    Ok(())
}

#[cfg(not(target_family = "unix"))]
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
