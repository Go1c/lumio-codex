use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fns_protocol::{
    ClientId, OperationId, WorkspaceConflictCreatedMessage, WorkspaceConflictResolvedMessage,
    WorkspaceConflictResolvedRequest, WorkspaceEventMessage, WorkspaceId, WorkspaceMutation,
    WorkspacePath, WorkspacePathState, WorkspaceRevision, WorkspaceSnapshotEntryMessage,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    types::ValueRef,
};

use crate::error::{SyncError, corrupt, storage_error};
use crate::ids::now_ms;
use crate::model::{
    AppliedOperationReceiptKind, ApplyCommitPlan, ApplyItemKind, ApplyJournalRecord,
    ApplyNamespace, ApplyStage, ConflictRecord, ConflictStatus, OutboxRecord, OutboxStage,
    RemoteApplyOperation, StreamConflictRecord, StreamConflictStatus, StreamEntryRecord,
    StreamItemStatus, StreamRevisionItemRecord, StreamStateRecord, WorkspaceCursor,
};

const MIGRATION_0001: &str = include_str!("../migrations/0001_client_state.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_applied_operation_receipts.sql");
const MIGRATION_0003: &str =
    include_str!("../migrations/0003_provisional_mutation_acceptances.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_apply_journal_v2.sql");
const TABLES: [&str; 13] = [
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
    "provisional_mutation_acceptances",
    "conflicts",
    "hash_cache",
];

pub struct SqliteState {
    pub(crate) conn: Connection,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) client_id: ClientId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistedIdentity {
    pub workspace_id: WorkspaceId,
    pub client_id: ClientId,
}

/// Read an existing state's durable identity without creating or migrating it.
///
/// `Ok(None)` is reserved for a database path that does not exist. An existing
/// but incomplete, unsupported, or malformed database fails closed so callers
/// cannot accidentally replace durable cursor or outbox state with a new
/// client identity.
pub fn read_persisted_identity<P: AsRef<Path>>(
    path: P,
) -> Result<Option<PersistedIdentity>, SyncError> {
    let path = path.as_ref();
    match path.try_exists() {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(_) => return Err(SyncError::StorageUnavailable),
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
    let connection = Connection::open_with_flags(path, flags).map_err(storage_error)?;
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(storage_error)?;

    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(persisted_database_error)?;
    match version {
        1 => validate_v1_schema(&connection)?,
        2 => validate_v2_schema(&connection)?,
        3 => validate_v3_schema(&connection)?,
        4 => validate_v4_schema(&connection)?,
        _ => return Err(corrupt("schema", "user_version")),
    }

    let mut statement = connection
        .prepare(
            "SELECT workspace_id, client_id FROM workspace_cursor ORDER BY workspace_id LIMIT 2",
        )
        .map_err(persisted_database_error)?;
    let mut rows = statement.query([]).map_err(persisted_database_error)?;
    let Some(row) = rows.next().map_err(persisted_database_error)? else {
        return Err(corrupt("workspace_cursor", "workspace_id"));
    };
    let workspace = match row.get_ref(0).map_err(persisted_database_error)? {
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map_err(|_| corrupt("workspace_cursor", "workspace_id"))?
            .to_owned(),
        _ => return Err(corrupt("workspace_cursor", "workspace_id")),
    };
    let client = match row.get_ref(1).map_err(persisted_database_error)? {
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map_err(|_| corrupt("workspace_cursor", "client_id"))?
            .to_owned(),
        _ => return Err(corrupt("workspace_cursor", "client_id")),
    };
    if rows.next().map_err(persisted_database_error)?.is_some() {
        return Err(corrupt("workspace_cursor", "workspace_id"));
    }

    let workspace_id =
        WorkspaceId::parse(&workspace).map_err(|_| corrupt("workspace_cursor", "workspace_id"))?;
    let client_id =
        ClientId::parse(&client).map_err(|_| corrupt("workspace_cursor", "client_id"))?;
    Ok(Some(PersistedIdentity {
        workspace_id,
        client_id,
    }))
}

fn persisted_database_error(error: rusqlite::Error) -> SyncError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    ) {
        corrupt("schema", "integrity")
    } else {
        storage_error(error)
    }
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

        match user_version_of(&conn)? {
            0 => {
                let transaction = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0001)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0002)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0003)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0004)
                    .map_err(storage_error)?;
                validate_v4_schema(&transaction)?;
                transaction.commit().map_err(storage_error)?;
            }
            1 => {
                validate_v1_schema(&conn)?;
                let transaction = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0002)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0003)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0004)
                    .map_err(storage_error)?;
                validate_v4_schema(&transaction)?;
                transaction.commit().map_err(storage_error)?;
            }
            2 => {
                validate_v2_schema(&conn)?;
                let transaction = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0003)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0004)
                    .map_err(storage_error)?;
                validate_v4_schema(&transaction)?;
                transaction.commit().map_err(storage_error)?;
            }
            3 => {
                validate_v3_schema(&conn)?;
                let transaction = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                transaction
                    .execute_batch(MIGRATION_0004)
                    .map_err(storage_error)?;
                validate_v4_schema(&transaction)?;
                transaction.commit().map_err(storage_error)?;
            }
            4 => validate_v4_schema(&conn)?,
            _ => return Err(corrupt("schema", "user_version")),
        }

        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1_000_i64)
            .map_err(storage_error)?;
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

    pub(crate) fn close(&mut self) -> Result<(), SyncError> {
        // Replace the live connection so dropping it releases the SQLite
        // handle while preserving the state object's closed identity.
        let replacement = Connection::open_in_memory().map_err(storage_error)?;
        let connection = std::mem::replace(&mut self.conn, replacement);
        drop(connection);
        Ok(())
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
        if self
            .cursor()?
            .pending_ack_revision
            .is_some_and(|current| revision < current)
        {
            return Err(SyncError::StreamInvariant {
                reason: "pending_ack_regression",
            });
        }
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
        if revision < self.cursor()?.last_ack_revision {
            return Err(SyncError::StreamInvariant {
                reason: "last_ack_regression",
            });
        }
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
        if revision < self.cursor()?.last_applied_revision {
            return Err(SyncError::StreamInvariant {
                reason: "last_applied_regression",
            });
        }
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
            client_id: self.client_id,
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

    /// Commit the one legacy live-event gap produced by clients that allowed
    /// later exact receipts to advance the cursor past a blob-backed event.
    ///
    /// This is intentionally separate from `set_last_applied_revision`: the
    /// normal cursor API never permits a regression. Every durable predicate
    /// identifying the legacy state is rechecked under one immediate
    /// transaction before the narrowly scoped repair is committed.
    #[doc(hidden)]
    pub fn commit_legacy_live_event_gap(
        &mut self,
        record: &ApplyJournalRecord,
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        commit_legacy_live_event_gap_tx(&transaction, self.workspace_id, self.client_id, record)?;
        transaction.commit().map_err(storage_error)
    }

    pub(crate) fn preflight_legacy_live_event_gap(
        &self,
        record: &ApplyJournalRecord,
    ) -> Result<(), SyncError> {
        let persisted =
            self.apply_journal(record.apply_id)?
                .ok_or(SyncError::ProtocolInvariant {
                    reason: "apply_journal_not_found",
                })?;
        if persisted != *record {
            return Err(SyncError::OperationChanged);
        }
        validate_legacy_live_event_gap(
            &self.conn,
            self.workspace_id,
            self.client_id,
            record,
            LegacyGapValidationPhase::Preflight,
        )?;
        Ok(())
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryAppliedOperationResult<'a> {
    mutation: &'a WorkspaceMutation,
    path_state: &'a WorkspacePathState,
    old_path_state: Option<&'a WorkspacePathState>,
    new_path_state: Option<&'a WorkspacePathState>,
}

#[derive(Clone, Copy)]
enum LegacyGapValidationPhase {
    Preflight,
    Commit,
}

struct ValidatedLegacyLiveEventGap {
    event: WorkspaceEventMessage,
    remove_outbox: bool,
    post_states: Vec<WorkspacePathState>,
    mutation_json: Vec<u8>,
    mutation_digest: [u8; 32],
    last_ack: WorkspaceRevision,
    last_applied: WorkspaceRevision,
}

fn commit_legacy_live_event_gap_tx(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
    client_id: ClientId,
    record: &ApplyJournalRecord,
) -> Result<(), SyncError> {
    // A same-stage write proves that the immutable row and filesystem receipt
    // supplied by recovery still match the row locked by this transaction.
    // It rolls back with every later validation or commit failure.
    crate::store::put_apply_journal_tx(transaction, workspace_id, record)?;
    let validated = validate_legacy_live_event_gap(
        transaction,
        workspace_id,
        client_id,
        record,
        LegacyGapValidationPhase::Commit,
    )?;

    for state in &validated.post_states {
        crate::store::put_path_state_tx(transaction, workspace_id, state)?;
    }
    let result = RecoveryAppliedOperationResult {
        mutation: &validated.event.mutation,
        path_state: &validated.event.path_state,
        old_path_state: validated.event.old_path_state.as_ref(),
        new_path_state: validated.event.new_path_state.as_ref(),
    };
    let result_digest = crate::store::digest(&result)?;
    crate::store::record_applied_operation_tx(
        transaction,
        crate::store::AppliedOperationWrite {
            origin_client_id: validated.event.origin_client_id,
            operation_id: validated.event.operation_id,
            revision: validated.event.revision,
            body_digest: result_digest,
            receipt_kind: AppliedOperationReceiptKind::MutationResult,
            mutation_json: Some(&validated.mutation_json),
            legacy_body_digest: None,
        },
    )?;
    crate::store::remove_provisional_mutation_acceptance_tx(
        transaction,
        validated.event.origin_client_id,
        validated.event.operation_id,
    )?;

    if validated.remove_outbox {
        let changed = transaction
            .execute(
                "DELETE FROM outbox WHERE client_id = ?1 AND operation_id = ?2 AND workspace_id = ?3 AND body_json = ?4 AND body_digest = ?5",
                params![
                    client_id.to_string(),
                    validated.event.operation_id.to_string(),
                    workspace_id.to_string(),
                    validated.mutation_json,
                    validated.mutation_digest.as_slice(),
                ],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Err(SyncError::ProtocolInvariant {
                reason: "legacy_live_event_gap_outbox_changed",
            });
        }
    }

    let changed = transaction
        .execute(
            "UPDATE workspace_cursor SET last_applied_revision = ?1, pending_ack_revision = NULL, updated_at_ms = ?2 WHERE workspace_id = ?3 AND client_id = ?4 AND last_ack_revision = ?5 AND last_applied_revision = ?6 AND pending_ack_revision = ?6",
            params![
                validated.event.revision.to_string(),
                now_ms(),
                workspace_id.to_string(),
                client_id.to_string(),
                validated.last_ack.to_string(),
                validated.last_applied.to_string(),
            ],
        )
        .map_err(storage_error)?;
    if changed != 1 {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_cursor_changed",
        });
    }
    crate::store::set_apply_stage_tx(
        transaction,
        workspace_id,
        record.apply_id,
        ApplyStage::DatabaseCommitted,
    )
}

fn validate_legacy_live_event_gap(
    connection: &Connection,
    workspace_id: WorkspaceId,
    client_id: ClientId,
    record: &ApplyJournalRecord,
    phase: LegacyGapValidationPhase,
) -> Result<ValidatedLegacyLiveEventGap, SyncError> {
    if record.workspace_id != workspace_id
        || record.item_kind != ApplyItemKind::Event
        || record.apply_namespace != ApplyNamespace::LiveEvent
    {
        return Err(corrupt("apply_journal", "apply_namespace"));
    }
    let filesystem_receipt = match (
        phase,
        record.stage,
        record.filesystem_receipt_json.as_deref(),
    ) {
        (
            LegacyGapValidationPhase::Preflight,
            ApplyStage::Prepared | ApplyStage::FilesystemStarted,
            None,
        ) => None,
        (
            LegacyGapValidationPhase::Preflight | LegacyGapValidationPhase::Commit,
            ApplyStage::FilesystemApplied,
            Some(filesystem_receipt_json),
        ) => {
            let filesystem_receipt: fns_fs::ApplyReceipt =
                parse_canonical_recovery_json(filesystem_receipt_json, "filesystem_receipt_json")?;
            if filesystem_receipt.apply_id != record.apply_id {
                return Err(corrupt("apply_journal", "filesystem_receipt_json"));
            }
            Some(filesystem_receipt)
        }
        _ => {
            return Err(SyncError::StreamInvariant {
                reason: "legacy_live_event_gap_stage",
            });
        }
    };
    if crate::store::apply_journal_immutable_digest(record) != record.operation_body_digest {
        return Err(corrupt("apply_journal", "operation_body_digest"));
    }

    let plan: ApplyCommitPlan = parse_canonical_recovery_json(&record.commit_json, "commit_json")?;
    let (event, remove_outbox) = match plan {
        ApplyCommitPlan::LiveEvent {
            event,
            remove_outbox,
        } => (event, remove_outbox),
        _ => return Err(corrupt("apply_journal", "commit_json")),
    };
    event
        .validate()
        .map_err(|_| corrupt("apply_journal", "commit_json"))?;
    if event.workspace_id != workspace_id
        || event.stream_id != record.stream_id
        || event.revision.to_string() != record.item_key
    {
        return Err(corrupt("apply_journal", "commit_json"));
    }

    let operation: RemoteApplyOperation =
        parse_canonical_recovery_json(&record.operation_json, "operation_json")?;
    let expected_operation = recovery_operation_for_event(&event)?;
    if operation != expected_operation {
        return Err(corrupt("apply_journal", "operation_json"));
    }
    if record.preimage_json != record.filesystem_operation_json {
        return Err(corrupt("apply_journal", "preimage_json"));
    }
    let filesystem_operation: fns_fs::FsOperation = parse_canonical_recovery_json(
        &record.filesystem_operation_json,
        "filesystem_operation_json",
    )?;
    let post_states: Vec<WorkspacePathState> =
        parse_canonical_recovery_json(&record.postimage_json, "postimage_json")?;
    let expected_post_states = recovery_post_states_for_event(&event)?;
    if post_states != expected_post_states
        || post_states.iter().any(|state| state.validate().is_err())
    {
        return Err(corrupt("apply_journal", "postimage_json"));
    }
    let operation_is_supported = matches!(
        (
            event.mutation.kind,
            &operation,
            &filesystem_operation,
            post_states.as_slice(),
        ),
        (
            fns_protocol::WorkspaceMutationKind::UpsertFile,
            RemoteApplyOperation::Upsert { state: operation_state },
            fns_fs::FsOperation::UpsertFile {
                path,
                content_hash,
                metadata,
                ..
            },
            [post_state],
        ) if operation_state == post_state
            && post_state.kind == fns_protocol::WorkspaceEntryKind::File
            && path == &post_state.path
            && post_state.content_hash.clone().into_option().as_ref() == Some(content_hash)
            && metadata == &post_state.metadata
    );
    if !operation_is_supported {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_operation_kind",
        });
    }
    if let Some(filesystem_receipt) = filesystem_receipt.as_ref() {
        validate_legacy_gap_receipt_shape(filesystem_receipt, &filesystem_operation, &post_states)?;
    }

    let journal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM apply_journal WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if journal_count != 1 {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_journal_count",
        });
    }
    let stream_row_count: i64 = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM stream_state WHERE workspace_id = ?1) + (SELECT COUNT(*) FROM stream_entries WHERE workspace_id = ?1) + (SELECT COUNT(*) FROM stream_revision_items WHERE workspace_id = ?1) + (SELECT COUNT(*) FROM stream_conflicts WHERE workspace_id = ?1)",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if stream_row_count != 0 {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_active_stream",
        });
    }

    let raw_cursor: (String, String, String, String, Option<String>) = connection
        .query_row(
            "SELECT workspace_id, client_id, last_ack_revision, last_applied_revision, pending_ack_revision FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(storage_error)?;
    if raw_cursor.0 != workspace_id.to_string() || raw_cursor.1 != client_id.to_string() {
        return Err(corrupt("workspace_cursor", "client_id"));
    }
    let last_ack = WorkspaceRevision::parse(&raw_cursor.2)
        .map_err(|_| corrupt("workspace_cursor", "last_ack_revision"))?;
    let last_applied = WorkspaceRevision::parse(&raw_cursor.3)
        .map_err(|_| corrupt("workspace_cursor", "last_applied_revision"))?;
    let pending_ack = raw_cursor
        .4
        .as_deref()
        .map(WorkspaceRevision::parse)
        .transpose()
        .map_err(|_| corrupt("workspace_cursor", "pending_ack_revision"))?;
    let next_revision = last_ack
        .get()
        .checked_add(1)
        .map(WorkspaceRevision::new)
        .ok_or(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_not_contiguous",
        })?;
    if event.revision != next_revision {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_not_contiguous",
        });
    }
    if event.revision >= last_applied {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_not_superseded",
        });
    }
    if pending_ack != Some(last_applied) {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_pending_ack",
        });
    }

    let receipts_at_revision: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM applied_operations WHERE revision = ?1",
            params![event.revision.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if receipts_at_revision != 0 {
        return Err(SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_receipt_present",
        });
    }

    for state in &post_states {
        let existing = connection
            .query_row(
                "SELECT state_json, state_digest FROM path_states WHERE workspace_id = ?1 AND path = ?2",
                params![workspace_id.to_string(), state.path.as_str()],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some((state_json, state_digest)) = existing {
            let existing_state: WorkspacePathState = serde_json::from_slice(&state_json)
                .map_err(|_| corrupt("path_states", "state_json"))?;
            if existing_state.validate().is_err()
                || existing_state.path != state.path
                || crate::store::canonical_json(&existing_state)? != state_json
                || state_digest.as_slice() != crate::store::body_digest(&state_json)
            {
                return Err(corrupt("path_states", "state_json"));
            }
            if existing_state.path_revision > state.path_revision {
                return Err(SyncError::StreamInvariant {
                    reason: "legacy_live_event_gap_newer_path_state",
                });
            }
            if existing_state.path_revision == state.path_revision && existing_state != *state {
                return Err(SyncError::StreamInvariant {
                    reason: "legacy_live_event_gap_path_state_changed",
                });
            }
        }
    }

    let mutation_json = crate::store::canonical_json(&event.mutation)?;
    let mutation_digest = crate::store::body_digest(&mutation_json);
    if remove_outbox {
        if event.origin_client_id != client_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "legacy_live_event_gap_outbox_identity",
            });
        }
        let outbox = connection
            .query_row(
                "SELECT workspace_id, body_json, body_digest FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![client_id.to_string(), event.operation_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SyncError::ProtocolInvariant {
                reason: "legacy_live_event_gap_outbox_missing",
            })?;
        if outbox.0 != workspace_id.to_string()
            || outbox.1 != mutation_json
            || outbox.2.as_slice() != mutation_digest
        {
            return Err(SyncError::ProtocolInvariant {
                reason: "legacy_live_event_gap_outbox_mismatch",
            });
        }
    }

    Ok(ValidatedLegacyLiveEventGap {
        event,
        remove_outbox,
        post_states,
        mutation_json,
        mutation_digest,
        last_ack,
        last_applied,
    })
}

fn validate_legacy_gap_receipt_shape(
    receipt: &fns_fs::ApplyReceipt,
    filesystem_operation: &fns_fs::FsOperation,
    post_states: &[WorkspacePathState],
) -> Result<(), SyncError> {
    let invalid = || corrupt("apply_journal", "filesystem_receipt_json");
    if receipt.touched.is_empty()
        && receipt.postimages.is_empty()
        && receipt.postimage_hashes.is_empty()
        && receipt.cleanup_name.is_none()
    {
        return Ok(());
    }
    let (
        fns_fs::FsOperation::UpsertFile {
            path,
            content_hash,
            metadata,
            expected,
            ..
        },
        [post_state],
        [touched],
        [Some(observed)],
        [Some(observed_hash)],
    ) = (
        filesystem_operation,
        post_states,
        receipt.touched.as_slice(),
        receipt.postimages.as_slice(),
        receipt.postimage_hashes.as_slice(),
    )
    else {
        return Err(invalid());
    };
    let cleanup_leaf = format!(".fns-delete-{}", receipt.apply_id.0);
    let expected_cleanup = match expected {
        fns_fs::ExpectedEntry::Missing => None,
        fns_fs::ExpectedEntry::Present { .. } => Some(path.as_str().rsplit_once('/').map_or_else(
            || cleanup_leaf.clone(),
            |(parent, _)| format!("{parent}/{cleanup_leaf}"),
        )),
    };
    if touched != path
        || observed_hash != content_hash
        || post_state.path != *path
        || post_state.kind != fns_protocol::WorkspaceEntryKind::File
        || post_state.content_hash.clone().into_option().as_ref() != Some(content_hash)
        || post_state.metadata != *metadata
        || observed.path != *path
        || observed.kind != fns_protocol::WorkspaceEntryKind::File
        || observed.metadata != *metadata
        || receipt.cleanup_name != expected_cleanup
    {
        return Err(invalid());
    }
    Ok(())
}

fn parse_canonical_recovery_json<T>(bytes: &[u8], field: &'static str) -> Result<T, SyncError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value = serde_json::from_slice(bytes).map_err(|_| corrupt("apply_journal", field))?;
    if crate::store::canonical_json(&value)? != bytes {
        return Err(corrupt("apply_journal", field));
    }
    Ok(value)
}

fn recovery_operation_for_event(
    event: &WorkspaceEventMessage,
) -> Result<RemoteApplyOperation, SyncError> {
    if event.mutation.kind == fns_protocol::WorkspaceMutationKind::Rename {
        return Ok(RemoteApplyOperation::Rename {
            old_state: event
                .old_path_state
                .clone()
                .ok_or(corrupt("apply_journal", "operation_json"))?,
            new_state: event
                .new_path_state
                .clone()
                .ok_or(corrupt("apply_journal", "operation_json"))?,
        });
    }
    if event.path_state.kind == fns_protocol::WorkspaceEntryKind::Tombstone {
        Ok(RemoteApplyOperation::Delete {
            state: event.path_state.clone(),
        })
    } else {
        Ok(RemoteApplyOperation::Upsert {
            state: event.path_state.clone(),
        })
    }
}

fn recovery_post_states_for_event(
    event: &WorkspaceEventMessage,
) -> Result<Vec<WorkspacePathState>, SyncError> {
    if event.mutation.kind == fns_protocol::WorkspaceMutationKind::Rename {
        Ok(vec![
            event
                .old_path_state
                .clone()
                .ok_or(corrupt("apply_journal", "postimage_json"))?,
            event
                .new_path_state
                .clone()
                .ok_or(corrupt("apply_journal", "postimage_json"))?,
        ])
    } else {
        Ok(vec![event.path_state.clone()])
    }
}

pub struct StateTransaction<'a> {
    pub(crate) transaction: Transaction<'a>,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) client_id: ClientId,
}

impl StateTransaction<'_> {
    pub fn put_path_state(&mut self, state: &WorkspacePathState) -> Result<(), SyncError> {
        crate::store::put_path_state_tx(&self.transaction, self.workspace_id(), state)
    }

    pub fn remove_path_state(&mut self, path: &WorkspacePath) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "DELETE FROM path_states WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn put_local_intent(
        &mut self,
        path: &str,
        intent_json: &[u8],
        updated_at_ms: i64,
    ) -> Result<(), SyncError> {
        let path = WorkspacePath::parse(path).map_err(|_| SyncError::InvalidConfiguration {
            reason: "invalid_path",
        })?;
        crate::store::put_local_intent_tx(
            &self.transaction,
            self.workspace_id(),
            &path,
            intent_json,
            updated_at_ms,
        )
    }

    pub fn remove_local_intent(&mut self, path: &WorkspacePath) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "DELETE FROM local_intents WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_pending_ack(&mut self, revision: WorkspaceRevision) -> Result<(), SyncError> {
        crate::state::set_pending_ack_tx(&self.transaction, self.workspace_id(), revision)
    }

    pub fn clear_pending_ack(&mut self) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "UPDATE workspace_cursor SET pending_ack_revision = NULL, updated_at_ms = ?1 WHERE workspace_id = ?2",
                params![now_ms(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_last_ack_revision(&mut self, revision: WorkspaceRevision) -> Result<(), SyncError> {
        crate::state::set_last_ack_revision_tx(&self.transaction, self.workspace_id(), revision)
    }

    pub fn set_last_applied_revision(
        &mut self,
        revision: WorkspaceRevision,
    ) -> Result<(), SyncError> {
        crate::state::set_last_applied_revision_tx(&self.transaction, self.workspace_id(), revision)
    }

    pub fn enqueue_mutation(
        &mut self,
        mutation: &WorkspaceMutation,
    ) -> Result<OutboxRecord, SyncError> {
        self.enqueue_mutation_at(mutation, now_ms())
    }

    pub fn enqueue_mutation_at(
        &mut self,
        mutation: &WorkspaceMutation,
        created_at_ms: i64,
    ) -> Result<OutboxRecord, SyncError> {
        mutation
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_mutation",
            })?;
        if mutation.workspace_id != self.workspace_id || mutation.client_id != self.client_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "outbox_identity_mismatch",
            });
        }
        let body_json = crate::store::canonical_json(mutation)?;
        crate::store::enqueue_outbox_tx(
            &self.transaction,
            self.client_id,
            mutation.operation_id,
            mutation.workspace_id,
            body_json,
            created_at_ms,
        )
    }

    pub fn enqueue_conflict_resolution(
        &mut self,
        resolution: &WorkspaceConflictResolvedRequest,
    ) -> Result<OutboxRecord, SyncError> {
        resolution
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict_resolution",
            })?;
        if resolution.workspace_id != self.workspace_id || resolution.client_id != self.client_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "outbox_identity_mismatch",
            });
        }
        let body_json = crate::store::canonical_json(resolution)?;
        crate::store::enqueue_outbox_tx(
            &self.transaction,
            self.client_id,
            resolution.operation_id,
            resolution.workspace_id,
            body_json,
            now_ms(),
        )
    }

    pub fn set_outbox_stage(
        &mut self,
        operation_id: OperationId,
        stage: OutboxStage,
    ) -> Result<(), SyncError> {
        crate::store::set_outbox_stage_tx(&self.transaction, self.client_id, operation_id, stage)
    }

    pub fn remove_outbox(&mut self, operation_id: OperationId) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "DELETE FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![self.client_id.to_string(), operation_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn put_stream_state(&mut self, state: &StreamStateRecord) -> Result<(), SyncError> {
        crate::store::put_stream_state_tx(&self.transaction, self.workspace_id(), state)
    }

    pub fn set_stream_end_received(&mut self, received: bool) -> Result<(), SyncError> {
        crate::store::set_stream_end_received_tx(&self.transaction, self.workspace_id(), received)
    }

    pub fn advance_stream_index(&mut self, next_index: u32) -> Result<(), SyncError> {
        crate::store::advance_stream_index_tx(&self.transaction, self.workspace_id(), next_index)
    }

    pub fn clear_stream(&mut self) -> Result<(), SyncError> {
        for table in [
            "stream_entries",
            "stream_revision_items",
            "stream_conflicts",
        ] {
            let query = format!("DELETE FROM {table} WHERE workspace_id = ?1");
            self.transaction
                .execute(&query, params![self.workspace_id.to_string()])
                .map_err(storage_error)?;
        }
        self.transaction
            .execute(
                "DELETE FROM stream_state WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn put_stream_entry(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
    ) -> Result<StreamEntryRecord, SyncError> {
        crate::store::put_stream_entry_tx(&self.transaction, self.workspace_id(), entry, status)
    }

    pub fn put_stream_event(
        &mut self,
        event: &WorkspaceEventMessage,
        status: StreamItemStatus,
    ) -> Result<StreamRevisionItemRecord, SyncError> {
        let body_json = crate::store::canonical_json(event)?;
        crate::store::put_stream_revision_item_tx(
            &self.transaction,
            self.workspace_id(),
            event.stream_id,
            event.revision,
            crate::model::StreamRevisionItemKind::Event,
            Some(event.index),
            body_json.clone(),
            crate::store::body_digest(&body_json),
            status,
        )
    }

    pub fn put_stream_conflict_resolved(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        status: StreamItemStatus,
    ) -> Result<StreamRevisionItemRecord, SyncError> {
        let stream_id = crate::store::active_stream_state(&self.transaction, self.workspace_id())?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?
            .stream_id;
        let body_json = crate::store::canonical_json(message)?;
        crate::store::put_stream_revision_item_tx(
            &self.transaction,
            self.workspace_id(),
            stream_id,
            message.revision,
            crate::model::StreamRevisionItemKind::ConflictResolved,
            None,
            body_json.clone(),
            crate::store::body_digest(&body_json),
            status,
        )
    }

    pub fn put_stream_conflict(
        &mut self,
        message: &WorkspaceConflictCreatedMessage,
        status: StreamConflictStatus,
        stream_id: fns_protocol::StreamId,
    ) -> Result<StreamConflictRecord, SyncError> {
        crate::store::put_stream_conflict_tx(
            &self.transaction,
            self.workspace_id(),
            message,
            stream_id,
            status,
        )
    }

    pub fn replace_authoritative_conflicts(
        &mut self,
        stream_id: fns_protocol::StreamId,
    ) -> Result<(), SyncError> {
        crate::store::replace_authoritative_conflicts_tx(
            &self.transaction,
            self.workspace_id(),
            stream_id,
        )
    }

    pub fn put_apply_journal(&mut self, record: &ApplyJournalRecord) -> Result<(), SyncError> {
        crate::store::put_apply_journal_tx(&self.transaction, self.workspace_id(), record)
    }

    pub fn set_apply_stage(
        &mut self,
        apply_id: crate::ApplyId,
        stage: ApplyStage,
    ) -> Result<(), SyncError> {
        crate::store::set_apply_stage_tx(&self.transaction, self.workspace_id(), apply_id, stage)
    }

    pub fn set_apply_filesystem_applied(
        &mut self,
        apply_id: crate::ApplyId,
        receipt_json: &[u8],
    ) -> Result<(), SyncError> {
        crate::store::set_apply_filesystem_applied_tx(
            &self.transaction,
            self.workspace_id(),
            apply_id,
            receipt_json,
        )
    }

    pub fn remove_apply_journal(&mut self, apply_id: crate::ApplyId) -> Result<(), SyncError> {
        crate::store::remove_apply_journal_tx(&self.transaction, self.workspace_id(), apply_id)
    }

    pub fn record_mutation_applied_operation(
        &mut self,
        origin_client_id: ClientId,
        operation_id: OperationId,
        revision: WorkspaceRevision,
        body_digest: [u8; 32],
        mutation: &WorkspaceMutation,
        legacy_body_digest: Option<[u8; 32]>,
    ) -> Result<(), SyncError> {
        let mutation_json = crate::store::canonical_json(mutation)?;
        crate::store::record_applied_operation_tx(
            &self.transaction,
            crate::store::AppliedOperationWrite {
                origin_client_id,
                operation_id,
                revision,
                body_digest,
                receipt_kind: AppliedOperationReceiptKind::MutationResult,
                mutation_json: Some(&mutation_json),
                legacy_body_digest,
            },
        )?;
        crate::store::remove_provisional_mutation_acceptance_tx(
            &self.transaction,
            origin_client_id,
            operation_id,
        )
    }

    pub fn record_provisional_mutation_acceptance(
        &mut self,
        accepted: &fns_protocol::WorkspaceMutationAcceptedMessage,
    ) -> Result<(), SyncError> {
        crate::store::record_provisional_mutation_acceptance_tx(&self.transaction, accepted)
    }

    pub fn record_conflict_applied_operation(
        &mut self,
        origin_client_id: ClientId,
        operation_id: OperationId,
        revision: WorkspaceRevision,
        body_digest: [u8; 32],
        legacy_body_digest: Option<[u8; 32]>,
    ) -> Result<(), SyncError> {
        crate::store::record_applied_operation_tx(
            &self.transaction,
            crate::store::AppliedOperationWrite {
                origin_client_id,
                operation_id,
                revision,
                body_digest,
                receipt_kind: AppliedOperationReceiptKind::ConflictResolution,
                mutation_json: None,
                legacy_body_digest,
            },
        )
    }

    pub fn put_conflict(&mut self, conflict: &ConflictRecord) -> Result<(), SyncError> {
        crate::store::put_conflict_tx(&self.transaction, self.workspace_id(), conflict)
    }

    pub fn set_conflict_status(
        &mut self,
        conflict_id: fns_protocol::ConflictId,
        status: ConflictStatus,
    ) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "UPDATE conflicts SET status = ?1 WHERE conflict_id = ?2 AND workspace_id = ?3",
                params![
                    status.as_str(),
                    conflict_id.to_string(),
                    self.workspace_id.to_string()
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn remove_conflict(
        &mut self,
        conflict_id: fns_protocol::ConflictId,
    ) -> Result<(), SyncError> {
        self.transaction
            .execute(
                "DELETE FROM conflicts WHERE conflict_id = ?1 AND workspace_id = ?2",
                params![conflict_id.to_string(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub(crate) fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
}

pub(crate) fn set_pending_ack_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    revision: WorkspaceRevision,
) -> Result<(), SyncError> {
    let current: Option<String> = connection
        .query_row(
            "SELECT pending_ack_revision FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if current
        .as_deref()
        .map(WorkspaceRevision::parse)
        .transpose()
        .map_err(|_| corrupt("workspace_cursor", "pending_ack_revision"))?
        .is_some_and(|current| revision < current)
    {
        return Err(SyncError::StreamInvariant {
            reason: "pending_ack_regression",
        });
    }
    connection
        .execute(
            "UPDATE workspace_cursor SET pending_ack_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
            params![revision.to_string(), now_ms(), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn set_last_ack_revision_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    revision: WorkspaceRevision,
) -> Result<(), SyncError> {
    let current: String = connection
        .query_row(
            "SELECT last_ack_revision FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let current = WorkspaceRevision::parse(&current)
        .map_err(|_| corrupt("workspace_cursor", "last_ack_revision"))?;
    if revision < current {
        return Err(SyncError::StreamInvariant {
            reason: "last_ack_regression",
        });
    }
    connection
        .execute(
            "UPDATE workspace_cursor SET last_ack_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
            params![revision.to_string(), now_ms(), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn set_last_applied_revision_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    revision: WorkspaceRevision,
) -> Result<(), SyncError> {
    let current: String = connection
        .query_row(
            "SELECT last_applied_revision FROM workspace_cursor WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let current = WorkspaceRevision::parse(&current)
        .map_err(|_| corrupt("workspace_cursor", "last_applied_revision"))?;
    if revision < current {
        return Err(SyncError::StreamInvariant {
            reason: "last_applied_regression",
        });
    }
    connection
        .execute(
            "UPDATE workspace_cursor SET last_applied_revision = ?1, updated_at_ms = ?2 WHERE workspace_id = ?3",
            params![revision.to_string(), now_ms(), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn user_version_of(connection: &Connection) -> Result<i64, SyncError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(storage_error)
}

type SchemaObject = (String, String, String, Option<String>);

fn schema_objects(connection: &Connection) -> Result<Vec<SchemaObject>, SyncError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name, tbl_name",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(storage_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
}

fn validate_v2_schema(connection: &Connection) -> Result<(), SyncError> {
    if user_version_of(connection)? != 2 {
        return Err(corrupt("schema", "user_version"));
    }
    let reference = Connection::open_in_memory().map_err(storage_error)?;
    reference
        .execute_batch(MIGRATION_0001)
        .map_err(storage_error)?;
    reference
        .execute_batch(MIGRATION_0002)
        .map_err(storage_error)?;
    if schema_objects(connection)? != schema_objects(&reference)? {
        return Err(corrupt("schema", "layout"));
    }
    Ok(())
}

fn validate_v1_schema(connection: &Connection) -> Result<(), SyncError> {
    validate_schema(connection, 1, &[MIGRATION_0001])
}

fn validate_v3_schema(connection: &Connection) -> Result<(), SyncError> {
    validate_schema(
        connection,
        3,
        &[MIGRATION_0001, MIGRATION_0002, MIGRATION_0003],
    )
}

fn validate_v4_schema(connection: &Connection) -> Result<(), SyncError> {
    validate_schema(
        connection,
        4,
        &[
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
        ],
    )
}

fn validate_schema(
    connection: &Connection,
    version: i64,
    migrations: &[&str],
) -> Result<(), SyncError> {
    if user_version_of(connection)? != version {
        return Err(corrupt("schema", "user_version"));
    }
    let reference = Connection::open_in_memory().map_err(storage_error)?;
    for migration in migrations {
        reference.execute_batch(migration).map_err(storage_error)?;
    }
    if schema_objects(connection)? != schema_objects(&reference)? {
        return Err(corrupt("schema", "layout"));
    }
    Ok(())
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
