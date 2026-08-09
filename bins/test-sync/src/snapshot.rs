use crate::manifest::{build_manifest, Manifest};
use crate::{io_error, HarnessError, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointSample {
    pub manifest_a: Manifest,
    pub manifest_b: Manifest,
    pub client_a: ClientSnapshot,
    pub client_b: ClientSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientSnapshot {
    pub cursor: Option<CursorSnapshot>,
    pub outbox: BTreeMap<String, u64>,
    pub intents: u64,
    pub stream: StreamSnapshot,
    pub journal: BTreeMap<String, u64>,
    pub conflicts: Vec<ConflictSnapshot>,
    pub sqlite_quick_check: String,
    pub sqlite_journal_mode: String,
    pub runtime: RuntimeSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CursorSnapshot {
    pub client_id: String,
    pub last_ack_revision: String,
    pub last_applied_revision: String,
    pub pending_ack_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ConflictSnapshot {
    pub conflict_id: String,
    pub conflict_revision: String,
    pub path: String,
    pub kind: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamSnapshot {
    pub active_streams: u64,
    pub entries: BTreeMap<String, u64>,
    pub revision_items: BTreeMap<String, u64>,
    pub conflicts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSnapshot {
    pub schema_version: String,
    pub workspace_id: String,
    pub running: bool,
    pub phase: String,
    pub pid: Option<u32>,
    pub connected: bool,
    pub last_ack_revision: String,
    pub pending_commands: u64,
    pub queued_watcher_batches: usize,
    pub active_transfers: usize,
    pub reconnect_attempt: u32,
    pub last_error_code: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct SnapshotExpectation<'a> {
    pub workspace_id: &'a str,
    pub client_id_a: &'a str,
    pub client_id_b: &'a str,
    pub pids: (i32, i32),
}

impl CheckpointSample {
    /// Return the durable checkpoint projection used for stability equality.
    ///
    /// `updated_at_ms` is the live Agent heartbeat rather than sync state. It
    /// remains present in evidence and must be positive for convergence, but
    /// cannot participate in the three-identical-samples comparison.
    pub fn stability_projection(&self) -> Self {
        let mut projected = self.clone();
        projected.client_a.runtime.updated_at_ms = 1;
        projected.client_b.runtime.updated_at_ms = 1;
        projected
    }

    pub fn global_revision<'a>(&'a self, expected: &SnapshotExpectation<'_>) -> Option<&'a str> {
        let left = self.client_a.consistent_revision(
            expected.workspace_id,
            expected.client_id_a,
            expected.pids.0,
        )?;
        let right = self.client_b.consistent_revision(
            expected.workspace_id,
            expected.client_id_b,
            expected.pids.1,
        )?;
        (left == right).then_some(left)
    }

    pub fn converged(&self, expected: &SnapshotExpectation<'_>) -> bool {
        self.manifest_a.sync_equivalent(&self.manifest_b)
            && self.client_a.quiescent()
            && self.client_b.quiescent()
            && self.global_revision(expected).is_some()
    }

    pub fn conflict_stable(
        &self,
        expected: &SnapshotExpectation<'_>,
        expected_path: &str,
        expected_kind: &str,
    ) -> bool {
        self.client_a.conflict_settled()
            && self.client_b.conflict_settled()
            && self.global_revision(expected).is_some()
            && !self.client_a.conflicts.is_empty()
            && self.client_a.conflicts == self.client_b.conflicts
            && self.client_a.conflicts.iter().all(|conflict| {
                conflict.path == expected_path
                    && conflict.kind == expected_kind
                    && conflict.status == "manual"
            })
    }
}

impl ClientSnapshot {
    fn quiescent(&self) -> bool {
        self.no_active_work() && self.conflicts.is_empty()
    }

    fn no_active_work(&self) -> bool {
        self.active_io_settled() && self.outbox.values().sum::<u64>() == 0 && self.intents == 0
    }

    fn conflict_settled(&self) -> bool {
        self.active_io_settled()
            && self.intents == 0
            && self
                .outbox
                .iter()
                .filter(|(stage, _)| stage.as_str() != "blocked_conflict")
                .all(|(_, count)| *count == 0)
    }

    fn active_io_settled(&self) -> bool {
        self.runtime.schema_version == "fns-agent-status/1"
            && self.runtime.running
            && self.runtime.phase == "online"
            && self.runtime.connected
            && self.runtime.last_error_code.is_none()
            && self.runtime.updated_at_ms > 0
            && self.runtime.pending_commands == 0
            && self.runtime.queued_watcher_batches == 0
            && self.runtime.active_transfers == 0
            && self
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.pending_ack_revision.is_none())
            && self.stream.active_streams == 0
            && self.stream.entries.values().sum::<u64>() == 0
            && self.stream.revision_items.values().sum::<u64>() == 0
            && self.stream.conflicts.values().sum::<u64>() == 0
            && self.journal.values().sum::<u64>() == 0
            && self.sqlite_quick_check == "ok"
            && self.sqlite_journal_mode == "wal"
    }

    fn consistent_revision(
        &self,
        expected_workspace_id: &str,
        expected_client_id: &str,
        expected_pid: i32,
    ) -> Option<&str> {
        let cursor = self.cursor.as_ref()?;
        let runtime_pid = self.runtime.pid.and_then(|pid| i32::try_from(pid).ok());
        (cursor.client_id == expected_client_id
            && cursor.last_ack_revision == cursor.last_applied_revision
            && cursor.pending_ack_revision.is_none()
            && self.runtime.workspace_id == expected_workspace_id
            && self.runtime.last_ack_revision == cursor.last_ack_revision
            && runtime_pid == Some(expected_pid))
        .then_some(cursor.last_ack_revision.as_str())
    }
}

pub fn capture(
    root_a: &Path,
    root_b: &Path,
    state_a: &Path,
    state_b: &Path,
    workspace_id: &str,
) -> Result<CheckpointSample> {
    Ok(CheckpointSample {
        manifest_a: build_manifest(root_a)?,
        manifest_b: build_manifest(root_b)?,
        client_a: inspect_client(state_a, workspace_id)?,
        client_b: inspect_client(state_b, workspace_id)?,
    })
}

pub fn inspect_client(state_dir: &Path, workspace_id: &str) -> Result<ClientSnapshot> {
    let database = state_dir.join("state.sqlite");
    let mut connection = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.pragma_update(None, "query_only", true)?;
    let transaction = connection.transaction()?;
    let cursor = transaction
        .query_row(
            "SELECT client_id, last_ack_revision, last_applied_revision, pending_ack_revision \
             FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id],
            |row| {
                Ok(CursorSnapshot {
                    client_id: row.get(0)?,
                    last_ack_revision: row.get(1)?,
                    last_applied_revision: row.get(2)?,
                    pending_ack_revision: row.get(3)?,
                })
            },
        )
        .optional()?;
    let outbox = grouped_counts(
        &transaction,
        "SELECT stage, COUNT(*) FROM outbox WHERE workspace_id = ?1 GROUP BY stage ORDER BY stage",
        workspace_id,
    )?;
    let intents = scalar_count(
        &transaction,
        "SELECT COUNT(*) FROM local_intents WHERE workspace_id = ?1",
        workspace_id,
    )?;
    let stream = StreamSnapshot {
        active_streams: scalar_count(
            &transaction,
            "SELECT COUNT(*) FROM stream_state WHERE workspace_id = ?1",
            workspace_id,
        )?,
        entries: grouped_counts(
            &transaction,
            "SELECT status, COUNT(*) FROM stream_entries WHERE workspace_id = ?1 GROUP BY status ORDER BY status",
            workspace_id,
        )?,
        revision_items: grouped_counts(
            &transaction,
            "SELECT status, COUNT(*) FROM stream_revision_items WHERE workspace_id = ?1 GROUP BY status ORDER BY status",
            workspace_id,
        )?,
        conflicts: grouped_counts(
            &transaction,
            "SELECT status, COUNT(*) FROM stream_conflicts WHERE workspace_id = ?1 GROUP BY status ORDER BY status",
            workspace_id,
        )?,
    };
    let journal = grouped_counts(
        &transaction,
        "SELECT stage, COUNT(*) FROM apply_journal WHERE workspace_id = ?1 GROUP BY stage ORDER BY stage",
        workspace_id,
    )?;
    let conflicts = inspect_conflicts(&transaction, workspace_id)?;
    let quick_check = transaction.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    let journal_mode = transaction.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    transaction.commit()?;
    let runtime = inspect_runtime(state_dir)?;
    Ok(ClientSnapshot {
        cursor,
        outbox,
        intents,
        stream,
        journal,
        conflicts,
        sqlite_quick_check: quick_check,
        sqlite_journal_mode: journal_mode,
        runtime,
    })
}

fn grouped_counts(
    connection: &Connection,
    sql: &str,
    workspace_id: &str,
) -> Result<BTreeMap<String, u64>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params![workspace_id], |row| {
        let count: i64 = row.get(1)?;
        Ok((row.get::<_, String>(0)?, count))
    })?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (key, count) = row?;
        result.insert(
            key,
            u64::try_from(count)
                .map_err(|_| HarnessError::InvalidConfiguration("SQLite count was negative"))?,
        );
    }
    Ok(result)
}

fn scalar_count(connection: &Connection, sql: &str, workspace_id: &str) -> Result<u64> {
    let count: i64 = connection.query_row(sql, params![workspace_id], |row| row.get(0))?;
    u64::try_from(count)
        .map_err(|_| HarnessError::InvalidConfiguration("SQLite count was negative"))
}

fn inspect_conflicts(connection: &Connection, workspace_id: &str) -> Result<Vec<ConflictSnapshot>> {
    #[derive(Deserialize)]
    struct CreatedPath {
        path: String,
        kind: String,
    }

    let mut statement = connection.prepare(
        "SELECT conflict_id, conflict_revision, created_json, status FROM conflicts \
         WHERE workspace_id = ?1 ORDER BY conflict_id",
    )?;
    let rows = statement.query_map(params![workspace_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut conflicts = Vec::new();
    for row in rows {
        let (conflict_id, conflict_revision, created_json, status) = row?;
        let created: CreatedPath = serde_json::from_slice(&created_json)?;
        conflicts.push(ConflictSnapshot {
            conflict_id,
            conflict_revision,
            path: created.path,
            kind: created.kind,
            status,
        });
    }
    conflicts.sort();
    Ok(conflicts)
}

fn inspect_runtime(state_dir: &Path) -> Result<RuntimeSnapshot> {
    let path = state_dir.join("runtime-status.json");
    let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
    let status: fns_agent::AgentStatus = serde_json::from_slice(&bytes)?;
    Ok(RuntimeSnapshot {
        schema_version: status.schema_version,
        workspace_id: status.workspace_id.to_string(),
        running: status.running,
        phase: format!("{:?}", status.phase).to_ascii_lowercase(),
        pid: status.pid,
        connected: status.connected,
        last_ack_revision: status.last_ack_revision.to_string(),
        pending_commands: status.pending_commands,
        queued_watcher_batches: status.queued_watcher_batches,
        active_transfers: status.active_transfers,
        reconnect_attempt: status.reconnect_attempt,
        last_error_code: status
            .last_error_code
            .map(|code| format!("{code:?}").to_ascii_lowercase()),
        updated_at_ms: status.updated_at_ms,
    })
}
