//! Engine worker: one bounded OS thread serializing every SyncEngine call.
//!
//! The SyncEngine owns SQLite and filesystem state that must not be shared across
//! threads. This module runs a dedicated `fns-sync-engine` thread that owns the
//! engine and processes calls from the async side through a bounded channel.

use crate::ENGINE_QUEUE_CAPACITY;
use crate::error::{TransportError, TransportErrorCode};

use fns_sync_core::{
    ConflictResolutionInput, ConflictResolutionReceipt, ConflictView, SyncEngine, WorkspaceCursor,
};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

enum BlobImportStage {
    Staging(Box<fns_fs::StagedContentImport>),
    Sealed(fns_fs::SealedContentImport),
}

/// Read-only durable engine metrics used by the agent status publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineRuntimeStatus {
    pub last_ack_revision: fns_protocol::WorkspaceRevision,
    pub last_applied_revision: fns_protocol::WorkspaceRevision,
    pub pending_commands: u64,
    pub pending_ack: u64,
    pub pending_segment_ack: u64,
    pub outbox_queued: u64,
    pub outbox_dispatched: u64,
    pub outbox_awaiting_blob: u64,
    pub outbox_blocked_conflict: u64,
    pub stream_active: bool,
}

/// Private request variants — each maps to exactly one SyncEngine method.
#[allow(dead_code)] // Not all variants are used until dispatch/stream tasks land.
enum EngineCall {
    PrepareConnectionAttempt {
        tx: oneshot::Sender<Result<usize, TransportError>>,
    },
    Cursor {
        tx: oneshot::Sender<Result<WorkspaceCursor, TransportError>>,
    },
    RuntimeStatus {
        tx: oneshot::Sender<Result<EngineRuntimeStatus, TransportError>>,
    },
    ListConflicts {
        tx: oneshot::Sender<Result<Vec<ConflictView>, TransportError>>,
    },
    ResolveConflict {
        input: ConflictResolutionInput,
        tx: oneshot::Sender<Result<ConflictResolutionReceipt, TransportError>>,
    },
    ActiveStreamMode {
        tx: oneshot::Sender<Result<Option<fns_sync_core::StreamMode>, TransportError>>,
    },
    CompletedStreamAckRevision {
        tx: oneshot::Sender<Result<Option<fns_protocol::WorkspaceRevision>, TransportError>>,
    },
    PendingCommands {
        limit: usize,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    RecordLocalChanges {
        changes: Vec<fns_fs::FsChange>,
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
    SnapshotBegin {
        message: fns_protocol::WorkspaceSnapshotBeginMessage,
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
    SnapshotEntry {
        message: fns_protocol::WorkspaceSnapshotEntryMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    SnapshotEnd {
        message: fns_protocol::WorkspaceSnapshotEndMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    WorkspaceEvent {
        message: fns_protocol::WorkspaceEventMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    MutationAccepted {
        message: fns_protocol::WorkspaceMutationAcceptedMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    MutationRejected {
        message: fns_protocol::WorkspaceMutationRejectedMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    ConflictCreated {
        message: fns_protocol::WorkspaceConflictCreatedMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    ConflictResolved {
        message: fns_protocol::WorkspaceConflictResolvedMessage,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    ConflictResolutionAccepted {
        message: fns_protocol::WorkspaceConflictResolvedMessage,
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
    ConflictResolutionRejected {
        operation_id: fns_protocol::OperationId,
        code: fns_protocol::WorkspaceV2ErrorCode,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    AckConfirmed {
        message: fns_protocol::WorkspaceAckRequest,
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
    /// Run an arbitrary job against the engine on the engine thread.
    ///
    /// Hosts need engine state the wire protocol never asks for — the conflict
    /// list behind a UI, aggregate progress counters, a user-chosen conflict
    /// resolution. Routing those through one boxed job keeps the "exactly one
    /// thread touches SyncEngine" invariant without growing a handle method per
    /// question. The job carries its own reply channel.
    Job {
        job: Box<dyn FnOnce(&mut SyncEngine) + Send>,
    },
    Shutdown {
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
}

/// Handle for sending commands to the engine worker from async context.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<EngineCall>,
}

/// The engine worker owns the SyncEngine on a dedicated thread.
/// Dropping or joining it shuts down the engine.
pub struct EngineWorker {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EngineWorker {
    /// Spawn a dedicated thread that owns the SyncEngine. Returns the worker
    /// (to be joined) and a handle for sending commands.
    pub fn spawn(engine: SyncEngine) -> (Self, EngineHandle) {
        let (tx, mut rx) = mpsc::channel::<EngineCall>(ENGINE_QUEUE_CAPACITY);
        let handle = EngineHandle { tx };

        let thread = std::thread::Builder::new()
            .name("fns-sync-engine".into())
            .spawn(move || {
                let mut engine = engine;
                let mut blob_imports = HashMap::new();
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                rt.block_on(async move {
                    while let Some(call) = rx.recv().await {
                        let should_stop = matches!(call, EngineCall::Shutdown { .. });
                        process_call(&mut engine, &mut blob_imports, call);
                        if should_stop {
                            break;
                        }
                    }
                });
            })
            .expect("failed to spawn fns-sync-engine thread");

        let worker = EngineWorker {
            thread: Some(thread),
        };
        (worker, handle)
    }

    /// Join the worker thread. Should be called after EngineHandle::shutdown.
    pub fn join(mut self) -> Result<(), TransportError> {
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        }
        Ok(())
    }
}

impl Drop for EngineWorker {
    fn drop(&mut self) {
        // Dropping the handle's sender ends the run loop.
        // The thread will exit when the channel is closed.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn process_call(
    engine: &mut SyncEngine,
    blob_imports: &mut HashMap<fns_protocol::TransferId, BlobImportStage>,
    call: EngineCall,
) {
    match call {
        EngineCall::PrepareConnectionAttempt { tx } => {
            let _ = tx.send(map_err_named(
                engine.prepare_connection_attempt(),
                "prepare_connection_attempt",
            ));
        }
        EngineCall::Cursor { tx } => {
            let _ = tx.send(map_err(engine.cursor()));
        }
        EngineCall::RuntimeStatus { tx } => {
            let status = engine.cursor().and_then(|cursor| {
                let mut outbox_queued = 0u64;
                let mut outbox_dispatched = 0u64;
                let mut outbox_awaiting_blob = 0u64;
                let mut outbox_blocked_conflict = 0u64;
                for record in engine.state().outbox()? {
                    match record.stage {
                        fns_sync_core::OutboxStage::Queued => outbox_queued += 1,
                        fns_sync_core::OutboxStage::Dispatched => outbox_dispatched += 1,
                        fns_sync_core::OutboxStage::AwaitingBlob => outbox_awaiting_blob += 1,
                        fns_sync_core::OutboxStage::BlockedConflict => outbox_blocked_conflict += 1,
                    }
                }
                let stream_active = engine.active_stream_mode()?.is_some();
                Ok(EngineRuntimeStatus {
                    last_ack_revision: cursor.last_ack_revision,
                    last_applied_revision: cursor.last_applied_revision,
                    pending_commands: engine.state().pending_work_count()?,
                    pending_ack: u64::from(cursor.pending_ack_revision.is_some()),
                    pending_segment_ack: u64::from(cursor.pending_segment_ack_revision.is_some()),
                    outbox_queued,
                    outbox_dispatched,
                    outbox_awaiting_blob,
                    outbox_blocked_conflict,
                    stream_active,
                })
            });
            let _ = tx.send(map_err_named(status, "runtime_status"));
        }
        EngineCall::ListConflicts { tx } => {
            let _ = tx.send(map_conflict_control_error(
                engine.list_conflicts(),
                "list_conflicts",
            ));
        }
        EngineCall::ResolveConflict { input, tx } => {
            let _ = tx.send(map_conflict_control_error(
                engine.resolve_conflict(input.conflict_id, input.conflict_revision, input.choice),
                "resolve_conflict",
            ));
        }
        EngineCall::ActiveStreamMode { tx } => {
            let _ = tx.send(map_err(engine.active_stream_mode()));
        }
        EngineCall::CompletedStreamAckRevision { tx } => {
            let _ = tx.send(map_err(engine.completed_stream_ack_revision()));
        }
        EngineCall::PendingCommands { limit, tx } => {
            let _ = tx.send(map_err_named(
                engine.pending_commands(limit),
                "pending_commands",
            ));
        }
        EngineCall::RecordLocalChanges { changes, tx } => {
            let _ = tx.send(map_err(engine.record_local_changes(changes)));
        }
        EngineCall::SnapshotBegin { message, tx } => {
            let _ = tx.send(map_err(engine.snapshot_begin(message)));
        }
        EngineCall::SnapshotEntry { message, tx } => {
            let _ = tx.send(map_err(engine.snapshot_entry(message)));
        }
        EngineCall::SnapshotEnd { message, tx } => {
            let _ = tx.send(map_err_named(engine.snapshot_end(message), "snapshot_end"));
        }
        EngineCall::WorkspaceEvent { message, tx } => {
            let result = match engine.active_stream_mode() {
                Ok(Some(fns_sync_core::StreamMode::Incremental)) => engine.workspace_event(message),
                Ok(Some(fns_sync_core::StreamMode::Snapshot)) | Ok(None) => engine.event(message),
                Err(error) => Err(error),
            };
            let _ = tx.send(map_err_named(result, "workspace_event"));
        }
        EngineCall::MutationAccepted { message, tx } => {
            let _ = tx.send(map_err(engine.mutation_accepted(message)));
        }
        EngineCall::MutationRejected { message, tx } => {
            let _ = tx.send(map_err(engine.mutation_rejected(message)));
        }
        EngineCall::ConflictCreated { message, tx } => {
            let _ = tx.send(map_err_named(
                engine.conflict_created(message),
                "conflict_created",
            ));
        }
        EngineCall::ConflictResolved { message, tx } => {
            let _ = tx.send(map_err_named(
                engine.conflict_resolved(message),
                "conflict_resolved",
            ));
        }
        EngineCall::ConflictResolutionAccepted { message, tx } => {
            let _ = tx.send(map_err_named(
                engine.conflict_resolution_accepted(message),
                "conflict_resolution_accepted",
            ));
        }
        EngineCall::ConflictResolutionRejected {
            operation_id,
            code,
            tx,
        } => {
            let _ = tx.send(map_err_named(
                engine.conflict_resolution_rejected(operation_id, code),
                "conflict_resolution_rejected",
            ));
        }
        EngineCall::AckConfirmed { message, tx } => {
            let _ = tx.send(map_err_named(
                engine.ack_confirmed(message),
                "ack_confirmed",
            ));
        }
        EngineCall::OpenBlob { content_hash, tx } => {
            let _ = tx.send(map_err(engine.open_blob(&content_hash)));
        }
        EngineCall::BeginBlobImport {
            transfer_id,
            content_hash,
            size,
            tx,
        } => {
            let result = match blob_imports.entry(transfer_id) {
                std::collections::hash_map::Entry::Vacant(entry) => engine
                    .begin_blob_import(content_hash, size)
                    .map(|staged| {
                        entry.insert(BlobImportStage::Staging(Box::new(staged)));
                    })
                    .map_err(map_sync_error),
                std::collections::hash_map::Entry::Occupied(_) => {
                    Err(TransportError::new(TransportErrorCode::Protocol, false))
                }
            };
            let _ = tx.send(result);
        }
        EngineCall::WriteBlobChunk {
            transfer_id,
            data,
            tx,
        } => {
            let result = match blob_imports.get_mut(&transfer_id) {
                Some(BlobImportStage::Staging(staged)) => {
                    staged.write_chunk(&data).map_err(map_blob_stage_error)
                }
                Some(BlobImportStage::Sealed(_)) | None => {
                    Err(TransportError::new(TransportErrorCode::Protocol, false))
                }
            };
            if result.is_err() {
                blob_imports.remove(&transfer_id);
            }
            let _ = tx.send(result);
        }
        EngineCall::SealBlobImport { transfer_id, tx } => {
            let result = match blob_imports.remove(&transfer_id) {
                Some(BlobImportStage::Staging(staged)) => (*staged)
                    .seal()
                    .map(|sealed| {
                        blob_imports.insert(transfer_id, BlobImportStage::Sealed(sealed));
                    })
                    .map_err(map_blob_stage_error),
                Some(BlobImportStage::Sealed(sealed)) => {
                    blob_imports.insert(transfer_id, BlobImportStage::Sealed(sealed));
                    Ok(())
                }
                None => Err(TransportError::new(TransportErrorCode::Protocol, false)),
            };
            let _ = tx.send(result);
        }
        EngineCall::CommitBlobImport { transfer_id, tx } => {
            let result = match blob_imports.remove(&transfer_id) {
                Some(BlobImportStage::Sealed(sealed)) => {
                    engine.commit_blob_import(sealed).map_err(map_sync_error)
                }
                Some(BlobImportStage::Staging(staged)) => {
                    blob_imports.insert(transfer_id, BlobImportStage::Staging(staged));
                    Err(TransportError::new(TransportErrorCode::Protocol, false))
                }
                None => Err(TransportError::new(TransportErrorCode::Protocol, false)),
            };
            let _ = tx.send(result);
        }
        EngineCall::AbortBlobImport { transfer_id, tx } => {
            blob_imports.remove(&transfer_id);
            let _ = tx.send(Ok(()));
        }
        EngineCall::AbortBlobImports { tx } => {
            blob_imports.clear();
            let _ = tx.send(Ok(()));
        }
        EngineCall::BlobUploaded { operation_id, tx } => {
            let _ = tx.send(map_err(engine.blob_uploaded(operation_id)));
        }
        EngineCall::Job { job } => job(engine),
        EngineCall::Shutdown { tx } => {
            blob_imports.clear();
            let _ = tx.send(map_err(engine.close()));
        }
    }
}

impl EngineHandle {
    pub async fn prepare_connection_attempt(&self) -> Result<usize, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::PrepareConnectionAttempt { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn cursor(&self) -> Result<WorkspaceCursor, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::Cursor { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn runtime_status(&self) -> Result<EngineRuntimeStatus, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::RuntimeStatus { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn list_conflicts(&self) -> Result<Vec<ConflictView>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ListConflicts { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn resolve_conflict(
        &self,
        conflict_id: fns_protocol::ConflictId,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
        choice: fns_protocol::WorkspaceConflictChoice,
    ) -> Result<ConflictResolutionReceipt, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ResolveConflict {
                input: ConflictResolutionInput {
                    conflict_id,
                    conflict_revision,
                    choice,
                },
                tx,
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn active_stream_mode(
        &self,
    ) -> Result<Option<fns_sync_core::StreamMode>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ActiveStreamMode { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn completed_stream_ack_revision(
        &self,
    ) -> Result<Option<fns_protocol::WorkspaceRevision>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::CompletedStreamAckRevision { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn pending_commands(
        &self,
        limit: usize,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::PendingCommands { limit, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn record_local_changes(
        &self,
        changes: Vec<fns_fs::FsChange>,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::RecordLocalChanges { changes, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    /// Run `job` on the engine thread and await its value.
    ///
    /// Use this for engine state a host needs but the session loop does not
    /// drive, such as the conflict list or the counters behind a status
    /// indicator. The job runs to completion between two protocol calls, so it
    /// must not block.
    pub async fn with_engine<T, F>(&self, job: F) -> Result<T, TransportError>
    where
        F: FnOnce(&mut SyncEngine) -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::Job {
                job: Box::new(move |engine| {
                    let _ = tx.send(job(engine));
                }),
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))
    }

    pub async fn shutdown(&self) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::Shutdown { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn snapshot_begin(
        &self,
        message: fns_protocol::WorkspaceSnapshotBeginMessage,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::SnapshotBegin { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn snapshot_entry(
        &self,
        message: fns_protocol::WorkspaceSnapshotEntryMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::SnapshotEntry { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn snapshot_end(
        &self,
        message: fns_protocol::WorkspaceSnapshotEndMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::SnapshotEnd { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn workspace_event(
        &self,
        message: fns_protocol::WorkspaceEventMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::WorkspaceEvent { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn mutation_accepted(
        &self,
        message: fns_protocol::WorkspaceMutationAcceptedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::MutationAccepted { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn mutation_rejected(
        &self,
        message: fns_protocol::WorkspaceMutationRejectedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::MutationRejected { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn conflict_created(
        &self,
        message: fns_protocol::WorkspaceConflictCreatedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ConflictCreated { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn conflict_resolved(
        &self,
        message: fns_protocol::WorkspaceConflictResolvedMessage,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ConflictResolved { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn conflict_resolution_accepted(
        &self,
        message: fns_protocol::WorkspaceConflictResolvedMessage,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ConflictResolutionAccepted { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn conflict_resolution_rejected(
        &self,
        operation_id: fns_protocol::OperationId,
        code: fns_protocol::WorkspaceV2ErrorCode,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::ConflictResolutionRejected {
                operation_id,
                code,
                tx,
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn ack_confirmed(
        &self,
        message: fns_protocol::WorkspaceAckRequest,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::AckConfirmed { message, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn open_blob(
        &self,
        content_hash: &fns_protocol::WorkspaceContentHash,
    ) -> Result<std::fs::File, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::OpenBlob {
                content_hash: content_hash.clone(),
                tx,
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn begin_blob_import(
        &self,
        transfer_id: fns_protocol::TransferId,
        content_hash: fns_protocol::WorkspaceContentHash,
        size: u64,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::BeginBlobImport {
                transfer_id,
                content_hash,
                size,
                tx,
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn write_blob_chunk(
        &self,
        transfer_id: fns_protocol::TransferId,
        data: tokio_tungstenite::tungstenite::Bytes,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::WriteBlobChunk {
                transfer_id,
                data,
                tx,
            })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn seal_blob_import(
        &self,
        transfer_id: fns_protocol::TransferId,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::SealBlobImport { transfer_id, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn commit_blob_import(
        &self,
        transfer_id: fns_protocol::TransferId,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::CommitBlobImport { transfer_id, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn abort_blob_import(
        &self,
        transfer_id: fns_protocol::TransferId,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::AbortBlobImport { transfer_id, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    pub async fn abort_blob_imports(&self) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::AbortBlobImports { tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }

    /// Notify the engine that a blob upload completed successfully.
    /// This flips the outbox entry from AwaitingBlob → Dispatched so the
    /// mutation will be re-sent on the next pending_commands cycle.
    pub async fn blob_uploaded(
        &self,
        operation_id: fns_protocol::OperationId,
    ) -> Result<(), TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::BlobUploaded { operation_id, tx })
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?;
        rx.await
            .map_err(|_| TransportError::new(TransportErrorCode::Core, false))?
    }
}

fn map_err<T>(result: Result<T, fns_sync_core::SyncError>) -> Result<T, TransportError> {
    result.map_err(map_sync_error)
}

fn map_err_named<T>(
    result: Result<T, fns_sync_core::SyncError>,
    call: &'static str,
) -> Result<T, TransportError> {
    result.map_err(|error| {
        if matches!(&error, fns_sync_core::SyncError::ResourceLimit { .. }) {
            tracing::warn!(call, "sync engine work limit reached");
            return TransportError::new(TransportErrorCode::ResourceLimit, true);
        }
        tracing::error!(call, error = ?error, "sync engine call failed");
        TransportError::new(TransportErrorCode::Core, false)
    })
}

fn map_sync_error(error: fns_sync_core::SyncError) -> TransportError {
    if matches!(&error, fns_sync_core::SyncError::ResourceLimit { .. }) {
        tracing::warn!("sync engine work limit reached");
        return TransportError::new(TransportErrorCode::ResourceLimit, true);
    }
    if matches!(&error, fns_sync_core::SyncError::Filesystem(_)) {
        tracing::error!(error = ?error, "sync engine filesystem call failed");
        return TransportError::new(TransportErrorCode::Filesystem, false);
    }
    tracing::error!(error = ?error, "sync engine call failed");
    TransportError::new(TransportErrorCode::Core, false)
}

fn map_conflict_control_error<T>(
    result: Result<T, fns_sync_core::SyncError>,
    call: &'static str,
) -> Result<T, TransportError> {
    result.map_err(|error| {
        use fns_sync_core::{ConflictBlockedReason, SyncError};

        let (code, retryable) = match &error {
            SyncError::InvalidConfiguration { .. } => {
                (TransportErrorCode::InvalidConfiguration, false)
            }
            SyncError::CorruptState { .. } => (TransportErrorCode::StateCorrupt, false),
            SyncError::ProtocolInvariant { .. } | SyncError::StreamInvariant { .. } => {
                (TransportErrorCode::Protocol, false)
            }
            SyncError::ConflictUnavailable => (TransportErrorCode::ConflictUnavailable, false),
            SyncError::ConflictRevisionStale => (TransportErrorCode::ConflictRevisionStale, false),
            SyncError::ConflictResolutionChanged | SyncError::OperationChanged => {
                (TransportErrorCode::ConflictResolutionChanged, false)
            }
            SyncError::ConflictResolutionBlocked { reason } => match reason {
                ConflictBlockedReason::WaitingBlobs => {
                    (TransportErrorCode::ConflictWaitingBlobs, true)
                }
                ConflictBlockedReason::AutomaticResolutionPending => {
                    (TransportErrorCode::ConflictAutomaticResolutionPending, true)
                }
                ConflictBlockedReason::ResolutionPending => {
                    (TransportErrorCode::ConflictResolutionPending, true)
                }
                ConflictBlockedReason::RefreshRequired => {
                    (TransportErrorCode::ConflictRefreshRequired, true)
                }
                ConflictBlockedReason::SelectedSideDeleted => {
                    (TransportErrorCode::ConflictSelectedSideDeleted, false)
                }
            },
            SyncError::MergeRejected {
                reason: "merged_file_required",
            } => (TransportErrorCode::MergeFileRequired, false),
            SyncError::MergeRejected {
                reason: "merged_content_unavailable",
            } => (TransportErrorCode::MergeContentUnavailable, false),
            SyncError::MergeRejected { .. } => (TransportErrorCode::Core, false),
            SyncError::ResourceLimit { .. } => (TransportErrorCode::ResourceLimit, true),
            SyncError::Filesystem(_) | SyncError::ScanIncomplete => {
                (TransportErrorCode::Filesystem, false)
            }
            SyncError::StorageUnavailable => (TransportErrorCode::Core, false),
        };
        if retryable {
            tracing::warn!(call, error = ?error, "sync conflict control call deferred");
        } else {
            tracing::error!(call, error = ?error, "sync conflict control call failed");
        }
        TransportError::new(code, retryable)
    })
}

fn map_blob_stage_error(error: fns_fs::FsError) -> TransportError {
    match error {
        fns_fs::FsError::ContentMismatch | fns_fs::FsError::SizeMismatch => {
            tracing::warn!("workspace blob content failed integrity validation");
            TransportError::new(TransportErrorCode::Protocol, false)
        }
        _ => {
            tracing::error!("workspace blob staging filesystem call failed");
            TransportError::new(TransportErrorCode::Filesystem, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_resource_limit_remains_retryable_at_transport_boundary() {
        let error = map_sync_error(fns_sync_core::SyncError::ResourceLimit {
            resource: "pending_live_events",
        });

        assert_eq!(error.code(), TransportErrorCode::ResourceLimit);
        assert!(error.retryable());
    }

    #[test]
    fn conflict_control_errors_keep_stable_actionable_codes() {
        let cases = [
            (
                fns_sync_core::SyncError::CorruptState {
                    table: "conflicts",
                    field: "created_json",
                },
                TransportErrorCode::StateCorrupt,
                false,
            ),
            (
                fns_sync_core::SyncError::ConflictUnavailable,
                TransportErrorCode::ConflictUnavailable,
                false,
            ),
            (
                fns_sync_core::SyncError::ConflictRevisionStale,
                TransportErrorCode::ConflictRevisionStale,
                false,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionChanged,
                TransportErrorCode::ConflictResolutionChanged,
                false,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionBlocked {
                    reason: fns_sync_core::ConflictBlockedReason::WaitingBlobs,
                },
                TransportErrorCode::ConflictWaitingBlobs,
                true,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionBlocked {
                    reason: fns_sync_core::ConflictBlockedReason::AutomaticResolutionPending,
                },
                TransportErrorCode::ConflictAutomaticResolutionPending,
                true,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionBlocked {
                    reason: fns_sync_core::ConflictBlockedReason::ResolutionPending,
                },
                TransportErrorCode::ConflictResolutionPending,
                true,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionBlocked {
                    reason: fns_sync_core::ConflictBlockedReason::RefreshRequired,
                },
                TransportErrorCode::ConflictRefreshRequired,
                true,
            ),
            (
                fns_sync_core::SyncError::ConflictResolutionBlocked {
                    reason: fns_sync_core::ConflictBlockedReason::SelectedSideDeleted,
                },
                TransportErrorCode::ConflictSelectedSideDeleted,
                false,
            ),
            (
                fns_sync_core::SyncError::MergeRejected {
                    reason: "merged_file_required",
                },
                TransportErrorCode::MergeFileRequired,
                false,
            ),
            (
                fns_sync_core::SyncError::MergeRejected {
                    reason: "merged_content_unavailable",
                },
                TransportErrorCode::MergeContentUnavailable,
                false,
            ),
        ];

        for (source, expected, retryable) in cases {
            let error = map_conflict_control_error::<()>(Err(source), "test")
                .expect_err("mapped conflict error");
            assert_eq!(error.code(), expected);
            assert_eq!(error.retryable(), retryable);
        }

        let filesystem = map_conflict_control_error::<()>(
            Err(fns_sync_core::SyncError::Filesystem(fns_fs::FsError::Io {
                operation: "test",
            })),
            "test",
        )
        .expect_err("mapped filesystem error");
        assert_eq!(filesystem.code(), TransportErrorCode::Filesystem);
    }
}
