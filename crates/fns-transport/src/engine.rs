//! Engine worker: one bounded OS thread serializing every SyncEngine call.
//!
//! The SyncEngine owns SQLite and filesystem state that must not be shared across
//! threads. This module runs a dedicated `fns-sync-engine` thread that owns the
//! engine and processes calls from the async side through a bounded channel.

use crate::ENGINE_QUEUE_CAPACITY;
use crate::error::{TransportError, TransportErrorCode};

use fns_sync_core::{SyncEngine, WorkspaceCursor};
use tokio::sync::{mpsc, oneshot};

/// Private request variants — each maps to exactly one SyncEngine method.
#[allow(dead_code)] // Not all variants are used until dispatch/stream tasks land.
enum EngineCall {
    Cursor {
        tx: oneshot::Sender<Result<WorkspaceCursor, TransportError>>,
    },
    ActiveStreamMode {
        tx: oneshot::Sender<Result<Option<fns_sync_core::StreamMode>, TransportError>>,
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
    AckConfirmed {
        message: fns_protocol::WorkspaceAckRequest,
        tx: oneshot::Sender<Result<(), TransportError>>,
    },
    OpenBlob {
        content_hash: fns_protocol::WorkspaceContentHash,
        tx: oneshot::Sender<Result<std::fs::File, TransportError>>,
    },
    BlobAvailable {
        content_hash: fns_protocol::WorkspaceContentHash,
        size: u64,
        data: Vec<u8>,
        tx: oneshot::Sender<Result<Vec<fns_sync_core::SyncCommand>, TransportError>>,
    },
    BlobUploaded {
        operation_id: fns_protocol::OperationId,
        tx: oneshot::Sender<Result<(), TransportError>>,
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
                        process_call(&mut engine, call);
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

fn process_call(engine: &mut SyncEngine, call: EngineCall) {
    match call {
        EngineCall::Cursor { tx } => {
            let _ = tx.send(map_err(engine.cursor()));
        }
        EngineCall::ActiveStreamMode { tx } => {
            let _ = tx.send(map_err(engine.active_stream_mode()));
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
        EngineCall::AckConfirmed { message, tx } => {
            let _ = tx.send(map_err_named(
                engine.ack_confirmed(message),
                "ack_confirmed",
            ));
        }
        EngineCall::OpenBlob { content_hash, tx } => {
            let _ = tx.send(map_err(engine.open_blob(&content_hash)));
        }
        EngineCall::BlobAvailable {
            content_hash,
            size,
            data,
            tx,
        } => {
            let reader = std::io::Cursor::new(data);
            let _ = tx.send(map_err_named(
                engine.blob_available(content_hash, size, reader),
                "blob_available",
            ));
        }
        EngineCall::BlobUploaded { operation_id, tx } => {
            let _ = tx.send(map_err(engine.blob_uploaded(operation_id)));
        }
        EngineCall::Shutdown { tx } => {
            let _ = tx.send(map_err(engine.close()));
        }
    }
}

impl EngineHandle {
    pub async fn cursor(&self) -> Result<WorkspaceCursor, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::Cursor { tx })
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

    pub async fn blob_available(
        &self,
        content_hash: fns_protocol::WorkspaceContentHash,
        size: u64,
        data: Vec<u8>,
    ) -> Result<Vec<fns_sync_core::SyncCommand>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EngineCall::BlobAvailable {
                content_hash,
                size,
                data,
                tx,
            })
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

fn map_err<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, TransportError> {
    result.map_err(|error| {
        tracing::error!(error = ?error, "sync engine call failed");
        TransportError::new(TransportErrorCode::Core, false)
    })
}

fn map_err_named<T, E: std::fmt::Debug>(
    result: Result<T, E>,
    call: &'static str,
) -> Result<T, TransportError> {
    result.map_err(|error| {
        tracing::error!(call, error = ?error, "sync engine call failed");
        TransportError::new(TransportErrorCode::Core, false)
    })
}
