//! Fail/timeout/cancel/crash cleanup for self-test runs.
//!
//! Tracks child PIDs, temporary workspaces, and state directories. Always removes
//! plaintext credential files from state dirs and tears down temp roots.

use crate::{io_error, HarnessError, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Names treated as plaintext credential material under state directories.
const PLAINTEXT_CREDENTIAL_NAMES: &[&str] = &[
    "token",
    "jwt",
    "auth-token",
    "access-token",
    "refresh-token",
    "ipc-token",
    "ipc-token-not-on-disk",
    "credentials",
    "password",
    "secret",
    "api-key",
    "private-key",
    "ssh-private-key",
];

/// Substrings that mark a file as credential material (case-insensitive).
const PLAINTEXT_CREDENTIAL_SUBSTRINGS: &[&str] = &[
    "token",
    "password",
    "secret",
    "credential",
    "privatekey",
    "private-key",
    "jwt",
];

/// Abstraction over process termination so unit tests can inject a mock killer.
pub trait ProcessKiller: Send {
    fn kill_pid(&self, pid: i32) -> Result<()>;
    fn is_alive(&self, pid: i32) -> bool;
}

/// Production killer: SIGTERM then SIGKILL if still alive.
#[derive(Debug, Default)]
pub struct OsProcessKiller;

impl ProcessKiller for OsProcessKiller {
    fn kill_pid(&self, pid: i32) -> Result<()> {
        if pid <= 0 {
            return Err(HarnessError::ProcessDetail(format!(
                "refusing to kill invalid pid {pid}"
            )));
        }
        #[cfg(unix)]
        {
            // SAFETY: pid is a positive process id we previously tracked.
            let term = unsafe { libc::kill(pid, libc::SIGTERM) };
            if term != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(());
                }
                return Err(HarnessError::ProcessDetail(format!(
                    "SIGTERM pid {pid}: {err}"
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            if self.is_alive(pid) {
                let kill = unsafe { libc::kill(pid, libc::SIGKILL) };
                if kill != 0 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::ESRCH) {
                        return Ok(());
                    }
                    return Err(HarnessError::ProcessDetail(format!(
                        "SIGKILL pid {pid}: {err}"
                    )));
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            Err(HarnessError::Process(
                "process cleanup requires Unix signals",
            ))
        }
    }

    fn is_alive(&self, pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        #[cfg(unix)]
        {
            // SAFETY: signal 0 is a liveness probe; no signal is delivered.
            let result = unsafe { libc::kill(pid, 0) };
            result == 0
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }
}

/// In-memory process table for tests.
#[derive(Clone, Debug, Default)]
pub struct MockProcessKiller {
    alive: Arc<Mutex<HashSet<i32>>>,
    kill_log: Arc<Mutex<Vec<i32>>>,
}

impl MockProcessKiller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, pid: i32) {
        self.alive.lock().expect("mock alive lock").insert(pid);
    }

    pub fn kill_log(&self) -> Vec<i32> {
        self.kill_log.lock().expect("mock kill log").clone()
    }

    pub fn alive_pids(&self) -> Vec<i32> {
        let set = self.alive.lock().expect("mock alive lock");
        let mut pids: Vec<_> = set.iter().copied().collect();
        pids.sort_unstable();
        pids
    }
}

impl ProcessKiller for MockProcessKiller {
    fn kill_pid(&self, pid: i32) -> Result<()> {
        self.kill_log.lock().expect("mock kill log").push(pid);
        self.alive.lock().expect("mock alive lock").remove(&pid);
        Ok(())
    }

    fn is_alive(&self, pid: i32) -> bool {
        self.alive.lock().expect("mock alive lock").contains(&pid)
    }
}

/// Report produced by a cleanup pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    pub killed_pids: Vec<i32>,
    pub removed_workspaces: Vec<PathBuf>,
    pub removed_credential_files: Vec<PathBuf>,
    pub removed_state_dirs: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// Tracks resources that must be reclaimed on fail / timeout / cancel / crash / Drop.
pub struct CleanupGuard {
    pids: Vec<i32>,
    workspace_dirs: Vec<PathBuf>,
    state_dirs: Vec<PathBuf>,
    killer: Box<dyn ProcessKiller>,
    cleaned: bool,
}

impl std::fmt::Debug for CleanupGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CleanupGuard")
            .field("pids", &self.pids)
            .field("workspace_dirs", &self.workspace_dirs)
            .field("state_dirs", &self.state_dirs)
            .field("cleaned", &self.cleaned)
            .finish_non_exhaustive()
    }
}

impl CleanupGuard {
    pub fn with_killer(killer: Box<dyn ProcessKiller>) -> Self {
        Self {
            pids: Vec::new(),
            workspace_dirs: Vec::new(),
            state_dirs: Vec::new(),
            killer,
            cleaned: false,
        }
    }

    pub fn new() -> Self {
        Self::with_killer(Box::new(OsProcessKiller))
    }

    pub fn track_pid(&mut self, pid: i32) {
        if pid > 0 && !self.pids.contains(&pid) {
            self.pids.push(pid);
        }
    }

    pub fn track_workspace(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.workspace_dirs.iter().any(|existing| existing == &path) {
            self.workspace_dirs.push(path);
        }
    }

    pub fn track_state_dir(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.state_dirs.iter().any(|existing| existing == &path) {
            self.state_dirs.push(path);
        }
    }

    pub fn tracked_pids(&self) -> &[i32] {
        &self.pids
    }

    /// Run cleanup once. Safe to call multiple times (idempotent).
    pub fn cleanup(&mut self) -> CleanupReport {
        if self.cleaned {
            return CleanupReport::default();
        }
        self.cleaned = true;
        let mut report = CleanupReport::default();

        for pid in self.pids.drain(..) {
            match self.killer.kill_pid(pid) {
                Ok(()) => report.killed_pids.push(pid),
                Err(error) => report.errors.push(format!("kill pid {pid}: {error}")),
            }
        }

        for state_dir in &self.state_dirs {
            match remove_plaintext_credentials(state_dir) {
                Ok(removed) => report.removed_credential_files.extend(removed),
                Err(error) => report
                    .errors
                    .push(format!("credential scrub {}: {error}", state_dir.display())),
            }
        }

        for workspace in self.workspace_dirs.drain(..) {
            match remove_path_if_exists(&workspace) {
                Ok(true) => report.removed_workspaces.push(workspace),
                Ok(false) => {}
                Err(error) => report
                    .errors
                    .push(format!("remove workspace {}: {error}", workspace.display())),
            }
        }

        for state_dir in self.state_dirs.drain(..) {
            match remove_path_if_exists(&state_dir) {
                Ok(true) => report.removed_state_dirs.push(state_dir),
                Ok(false) => {}
                Err(error) => report
                    .errors
                    .push(format!("remove state dir {}: {error}", state_dir.display())),
            }
        }

        report
    }

    /// Whether cleanup has already run.
    pub fn is_cleaned(&self) -> bool {
        self.cleaned
    }
}

impl Default for CleanupGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Remove plaintext credential files under a state directory (non-recursive names + one level).
pub fn remove_plaintext_credentials(state_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    if !state_dir.exists() {
        return Ok(removed);
    }
    remove_plaintext_in_dir(state_dir, &mut removed)?;
    Ok(removed)
}

fn remove_plaintext_in_dir(dir: &Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(dir, error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| io_error(dir, error))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| io_error(&path, error))?;
        if file_type.is_dir() {
            // Only scrub one level of nested credential dirs (e.g. secrets/).
            remove_plaintext_in_dir(&path, removed)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if is_plaintext_credential_name(&name) {
            fs::remove_file(&path).map_err(|error| io_error(&path, error))?;
            removed.push(path);
        }
    }
    Ok(())
}

fn is_plaintext_credential_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if PLAINTEXT_CREDENTIAL_NAMES
        .iter()
        .any(|candidate| lower == *candidate)
    {
        return true;
    }
    PLAINTEXT_CREDENTIAL_SUBSTRINGS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

fn remove_path_if_exists(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|error| io_error(path, error))?;
    } else {
        fs::remove_file(path).map_err(|error| io_error(path, error))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_killer_removes_tracked_pids() {
        let killer = MockProcessKiller::new();
        killer.register(101);
        killer.register(202);
        let mut guard = CleanupGuard::with_killer(Box::new(killer.clone()));
        guard.track_pid(101);
        guard.track_pid(202);
        let report = guard.cleanup();
        assert_eq!(report.killed_pids, vec![101, 202]);
        assert!(killer.alive_pids().is_empty());
        // Second cleanup is a no-op.
        let again = guard.cleanup();
        assert!(again.killed_pids.is_empty());
    }

    #[test]
    fn drop_runs_cleanup() {
        let killer = MockProcessKiller::new();
        killer.register(7);
        {
            let mut guard = CleanupGuard::with_killer(Box::new(killer.clone()));
            guard.track_pid(7);
        }
        assert!(killer.alive_pids().is_empty());
        assert_eq!(killer.kill_log(), vec![7]);
    }

    #[test]
    fn scrub_removes_plaintext_credentials_but_keeps_state_db() {
        let temporary = tempfile::tempdir().expect("temp");
        let state = temporary.path().join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(state.join("state.sqlite"), b"db").expect("db");
        fs::write(state.join("token"), b"super-secret-jwt").expect("token");
        fs::write(state.join("ipc-token"), b"ipc").expect("ipc");
        fs::write(state.join("runtime-status.json"), b"{}").expect("runtime");
        let nested = state.join("secrets");
        fs::create_dir_all(&nested).expect("secrets dir");
        fs::write(nested.join("password"), b"pw").expect("password");

        let removed = remove_plaintext_credentials(&state).expect("scrub");
        assert!(state.join("token").exists().not());
        assert!(state.join("ipc-token").exists().not());
        assert!(nested.join("password").exists().not());
        assert!(state.join("state.sqlite").exists());
        assert!(state.join("runtime-status.json").exists());
        assert!(removed.len() >= 3);
    }

    #[test]
    fn cleanup_removes_workspace_and_credentials() {
        let temporary = tempfile::tempdir().expect("temp");
        let workspace = temporary.path().join("ws");
        let state = temporary.path().join("state");
        fs::create_dir_all(&workspace).expect("ws");
        fs::create_dir_all(&state).expect("state");
        fs::write(workspace.join("file.txt"), b"x").expect("file");
        fs::write(state.join("auth-token"), b"secret").expect("cred");

        let killer = MockProcessKiller::new();
        killer.register(9);
        let mut guard = CleanupGuard::with_killer(Box::new(killer.clone()));
        guard.track_pid(9);
        guard.track_workspace(&workspace);
        guard.track_state_dir(&state);
        let report = guard.cleanup();

        assert!(!workspace.exists());
        assert!(!state.exists());
        assert!(killer.alive_pids().is_empty());
        assert!(report
            .removed_credential_files
            .iter()
            .any(|path| path.ends_with("auth-token")));
    }

    trait Not {
        fn not(self) -> bool;
    }
    impl Not for bool {
        fn not(self) -> bool {
            !self
        }
    }
}
