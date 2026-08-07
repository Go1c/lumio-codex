use fns_fs::{FileFingerprint, HashCache, HashCacheError};
use fns_protocol::{
    ClientId, ConflictId, OperationId, StreamId, WorkspaceConflictCreatedMessage,
    WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest, WorkspaceContentHash,
    WorkspaceEventMessage, WorkspaceId, WorkspaceMutation, WorkspacePath, WorkspaceRevision,
    WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEntryMessage, WorkspaceSnapshotMode,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{SyncError, corrupt, storage_error};
use crate::ids::now_ms;
use crate::model::{
    AppliedOperationRecord, ApplyItemKind, ApplyJournalRecord, ApplyStage, ConflictRecord,
    ConflictStatus, LocalIntentRecord, OutboxRecord, OutboxStage, PathStateRecord,
    StreamConflictRecord, StreamConflictStatus, StreamEntryRecord, StreamItemStatus,
    StreamRevisionItemKind, StreamRevisionItemRecord, StreamStateRecord,
};
use crate::state::SqliteState;

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SyncError> {
    serde_json::to_vec(value).map_err(|_| SyncError::InvalidConfiguration {
        reason: "serialization_failed",
    })
}

pub fn body_digest(body: &[u8]) -> [u8; 32] {
    *blake3::hash(body).as_bytes()
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
        let body_digest = body_digest(&body_json);
        self.enqueue_outbox_bytes(
            mutation.operation_id,
            mutation.workspace_id,
            body_json,
            body_digest,
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
        let body_digest = body_digest(&body_json);
        self.enqueue_outbox_bytes(
            resolution.operation_id,
            resolution.workspace_id,
            body_json,
            body_digest,
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

    pub fn set_outbox_stage(
        &mut self,
        operation_id: OperationId,
        stage: OutboxStage,
    ) -> Result<(), SyncError> {
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
            if existing.stream_id != begin.stream_id {
                return Err(SyncError::StreamInvariant {
                    reason: "active_stream_exists",
                });
            }
            let candidate = stream_state_from_begin(begin);
            if existing != candidate {
                return Err(SyncError::StreamInvariant {
                    reason: "stream_begin_changed",
                });
            }
            transaction.commit().map_err(storage_error)?;
            return Ok(existing);
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
        self.conn
            .execute(
                "INSERT INTO stream_state (workspace_id, stream_id, mode, from_revision, final_revision, expected_entry_count, expected_event_count, expected_conflict_count, next_event_index, end_received) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(workspace_id) DO UPDATE SET stream_id = excluded.stream_id, mode = excluded.mode, from_revision = excluded.from_revision, final_revision = excluded.final_revision, expected_entry_count = excluded.expected_entry_count, expected_event_count = excluded.expected_event_count, expected_conflict_count = excluded.expected_conflict_count, next_event_index = excluded.next_event_index, end_received = excluded.end_received",
                params![
                    self.workspace_id.to_string(),
                    state.stream_id.to_string(),
                    mode_string(state.mode),
                    state.from_revision.to_string(),
                    state.final_revision.to_string(),
                    i64::from(state.expected_entry_count),
                    i64::from(state.expected_event_count),
                    i64::from(state.expected_conflict_count),
                    i64::from(state.next_event_index),
                    i64::from(state.end_received as u8),
                ],
            )
            .map_err(storage_error)?;
        Ok(())
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
        self.conn
            .execute(
                "DELETE FROM stream_state WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_stream_end_received(&mut self, received: bool) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE stream_state SET end_received = ?1 WHERE workspace_id = ?2",
                params![i64::from(received as u8), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn put_stream_entry(
        &mut self,
        entry: &WorkspaceSnapshotEntryMessage,
        status: StreamItemStatus,
    ) -> Result<StreamEntryRecord, SyncError> {
        entry.validate().map_err(|_| SyncError::StreamInvariant {
            reason: "invalid_stream_entry",
        })?;
        if entry.workspace_id != self.workspace_id {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_workspace_mismatch",
            });
        }
        let body_json = canonical_json(entry)?;
        let body_digest = body_digest(&body_json);
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT workspace_id, stream_id, entry_index, body_json, body_digest, status FROM stream_entries WHERE workspace_id = ?1 AND stream_id = ?2 AND entry_index = ?3",
                params![self.workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index)],
                row_to_stream_entry,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.body_digest != body_digest {
                return Err(SyncError::OperationChanged);
            }
            transaction
                .execute(
                    "UPDATE stream_entries SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND entry_index = ?4",
                    params![status.as_str(), self.workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index)],
                )
                .map_err(storage_error)?;
        } else {
            transaction
                .execute(
                    "INSERT INTO stream_entries (workspace_id, stream_id, entry_index, body_json, body_digest, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![self.workspace_id.to_string(), entry.stream_id.to_string(), i64::from(entry.index), body_json, body_digest.as_slice(), status.as_str()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(StreamEntryRecord {
            workspace_id: self.workspace_id,
            stream_id: entry.stream_id,
            entry_index: entry.index,
            body_json,
            body_digest,
            status,
        })
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

    pub fn advance_stream_index(&mut self, next_index: u32) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE stream_state SET next_event_index = ?1 WHERE workspace_id = ?2",
                params![i64::from(next_index), self.workspace_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
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
        if stored_digest != crate::store::body_digest(&body_json) {
            return Err(SyncError::ProtocolInvariant {
                reason: "stream_body_digest_mismatch",
            });
        }
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 AND revision = ?3",
                params![self.workspace_id.to_string(), stream_id.to_string(), revision.to_string()],
                row_to_stream_revision_item,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.body_digest != stored_digest || existing.item_kind != item_kind {
                return Err(SyncError::OperationChanged);
            }
            transaction
                .execute(
                    "UPDATE stream_revision_items SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND revision = ?4",
                    params![status.as_str(), self.workspace_id.to_string(), stream_id.to_string(), revision.to_string()],
                )
                .map_err(storage_error)?;
        } else {
            transaction
                .execute(
                    "INSERT INTO stream_revision_items (workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![self.workspace_id.to_string(), stream_id.to_string(), revision.to_string(), item_kind.as_str(), body_json.clone(), stored_digest.as_slice(), event_index.map(i64::from), status.as_str()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(StreamRevisionItemRecord {
            workspace_id: self.workspace_id,
            stream_id,
            revision,
            item_kind,
            body_json,
            body_digest: stored_digest,
            event_index,
            status,
        })
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
                "SELECT workspace_id, stream_id, revision, item_kind, body_json, body_digest, event_index, status FROM stream_revision_items WHERE workspace_id = ?1 AND stream_id = ?2 ORDER BY revision",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![self.workspace_id.to_string(), stream_id.to_string()],
                row_to_stream_revision_item,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
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
            transaction
                .execute(
                    "UPDATE stream_conflicts SET status = ?1 WHERE workspace_id = ?2 AND stream_id = ?3 AND conflict_id = ?4",
                    params![status.as_str(), self.workspace_id.to_string(), stream_id.to_string(), message.conflict_id.to_string()],
                )
                .map_err(storage_error)?;
        } else {
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
        let existing = transaction
            .query_row(
                "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, operation_json, preimage_json, postimage_json, stage FROM apply_journal WHERE apply_id = ?1",
                params![record.apply_id],
                row_to_apply_journal,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.operation_json != record.operation_json
                || existing.preimage_json != record.preimage_json
                || existing.postimage_json != record.postimage_json
                || existing.item_key != record.item_key
            {
                return Err(SyncError::OperationChanged);
            }
            transaction
                .execute(
                    "UPDATE apply_journal SET stage = ?1 WHERE apply_id = ?2",
                    params![record.stage.as_str(), record.apply_id],
                )
                .map_err(storage_error)?;
        } else {
            transaction
                .execute(
                    "INSERT INTO apply_journal (apply_id, workspace_id, stream_id, item_kind, item_key, operation_json, preimage_json, postimage_json, stage) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![record.apply_id, record.workspace_id.to_string(), record.stream_id.to_string(), record.item_kind.as_str(), record.item_key, record.operation_json, record.preimage_json, record.postimage_json, record.stage.as_str()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn insert_apply_journal(&mut self, record: &ApplyJournalRecord) -> Result<(), SyncError> {
        self.put_apply_journal(record)
    }

    pub fn apply_journal(&self, apply_id: &str) -> Result<Option<ApplyJournalRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, operation_json, preimage_json, postimage_json, stage FROM apply_journal WHERE apply_id = ?1",
                params![apply_id],
                row_to_apply_journal,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn apply_journals(&self) -> Result<Vec<ApplyJournalRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT apply_id, workspace_id, stream_id, item_kind, item_key, operation_json, preimage_json, postimage_json, stage FROM apply_journal WHERE workspace_id = ?1 ORDER BY apply_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![self.workspace_id.to_string()], row_to_apply_journal)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn set_apply_stage(&mut self, apply_id: &str, stage: ApplyStage) -> Result<(), SyncError> {
        self.conn
            .execute(
                "UPDATE apply_journal SET stage = ?1 WHERE apply_id = ?2",
                params![stage.as_str(), apply_id],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn remove_apply_journal(&mut self, apply_id: &str) -> Result<(), SyncError> {
        self.conn
            .execute(
                "DELETE FROM apply_journal WHERE apply_id = ?1",
                params![apply_id],
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
    ) -> Result<(), SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT origin_client_id, operation_id, revision, body_digest FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
                params![origin_client_id.to_string(), operation_id.to_string()],
                row_to_applied_operation,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing.body_digest != body_digest || existing.revision != revision {
                return Err(SyncError::OperationChanged);
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO applied_operations (origin_client_id, operation_id, revision, body_digest) VALUES (?1, ?2, ?3, ?4)",
                    params![origin_client_id.to_string(), operation_id.to_string(), revision.to_string(), body_digest.as_slice()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn applied_operation(
        &self,
        origin_client_id: ClientId,
        operation_id: OperationId,
    ) -> Result<Option<AppliedOperationRecord>, SyncError> {
        self.conn
            .query_row(
                "SELECT origin_client_id, operation_id, revision, body_digest FROM applied_operations WHERE origin_client_id = ?1 AND operation_id = ?2",
                params![origin_client_id.to_string(), operation_id.to_string()],
                row_to_applied_operation,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn applied_operations(&self) -> Result<Vec<AppliedOperationRecord>, SyncError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT origin_client_id, operation_id, revision, body_digest FROM applied_operations ORDER BY origin_client_id, operation_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], row_to_applied_operation)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error))
            .collect::<Result<Vec<_>, _>>()
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
            .map_err(storage_error)
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
        rows.map(|row| row.map_err(storage_error))
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
        body_digest: [u8; 32],
        created_at_ms: i64,
    ) -> Result<OutboxRecord, SyncError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT client_id, operation_id, workspace_id, body_json, body_digest, stage, created_at_ms FROM outbox WHERE client_id = ?1 AND operation_id = ?2",
                params![self.client_id.to_string(), operation_id.to_string()],
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
                        params![workspace_id.to_string(), body_json.clone(), body_digest.as_slice(), created_at_ms, self.client_id.to_string(), operation_id.to_string()],
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
                    params![self.client_id.to_string(), operation_id.to_string(), workspace_id.to_string(), body_json.clone(), body_digest.as_slice(), created_at_ms],
                )
                .map_err(storage_error)?;
            OutboxRecord {
                client_id: self.client_id,
                operation_id,
                workspace_id,
                body_json,
                body_digest,
                stage: OutboxStage::Queued,
                created_at_ms,
            }
        };
        transaction.commit().map_err(storage_error)?;
        Ok(record)
    }
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
    Ok(StreamStateRecord {
        workspace_id: parse_workspace_id(&row.get::<_, String>(0)?)?,
        stream_id: parse_stream_id(&row.get::<_, String>(1)?)?,
        mode,
        from_revision: parse_revision(&row.get::<_, String>(3)?)?,
        final_revision: parse_revision(&row.get::<_, String>(4)?)?,
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
            if message.workspace_id != workspace_id || message.revision != revision {
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
        apply_id: row.get(0)?,
        workspace_id: parse_workspace_id(&row.get::<_, String>(1)?)?,
        stream_id: parse_stream_id(&row.get::<_, String>(2)?)?,
        item_kind: ApplyItemKind::parse(&row.get::<_, String>(3)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        item_key: row.get(4)?,
        operation_json: row.get(5)?,
        preimage_json: row.get(6)?,
        postimage_json: row.get(7)?,
        stage: ApplyStage::parse(&row.get::<_, String>(8)?).ok_or(rusqlite::Error::InvalidQuery)?,
    })
}

fn row_to_applied_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppliedOperationRecord> {
    Ok(AppliedOperationRecord {
        origin_client_id: parse_client_id(&row.get::<_, String>(0)?)?,
        operation_id: parse_operation_id(&row.get::<_, String>(1)?)?,
        revision: parse_revision(&row.get::<_, String>(2)?)?,
        body_digest: parse_digest(&row.get::<_, Vec<u8>>(3)?)?,
    })
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

fn parse_conflict_id(value: &str) -> rusqlite::Result<ConflictId> {
    ConflictId::parse(value).map_err(|_| rusqlite::Error::InvalidQuery)
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
