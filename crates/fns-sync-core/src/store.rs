use fns_fs::{ApplyId, FileFingerprint, HashCache, HashCacheError};
use fns_protocol::{
    ClientId, ConflictId, OperationId, StreamId, WorkspaceConflictCreatedMessage,
    WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest, WorkspaceContentHash,
    WorkspaceEventMessage, WorkspaceId, WorkspaceMutation, WorkspaceMutationAcceptedMessage,
    WorkspacePath, WorkspaceRevision, WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEntryMessage,
    WorkspaceSnapshotMode,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{SyncError, corrupt, storage_error};
use crate::ids::now_ms;
use crate::model::{
    AppliedOperationReceiptKind, AppliedOperationRecord, ApplyItemKind, ApplyJournalRecord,
    ApplyNamespace, ApplyStage, ConflictRecord, ConflictStatus, LocalIntentRecord, OutboxRecord,
    OutboxStage, PathStateRecord, StreamConflictRecord, StreamConflictStatus, StreamEntryRecord,
    StreamItemStatus, StreamRevisionItemKind, StreamRevisionItemRecord, StreamStateRecord,
};
use crate::state::SqliteState;

pub(crate) struct BoundedPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) deferred_by_byte_budget: bool,
}

pub(crate) struct StreamTableSummary {
    pub(crate) entry_count: u64,
    pub(crate) pending_entry_count: u64,
    pub(crate) revision_count: u64,
    pub(crate) pending_revision_count: u64,
    pub(crate) conflict_count: u64,
}

pub(crate) struct AppliedOperationWrite<'a> {
    pub(crate) origin_client_id: ClientId,
    pub(crate) operation_id: OperationId,
    pub(crate) revision: WorkspaceRevision,
    pub(crate) body_digest: [u8; 32],
    pub(crate) receipt_kind: AppliedOperationReceiptKind,
    pub(crate) mutation_json: Option<&'a [u8]>,
    pub(crate) legacy_body_digest: Option<[u8; 32]>,
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SyncError> {
    serde_json::to_vec(value).map_err(|_| SyncError::InvalidConfiguration {
        reason: "serialization_failed",
    })
}

pub fn body_digest(body: &[u8]) -> [u8; 32] {
    *blake3::hash(body).as_bytes()
}

pub fn apply_journal_immutable_digest(record: &ApplyJournalRecord) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for field in [
        record.apply_id.0.to_string().as_bytes(),
        record.workspace_id.to_string().as_bytes(),
        record.stream_id.to_string().as_bytes(),
        record.item_kind.as_str().as_bytes(),
        record.item_key.as_bytes(),
        record.apply_namespace.as_str().as_bytes(),
        record.operation_json.as_slice(),
        record.filesystem_operation_json.as_slice(),
        record.commit_json.as_slice(),
        record.preimage_json.as_slice(),
        record.postimage_json.as_slice(),
    ] {
        hasher.update(&(field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    *hasher.finalize().as_bytes()
}

pub fn digest<T: Serialize>(value: &T) -> Result<[u8; 32], SyncError> {
    Ok(body_digest(&canonical_json(value)?))
}

impl SqliteState {
    pub fn path_state(&self, path: &str) -> Result<Option<PathStateRecord>, SyncError> {
        let path = WorkspacePath::parse(path).map_err(|_| SyncError::InvalidConfiguration {
            reason: "invalid_path",
        })?;
        self.conn
            .query_row(
                "SELECT workspace_id, path, state_json, state_digest FROM path_states WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
                row_to_path_state,
            )
            .optional()
            .map_err(|error| match error {
                rusqlite::Error::InvalidQuery => corrupt("path_states", "state_json"),
                _ => storage_error(error),
            })
    }

    pub fn get_path_state(
        &self,
        path: &WorkspacePath,
    ) -> Result<Option<PathStateRecord>, SyncError> {
        self.path_state(path.as_str())
    }

    pub fn path_states(&self) -> Result<Vec<PathStateRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, path, state_json, state_digest FROM path_states WHERE workspace_id = ?1 ORDER BY path",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.workspace_id.to_string()], row_to_path_state)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn list_path_states(&self) -> Result<Vec<PathStateRecord>, SyncError> {
        self.path_states()
    }

    pub fn put_path_state(
        &mut self,
        state: &fns_protocol::WorkspacePathState,
    ) -> Result<(), SyncError> {
        put_path_state_tx(&self.conn, self.workspace_id, state)
    }

    pub fn upsert_path_state(
        &mut self,
        state: &fns_protocol::WorkspacePathState,
    ) -> Result<(), SyncError> {
        self.put_path_state(state)
    }

    pub fn remove_path_state(&mut self, path: &WorkspacePath) -> Result<(), SyncError> {
        self.conn
            .execute(
                "DELETE FROM path_states WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn delete_path_state(&mut self, path: &WorkspacePath) -> Result<(), SyncError> {
        self.remove_path_state(path)
    }

    pub fn enqueue_mutation(
        &mut self,
        mutation: &WorkspaceMutation,
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
        let body_json = canonical_json(mutation)?;
        self.enqueue_outbox_bytes(
            mutation.operation_id,
            mutation.workspace_id,
            body_json,
            now_ms(),
        )
    }

    pub fn insert_outbox(
        &mut self,
        mutation: &WorkspaceMutation,
    ) -> Result<OutboxRecord, SyncError> {
        self.enqueue_mutation(mutation)
    }

    pub fn put_outbox(&mut self, mutation: &WorkspaceMutation) -> Result<OutboxRecord, SyncError> {
        self.enqueue_mutation(mutation)
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
        let body_json = canonical_json(resolution)?;
        self.enqueue_outbox_bytes(
            resolution.operation_id,
            resolution.workspace_id,
            body_json,
            now_ms(),
        )
    }

    pub fn outbox(&self) -> Result<Vec<OutboxRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 ORDER BY created_at_ms, operation_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.client_id.to_string()], row_to_outbox)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn list_outbox(&self) -> Result<Vec<OutboxRecord>, SyncError> {
        self.outbox()
    }

    /// Count durable work that must settle before this client is quiescent.
    ///
    /// This is intentionally read-only: status reporting must never dispatch an
    /// outbox row or advance stream state merely by observing it.
    pub fn pending_work_count(&self) -> Result<u64, SyncError> {
        let count = self
            .conn
            .query_row(
                "SELECT \
                    (SELECT COUNT(*) FROM outbox WHERE client_id = ?1 AND stage != 'blocked_conflict') + \
                    (SELECT COUNT(*) FROM local_intents WHERE workspace_id = ?2) + \
                    (SELECT COUNT(*) FROM stream_state WHERE workspace_id = ?2) + \
                    (SELECT COUNT(*) FROM stream_entries WHERE workspace_id = ?2 AND status NOT IN ('applied','preserved')) + \
                    (SELECT COUNT(*) FROM stream_revision_items WHERE workspace_id = ?2 AND status NOT IN ('applied','preserved')) + \
                    (SELECT COUNT(*) FROM apply_journal WHERE workspace_id = ?2) + \
                    (SELECT COUNT(*) FROM workspace_cursor WHERE workspace_id = ?2 AND pending_ack_revision IS NOT NULL)",
                params![self.client_id.to_string(), self.workspace_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)?;
        bounded_count(count, "runtime_pending_work")
    }

    pub fn outbox_entry(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<OutboxRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![self.client_id.to_string(), operation_id.to_string()],
                row_to_outbox,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn mark_dispatched(&mut self, operation_id: OperationId) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE outbox SET stage = 'dispatched' WHERE client_id = ?1 AND operation_id = ?2 AND stage = 'queued'",
                params![self.client_id.to_string(), operation_id.to_string()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn dispatch_next(&mut self) -> Result<Option<OutboxRecord>, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let selected = transaction
            .query_row(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND stage = 'queued' ORDER BY created_at_ms, operation_id LIMIT 1",
                params![self.client_id.to_string()],
                row_to_outbox,
            )
            .optional()
            .map_err(storage_error)?;
        let Some(mut selected) = selected else {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        };
        transaction
            .execute(
                "UPDATE outbox SET stage = 'dispatched' WHERE client_id = ?1 AND operation_id = ?2 AND stage = 'queued'",
                params![self.client_id.to_string(), selected.operation_id.to_string()],
            )
            .map_err(storage_error)?;
        selected.stage = OutboxStage::Dispatched;
        transaction.commit().map_err(storage_error)?;
        Ok(Some(selected))
    }

    pub fn pending_outbox(&mut self, limit: usize) -> Result<Vec<OutboxRecord>, SyncError> {
        let mut records = Vec::new();
        for _ in 0..limit {
            let Some(record) = self.dispatch_next()? else {
                break;
            };
            records.push(record);
        }
        Ok(records)
    }

    pub fn dispatch_pending(&mut self, limit: usize) -> Result<Vec<OutboxRecord>, SyncError> {
        self.pending_outbox(limit)
    }

    /// Select outstanding operations for transport replay. A queued row is
    /// promoted to dispatched in the same immediate transaction as selection;
    /// an already-dispatched row is intentionally selected again so reopening
    /// after a crash replays the exact immutable operation body.
    pub fn pending_outbox_replay(&mut self, limit: usize) -> Result<Vec<OutboxRecord>, SyncError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let mut statement = transaction
            .prepare(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND stage IN ('queued','dispatched') ORDER BY created_at_ms, operation_id",
            )
            .map_err(storage_error)?;
        let mut rows = statement
            .query(params![self.client_id.to_string()])
            .map_err(storage_error)?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(storage_error)? {
            let record = row_to_outbox(row).map_err(|_| corrupt("outbox", "body_json"))?;
            record
                .decoded_body()
                .map_err(|_| corrupt("outbox", "body_json"))?;
            records.push(record);
            if records.len() == limit {
                break;
            }
        }
        drop(rows);
        drop(statement);
        for record in &mut records {
            if record.stage == OutboxStage::Queued {
                transaction
                    .execute(
                        "UPDATE outbox SET stage = 'dispatched' WHERE client_id = ?1 AND operation_id = ?2 AND stage = 'queued'",
                        params![self.client_id.to_string(), record.operation_id.to_string()],
                    )
                    .map_err(storage_error)?;
                record.stage = OutboxStage::Dispatched;
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(records)
    }

    pub fn set_outbox_stage(
        &mut self,
        operation_id: OperationId,
        stage: OutboxStage,
    ) -> Result<(), SyncError> {
        let existing = self
            .outbox_entry(operation_id)?
            .ok_or(SyncError::ProtocolInvariant {
                reason: "outbox_not_found",
            })?;
        if !outbox_stage_transition_allowed(existing.stage, stage) {
            return Err(SyncError::ProtocolInvariant {
                reason: "outbox_stage_regression",
            });
        }
        self.conn
            .execute(
                "UPDATE outbox SET stage = ?1 WHERE client_id = ?2 AND operation_id = ?3",
                params![
                    stage.as_str(),
                    self.client_id.to_string(),
                    operation_id.to_string()
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// Make every durable blob-waiting operation replayable for a new
    /// connection in one immediate transaction. The immutable body and digest
    /// are never rewritten.
    pub fn replay_awaiting_blob_mutations(&mut self) -> Result<usize, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let changed = transaction
            .execute(
                "UPDATE outbox SET stage = 'dispatched' WHERE client_id = ?1 AND stage = 'awaiting_blob'",
                params![self.client_id.to_string()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(changed)
    }

    pub fn remove_outbox(&mut self, operation_id: OperationId) -> Result<(), SyncError> {
        self.conn
            .execute(
                "DELETE FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![self.client_id.to_string(), operation_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn local_intent(&self, path: &str) -> Result<Option<LocalIntentRecord>, SyncError> {
        let path = WorkspacePath::parse(path).map_err(|_| SyncError::InvalidConfiguration {
            reason: "invalid_path",
        })?;
        self.conn
            .query_row(
                "SELECT workspace_id, path, intent_json, updated_at_ms FROM local_intents WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
                row_to_local_intent,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn local_intents(&self) -> Result<Vec<LocalIntentRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, path, intent_json, updated_at_ms FROM local_intents WHERE workspace_id = ?1 ORDER BY path",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.workspace_id.to_string()], row_to_local_intent)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn put_local_intent(
        &mut self,
        path: &WorkspacePath,
        intent_json: &[u8],
        updated_at_ms: i64,
    ) -> Result<(), SyncError> {
        put_local_intent_tx(
            &self.conn,
            self.workspace_id,
            path,
            intent_json,
            updated_at_ms,
        )
    }

    pub fn remove_local_intent(&mut self, path: &WorkspacePath) -> Result<(), SyncError> {
        self.conn
            .execute(
                "DELETE FROM local_intents WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn begin_stream(
        &mut self,
        begin: &WorkspaceSnapshotBeginMessage,
    ) -> Result<StreamStateRecord, SyncError> {
        begin.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_begin",
        })?;
        if begin.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT workspace_id, stream_id, mode, from_revision, final_revision, expected_entry_count, expected_event_count, expected_conflict_count, next_event_index, end_received FROM stream_state WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
                row_to_stream_state,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.stream_id == begin.stream_id {
                if !stream_begin_matches(&existing, begin) {
                    return Err(SyncError::StreamInvariant {
                        reason: "stream_begin_changed",
                    });
                }
                transaction.commit().map_err(storage_error)?;
                return Ok(existing);
            }

            let last_ack_revision = transaction
                .query_row(
                    "SELECT last_ack_revision FROM workspace_cursor WHERE workspace_id = ?1",
                    params![self.workspace_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_error)?;
            let last_ack_revision = WorkspaceRevision::parse(&last_ack_revision)
                .map_err(|_| corrupt("workspace_cursor", "last_ack_revision"))?;
            if begin.from_revision != last_ack_revision {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_begin_not_at_ack",
                });
            }
            let apply_in_progress = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM apply_journal WHERE workspace_id = ?1)",
                    params![self.workspace_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_error)?;
            if apply_in_progress {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_apply_in_progress",
                });
            }
            for table in [
                "stream_entries",
                "stream_revision_items",
                "stream_conflicts",
            ] {
                let query = format!("DELETE FROM {table} WHERE workspace_id = ?1");
                transaction
                    .execute(&query, params![self.workspace_id.to_string()])
                    .map_err(storage_error)?;
            }
            transaction
                .execute(
                    "DELETE FROM stream_state WHERE workspace_id = ?1",
                    params![self.workspace_id.to_string()],
                )
                .map_err(storage_error)?;
        }
        let state = stream_state_from_begin(begin);
        transaction
            .execute(
                "INSERT INTO stream_state (workspace_id, stream_id, mode, from_revision, final_revision, expected_entry_count, expected_event_count, expected_conflict_count, next_event_index, end_received) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0)",
                params![
                    self.workspace_id.to_string(),
                    begin.stream_id.to_string(),
                    mode_string(begin.mode),
                    begin.from_revision.to_string(),
                    begin.final_revision.to_string(),
                    i64::from(begin.entry_count),
                    i64::from(begin.event_count),
                    i64::from(begin.conflict_count),
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(state)
    }

    pub fn put_stream_state(&mut self, state: &StreamStateRecord) -> Result<(), SyncError> {
        if state.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        put_stream_state_tx(&transaction, self.workspace_id, state)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn stream_state(&self) -> Result<Option<StreamStateRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT workspace_id, stream_id, mode, from_revision, final_revision, expected_entry_count, expected_event_count, expected_conflict_count, next_event_index, end_received FROM stream_state WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
                row_to_stream_state,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn clear_stream(&mut self) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        for table in [
            "stream_entries",
            "stream_revision_items",
            "stream_conflicts",
        ] {
            let query = format!("DELETE FROM {table} WHERE workspace_id = ?1");
            transaction
                .execute(&query, params![self.workspace_id.to_string()])
                .map_err(storage_error)?;
        }
        transaction
            .execute(
                "DELETE FROM stream_state WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn set_stream_end_received(&mut self, received: bool) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        set_stream_end_received_tx(&transaction, self.workspace_id, received)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn put_stream_entry(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
    ) -> Result<StreamEntryRecord, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let result = put_stream_entry_tx(&transaction, self.workspace_id, entry, status)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn stream_entry(
        &self,
        stream_id: StreamId,
        index: u32,
    ) -> Result<Option<StreamEntryRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND entry_index = ?3",
                params![self.workspace_id.to_string(), stream_id.to_string(), i64::from(index)],
                row_to_stream_entry,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn stream_entries(&self, stream_id: StreamId) -> Result<Vec<StreamEntryRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 ORDER BY entry_index",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![self.workspace_id.to_string(), stream_id.to_string()],
                row_to_stream_entry,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub(crate) fn pending_stream_entries_page(
        &self,
        stream_id: StreamId,
        max_items: usize,
        max_serialized_bytes: usize,
        allow_oversized_first_item: bool,
    ) -> Result<BoundedPage<StreamEntryRecord>, SyncError> {
        if max_items == 0 {
            return Ok(BoundedPage {
                items: Vec::new(),
                deferred_by_byte_budget: false,
            });
        }
        let sql_limit = i64::try_from(max_items).map_err(|_| SyncError::InvalidConfiguration {
            reason: "page_item_limit_too_large",
        })?;
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status, length(body_json) FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND status NOT IN ('applied','preserved') ORDER BY entry_index LIMIT ?3",
            )
            .map_err(storage_error)?;
        let mut rows = statement
            .query(params![
                self.workspace_id.to_string(),
                stream_id.to_string(),
                sql_limit
            ])
            .map_err(storage_error)?;
        let mut items = Vec::new();
        let mut serialized_bytes = 0_usize;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let body_bytes = bounded_body_len(row, 6, "stream_entries")?;
            let next_bytes = serialized_bytes.checked_add(body_bytes);
            let exceeds_byte_budget = next_bytes
                .map(|next| next > max_serialized_bytes)
                .unwrap_or(true);
            if exceeds_byte_budget && (!items.is_empty() || !allow_oversized_first_item) {
                return Ok(BoundedPage {
                    items,
                    deferred_by_byte_budget: true,
                });
            }
            items.push(
                row_to_stream_entry(row).map_err(|_| corrupt("stream_entries", "body_json"))?,
            );
            serialized_bytes = next_bytes.ok_or(SyncError::ResourceLimit {
                resource: "stream_page_bytes",
            })?;
            if serialized_bytes > max_serialized_bytes {
                break;
            }
        }
        Ok(BoundedPage {
            items,
            deferred_by_byte_budget: false,
        })
    }

    pub fn advance_stream_index(&mut self, next_index: u32) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        advance_stream_index_tx(&transaction, self.workspace_id, next_index)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn put_stream_event(
        &mut self,
        event: &WorkspaceEventMessage,
        status: StreamItemStatus,
    ) -> Result<StreamRevisionItemRecord, SyncError> {
        event.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_stream_event",
        })?;
        if event.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let body_json = canonical_json(event)?;
        let body_digest = body_digest(&body_json);
        self.put_stream_revision_item(
            event.stream_id,
            event.revision,
            StreamRevisionItemKind::Event,
            Some(event.index),
            body_json,
            body_digest,
            status,
        )
    }

    pub fn put_stream_conflict_resolved(
        &mut self,
        message: &WorkspaceConflictResolvedMessage,
        event_index: Option<u32>,
        status: StreamItemStatus,
    ) -> Result<StreamRevisionItemRecord, SyncError> {
        message.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_stream_conflict_resolution",
        })?;
        if message.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let body_json = canonical_json(message)?;
        let body_digest = body_digest(&body_json);
        let stream_id = self
            .stream_state()?
            .ok_or(SyncError::StreamInvariant {
                reason: "no_active_stream",
            })?
            .stream_id;
        self.put_stream_revision_item(
            stream_id,
            message.revision,
            StreamRevisionItemKind::ConflictResolved,
            event_index,
            body_json,
            body_digest,
            status,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_stream_revision_item(
        &mut self,
        stream_id: StreamId,
        revision: WorkspaceRevision,
        item_kind: StreamRevisionItemKind,
        event_index: Option<u32>,
        body_json: Vec<u8>,
        stored_digest: [u8; 32],
        status: StreamItemStatus,
    ) -> Result<StreamRevisionItemRecord, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let result = put_stream_revision_item_tx(
            &transaction,
            self.workspace_id,
            stream_id,
            revision,
            item_kind,
            event_index,
            body_json,
            stored_digest,
            status,
        )?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn stream_revision_item(
        &self,
        stream_id: StreamId,
        revision: WorkspaceRevision,
    ) -> Result<Option<StreamRevisionItemRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 AND revision = ?3",
                params![self.workspace_id.to_string(), stream_id.to_string(), revision.to_string()],
                row_to_stream_revision_item,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn stream_revision_items(
        &self,
        stream_id: StreamId,
    ) -> Result<Vec<StreamRevisionItemRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![self.workspace_id.to_string(), stream_id.to_string()],
                row_to_stream_revision_item,
            )
            .map_err(storage_error)?;
        let mut items = rows
            .map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()?;
        items.sort_by_key(|item| item.revision);
        Ok(items)
    }

    pub(crate) fn pending_stream_revision_items_page(
        &self,
        stream_id: StreamId,
        max_items: usize,
        max_serialized_bytes: usize,
        allow_oversized_first_item: bool,
    ) -> Result<BoundedPage<StreamRevisionItemRecord>, SyncError> {
        if max_items == 0 {
            return Ok(BoundedPage {
                items: Vec::new(),
                deferred_by_byte_budget: false,
            });
        }
        let sql_limit = i64::try_from(max_items).map_err(|_| SyncError::InvalidConfiguration {
            reason: "page_item_limit_too_large",
        })?;
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status, length(body_json) FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 AND status NOT IN ('applied','preserved') ORDER BY length(revision), revision LIMIT ?3",
            )
            .map_err(storage_error)?;
        let mut rows = statement
            .query(params![
                self.workspace_id.to_string(),
                stream_id.to_string(),
                sql_limit
            ])
            .map_err(storage_error)?;
        let mut items = Vec::new();
        let mut serialized_bytes = 0_usize;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let body_bytes = bounded_body_len(row, 8, "stream_revision_items")?;
            let next_bytes = serialized_bytes.checked_add(body_bytes);
            let exceeds_byte_budget = next_bytes
                .map(|next| next > max_serialized_bytes)
                .unwrap_or(true);
            if exceeds_byte_budget && (!items.is_empty() || !allow_oversized_first_item) {
                return Ok(BoundedPage {
                    items,
                    deferred_by_byte_budget: true,
                });
            }
            items.push(
                row_to_stream_revision_item(row)
                    .map_err(|_| corrupt("stream_revision_items", "body_json"))?,
            );
            serialized_bytes = next_bytes.ok_or(SyncError::ResourceLimit {
                resource: "stream_page_bytes",
            })?;
            if serialized_bytes > max_serialized_bytes {
                break;
            }
        }
        Ok(BoundedPage {
            items,
            deferred_by_byte_budget: false,
        })
    }

    pub(crate) fn stream_table_summary(
        &self,
        stream_id: StreamId,
    ) -> Result<StreamTableSummary, SyncError> {
        let workspace_id = self.workspace_id.to_string();
        let stream_id = stream_id.to_string();
        let counts = self
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2), (SELECT COUNT(*) FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND status NOT IN ('applied','preserved')), (SELECT COUNT(*) FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2), (SELECT COUNT(*) FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 AND status NOT IN ('applied','preserved')), (SELECT COUNT(*) FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2)",
                params![workspace_id, stream_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        Ok(StreamTableSummary {
            entry_count: bounded_count(counts.0, "stream_entries")?,
            pending_entry_count: bounded_count(counts.1, "stream_entries")?,
            revision_count: bounded_count(counts.2, "stream_revision_items")?,
            pending_revision_count: bounded_count(counts.3, "stream_revision_items")?,
            conflict_count: bounded_count(counts.4, "stream_conflicts")?,
        })
    }

    pub(crate) fn snapshot_stream_contains_path(
        &self,
        stream_id: StreamId,
        path: &WorkspacePath,
    ) -> Result<bool, SyncError> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND json_extract(CAST(body_json AS TEXT), '$.entry.path') = ?3)",
                params![
                    self.workspace_id.to_string(),
                    stream_id.to_string(),
                    path.as_str()
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)
    }

    pub(crate) fn has_apply_journals(&self) -> Result<bool, SyncError> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM apply_journal WHERE workspace_id = ?1)",
                params![self.workspace_id.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)
    }

    pub fn put_stream_conflict(
        &mut self,
        message: &WorkspaceConflictCreatedMessage,
        status: StreamConflictStatus,
        stream_id: StreamId,
    ) -> Result<StreamConflictRecord, SyncError> {
        message.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_stream_conflict",
        })?;
        if message.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let created_json = canonical_json(message)?;
        let conflict_revision = message.conflict_revision;
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let active = active_stream_state(&transaction, self.workspace_id)?.ok_or(
            SyncError::StreamInvariant {
                reason: "no_active_stream",
            },
        )?;
        if active.stream_id != stream_id {
            return Err(SyncError::StreamInvariant {
                reason: "active_stream_mismatch",
            });
        }
        let existing = transaction
            .query_row(
                "SELECT workspace_id, stream_id, conflict_id, conflict_revision, created_json, status FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2 AND conflict_id = ?3",
                params![self.workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string()],
                row_to_stream_conflict,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.created_json != created_json {
                return Err(SyncError::OperationChanged);
            }
            if matches!(
                existing.status,
                StreamConflictStatus::Replaced | StreamConflictStatus::Pruned
            ) {
                transaction.commit().map_err(storage_error)?;
                return Ok(existing);
            }
            if !stream_conflict_status_transition_allowed(existing.status, status) {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_conflict_status_regression",
                });
            }
            transaction
                .execute(
                    "UPDATE stream_conflicts SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND conflict_id = ?4",
                    params![status.as_str(), self.workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string()],
                )
                .map_err(storage_error)?;
        } else {
            let count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2",
                    params![self.workspace_id.to_string(), stream_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            let count =
                u32::try_from(count).map_err(|_| corrupt("stream_conflicts", "conflict_id"))?;
            if count >= active.expected_conflict_count {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_conflict_count_exceeded",
                });
            }
            transaction
                .execute(
                    "INSERT INTO stream_conflicts (workspace_id, stream_id, conflict_id, conflict_revision, created_json, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![self.workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string(), conflict_revision_string(&conflict_revision)?, created_json.clone(), status.as_str()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(StreamConflictRecord {
            workspace_id: self.workspace_id,
            stream_id,
            conflict_id: message.conflict_id,
            conflict_revision,
            created_json,
            status,
        })
    }

    pub fn stream_conflicts(
        &self,
        stream_id: StreamId,
    ) -> Result<Vec<StreamConflictRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT workspace_id, stream_id, conflict_id, conflict_revision, created_json, status FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2 ORDER BY conflict_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![self.workspace_id.to_string(), stream_id.to_string()],
                row_to_stream_conflict,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn replace_authoritative_conflicts(
        &mut self,
        stream_id: StreamId,
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        replace_authoritative_conflicts_tx(&transaction, self.workspace_id, stream_id)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn put_apply_journal(&mut self, record: &ApplyJournalRecord) -> Result<(), SyncError> {
        if record.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "apply_workspace_mismatch",
            });
        }
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        put_apply_journal_tx(&transaction, self.workspace_id, record)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn insert_apply_journal(&mut self, record: &ApplyJournalRecord) -> Result<(), SyncError> {
        self.put_apply_journal(record)
    }

    pub fn apply_journal(
        &self,
        apply_id: ApplyId,
    ) -> Result<Option<ApplyJournalRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, apply_namespace, operation_body_digest, operation_json, filesystem_operation_json, commit_json, preimage_json, postimage_json, filesystem_receipt_json, stage FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
                params![apply_id.0.to_string(), self.workspace_id.to_string()],
                row_to_apply_journal,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn apply_journals(&self) -> Result<Vec<ApplyJournalRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, apply_namespace, operation_body_digest, operation_json, filesystem_operation_json, commit_json, preimage_json, postimage_json, filesystem_receipt_json, stage FROM apply_journal WHERE workspace_id = ?1 ORDER BY apply_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.workspace_id.to_string()], row_to_apply_journal)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn set_apply_stage(
        &mut self,
        apply_id: ApplyId,
        stage: ApplyStage,
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        set_apply_stage_tx(&transaction, self.workspace_id, apply_id, stage)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn set_apply_filesystem_applied(
        &mut self,
        apply_id: ApplyId,
        receipt_json: &[u8],
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        set_apply_filesystem_applied_tx(&transaction, self.workspace_id, apply_id, receipt_json)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn remove_apply_journal(&mut self, apply_id: ApplyId) -> Result<(), SyncError> {
        self.conn
            .execute(
                "DELETE FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
                params![apply_id.0.to_string(), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn record_applied_operation(
        &mut self,
        origin_client_id: ClientId,
        operation_id: OperationId,
        revision: WorkspaceRevision,
        body_digest: [u8; 32],
        receipt_kind: AppliedOperationReceiptKind,
        mutation_json: Option<&[u8]>,
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        record_applied_operation_tx(
            &transaction,
            AppliedOperationWrite {
                origin_client_id,
                operation_id,
                revision,
                body_digest,
                receipt_kind,
                mutation_json,
                legacy_body_digest: None,
            },
        )?;
        transaction.commit().map_err(storage_error)
    }

    pub fn applied_operation(
        &self,
        origin_client_id: ClientId,
        operation_id: OperationId,
    ) -> Result<Option<AppliedOperationRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
                params![origin_client_id.to_string(), operation_id.to_string()],
                row_to_applied_operation,
            )
            .optional()
            .map_err(applied_operation_read_error)
    }

    pub fn applied_operations(&self) -> Result<Vec<AppliedOperationRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json FROM applied_operations ORDER BY origin_client_id, operation_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], row_to_applied_operation)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(applied_operation_read_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub(crate) fn provisional_mutation_acceptance(
        &self,
        origin_client_id: ClientId,
        operation_id: OperationId,
    ) -> Result<Option<WorkspaceMutationAcceptedMessage>, SyncError> {
        self.conn
            .query_row(
                "SELECT origin_client_id, operation_id, workspace_id, revision, accepted_json, accepted_digest FROM provisional_mutation_acceptances WHERE origin_client_id = ?1 AND operation_id = ?2",
                params![origin_client_id.to_string(), operation_id.to_string()],
                row_to_provisional_mutation_acceptance,
            )
            .optional()
            .map_err(provisional_acceptance_read_error)
    }

    pub(crate) fn provisional_mutation_acceptances(
        &self,
        revision: WorkspaceRevision,
    ) -> Result<Vec<WorkspaceMutationAcceptedMessage>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT origin_client_id, operation_id, workspace_id, revision, accepted_json, accepted_digest FROM provisional_mutation_acceptances WHERE revision = ?1 ORDER BY origin_client_id, operation_id LIMIT 2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![revision.to_string()],
                row_to_provisional_mutation_acceptance,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(provisional_acceptance_read_error))
            .collect()
    }

    pub(crate) fn mutation_receipts_at_revision(
        &self,
        revision: WorkspaceRevision,
    ) -> Result<Vec<AppliedOperationRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json FROM applied_operations WHERE revision = ?1 AND receipt_kind = 'mutation_result' ORDER BY origin_client_id, operation_id LIMIT 2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![revision.to_string()], row_to_applied_operation)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(applied_operation_read_error))
            .collect()
    }

    pub fn put_conflict(&mut self, conflict: &ConflictRecord) -> Result<(), SyncError> {
        if conflict.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "conflict_workspace_mismatch",
            });
        }
        let created: fns_protocol::WorkspaceConflictCreatedMessage =
            parse_json_safe(&conflict.created_json, "conflicts", "created_json")?;
        created
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict",
            })?;
        if created.workspace_id != conflict.workspace_id
            || created.conflict_id != conflict.conflict_id
            || created.conflict_revision != conflict.conflict_revision
        {
            return Err(SyncError::ProtocolInvariant {
                reason: "conflict_identity_mismatch",
            });
        }
        if let Some(candidate_hash) = &conflict.candidate_hash {
            WorkspaceContentHash::parse(candidate_hash)
                .map_err(|_| corrupt("conflicts", "candidate_hash"))?;
        }
        if let Some(resolution_digest) = conflict.resolution_digest {
            if let Some(resolution_json) = &conflict.resolution_json {
                if resolution_digest != body_digest(resolution_json) {
                    return Err(SyncError::OperationChanged);
                }
            } else {
                return Err(corrupt("conflicts", "resolution_digest"));
            }
        }
        self.conn
            .execute(
                "INSERT INTO conflicts (conflict_id, workspace_id, conflict_revision, created_json, status, candidate_hash, resolution_json, resolution_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(conflict_id) DO UPDATE SET workspace_id = excluded.workspace_id, conflict_revision = excluded.conflict_revision, created_json = excluded.created_json, status = excluded.status, candidate_hash = excluded.candidate_hash, resolution_json = excluded.resolution_json, resolution_digest = excluded.resolution_digest",
                params![
                    conflict.conflict_id.to_string(),
                    conflict.workspace_id.to_string(),
                    conflict_revision_string(&conflict.conflict_revision)?,
                    conflict.created_json,
                    conflict.status.as_str(),
                    conflict.candidate_hash,
                    conflict.resolution_json,
                    conflict.resolution_digest.map(|digest| digest.to_vec()),
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn record_conflict(
        &mut self,
        message: &WorkspaceConflictCreatedMessage,
        status: ConflictStatus,
    ) -> Result<ConflictRecord, SyncError> {
        message
            .validate()
            .map_err(|_| SyncError::ProtocolInvariant {
                reason: "invalid_conflict",
            })?;
        if message.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "conflict_workspace_mismatch",
            });
        }
        let record = ConflictRecord {
            conflict_id: message.conflict_id,
            workspace_id: message.workspace_id,
            conflict_revision: message.conflict_revision,
            created_json: canonical_json(message)?,
            status,
            candidate_hash: None,
            resolution_json: None,
            resolution_digest: None,
        };
        self.put_conflict(&record)?;
        Ok(record)
    }

    pub fn conflict(&self, conflict_id: ConflictId) -> Result<Option<ConflictRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT conflict_id, workspace_id, conflict_revision, created_json, status, candidate_hash, resolution_json, resolution_digest FROM conflicts WHERE conflict_id = ?1",
                params![conflict_id.to_string()],
                row_to_conflict,
            )
            .optional()
            .map_err(conflict_read_error)
    }

    pub fn conflicts(&self) -> Result<Vec<ConflictRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT conflict_id, workspace_id, conflict_revision, created_json, status, candidate_hash, resolution_json, resolution_digest FROM conflicts WHERE workspace_id = ?1 ORDER BY conflict_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.workspace_id.to_string()], row_to_conflict)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(conflict_read_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn set_conflict_status(
        &mut self,
        conflict_id: ConflictId,
        status: ConflictStatus,
    ) -> Result<(), SyncError> {
        self.conn
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

    pub fn row_count(&self, table: &str) -> Result<usize, SyncError> {
        let counts = self.row_counts()?;
        counts
            .get(table)
            .copied()
            .ok_or(SyncError::InvalidConfiguration {
                reason: "unsupported_table",
            })
    }

    pub fn state_digest<T: Serialize>(value: &T) -> Result<[u8; 32], SyncError> {
        digest(value)
    }

    pub fn operation_digest<T: Serialize>(value: &T) -> Result<[u8; 32], SyncError> {
        digest(value)
    }
}

pub(crate) fn put_apply_journal_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    record: &ApplyJournalRecord,
) -> Result<(), SyncError> {
    if record.workspace_id != workspace_id {
        return Err(SyncError::ProtocolInvariant {
            reason: "apply_workspace_mismatch",
        });
    }
    let existing_workspace: Option<String> = transaction
        .query_row(
            "SELECT workspace_id FROM apply_journal WHERE apply_id = ?1",
            params![record.apply_id.0.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing_workspace) = existing_workspace
        && existing_workspace != workspace_id.to_string()
    {
        return Err(SyncError::OperationChanged);
    }
    let existing = transaction
        .query_row(
            "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, apply_namespace, operation_body_digest, operation_json, filesystem_operation_json, commit_json, preimage_json, postimage_json, filesystem_receipt_json, stage FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
            params![record.apply_id.0.to_string(), workspace_id.to_string()],
            row_to_apply_journal,
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        if existing.workspace_id != record.workspace_id
            || existing.stream_id != record.stream_id
            || existing.item_kind != record.item_kind
            || existing.apply_namespace != record.apply_namespace
            || existing.operation_body_digest != record.operation_body_digest
            || existing.operation_json != record.operation_json
            || existing.filesystem_operation_json != record.filesystem_operation_json
            || existing.commit_json != record.commit_json
            || existing.preimage_json != record.preimage_json
            || existing.postimage_json != record.postimage_json
            || existing.filesystem_receipt_json != record.filesystem_receipt_json
            || existing.item_key != record.item_key
        {
            return Err(SyncError::OperationChanged);
        }
        if !apply_stage_transition_allowed(existing.stage, record.stage) {
            return Err(SyncError::ProtocolInvariant {
                reason: "apply_stage_regression",
            });
        }
        transaction
            .execute(
                "UPDATE apply_journal SET stage = ?1 WHERE apply_id = ?2 AND workspace_id = ?3",
                params![
                    record.stage.as_str(),
                    record.apply_id.0.to_string(),
                    workspace_id.to_string(),
                ],
            )
            .map_err(storage_error)?;
    } else {
        let conflicting_identity: Option<String> = transaction
            .query_row(
                "SELECT apply_id FROM apply_journal WHERE workspace_id = ?1 AND stream_id = ?2 AND item_kind = ?3 AND item_key = ?4",
                params![
                    record.workspace_id.to_string(),
                    record.stream_id.to_string(),
                    record.item_kind.as_str(),
                    record.item_key,
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if conflicting_identity.is_some() {
            return Err(SyncError::OperationChanged);
        }
        transaction
            .execute(
                "INSERT INTO apply_journal (apply_id, workspace_id, stream_id, item_kind, item_key, apply_namespace, operation_body_digest, operation_json, filesystem_operation_json, commit_json, preimage_json, postimage_json, filesystem_receipt_json, stage) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![record.apply_id.0.to_string(), record.workspace_id.to_string(), record.stream_id.to_string(), record.item_kind.as_str(), record.item_key, record.apply_namespace.as_str(), record.operation_body_digest.as_slice(), record.operation_json, record.filesystem_operation_json, record.commit_json, record.preimage_json, record.postimage_json, record.filesystem_receipt_json, record.stage.as_str()],
            )
            .map_err(storage_error)?;
    }
    Ok(())
}

pub(crate) fn set_apply_filesystem_applied_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    apply_id: ApplyId,
    receipt_json: &[u8],
) -> Result<(), SyncError> {
    let current = transaction
        .query_row(
            "SELECT stage, filesystem_receipt_json FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
            params![apply_id.0.to_string(), workspace_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .optional()
        .map_err(storage_error)?
        .ok_or(SyncError::ProtocolInvariant {
            reason: "apply_journal_not_found",
        })?;
    let stage = ApplyStage::parse(&current.0).ok_or(corrupt("apply_journal", "stage"))?;
    if stage == ApplyStage::FilesystemApplied {
        return if current.1.as_deref() == Some(receipt_json) {
            Ok(())
        } else {
            Err(SyncError::OperationChanged)
        };
    }
    if stage != ApplyStage::FilesystemStarted {
        return Err(SyncError::ProtocolInvariant {
            reason: "apply_stage_regression",
        });
    }
    transaction
        .execute(
            "UPDATE apply_journal SET filesystem_receipt_json = ?1, stage = 'filesystem_applied' WHERE apply_id = ?2 AND workspace_id = ?3 AND stage = 'filesystem_started'",
            params![receipt_json, apply_id.0.to_string(), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn set_apply_stage_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    apply_id: ApplyId,
    stage: ApplyStage,
) -> Result<(), SyncError> {
    let (current, receipt): (String, Option<Vec<u8>>) = transaction
        .query_row(
            "SELECT stage, filesystem_receipt_json FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
            params![apply_id.0.to_string(), workspace_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?
        .ok_or(SyncError::ProtocolInvariant {
            reason: "apply_journal_not_found",
        })?;
    let current = ApplyStage::parse(&current).ok_or(corrupt("apply_journal", "stage"))?;
    if matches!(stage, ApplyStage::DatabaseCommitted | ApplyStage::Finalized) && receipt.is_none() {
        return Err(SyncError::ProtocolInvariant {
            reason: "apply_receipt_missing",
        });
    }
    if !apply_stage_transition_allowed(current, stage) {
        return Err(SyncError::ProtocolInvariant {
            reason: "apply_stage_regression",
        });
    }
    let changed = transaction
        .execute(
            "UPDATE apply_journal SET stage = ?1 WHERE apply_id = ?2 AND workspace_id = ?3",
            params![
                stage.as_str(),
                apply_id.0.to_string(),
                workspace_id.to_string()
            ],
        )
        .map_err(storage_error)?;
    if changed == 0 {
        return Err(SyncError::ProtocolInvariant {
            reason: "apply_journal_not_found",
        });
    }
    Ok(())
}

pub(crate) fn remove_apply_journal_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    apply_id: ApplyId,
) -> Result<(), SyncError> {
    transaction
        .execute(
            "DELETE FROM apply_journal WHERE apply_id = ?1 AND workspace_id = ?2",
            params![apply_id.0.to_string(), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn record_applied_operation_tx(
    transaction: &rusqlite::Transaction<'_>,
    write: AppliedOperationWrite<'_>,
) -> Result<(), SyncError> {
    validate_applied_operation_write(
        write.origin_client_id,
        write.operation_id,
        write.receipt_kind,
        write.mutation_json,
    )?;
    let existing = transaction
        .query_row(
            "SELECT origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
            params![write.origin_client_id.to_string(), write.operation_id.to_string()],
            row_to_applied_operation,
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        if existing.revision == write.revision
            && existing.body_digest == write.body_digest
            && existing.receipt_kind == write.receipt_kind
            && existing.mutation_json.as_deref() == write.mutation_json
        {
            return Ok(());
        }
        if existing.receipt_kind == AppliedOperationReceiptKind::Legacy
            && existing.revision == write.revision
            && write.legacy_body_digest == Some(existing.body_digest)
        {
            let changed = transaction
                .execute(
                    "UPDATE applied_operations SET body_digest = ?1, receipt_kind = ?2, mutation_json = ?3 WHERE origin_client_id = ?4 AND operation_id = ?5 AND revision = ?6 AND receipt_kind = 'legacy' AND body_digest = ?7",
                    params![
                        write.body_digest.as_slice(),
                        write.receipt_kind.as_str(),
                        write.mutation_json,
                        write.origin_client_id.to_string(),
                        write.operation_id.to_string(),
                        write.revision.to_string(),
                        existing.body_digest.as_slice(),
                    ],
                )
                .map_err(storage_error)?;
            if changed != 1 {
                return Err(SyncError::OperationChanged);
            }
            return Ok(());
        }
        return Err(SyncError::OperationChanged);
    } else {
        transaction
            .execute(
                "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    write.origin_client_id.to_string(),
                    write.operation_id.to_string(),
                    write.revision.to_string(),
                    write.body_digest.as_slice(),
                    write.receipt_kind.as_str(),
                    write.mutation_json,
                ],
            )
            .map_err(storage_error)?;
    }
    Ok(())
}

pub(crate) fn record_provisional_mutation_acceptance_tx(
    transaction: &rusqlite::Transaction<'_>,
    accepted: &WorkspaceMutationAcceptedMessage,
) -> Result<(), SyncError> {
    let receipt = transaction
        .query_row(
            "SELECT origin_client_id, operation_id, revision, body_digest, receipt_kind, mutation_json FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
            params![accepted.client_id.to_string(), accepted.operation_id.to_string()],
            row_to_applied_operation,
        )
        .optional()
        .map_err(applied_operation_read_error)?
        .ok_or(SyncError::ProtocolInvariant {
            reason: "provisional_receipt_missing",
        })?;
    if receipt.receipt_kind != AppliedOperationReceiptKind::Legacy
        || receipt.revision != accepted.revision
    {
        return Err(SyncError::OperationChanged);
    }
    let accepted_json = canonical_json(accepted)?;
    let accepted_digest = body_digest(&accepted_json);
    let existing: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT accepted_json FROM provisional_mutation_acceptances WHERE origin_client_id = ?1 AND operation_id = ?2",
            params![accepted.client_id.to_string(), accepted.operation_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        return if existing == accepted_json {
            Ok(())
        } else {
            Err(SyncError::OperationChanged)
        };
    }
    transaction
        .execute(
            "INSERT INTO provisional_mutation_acceptances (origin_client_id, operation_id, workspace_id, revision, accepted_json, accepted_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                accepted.client_id.to_string(),
                accepted.operation_id.to_string(),
                accepted.workspace_id.to_string(),
                accepted.revision.to_string(),
                accepted_json,
                accepted_digest.as_slice(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn remove_provisional_mutation_acceptance_tx(
    transaction: &rusqlite::Transaction<'_>,
    origin_client_id: ClientId,
    operation_id: OperationId,
) -> Result<(), SyncError> {
    transaction
        .execute(
            "DELETE FROM provisional_mutation_acceptances WHERE origin_client_id = ?1 AND operation_id = ?2",
            params![origin_client_id.to_string(), operation_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn applied_operation_read_error(error: rusqlite::Error) -> SyncError {
    match error {
        rusqlite::Error::InvalidQuery => corrupt("applied_operations", "receipt"),
        _ => storage_error(error),
    }
}

fn validate_applied_operation_write(
    origin_client_id: ClientId,
    operation_id: OperationId,
    receipt_kind: AppliedOperationReceiptKind,
    mutation_json: Option<&[u8]>,
) -> Result<(), SyncError> {
    match receipt_kind {
        AppliedOperationReceiptKind::Legacy => {
            return Err(SyncError::InvalidConfiguration {
                reason: "legacy_receipt_write_forbidden",
            });
        }
        AppliedOperationReceiptKind::ConflictResolution if mutation_json.is_some() => {
            return Err(SyncError::InvalidConfiguration {
                reason: "conflict_receipt_has_mutation",
            });
        }
        AppliedOperationReceiptKind::ConflictResolution => return Ok(()),
        AppliedOperationReceiptKind::MutationResult => {}
    }
    let mutation_json = mutation_json.ok_or(SyncError::InvalidConfiguration {
        reason: "mutation_receipt_missing_mutation",
    })?;
    let mutation: WorkspaceMutation =
        serde_json::from_slice(mutation_json).map_err(|_| SyncError::InvalidConfiguration {
            reason: "mutation_receipt_invalid_mutation",
        })?;
    mutation
        .validate()
        .map_err(|_| SyncError::InvalidConfiguration {
            reason: "mutation_receipt_invalid_mutation",
        })?;
    if mutation.client_id != origin_client_id || mutation.operation_id != operation_id {
        return Err(SyncError::InvalidConfiguration {
            reason: "mutation_receipt_identity_mismatch",
        });
    }
    if canonical_json(&mutation)? != mutation_json {
        return Err(SyncError::InvalidConfiguration {
            reason: "mutation_receipt_not_canonical",
        });
    }
    Ok(())
}

pub(crate) fn put_conflict_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    conflict: &ConflictRecord,
) -> Result<(), SyncError> {
    if conflict.workspace_id != workspace_id {
        return Err(SyncError::ProtocolInvariant {
            reason: "conflict_workspace_mismatch",
        });
    }
    let created: fns_protocol::WorkspaceConflictCreatedMessage =
        parse_json_safe(&conflict.created_json, "conflicts", "created_json")?;
    created
        .validate()
        .map_err(|_| SyncError::ProtocolInvariant {
            reason: "invalid_conflict",
        })?;
    if created.workspace_id != conflict.workspace_id
        || created.conflict_id != conflict.conflict_id
        || created.conflict_revision != conflict.conflict_revision
    {
        return Err(SyncError::ProtocolInvariant {
            reason: "conflict_identity_mismatch",
        });
    }
    if let Some(candidate_hash) = &conflict.candidate_hash {
        WorkspaceContentHash::parse(candidate_hash)
            .map_err(|_| corrupt("conflicts", "candidate_hash"))?;
    }
    if let Some(resolution_digest) = conflict.resolution_digest {
        if let Some(resolution_json) = &conflict.resolution_json {
            if resolution_digest != body_digest(resolution_json) {
                return Err(SyncError::OperationChanged);
            }
        } else {
            return Err(corrupt("conflicts", "resolution_digest"));
        }
    }
    transaction
        .execute(
            "INSERT INTO conflicts (conflict_id, workspace_id, conflict_revision, created_json, status, candidate_hash, resolution_json, resolution_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(conflict_id) DO UPDATE SET workspace_id = excluded.workspace_id, conflict_revision = excluded.conflict_revision, created_json = excluded.created_json, status = excluded.status, candidate_hash = excluded.candidate_hash, resolution_json = excluded.resolution_json, resolution_digest = excluded.resolution_digest",
            params![
                conflict.conflict_id.to_string(),
                conflict.workspace_id.to_string(),
                conflict_revision_string(&conflict.conflict_revision)?,
                conflict.created_json,
                conflict.status.as_str(),
                conflict.candidate_hash,
                conflict.resolution_json,
                conflict.resolution_digest.map(|digest| digest.to_vec()),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

impl HashCache for SqliteState {
    fn lookup(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
    ) -> Result<Option<WorkspaceContentHash>, HashCacheError> {
        let row = self
            .conn
            .query_row(
                "SELECT fingerprint_json, content_hash FROM hash_cache WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
                |row| {
                    let fingerprint_json: Vec<u8> = row.get(0)?;
                    let content_hash: String = row.get(1)?;
                    Ok((fingerprint_json, content_hash))
                },
            )
            .optional()
            .map_err(|_| HashCacheError::Io)?;
        let Some((fingerprint_json, content_hash)) = row else {
            return Ok(None);
        };
        let stored: FileFingerprint =
            serde_json::from_slice(&fingerprint_json).map_err(|_| HashCacheError::Invalid)?;
        if &stored != fingerprint {
            return Ok(None);
        }
        WorkspaceContentHash::parse(&content_hash)
            .map(Some)
            .map_err(|_| HashCacheError::Invalid)
    }

    fn store(
        &mut self,
        path: &WorkspacePath,
        fingerprint: &FileFingerprint,
        hash: &WorkspaceContentHash,
    ) -> Result<(), HashCacheError> {
        let fingerprint_json =
            serde_json::to_vec(fingerprint).map_err(|_| HashCacheError::Invalid)?;
        WorkspaceContentHash::parse(hash.as_str()).map_err(|_| HashCacheError::Invalid)?;
        self.conn
            .execute(
                "INSERT INTO hash_cache (workspace_id, path, fingerprint_json, content_hash) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(workspace_id, path) DO UPDATE SET fingerprint_json = excluded.fingerprint_json, content_hash = excluded.content_hash",
                params![self.workspace_id.to_string(), path.as_str(), fingerprint_json, hash.as_str()],
            )
            .map_err(|_| HashCacheError::Io)?;
        Ok(())
    }

    fn invalidate(&mut self, path: &WorkspacePath) -> Result<(), HashCacheError> {
        self.conn
            .execute(
                "DELETE FROM hash_cache WHERE workspace_id = ?1 AND path = ?2",
                params![self.workspace_id.to_string(), path.as_str()],
            )
            .map_err(|_| HashCacheError::Io)?;
        Ok(())
    }
}

pub(crate) fn put_path_state_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    state: &fns_protocol::WorkspacePathState,
) -> Result<(), SyncError> {
    state.validate().map_err(|_| SyncError::ProtocolInvariant {
        reason: "invalid_path_state",
    })?;
    let body_json = canonical_json(state)?;
    let state_digest = body_digest(&body_json);
    connection
        .execute(
            "INSERT INTO path_states (workspace_id, path, state_json, state_digest) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(workspace_id, path) DO UPDATE SET state_json = excluded.state_json, state_digest = excluded.state_digest",
            params![workspace_id.to_string(), state.path.as_str(), body_json, state_digest.as_slice()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn put_local_intent_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    path: &WorkspacePath,
    intent_json: &[u8],
    updated_at_ms: i64,
) -> Result<(), SyncError> {
    connection
        .execute(
            "INSERT INTO local_intents (workspace_id, path, intent_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(workspace_id, path) DO UPDATE SET intent_json = excluded.intent_json, updated_at_ms = excluded.updated_at_ms",
            params![workspace_id.to_string(), path.as_str(), intent_json, updated_at_ms],
        )
        .map_err(storage_error)?;
    Ok(())
}

impl SqliteState {
    fn enqueue_outbox_bytes(
        &mut self,
        operation_id: OperationId,
        workspace_id: WorkspaceId,
        body_json: Vec<u8>,
        created_at_ms: i64,
    ) -> Result<OutboxRecord, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let record = enqueue_outbox_tx(
            &transaction,
            self.client_id,
            operation_id,
            workspace_id,
            body_json,
            created_at_ms,
        )?;
        transaction.commit().map_err(storage_error)?;
        Ok(record)
    }
}

pub(crate) fn enqueue_outbox_tx(
    transaction: &rusqlite::Transaction<'_>,
    client_id: ClientId,
    operation_id: OperationId,
    workspace_id: WorkspaceId,
    body_json: Vec<u8>,
    created_at_ms: i64,
) -> Result<OutboxRecord, SyncError> {
    let body_digest = body_digest(&body_json);
    let existing = transaction
            .query_row(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![client_id.to_string(), operation_id.to_string()],
                row_to_outbox,
            )
            .optional()
            .map_err(storage_error)?;
    let record = if let Some(mut existing) = existing {
        if existing.workspace_id != workspace_id || existing.body_digest != body_digest {
            if existing.stage != OutboxStage::Queued {
                return Err(SyncError::OperationChanged);
            }
            transaction
                    .execute(
                        "UPDATE outbox SET workspace_id = ?1, body_json = ?2, body_digest = ?3, created_at_ms = ?4 WHERE client_id = ?5 AND operation_id = ?6 AND stage = 'queued'",
                        params![workspace_id.to_string(), body_json.clone(), body_digest.as_slice(), created_at_ms, client_id.to_string(), operation_id.to_string()],
                    )
                    .map_err(storage_error)?;
            existing.workspace_id = workspace_id;
            existing.body_json = body_json;
            existing.body_digest = body_digest;
            existing.created_at_ms = created_at_ms;
        }
        existing
    } else {
        transaction
                .execute(
                    "INSERT INTO outbox (client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6)",
                    params![client_id.to_string(), operation_id.to_string(), workspace_id.to_string(), body_json.clone(), body_digest.as_slice(), created_at_ms],
                )
                .map_err(storage_error)?;
        OutboxRecord {
            client_id,
            operation_id,
            workspace_id,
            body_json,
            body_digest,
            stage: OutboxStage::Queued,
            created_at_ms,
        }
    };
    Ok(record)
}

pub(crate) fn set_outbox_stage_tx(
    transaction: &rusqlite::Transaction<'_>,
    client_id: ClientId,
    operation_id: OperationId,
    stage: OutboxStage,
) -> Result<(), SyncError> {
    let current = transaction
        .query_row(
            "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
            params![client_id.to_string(), operation_id.to_string()],
            row_to_outbox,
        )
        .optional()
        .map_err(storage_error)?
        .ok_or(SyncError::ProtocolInvariant {
            reason: "outbox_not_found",
        })?;
    if !outbox_stage_transition_allowed(current.stage, stage) {
        return Err(SyncError::ProtocolInvariant {
            reason: "outbox_stage_regression",
        });
    }
    transaction
        .execute(
            "UPDATE outbox SET stage = ?1 WHERE client_id = ?2 AND operation_id = ?3",
            params![
                stage.as_str(),
                client_id.to_string(),
                operation_id.to_string()
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn row_to_path_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<PathStateRecord> {
    let workspace_raw: String = row.get(0)?;
    let path_raw: String = row.get(1)?;
    let body_json: Vec<u8> = row.get(2)?;
    let digest_raw: Vec<u8> = row.get(3)?;
    let workspace_id = parse_workspace_id(&workspace_raw)?;
    let path = parse_path(&path_raw)?;
    let state: fns_protocol::WorkspacePathState = parse_json(&body_json)?;
    state
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if state.path != path {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let state_digest = parse_digest(&digest_raw)?;
    if state_digest != body_digest(&body_json) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(PathStateRecord {
        workspace_id,
        path,
        state_json: body_json,
        state_digest,
        state,
    })
}

fn bounded_body_len(
    row: &rusqlite::Row<'_>,
    index: usize,
    table: &'static str,
) -> Result<usize, SyncError> {
    let raw = row.get::<_, i64>(index).map_err(storage_error)?;
    usize::try_from(raw).map_err(|_| corrupt(table, "body_json"))
}

fn bounded_count(raw: i64, table: &'static str) -> Result<u64, SyncError> {
    u64::try_from(raw).map_err(|_| corrupt(table, "count"))
}

fn row_to_outbox(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRecord> {
    let client_id = parse_client_id(&row.get::<_, String>(0)?)?;
    let operation_id = parse_operation_id(&row.get::<_, String>(1)?)?;
    let workspace_id = parse_workspace_id(&row.get::<_, String>(2)?)?;
    let body_json: Vec<u8> = row.get(3)?;
    let stored_digest = parse_digest(&row.get::<_, Vec<u8>>(4)?)?;
    let stage_raw: String = row.get(5)?;
    let stage = OutboxStage::parse(&stage_raw).ok_or(rusqlite::Error::InvalidQuery)?;
    let created_at_ms: i64 = row.get(6)?;
    if stored_digest != crate::store::body_digest(&body_json) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if let Ok(mutation) = serde_json::from_slice::<WorkspaceMutation>(&body_json) {
        mutation
            .validate()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if mutation.workspace_id != workspace_id
            || mutation.client_id != client_id
            || mutation.operation_id != operation_id
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
    } else if let Ok(resolution) =
        serde_json::from_slice::<WorkspaceConflictResolvedRequest>(&body_json)
    {
        resolution
            .validate()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if resolution.workspace_id != workspace_id || resolution.client_id != client_id {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if resolution.operation_id != operation_id {
            return Err(rusqlite::Error::InvalidQuery);
        }
    } else {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(OutboxRecord {
        client_id,
        operation_id,
        workspace_id,
        body_json,
        body_digest: stored_digest,
        stage,
        created_at_ms,
    })
}

fn row_to_local_intent(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalIntentRecord> {
    Ok(LocalIntentRecord {
        workspace_id: parse_workspace_id(&row.get::<_, String>(0)?)?,
        path: parse_path(&row.get::<_, String>(1)?)?,
        intent_json: row.get(2)?,
        updated_at_ms: row.get(3)?,
    })
}

fn row_to_stream_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<StreamStateRecord> {
    let entry_count: i64 = row.get(5)?;
    let event_count: i64 = row.get(6)?;
    let conflict_count: i64 = row.get(7)?;
    let next_index: i64 = row.get(8)?;
    let end_received: i64 = row.get(9)?;
    let mode_raw: String = row.get(2)?;
    let mode = parse_mode(&mode_raw)?;
    let from_revision = parse_revision(&row.get::<_, String>(3)?)?;
    let final_revision = parse_revision(&row.get::<_, String>(4)?)?;
    if final_revision < from_revision
        || (mode == WorkspaceSnapshotMode::Snapshot && event_count != 0)
        || (mode == WorkspaceSnapshotMode::Incremental && entry_count != 0)
        || next_index < 0
        || next_index > event_count
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(StreamStateRecord {
        workspace_id: parse_workspace_id(&row.get::<_, String>(0)?)?,
        stream_id: parse_stream_id(&row.get::<_, String>(1)?)?,
        mode,
        from_revision,
        final_revision,
        expected_entry_count: u32::try_from(entry_count)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        expected_event_count: u32::try_from(event_count)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        expected_conflict_count: u32::try_from(conflict_count)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        next_event_index: u32::try_from(next_index).map_err(|_| rusqlite::Error::InvalidQuery)?,
        end_received: match end_received {
            0 => false,
            1 => true,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
    })
}

fn stream_state_from_begin(begin: &WorkspaceSnapshotBeginMessage) -> StreamStateRecord {
    StreamStateRecord {
        workspace_id: begin.workspace_id,
        stream_id: begin.stream_id,
        mode: begin.mode,
        from_revision: begin.from_revision,
        final_revision: begin.final_revision,
        expected_entry_count: begin.entry_count,
        expected_event_count: begin.event_count,
        expected_conflict_count: begin.conflict_count,
        next_event_index: 0,
        end_received: false,
    }
}

pub(crate) fn active_stream_state(
    connection: &Connection,
    workspace_id: WorkspaceId,
) -> Result<Option<StreamStateRecord>, SyncError> {
    connection
        .query_row(
            "SELECT workspace_id, stream_id, mode, from_revision, final_revision, expected_entry_count, expected_event_count, expected_conflict_count, next_event_index, end_received FROM stream_state WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            row_to_stream_state,
        )
        .optional()
        .map_err(storage_error)
}

pub(crate) fn put_stream_state_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    state: &StreamStateRecord,
) -> Result<(), SyncError> {
    if state.final_revision < state.from_revision
        || (state.mode == WorkspaceSnapshotMode::Snapshot && state.expected_event_count != 0)
        || (state.mode == WorkspaceSnapshotMode::Incremental && state.expected_entry_count != 0)
        || state.next_event_index > state.expected_event_count
    {
        return Err(SyncError::StreamInvariant {
            reason: "invalid_stream_state",
        });
    }
    let existing =
        active_stream_state(connection, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if existing.stream_id != state.stream_id {
        return Err(SyncError::StreamInvariant {
            reason: "active_stream_mismatch",
        });
    }
    if existing.mode != state.mode
        || existing.from_revision != state.from_revision
        || existing.final_revision != state.final_revision
        || existing.expected_entry_count != state.expected_entry_count
        || existing.expected_event_count != state.expected_event_count
        || existing.expected_conflict_count != state.expected_conflict_count
    {
        return Err(SyncError::StreamInvariant {
            reason: "stream_header_changed",
        });
    }
    if state.next_event_index < existing.next_event_index
        || state.next_event_index > state.expected_event_count
        || (existing.end_received && !state.end_received)
    {
        return Err(SyncError::StreamInvariant {
            reason: "stream_progress_regression",
        });
    }
    connection
        .execute(
            "UPDATE stream_state SET next_event_index = ?1, end_received = ?2 WHERE workspace_id = ?3",
            params![
                i64::from(state.next_event_index),
                i64::from(state.end_received as u8),
                workspace_id.to_string()
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn set_stream_end_received_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    received: bool,
) -> Result<(), SyncError> {
    let existing =
        active_stream_state(connection, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if existing.end_received && !received {
        return Err(SyncError::StreamInvariant {
            reason: "stream_end_regression",
        });
    }
    connection
        .execute(
            "UPDATE stream_state SET end_received = ?1 WHERE workspace_id = ?2",
            params![i64::from(received as u8), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(crate) fn advance_stream_index_tx(
    connection: &Connection,
    workspace_id: WorkspaceId,
    next_index: u32,
) -> Result<(), SyncError> {
    let existing =
        active_stream_state(connection, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if next_index < existing.next_event_index || next_index > existing.expected_event_count {
        return Err(SyncError::StreamInvariant {
            reason: "stream_index_regression",
        });
    }
    connection
        .execute(
            "UPDATE stream_state SET next_event_index = ?1 WHERE workspace_id = ?2",
            params![i64::from(next_index), workspace_id.to_string()],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn stream_status_transition_allowed(from: StreamItemStatus, to: StreamItemStatus) -> bool {
    match from {
        StreamItemStatus::Received => true,
        StreamItemStatus::WaitingBlob => matches!(
            to,
            StreamItemStatus::WaitingBlob
                | StreamItemStatus::Ready
                | StreamItemStatus::Applied
                | StreamItemStatus::Preserved
        ),
        StreamItemStatus::Ready => matches!(
            to,
            StreamItemStatus::Ready | StreamItemStatus::Applied | StreamItemStatus::Preserved
        ),
        StreamItemStatus::Applied => matches!(to, StreamItemStatus::Applied),
        StreamItemStatus::Preserved => matches!(to, StreamItemStatus::Preserved),
    }
}

pub(crate) fn put_stream_entry_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    entry: &WorkspaceSnapshotEntryMessage,
    status: StreamItemStatus,
) -> Result<StreamEntryRecord, SyncError> {
    entry.validate().map_err(|_| SyncError::StreamInvariant {
        reason: "invalid_stream_entry",
    })?;
    if entry.workspace_id != workspace_id {
        return Err(SyncError::ProtocolInvariant {
            reason: "stream_workspace_mismatch",
        });
    }
    let body_json = canonical_json(entry)?;
    let body_digest = body_digest(&body_json);
    let active =
        active_stream_state(transaction, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if active.stream_id != entry.stream_id {
        return Err(SyncError::StreamInvariant {
            reason: "active_stream_mismatch",
        });
    }
    if active.mode != WorkspaceSnapshotMode::Snapshot {
        return Err(SyncError::StreamInvariant {
            reason: "entry_not_allowed_in_incremental",
        });
    }
    let existing = transaction
        .query_row(
            "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND entry_index = ?3",
            params![workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index)],
            row_to_stream_entry,
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        if existing.body_digest != body_digest {
            return Err(SyncError::OperationChanged);
        }
        if matches!(
            existing.status,
            StreamItemStatus::Applied | StreamItemStatus::Preserved
        ) {
            return Ok(existing);
        }
        if !stream_status_transition_allowed(existing.status, status) {
            return Err(SyncError::StreamInvariant {
                reason: "stream_status_regression",
            });
        }
        transaction
            .execute(
                "UPDATE stream_entries SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND entry_index = ?4",
                params![status.as_str(), workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index)],
            )
            .map_err(storage_error)?;
    } else {
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2",
                params![workspace_id.to_string(), entry.stream_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let expected_index =
            u32::try_from(count).map_err(|_| corrupt("stream_entries", "entry_index"))?;
        if entry.index != expected_index || entry.index >= active.expected_entry_count {
            return Err(SyncError::StreamInvariant {
                reason: "stream_entry_order",
            });
        }
        if let Some(previous) = transaction
            .query_row(
                "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 ORDER BY entry_index DESC LIMIT 1",
                params![workspace_id.to_string(), entry.stream_id.to_string()],
                row_to_stream_entry,
            )
            .optional()
            .map_err(storage_error)?
        {
            let previous_entry: WorkspaceSnapshotEntryMessage =
                serde_json::from_slice(&previous.body_json)
                    .map_err(|_| corrupt("stream_entries", "body_json"))?;
            if previous_entry.entry.path.as_str().as_bytes()
                >= entry.entry.path.as_str().as_bytes()
            {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_entry_path_order",
                });
            }
        }
        transaction
            .execute(
                "INSERT INTO stream_entries (workspace_id, stream_id, entry_index, body_json, body_digest, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index), body_json.clone(), body_digest.as_slice(), status.as_str()],
            )
            .map_err(storage_error)?;
    }
    Ok(StreamEntryRecord {
        workspace_id,
        stream_id: entry.stream_id,
        entry_index: entry.index,
        body_json,
        body_digest,
        status,
    })
}

fn stream_conflict_status_transition_allowed(
    from: StreamConflictStatus,
    to: StreamConflictStatus,
) -> bool {
    match from {
        StreamConflictStatus::Received => true,
        StreamConflictStatus::Replaced => matches!(to, StreamConflictStatus::Replaced),
        StreamConflictStatus::Pruned => matches!(to, StreamConflictStatus::Pruned),
    }
}

fn outbox_stage_transition_allowed(from: OutboxStage, to: OutboxStage) -> bool {
    match from {
        OutboxStage::Queued => true,
        OutboxStage::Dispatched => !matches!(to, OutboxStage::Queued),
        OutboxStage::AwaitingBlob => !matches!(to, OutboxStage::Queued),
        OutboxStage::BlockedConflict => !matches!(to, OutboxStage::Queued),
    }
}

fn apply_stage_transition_allowed(from: ApplyStage, to: ApplyStage) -> bool {
    matches!(
        (from, to),
        (
            ApplyStage::Prepared,
            ApplyStage::Prepared | ApplyStage::FilesystemStarted
        ) | (ApplyStage::FilesystemStarted, ApplyStage::FilesystemStarted)
            | (
                ApplyStage::FilesystemApplied,
                ApplyStage::FilesystemApplied | ApplyStage::DatabaseCommitted
            )
            | (
                ApplyStage::DatabaseCommitted,
                ApplyStage::DatabaseCommitted | ApplyStage::Finalized
            )
            | (ApplyStage::Finalized, ApplyStage::Finalized)
    )
}

fn stream_begin_matches(
    existing: &StreamStateRecord,
    begin: &WorkspaceSnapshotBeginMessage,
) -> bool {
    existing.workspace_id == begin.workspace_id
        && existing.stream_id == begin.stream_id
        && existing.mode == begin.mode
        && existing.from_revision == begin.from_revision
        && existing.final_revision == begin.final_revision
        && existing.expected_entry_count == begin.entry_count
        && existing.expected_event_count == begin.event_count
        && existing.expected_conflict_count == begin.conflict_count
}

type CanonicalRevisionItem = (Vec<u8>, [u8; 32], Option<u32>);

fn canonicalize_revision_item(
    workspace_id: WorkspaceId,
    stream_id: StreamId,
    revision: WorkspaceRevision,
    item_kind: StreamRevisionItemKind,
    event_index: Option<u32>,
    body_json: &[u8],
) -> Result<CanonicalRevisionItem, SyncError> {
    match item_kind {
        StreamRevisionItemKind::Event => {
            let event: WorkspaceEventMessage =
                serde_json::from_slice(body_json).map_err(|_| SyncError::ProtocolInvariant {
                    reason: "invalid_stream_event",
                })?;
            event.validate().map_err(|_| SyncError::StreamInvariant {
                reason: "invalid_stream_event",
            })?;
            if event.workspace_id != workspace_id
                || event.stream_id != stream_id
                || event.revision != revision
                || event_index != Some(event.index)
            {
                return Err(SyncError::ProtocolInvariant {
                    reason: "stream_item_identity_mismatch",
                });
            }
            let canonical = canonical_json(&event)?;
            let digest = body_digest(&canonical);
            Ok((canonical, digest, Some(event.index)))
        }
        StreamRevisionItemKind::ConflictResolved => {
            let message: WorkspaceConflictResolvedMessage = serde_json::from_slice(body_json)
                .map_err(|_| SyncError::ProtocolInvariant {
                    reason: "invalid_stream_conflict_resolution",
                })?;
            message.validate().map_err(|_| SyncError::StreamInvariant {
                reason: "invalid_stream_conflict_resolution",
            })?;
            if message.workspace_id != workspace_id || message.revision != revision {
                return Err(SyncError::ProtocolInvariant {
                    reason: "stream_item_identity_mismatch",
                });
            }
            let canonical = canonical_json(&message)?;
            let digest = body_digest(&canonical);
            Ok((canonical, digest, None))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn put_stream_revision_item_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    stream_id: StreamId,
    revision: WorkspaceRevision,
    item_kind: StreamRevisionItemKind,
    event_index: Option<u32>,
    body_json: Vec<u8>,
    stored_digest: [u8; 32],
    status: StreamItemStatus,
) -> Result<StreamRevisionItemRecord, SyncError> {
    if stored_digest != body_digest(&body_json) {
        return Err(SyncError::ProtocolInvariant {
            reason: "stream_body_digest_mismatch",
        });
    }
    let (body_json, body_digest, event_index) = canonicalize_revision_item(
        workspace_id,
        stream_id,
        revision,
        item_kind,
        event_index,
        &body_json,
    )?;
    let active =
        active_stream_state(transaction, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if active.stream_id != stream_id {
        return Err(SyncError::StreamInvariant {
            reason: "active_stream_mismatch",
        });
    }
    if active.mode != WorkspaceSnapshotMode::Incremental {
        return Err(SyncError::StreamInvariant {
            reason: "revision_item_not_allowed_in_snapshot",
        });
    }
    let existing = transaction
        .query_row(
            "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 AND revision = ?3",
            params![workspace_id.to_string(), stream_id.to_string(), revision.to_string()],
            row_to_stream_revision_item,
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        if existing.event_index != event_index {
            return Err(SyncError::StreamInvariant {
                reason: "stream_revision_order",
            });
        }
        if existing.body_digest != body_digest
            || existing.item_kind != item_kind
            || existing.event_index != event_index
        {
            return Err(SyncError::OperationChanged);
        }
        if matches!(
            existing.status,
            StreamItemStatus::Applied | StreamItemStatus::Preserved
        ) {
            return Ok(existing);
        }
        if !stream_status_transition_allowed(existing.status, status) {
            return Err(SyncError::StreamInvariant {
                reason: "stream_status_regression",
            });
        }
        transaction
            .execute(
                "UPDATE stream_revision_items SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND revision = ?4",
                params![status.as_str(), workspace_id.to_string(), stream_id.to_string(), revision.to_string()],
            )
            .map_err(storage_error)?;
    } else {
        let item_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2",
                params![workspace_id.to_string(), stream_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let item_count =
            u32::try_from(item_count).map_err(|_| corrupt("stream_revision_items", "revision"))?;
        if item_count >= active.expected_event_count {
            return Err(SyncError::StreamInvariant {
                reason: "stream_event_count_exceeded",
            });
        }
        if revision <= active.from_revision || revision > active.final_revision {
            return Err(SyncError::StreamInvariant {
                reason: "stream_revision_out_of_range",
            });
        }
        let previous_revision = {
            let mut statement = transaction
                .prepare(
                    "SELECT revision FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2",
                )
                .map_err(storage_error)?;
            let rows = statement
                .query_map(
                    params![workspace_id.to_string(), stream_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_error)?;
            let mut previous_revision = None;
            for row in rows {
                let revision = parse_revision(&row.map_err(storage_error)?)
                    .map_err(|_| corrupt("stream_revision_items", "revision"))?;
                previous_revision = match previous_revision {
                    None => Some(revision),
                    Some(previous) if revision > previous => Some(revision),
                    Some(previous) => Some(previous),
                };
            }
            previous_revision
        };
        if let Some(previous_revision) = previous_revision
            && revision <= previous_revision
        {
            return Err(SyncError::StreamInvariant {
                reason: "stream_revision_order",
            });
        }
        match item_kind {
            StreamRevisionItemKind::Event => {
                let index = event_index.ok_or(SyncError::StreamInvariant {
                    reason: "event_index_required",
                })?;
                if index != active.next_event_index || index >= active.expected_event_count {
                    return Err(SyncError::StreamInvariant {
                        reason: "event_index_order",
                    });
                }
                transaction
                    .execute(
                        "UPDATE stream_state SET next_event_index = ?1 WHERE workspace_id = ?2",
                        params![i64::from(index + 1), workspace_id.to_string()],
                    )
                    .map_err(storage_error)?;
            }
            StreamRevisionItemKind::ConflictResolved => {
                if event_index.is_some() {
                    return Err(SyncError::StreamInvariant {
                        reason: "conflict_resolved_index_forbidden",
                    });
                }
            }
        }
        transaction
            .execute(
                "INSERT INTO stream_revision_items (workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![workspace_id.to_string(), stream_id.to_string(), revision.to_string(), item_kind.as_str(), body_json.clone(), body_digest.as_slice(), event_index.map(i64::from), status.as_str()],
            )
            .map_err(storage_error)?;
    }
    Ok(StreamRevisionItemRecord {
        workspace_id,
        stream_id,
        revision,
        item_kind,
        body_json,
        body_digest,
        event_index,
        status,
    })
}

pub(crate) fn put_stream_conflict_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    message: &WorkspaceConflictCreatedMessage,
    stream_id: StreamId,
    status: StreamConflictStatus,
) -> Result<StreamConflictRecord, SyncError> {
    message.validate().map_err(|_| SyncError::StreamInvariant {
        reason: "invalid_stream_conflict",
    })?;
    if message.workspace_id != workspace_id {
        return Err(SyncError::ProtocolInvariant {
            reason: "stream_workspace_mismatch",
        });
    }
    let created_json = canonical_json(message)?;
    let conflict_revision = message.conflict_revision;
    let active =
        active_stream_state(transaction, workspace_id)?.ok_or(SyncError::StreamInvariant {
            reason: "no_active_stream",
        })?;
    if active.stream_id != stream_id {
        return Err(SyncError::StreamInvariant {
            reason: "active_stream_mismatch",
        });
    }
    let existing = transaction
        .query_row(
            "SELECT workspace_id, stream_id, conflict_id, conflict_revision, created_json, status FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2 AND conflict_id = ?3",
            params![workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string()],
            row_to_stream_conflict,
        )
        .optional()
        .map_err(storage_error)?;
    if let Some(existing) = existing {
        if existing.created_json != created_json {
            return Err(SyncError::OperationChanged);
        }
        if matches!(
            existing.status,
            StreamConflictStatus::Replaced | StreamConflictStatus::Pruned
        ) {
            return Ok(existing);
        }
        if !stream_conflict_status_transition_allowed(existing.status, status) {
            return Err(SyncError::StreamInvariant {
                reason: "stream_conflict_status_regression",
            });
        }
        transaction
            .execute(
                "UPDATE stream_conflicts SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND conflict_id = ?4",
                params![status.as_str(), workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string()],
            )
            .map_err(storage_error)?;
    } else {
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM stream_conflicts WHERE workspace_id = ?1 AND stream_id = ?2",
                params![workspace_id.to_string(), stream_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let count = u32::try_from(count).map_err(|_| corrupt("stream_conflicts", "conflict_id"))?;
        if count >= active.expected_conflict_count {
            return Err(SyncError::StreamInvariant {
                reason: "stream_conflict_count_exceeded",
            });
        }
        transaction
            .execute(
                "INSERT INTO stream_conflicts (workspace_id, stream_id, conflict_id, conflict_revision, created_json, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string(), conflict_revision_string(&conflict_revision)?, created_json.clone(), status.as_str()],
            )
            .map_err(storage_error)?;
    }
    Ok(StreamConflictRecord {
        workspace_id,
        stream_id,
        conflict_id: message.conflict_id,
        conflict_revision,
        created_json,
        status,
    })
}

pub(crate) fn replace_authoritative_conflicts_tx(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: WorkspaceId,
    stream_id: StreamId,
) -> Result<(), SyncError> {
    let workspace_id = workspace_id.to_string();
    let stream_id = stream_id.to_string();
    transaction
        .execute(
            "DELETE FROM outbox WHERE workspace_id = ?1 AND EXISTS (SELECT 1 FROM conflicts AS existing JOIN stream_conflicts AS staged ON staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = existing.conflict_id WHERE existing.workspace_id = ?1 AND (existing.conflict_revision != staged.conflict_revision OR existing.status = 'refresh_required') AND existing.resolution_json IS NOT NULL AND existing.resolution_json = outbox.body_json AND existing.resolution_digest = outbox.body_digest)",
            params![workspace_id, stream_id],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "UPDATE conflicts SET conflict_revision = (SELECT staged.conflict_revision FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id), created_json = (SELECT staged.created_json FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id), status = CASE WHEN conflict_revision != (SELECT staged.conflict_revision FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id) OR status = 'refresh_required' OR resolution_json IS NULL THEN 'manual' ELSE 'refresh_required' END, candidate_hash = NULL, resolution_json = CASE WHEN conflict_revision != (SELECT staged.conflict_revision FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id) OR status = 'refresh_required' THEN NULL ELSE resolution_json END, resolution_digest = CASE WHEN conflict_revision != (SELECT staged.conflict_revision FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id) OR status = 'refresh_required' THEN NULL ELSE resolution_digest END WHERE workspace_id = ?1 AND EXISTS (SELECT 1 FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id)",
            params![workspace_id, stream_id],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "UPDATE conflicts SET status = 'refresh_required' WHERE workspace_id = ?1 AND resolution_json IS NOT NULL AND NOT EXISTS (SELECT 1 FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id)",
            params![workspace_id, stream_id],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "DELETE FROM conflicts WHERE workspace_id = ?1 AND resolution_json IS NULL AND NOT EXISTS (SELECT 1 FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND staged.conflict_id = conflicts.conflict_id)",
            params![workspace_id, stream_id],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO conflicts (conflict_id, workspace_id, conflict_revision, created_json, status, candidate_hash, resolution_json, resolution_digest) SELECT staged.conflict_id, staged.workspace_id, staged.conflict_revision, staged.created_json, 'manual', NULL, NULL, NULL FROM stream_conflicts AS staged WHERE staged.workspace_id = ?1 AND staged.stream_id = ?2 AND NOT EXISTS (SELECT 1 FROM conflicts WHERE conflicts.workspace_id = ?1 AND conflicts.conflict_id = staged.conflict_id)",
            params![workspace_id, stream_id],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn mode_string(mode: WorkspaceSnapshotMode) -> &'static str {
    match mode {
        WorkspaceSnapshotMode::Snapshot => "snapshot",
        WorkspaceSnapshotMode::Incremental => "incremental",
    }
}

fn parse_mode(value: &str) -> rusqlite::Result<WorkspaceSnapshotMode> {
    match value {
        "snapshot" => Ok(WorkspaceSnapshotMode::Snapshot),
        "incremental" => Ok(WorkspaceSnapshotMode::Incremental),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_workspace_id(value: &str) -> rusqlite::Result<WorkspaceId> {
    WorkspaceId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_client_id(value: &str) -> rusqlite::Result<ClientId> {
    ClientId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_operation_id(value: &str) -> rusqlite::Result<OperationId> {
    OperationId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_stream_id(value: &str) -> rusqlite::Result<StreamId> {
    StreamId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_path(value: &str) -> rusqlite::Result<WorkspacePath> {
    WorkspacePath::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_revision(value: &str) -> rusqlite::Result<WorkspaceRevision> {
    WorkspaceRevision::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_digest(value: &[u8]) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_json<T: DeserializeOwned>(value: &[u8]) -> rusqlite::Result<T> {
    serde_json::from_slice(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_json_safe<T: DeserializeOwned>(
    value: &[u8],
    table: &'static str,
    field: &'static str,
) -> Result<T, SyncError> {
    serde_json::from_slice(value).map_err(|_| corrupt(table, field))
}

fn row_to_stream_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<StreamEntryRecord> {
    let body_json: Vec<u8> = row.get(3)?;
    let body_digest = parse_digest(&row.get::<_, Vec<u8>>(4)?)?;
    let entry_index: i64 = row.get(2)?;
    let workspace_id = parse_workspace_id(&row.get::<_, String>(0)?)?;
    let stream_id = parse_stream_id(&row.get::<_, String>(1)?)?;
    let entry_index = u32::try_from(entry_index).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let entry: WorkspaceSnapshotEntryMessage = parse_json(&body_json)?;
    entry
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if entry.workspace_id != workspace_id
        || entry.stream_id != stream_id
        || entry.index != entry_index
        || body_digest != crate::store::body_digest(&body_json)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(StreamEntryRecord {
        workspace_id,
        stream_id,
        entry_index,
        body_json,
        body_digest,
        status: parse_item_status(&row.get::<_, String>(5)?)?,
    })
}

fn row_to_stream_revision_item(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StreamRevisionItemRecord> {
    let body_json: Vec<u8> = row.get(4)?;
    let body_digest = parse_digest(&row.get::<_, Vec<u8>>(5)?)?;
    let event_index: Option<i64> = row.get(6)?;
    let workspace_id = parse_workspace_id(&row.get::<_, String>(0)?)?;
    let stream_id = parse_stream_id(&row.get::<_, String>(1)?)?;
    let revision = parse_revision(&row.get::<_, String>(2)?)?;
    let item_kind = match row.get::<_, String>(3)?.as_str() {
        "event" => StreamRevisionItemKind::Event,
        "conflict_resolved" => StreamRevisionItemKind::ConflictResolved,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    match item_kind {
        StreamRevisionItemKind::Event => {
            let event: WorkspaceEventMessage = parse_json(&body_json)?;
            event
                .validate()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            if event.workspace_id != workspace_id
                || event.stream_id != stream_id
                || event.revision != revision
                || event_index != Some(i64::from(event.index))
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        StreamRevisionItemKind::ConflictResolved => {
            let message: WorkspaceConflictResolvedMessage = parse_json(&body_json)?;
            message
                .validate()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            if message.workspace_id != workspace_id
                || message.revision != revision
                || event_index.is_some()
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
    }
    if body_digest != crate::store::body_digest(&body_json) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(StreamRevisionItemRecord {
        workspace_id,
        stream_id,
        revision,
        item_kind,
        body_json,
        body_digest,
        event_index: event_index
            .map(|index| u32::try_from(index).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        status: parse_item_status(&row.get::<_, String>(7)?)?,
    })
}

fn row_to_stream_conflict(row: &rusqlite::Row<'_>) -> rusqlite::Result<StreamConflictRecord> {
    let workspace_id = parse_workspace_id(&row.get::<_, String>(0)?)?;
    let stream_id = parse_stream_id(&row.get::<_, String>(1)?)?;
    let conflict_id = parse_conflict_id(&row.get::<_, String>(2)?)?;
    let conflict_revision =
        fns_protocol::revision::WorkspaceConflictRevision::parse(&row.get::<_, String>(3)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status = StreamConflictStatus::parse(&row.get::<_, String>(5)?)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let created_json: Vec<u8> = row.get(4)?;
    let created: WorkspaceConflictCreatedMessage = parse_json(&created_json)?;
    created
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if created.workspace_id != workspace_id
        || created.conflict_id != conflict_id
        || created.conflict_revision != conflict_revision
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(StreamConflictRecord {
        workspace_id,
        stream_id,
        conflict_id,
        conflict_revision,
        created_json,
        status,
    })
}

fn row_to_apply_journal(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApplyJournalRecord> {
    Ok(ApplyJournalRecord {
        apply_id: parse_apply_id(&row.get::<_, String>(0)?)?,
        workspace_id: parse_workspace_id(&row.get::<_, String>(1)?)?,
        stream_id: parse_stream_id(&row.get::<_, String>(2)?)?,
        item_kind: ApplyItemKind::parse(&row.get::<_, String>(3)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        item_key: row.get(4)?,
        apply_namespace: ApplyNamespace::parse(&row.get::<_, String>(5)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        operation_body_digest: parse_digest(&row.get::<_, Vec<u8>>(6)?)?,
        operation_json: row.get(7)?,
        filesystem_operation_json: row.get(8)?,
        commit_json: row.get(9)?,
        preimage_json: row.get(10)?,
        postimage_json: row.get(11)?,
        filesystem_receipt_json: row.get(12)?,
        stage: ApplyStage::parse(&row.get::<_, String>(13)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
    })
}

fn row_to_applied_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppliedOperationRecord> {
    let receipt_kind = AppliedOperationReceiptKind::parse(&row.get::<_, String>(4)?)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let mutation_json: Option<Vec<u8>> = row.get(5)?;
    if (receipt_kind == AppliedOperationReceiptKind::MutationResult) != mutation_json.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(AppliedOperationRecord {
        origin_client_id: parse_client_id(&row.get::<_, String>(0)?)?,
        operation_id: parse_operation_id(&row.get::<_, String>(1)?)?,
        revision: parse_revision(&row.get::<_, String>(2)?)?,
        body_digest: parse_digest(&row.get::<_, Vec<u8>>(3)?)?,
        receipt_kind,
        mutation_json,
    })
}

fn row_to_provisional_mutation_acceptance(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkspaceMutationAcceptedMessage> {
    let origin_client_id = parse_client_id(&row.get::<_, String>(0)?)?;
    let operation_id = parse_operation_id(&row.get::<_, String>(1)?)?;
    let workspace_id = parse_workspace_id(&row.get::<_, String>(2)?)?;
    let revision = parse_revision(&row.get::<_, String>(3)?)?;
    let accepted_json: Vec<u8> = row.get(4)?;
    let accepted_digest = parse_digest(&row.get::<_, Vec<u8>>(5)?)?;
    let accepted: WorkspaceMutationAcceptedMessage = parse_json(&accepted_json)?;
    accepted
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if accepted.client_id != origin_client_id
        || accepted.operation_id != operation_id
        || accepted.workspace_id != workspace_id
        || accepted.revision != revision
        || canonical_json(&accepted).map_err(|_| rusqlite::Error::InvalidQuery)? != accepted_json
        || body_digest(&accepted_json) != accepted_digest
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(accepted)
}

fn provisional_acceptance_read_error(error: rusqlite::Error) -> SyncError {
    match error {
        rusqlite::Error::InvalidQuery => {
            corrupt("provisional_mutation_acceptances", "accepted_json")
        }
        _ => storage_error(error),
    }
}

fn row_to_conflict(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConflictRecord> {
    let resolution_digest: Option<Vec<u8>> = row.get(7)?;
    let workspace_id = parse_workspace_id(&row.get::<_, String>(1)?)?;
    let conflict_id = parse_conflict_id(&row.get::<_, String>(0)?)?;
    let conflict_revision =
        fns_protocol::revision::WorkspaceConflictRevision::parse(&row.get::<_, String>(2)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let created_json: Vec<u8> = row.get(3)?;
    let created: WorkspaceConflictCreatedMessage = parse_json(&created_json)?;
    created
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if created.workspace_id != workspace_id
        || created.conflict_id != conflict_id
        || created.conflict_revision != conflict_revision
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let candidate_hash: Option<String> = row.get(5)?;
    if let Some(candidate_hash) = &candidate_hash {
        WorkspaceContentHash::parse(candidate_hash).map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    let resolution_json: Option<Vec<u8>> = row.get(6)?;
    if let Some(resolution_digest) = &resolution_digest
        && (resolution_digest.len() != 32
            || resolution_json.as_deref().is_none_or(|value| {
                parse_digest(resolution_digest)
                    .is_ok_and(|digest| crate::store::body_digest(value) != digest)
            }))
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(ConflictRecord {
        conflict_id,
        workspace_id,
        conflict_revision,
        created_json,
        status: ConflictStatus::parse(&row.get::<_, String>(4)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        candidate_hash,
        resolution_json,
        resolution_digest: resolution_digest
            .map(|digest| parse_digest(&digest))
            .transpose()?,
    })
}

fn conflict_read_error(error: rusqlite::Error) -> SyncError {
    match error {
        rusqlite::Error::InvalidQuery => corrupt("conflicts", "row"),
        _ => storage_error(error),
    }
}

fn parse_conflict_id(value: &str) -> rusqlite::Result<ConflictId> {
    ConflictId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_apply_id(value: &str) -> rusqlite::Result<ApplyId> {
    uuid::Uuid::parse_str(value)
        .map(ApplyId)
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn conflict_revision_string(
    value: &fns_protocol::revision::WorkspaceConflictRevision,
) -> Result<String, SyncError> {
    serde_json::to_string(value)
        .map(|serialized| serialized.trim_matches('"').to_owned())
        .map_err(|_| SyncError::InvalidConfiguration {
            reason: "serialization_failed",
        })
}

fn parse_item_status(value: &str) -> rusqlite::Result<StreamItemStatus> {
    StreamItemStatus::parse(value).ok_or(rusqlite::Error::InvalidQuery)
}
