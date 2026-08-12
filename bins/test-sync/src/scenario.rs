use crate::{io_error, HarnessError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    A,
    B,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointExpectation {
    Converged,
    Conflict { path: String, kind: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScenarioAction {
    CreateDirectory {
        endpoint: Endpoint,
        path: String,
    },
    CreateText {
        endpoint: Endpoint,
        path: String,
    },
    CreateInvalidUtf8 {
        endpoint: Endpoint,
        path: String,
    },
    CreateEmpty {
        endpoint: Endpoint,
        path: String,
    },
    CreateLargeStream {
        endpoint: Endpoint,
        path: String,
        size: u64,
    },
    Modify {
        endpoint: Endpoint,
        path: String,
    },
    SetMode {
        endpoint: Endpoint,
        path: String,
        mode: u32,
    },
    Delete {
        endpoint: Endpoint,
        path: String,
    },
    Rename {
        endpoint: Endpoint,
        from: String,
        to: String,
    },
    ConcurrentConflict {
        path: String,
    },
    ReconnectHook,
    RestartAgent {
        endpoint: Endpoint,
    },
    RestartAppHook,
    Checkpoint {
        name: String,
        expectation: CheckpointExpectation,
    },
}

pub fn deterministic_plan(large_file_bytes: u64) -> Result<Vec<ScenarioAction>> {
    if large_file_bytes < 1024 * 1024 {
        return Err(HarnessError::InvalidConfiguration(
            "large streamed fixture must be at least 1 MiB",
        ));
    }
    Ok(vec![
        ScenarioAction::CreateDirectory {
            endpoint: Endpoint::A,
            path: "directory".into(),
        },
        ScenarioAction::CreateText {
            endpoint: Endpoint::A,
            path: "text/plain.txt".into(),
        },
        ScenarioAction::SetMode {
            endpoint: Endpoint::A,
            path: "text/plain.txt".into(),
            mode: 0o755,
        },
        ScenarioAction::CreateInvalidUtf8 {
            endpoint: Endpoint::A,
            path: "binary/invalid-utf8.bin".into(),
        },
        ScenarioAction::CreateEmpty {
            endpoint: Endpoint::A,
            path: "empty/zero.dat".into(),
        },
        ScenarioAction::CreateLargeStream {
            endpoint: Endpoint::A,
            path: "large/streamed.bin".into(),
            size: large_file_bytes,
        },
        ScenarioAction::CreateText {
            endpoint: Endpoint::A,
            path: "nested/one/two/value.txt".into(),
        },
        ScenarioAction::Checkpoint {
            name: "seeded".into(),
            expectation: CheckpointExpectation::Converged,
        },
        ScenarioAction::Modify {
            endpoint: Endpoint::A,
            path: "text/plain.txt".into(),
        },
        ScenarioAction::SetMode {
            endpoint: Endpoint::A,
            path: "text/plain.txt".into(),
            mode: 0o644,
        },
        ScenarioAction::Delete {
            endpoint: Endpoint::A,
            path: "empty/zero.dat".into(),
        },
        ScenarioAction::Rename {
            endpoint: Endpoint::A,
            from: "nested/one/two/value.txt".into(),
            to: "nested/one/two/renamed.txt".into(),
        },
        ScenarioAction::Checkpoint {
            name: "modified-deleted-renamed".into(),
            expectation: CheckpointExpectation::Converged,
        },
        ScenarioAction::ReconnectHook,
        ScenarioAction::Checkpoint {
            name: "reconnected".into(),
            expectation: CheckpointExpectation::Converged,
        },
        ScenarioAction::ConcurrentConflict {
            path: "conflict/concurrent.txt".into(),
        },
        ScenarioAction::Checkpoint {
            name: "conflict".into(),
            expectation: CheckpointExpectation::Conflict {
                path: "conflict/concurrent.txt".into(),
                kind: "content".into(),
            },
        },
        ScenarioAction::RestartAgent {
            endpoint: Endpoint::A,
        },
        ScenarioAction::Checkpoint {
            name: "agent-restarted".into(),
            expectation: CheckpointExpectation::Conflict {
                path: "conflict/concurrent.txt".into(),
                kind: "content".into(),
            },
        },
        ScenarioAction::RestartAppHook,
        ScenarioAction::Checkpoint {
            name: "app-restarted".into(),
            expectation: CheckpointExpectation::Conflict {
                path: "conflict/concurrent.txt".into(),
                kind: "content".into(),
            },
        },
    ])
}

pub fn apply_action(root_a: &Path, root_b: &Path, action: &ScenarioAction) -> Result<()> {
    match action {
        ScenarioAction::CreateDirectory { endpoint, path } => {
            let path = checked_path(root(endpoint, root_a, root_b), path)?;
            fs::create_dir_all(&path).map_err(|error| io_error(path, error))
        }
        ScenarioAction::CreateText { endpoint, path } => write_fixture(
            root(endpoint, root_a, root_b),
            path,
            b"fns deterministic text fixture\n",
        ),
        ScenarioAction::CreateInvalidUtf8 { endpoint, path } => write_fixture(
            root(endpoint, root_a, root_b),
            path,
            &[0xff, 0xfe, 0x00, 0x80, b'F', b'N', b'S'],
        ),
        ScenarioAction::CreateEmpty { endpoint, path } => {
            write_fixture(root(endpoint, root_a, root_b), path, &[])
        }
        ScenarioAction::CreateLargeStream {
            endpoint,
            path,
            size,
        } => write_large_fixture(root(endpoint, root_a, root_b), path, *size),
        ScenarioAction::Modify { endpoint, path } => write_fixture(
            root(endpoint, root_a, root_b),
            path,
            b"fns deterministic modified fixture\n",
        ),
        ScenarioAction::SetMode {
            endpoint,
            path,
            mode,
        } => set_mode(root(endpoint, root_a, root_b), path, *mode),
        ScenarioAction::Delete { endpoint, path } => {
            let path = checked_path(root(endpoint, root_a, root_b), path)?;
            fs::remove_file(&path).map_err(|error| io_error(path, error))
        }
        ScenarioAction::Rename { endpoint, from, to } => {
            let root = root(endpoint, root_a, root_b);
            let from = checked_path(root, from)?;
            let to = checked_path(root, to)?;
            create_parent(&to)?;
            fs::rename(&from, &to).map_err(|error| io_error(from, error))
        }
        ScenarioAction::ConcurrentConflict { .. }
        | ScenarioAction::ReconnectHook
        | ScenarioAction::RestartAgent { .. }
        | ScenarioAction::RestartAppHook
        | ScenarioAction::Checkpoint { .. } => Ok(()),
    }
}

pub fn write_conflict_side(root: &Path, path: &str, endpoint: Endpoint) -> Result<()> {
    let contents: &[u8] = match endpoint {
        Endpoint::A => b"concurrent endpoint A\n",
        Endpoint::B => b"concurrent endpoint B\n",
    };
    write_fixture(root, path, contents)
}

fn root<'a>(endpoint: &Endpoint, root_a: &'a Path, root_b: &'a Path) -> &'a Path {
    match endpoint {
        Endpoint::A => root_a,
        Endpoint::B => root_b,
    }
}

fn checked_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(HarnessError::InvalidConfiguration(
            "scenario path is not a safe relative path",
        ));
    }
    Ok(root.join(relative))
}

fn create_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or(HarnessError::InvalidConfiguration(
        "fixture path has no parent",
    ))?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))
}

fn write_fixture(root: &Path, relative: &str, contents: &[u8]) -> Result<()> {
    let path = checked_path(root, relative)?;
    create_parent(&path)?;
    fs::write(&path, contents).map_err(|error| io_error(path, error))
}

fn write_large_fixture(root: &Path, relative: &str, size: u64) -> Result<()> {
    let path = checked_path(root, relative)?;
    create_parent(&path)?;
    let mut file = fs::File::create(&path).map_err(|error| io_error(&path, error))?;
    let mut generator = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"fns-e2e-large-stream-v1");
        hasher.finalize_xof()
    };
    let mut remaining = size;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| HarnessError::InvalidConfiguration("large fixture size overflow"))?;
        generator.fill(&mut buffer[..count]);
        file.write_all(&buffer[..count])
            .map_err(|error| io_error(&path, error))?;
        remaining -= count as u64;
    }
    file.sync_all().map_err(|error| io_error(path, error))
}

#[cfg(unix)]
fn set_mode(root: &Path, relative: &str, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if mode & !0o777 != 0 {
        return Err(HarnessError::InvalidConfiguration(
            "scenario mode is not a portable Unix permission mode",
        ));
    }
    let path = checked_path(root, relative)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(mode))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_mode(_root: &Path, _relative: &str, _mode: u32) -> Result<()> {
    Err(HarnessError::InvalidConfiguration(
        "mode propagation scenarios require Unix",
    ))
}
