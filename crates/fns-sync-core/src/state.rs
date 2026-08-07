use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fns_protocol::{ClientId, WorkspaceId};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    types::ValueRef,
};

use crate::error::{SyncError, corrupt, storage_error};
use crate::ids::now_ms;
use crate::model::WorkspaceCursor;

const MIGRATION_0001: &str = include_str!("../migrations/0001_client_state.sql");
const TABLES: [&str; 12] = [
    "workspace_cursor",
    "path_states",
    "outbox",
    "local_intents",
    "stream_state",
    "stream_entries",
    "stream_revision_items",
    "stream_conflicts",
    "apply_journal",
    "applied_operations",
    "conflicts",
    "hash_cache",
];

pub struct SqliteState {
    pub(crate) conn: Connection,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) client_id: ClientId,
}

impl std::fmt::Debug for SqliteState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteState")
            .field("workspace_id", &self.workspace_id)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl SqliteState {
    pub fn open<P: AsRef<Path>>(
        path: P,
        workspace_id: WorkspaceId,
        client_id: ClientId,
    ) -> Result<Self, SyncError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
        let mut conn = Connection::open_with_flags(path.as_ref(), flags).map_err(storage_error)?;
        conn.busy_timeout(Duration::from_millis(5_000))
            .map_err(storage_error)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1_000_i64)
            .map_err(storage_error)?;

        let version = user_version_of(&conn)?;
        match version {
            0 => {
                let transaction = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0001)
                    .map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
            }
            1 => {}
            _ => return Err(corrupt("schema", "user_version")),
        }

        ensure_identity(&conn, workspace_id, client_id)?;
        Ok(Self {
            conn,
            workspace_id,
            client_id,
        })
    }

    pub fn open_in_memory(
        workspace_id: WorkspaceId,
        client_id: ClientId,
    ) -> Result<Self, SyncError> {
        Self::open(":memory:", workspace_id, client_id)
    }

    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    pub const fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn user_version(&self) -> Result<i64, SyncError> {
        user_version_of(&self.conn)
    }

    pub fn pragma(&self, name: &str) -> Result<String, SyncError> {
        if !matches!(
            name,
            "journal_mode"
                | "synchronous"
                | "foreign_keys"
                | "busy_timeout"
                | "wal_autocheckpoint"
                | "user_version"
        ) {
            return Err(SyncError::InvalidConfiguration {
                reason: "unsupported_pragma",
            });
        }
        self.conn
            .pragma_query_value(None, name, |row| {
                let value = row.get_ref(0)?;
                Ok(match value {
                    ValueRef::Integer(value) => value.to_string(),
                    ValueRef::Real(value) => value.to_string(),
                    ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
                    ValueRef::Null => String::new(),
                    ValueRef::Blob(value) => String::from_utf8_lossy(value).into_owned(),
                })
            })
            .map_err(storage_error)
    }

    pub fn cursor(&self) -> Result<WorkspaceCursor, SyncError> {
        let raw = self
            .conn
            .query_row(
                "SELECT workspace_id, client_id, last_ack_revision, last_applied_revision, pending_ack_revision FROM workspace_cursor WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => corrupt("workspace_cursor", "workspace_id"),
                _ => storage_error(error),
            })?;
        let workspace_id = fns_protocol::WorkspaceId::parse(&raw.0)
            .map_err(|_| corrupt("workspace_cursor", "workspace_id"))?;
        let client_id = fns_protocol::ClientId::parse(&raw.1)
            .map_err(|_| corrupt("workspace_cursor", "client_id"))?;
        let last_ack_revision = fns_protocol::WorkspaceRevision::parse(&raw.2)
            .map_err(|_| corrupt("workspace_cursor", "last_ack_revision"))?;
        let last_applied_revision = fns_protocol::WorkspaceRevision::parse(&raw.3)
            .map_err(|_| corrupt("workspace_cursor", "last_applied_revision"))?;
        let pending_ack_revision = raw
            .4
            .as_deref()
            .map(fns_protocol::WorkspaceRevision::parse)
            .transpose()
            .map_err(|_| corrupt("workspace_cursor", "pending_ack_revision"))?;
        Ok(WorkspaceCursor {
            workspace_id,
            client_id,
            last_ack_revision,
            last_applied_revision,
            pending_ack_revision,
        })
    }

    pub fn set_pending_ack(
        &mut self,
        revision: fns_protocol::WorkspaceRevision,
    ) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE workspace_cursor SET pending_ack_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
                params![revision.to_string(), now_ms(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn clear_pending_ack(&mut self) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE workspace_cursor SET pending_ack_revision = NULL, updated_at_ms = ?1 WHERE workspace_id = ?2",
                params![now_ms(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_last_ack_revision(
        &mut self,
        revision: fns_protocol::WorkspaceRevision,
    ) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE workspace_cursor SET last_ack_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
                params![revision.to_string(), now_ms(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_last_applied_revision(
        &mut self,
        revision: fns_protocol::WorkspaceRevision,
    ) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE workspace_cursor SET last_applied_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
                params![revision.to_string(), now_ms(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn transaction<F, T>(&mut self, operation: F) -> Result<T, SyncError>
    where
        F: FnOnce(&mut StateTransaction<'_>) -> Result<T, SyncError>,
    {
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let mut transaction = StateTransaction {
            transaction,
            workspace_id: self.workspace_id,
        };
        let result = operation(&mut transaction);
        match result {
            Ok(value) => {
                transaction.transaction.commit().map_err(storage_error)?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    pub fn with_transaction<F, T>(&mut self, operation: F) -> Result<T, SyncError>
    where
        F: FnOnce(&mut StateTransaction<'_>) -> Result<T, SyncError>,
    {
        self.transaction(operation)
    }

    pub fn row_counts(&self) -> Result<BTreeMap<String, usize>, SyncError> {
        let mut counts = BTreeMap::new();
        for table in TABLES {
            let query = format!("SELECT COUNT(*) FROM {table}");
            let count: i64 = self
                .conn
                .query_row(&query, [], |row| row.get(0))
                .map_err(storage_error)?;
            let count = usize::try_from(count).map_err(|_| corrupt(table, "row_count"))?;
            counts.insert(table.to_owned(), count);
        }
        Ok(counts)
    }
}

pub struct StateTransaction<'a> {
    pub(crate) transaction: Transaction<'a>,
    pub(crate) workspace_id: WorkspaceId,
}

impl StateTransaction<'_> {
    pub fn execute<P>(&mut self, sql: &str, parameters: P) -> Result<usize, SyncError>
    where
        P: rusqlite::Params,
    {
        self.transaction
            .execute(sql, parameters)
            .map_err(storage_error)
    }

    pub fn put_path_state(
        &mut self,
        state: &fns_protocol::WorkspacePathState,
    ) -> Result<(), SyncError> {
        crate::store::put_path_state_tx(&self.transaction, self.workspace_id(), state)
    }

    pub fn put_local_intent(
        &mut self,
        path: &str,
        intent_json: &[u8],
        updated_at_ms: i64,
    ) -> Result<(), SyncError> {
        let path = fns_protocol::WorkspacePath::parse(path).map_err(|_| {
            SyncError::InvalidConfiguration {
                reason: "invalid_path",
            }
        })?;
        crate::store::put_local_intent_tx(
            &self.transaction,
            self.workspace_id(),
            &path,
            intent_json,
            updated_at_ms,
        )
    }

    pub(crate) fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
}

fn user_version_of(connection: &Connection) -> Result<i64, SyncError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(storage_error)
}

fn ensure_identity(
    connection: &Connection,
    workspace_id: WorkspaceId,
    client_id: ClientId,
) -> Result<(), SyncError> {
    let workspace = workspace_id.to_string();
    let client = client_id.to_string();
    let timestamp = now_ms();
    let existing_identity = connection
        .query_row(
            "SELECT workspace_id, client_id FROM workspace_cursor LIMIT 1",
            [],
            |row| {
                let stored_workspace: String = row.get(0)?;
                let stored_client: String = row.get(1)?;
                Ok((stored_workspace, stored_client))
            },
        )
        .optional()
        .map_err(storage_error)?;
    if let Some((stored_workspace, stored_client)) = existing_identity
        && (stored_workspace != workspace || stored_client != client)
    {
        return Err(SyncError::InvalidConfiguration {
            reason: "identity_mismatch",
        });
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO workspace_cursor (workspace_id, client_id, last_ack_revision, last_applied_revision, pending_ack_revision, created_at_ms, updated_at_ms) VALUES (?1, ?2, '0', '0', NULL, ?3, ?3)",
            params![workspace, client, timestamp],
        )
        .map_err(storage_error)?;
    let stored_client: String = connection
        .query_row(
            "SELECT client_id FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => corrupt("workspace_cursor", "workspace_id"),
            _ => storage_error(error),
        })?;
    if stored_client != client_id.to_string() {
        return Err(SyncError::InvalidConfiguration {
            reason: "identity_mismatch",
        });
    }
    Ok(())
}
