use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use fns_protocol::revision::WorkspaceConflictRevision;
use fns_protocol::{
    ClientId, ConflictId, OperationId, RequiredNullable, StreamId, WorkspaceAckRequest,
    WorkspaceConflictChoice, WorkspaceConflictCreatedMessage, WorkspaceConflictKind,
    WorkspaceConflictResolvedMessage, WorkspaceConflictSide, WorkspaceContentHash,
    WorkspaceEntryKind, WorkspaceEventMessage, WorkspaceFileMetadata, WorkspaceId,
    WorkspaceMutation, WorkspaceMutationKind, WorkspacePath, WorkspacePathState, WorkspaceRevision,
    WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage, WorkspaceSnapshotMode,
};
use fns_sync_core::{
    AppliedOperationReceiptKind, ApplyCommitPlan, ApplyItemKind, ApplyJournalRecord,
    ApplyNamespace, ApplyStage, ConflictRecord, ConflictStatus, LocalIntentRecord, OutboxRecord,
    PathStateRecord, SqliteState, SyncCommand, SyncEngine, SyncEngineConfig, SyncError,
    WorkspaceCursor,
};
use rusqlite::{Connection, params};

const CHILD_ENV: &str = "FNS_APPLY_JOURNAL_CRASH_CHILD";
const SCENARIO_ENV: &str = "FNS_APPLY_JOURNAL_SCENARIO";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Boundary {
    Prepared,
    FilesystemStarted,
    FilesystemApplied,
    DatabaseCommitted,
    FilesystemFinalized,
    Finalized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterBoundary {
    TempSynced,
    PreimageValidated,
    DestinationBackedUp,
    SourceBackedUp,
    FilesystemCommitted,
}

impl WriterBoundary {
    const ALL: [Self; 5] = [
        Self::TempSynced,
        Self::PreimageValidated,
        Self::DestinationBackedUp,
        Self::SourceBackedUp,
        Self::FilesystemCommitted,
    ];

    const fn failpoint(self) -> &'static str {
        match self {
            Self::TempSynced => "temp_synced",
            Self::PreimageValidated => "preimage_validated",
            Self::DestinationBackedUp => "destination_backed_up",
            Self::SourceBackedUp => "source_backed_up",
            Self::FilesystemCommitted => "filesystem_committed",
        }
    }
}

impl Boundary {
    const ALL: [Self; 6] = [
        Self::Prepared,
        Self::FilesystemStarted,
        Self::FilesystemApplied,
        Self::DatabaseCommitted,
        Self::FilesystemFinalized,
        Self::Finalized,
    ];

    const fn failpoint(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::FilesystemStarted => "filesystem_started",
            Self::FilesystemApplied => "filesystem_applied",
            Self::DatabaseCommitted => "database_committed",
            Self::FilesystemFinalized => "filesystem_finalized",
            Self::Finalized => "finalized",
        }
    }

    const fn durable_stage(self) -> ApplyStage {
        match self {
            Self::Prepared => ApplyStage::Prepared,
            Self::FilesystemStarted => ApplyStage::FilesystemStarted,
            Self::FilesystemApplied => ApplyStage::FilesystemApplied,
            Self::DatabaseCommitted | Self::FilesystemFinalized => ApplyStage::DatabaseCommitted,
            Self::Finalized => ApplyStage::Finalized,
        }
    }

    const fn has_filesystem_receipt(self) -> bool {
        matches!(
            self,
            Self::FilesystemApplied
                | Self::DatabaseCommitted
                | Self::FilesystemFinalized
                | Self::Finalized
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    FileCreateEmpty,
    FileUpdateBinaryExecutable,
    FileUpdateLargeStreamed,
    FileDelete,
    FileRename,
    DirectoryCreate,
    DirectoryUpdateReplace,
    DirectoryDeleteNested,
    DirectoryRenameNested,
    LiveConflictResolved,
}

impl Scenario {
    const ALL: [Self; 10] = [
        Self::FileCreateEmpty,
        Self::FileUpdateBinaryExecutable,
        Self::FileUpdateLargeStreamed,
        Self::FileDelete,
        Self::FileRename,
        Self::DirectoryCreate,
        Self::DirectoryUpdateReplace,
        Self::DirectoryDeleteNested,
        Self::DirectoryRenameNested,
        Self::LiveConflictResolved,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::FileCreateEmpty => "file_create_empty",
            Self::FileUpdateBinaryExecutable => "file_update_binary_executable",
            Self::FileUpdateLargeStreamed => "file_update_large_streamed",
            Self::FileDelete => "file_delete",
            Self::FileRename => "file_rename",
            Self::DirectoryCreate => "directory_create",
            Self::DirectoryUpdateReplace => "directory_update_replace",
            Self::DirectoryDeleteNested => "directory_delete_nested",
            Self::DirectoryRenameNested => "directory_rename_nested",
            Self::LiveConflictResolved => "live_conflict_resolved",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == value)
    }

    const fn operation_number(self) -> u32 {
        match self {
            Self::FileCreateEmpty => 101,
            Self::FileUpdateBinaryExecutable => 102,
            Self::FileUpdateLargeStreamed => 103,
            Self::FileDelete => 104,
            Self::FileRename => 105,
            Self::DirectoryCreate => 106,
            Self::DirectoryUpdateReplace => 107,
            Self::DirectoryDeleteNested => 108,
            Self::DirectoryRenameNested => 109,
            Self::LiveConflictResolved => 110,
        }
    }

    fn operation_id(self) -> OperationId {
        operation_id(self.operation_number())
    }

    const fn is_live(self) -> bool {
        matches!(self, Self::LiveConflictResolved)
    }

    const fn reaches_writer_boundary(self, boundary: WriterBoundary) -> bool {
        match boundary {
            WriterBoundary::TempSynced => matches!(
                self,
                Self::FileCreateEmpty
                    | Self::FileUpdateBinaryExecutable
                    | Self::FileUpdateLargeStreamed
                    | Self::LiveConflictResolved
            ),
            WriterBoundary::PreimageValidated | WriterBoundary::FilesystemCommitted => true,
            WriterBoundary::DestinationBackedUp => matches!(
                self,
                Self::FileUpdateBinaryExecutable
                    | Self::FileDelete
                    | Self::FileRename
                    | Self::DirectoryUpdateReplace
                    | Self::DirectoryDeleteNested
                    | Self::DirectoryRenameNested
                    | Self::LiveConflictResolved
            ),
            WriterBoundary::SourceBackedUp => matches!(
                self,
                Self::FileDelete
                    | Self::FileRename
                    | Self::DirectoryUpdateReplace
                    | Self::DirectoryDeleteNested
                    | Self::DirectoryRenameNested
            ),
        }
    }

    fn desired_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::FileCreateEmpty => Some(Vec::new()),
            Self::FileUpdateBinaryExecutable => Some(b"new\0binary\xffpayload".to_vec()),
            Self::FileUpdateLargeStreamed => Some(large_bytes()),
            Self::FileRename => Some(b"rename-file\0bytes".to_vec()),
            Self::LiveConflictResolved => Some(b"resolved\0current".to_vec()),
            Self::FileDelete
            | Self::DirectoryCreate
            | Self::DirectoryUpdateReplace
            | Self::DirectoryDeleteNested
            | Self::DirectoryRenameNested => None,
        }
    }

    fn event(self) -> WorkspaceEventMessage {
        assert!(!self.is_live());
        let (path, kind, content_hash, metadata, new_path, old_path_state, new_path_state) =
            match self {
                Self::FileCreateEmpty => {
                    let bytes = self.desired_bytes().unwrap();
                    (
                        "empty.txt",
                        WorkspaceMutationKind::UpsertFile,
                        RequiredNullable::Value(content_hash(&bytes)),
                        file_metadata(&bytes, false),
                        None,
                        None,
                        None,
                    )
                }
                Self::FileUpdateBinaryExecutable => {
                    let bytes = self.desired_bytes().unwrap();
                    (
                        "binary/update.bin",
                        WorkspaceMutationKind::UpsertFile,
                        RequiredNullable::Value(content_hash(&bytes)),
                        file_metadata(&bytes, true),
                        None,
                        None,
                        None,
                    )
                }
                Self::FileUpdateLargeStreamed => {
                    let bytes = self.desired_bytes().unwrap();
                    (
                        "large/nested/streamed.bin",
                        WorkspaceMutationKind::UpsertFile,
                        RequiredNullable::Value(content_hash(&bytes)),
                        file_metadata(&bytes, false),
                        None,
                        None,
                        None,
                    )
                }
                Self::FileDelete => (
                    "delete/file.txt",
                    WorkspaceMutationKind::Delete,
                    RequiredNullable::Null,
                    zero_metadata(),
                    None,
                    None,
                    None,
                ),
                Self::FileRename => {
                    let bytes = self.desired_bytes().unwrap();
                    let metadata = file_metadata(&bytes, false);
                    let old = tombstone_state("rename/file-old.bin");
                    let new = state(
                        "rename/file-new.bin",
                        WorkspaceEntryKind::File,
                        RequiredNullable::Value(content_hash(&bytes)),
                        metadata.clone(),
                    );
                    (
                        "rename/file-old.bin",
                        WorkspaceMutationKind::Rename,
                        new.content_hash.clone(),
                        metadata,
                        Some("rename/file-new.bin"),
                        Some(old),
                        Some(new),
                    )
                }
                Self::DirectoryCreate => (
                    "directory-created",
                    WorkspaceMutationKind::Mkdir,
                    RequiredNullable::Null,
                    zero_metadata(),
                    None,
                    None,
                    None,
                ),
                Self::DirectoryUpdateReplace => {
                    let old = tombstone_state("directory-update-incoming");
                    let new = directory_state("directory-update");
                    (
                        "directory-update-incoming",
                        WorkspaceMutationKind::Rename,
                        RequiredNullable::Null,
                        zero_metadata(),
                        Some("directory-update"),
                        Some(old),
                        Some(new),
                    )
                }
                Self::DirectoryDeleteNested => (
                    "directory-delete",
                    WorkspaceMutationKind::Delete,
                    RequiredNullable::Null,
                    zero_metadata(),
                    None,
                    None,
                    None,
                ),
                Self::DirectoryRenameNested => {
                    let old = tombstone_state("directory-old");
                    let new = directory_state("directory-new");
                    (
                        "directory-old",
                        WorkspaceMutationKind::Rename,
                        RequiredNullable::Null,
                        zero_metadata(),
                        Some("directory-new"),
                        Some(old),
                        Some(new),
                    )
                }
                Self::LiveConflictResolved => unreachable!(),
            };
        let path = workspace_path(path);
        let path_state = match kind {
            WorkspaceMutationKind::Delete => tombstone_state(path.as_str()),
            WorkspaceMutationKind::Rename => new_path_state.clone().unwrap(),
            WorkspaceMutationKind::Mkdir => directory_state(path.as_str()),
            WorkspaceMutationKind::UpsertFile => state(
                path.as_str(),
                WorkspaceEntryKind::File,
                content_hash.clone(),
                metadata.clone(),
            ),
            WorkspaceMutationKind::UpsertSymlink => unreachable!(),
        };
        let mutation = WorkspaceMutation {
            workspace_id: workspace_id(),
            client_id: remote_client_id(),
            operation_id: self.operation_id(),
            path,
            base_path_revision: WorkspaceRevision::ZERO,
            kind,
            content_hash,
            metadata,
            new_path: new_path.map(workspace_path),
            target_base_path_revision: new_path.map(|_| WorkspaceRevision::ZERO),
        };
        let event = WorkspaceEventMessage {
            workspace_id: workspace_id(),
            stream_id: stream_id(),
            index: 0,
            revision: WorkspaceRevision::new(1),
            operation_id: mutation.operation_id,
            origin_client_id: mutation.client_id,
            mutation,
            path_state,
            old_path_state,
            new_path_state,
        };
        event.validate().unwrap();
        event
    }

    fn conflict_created(self) -> WorkspaceConflictCreatedMessage {
        assert!(self.is_live());
        let path = workspace_path("conflict/live.bin");
        let old = conflict_side(path.clone(), b"conflict-old");
        let current = conflict_side(path.clone(), &self.desired_bytes().unwrap());
        let incoming = conflict_side(path.clone(), b"conflict-incoming");
        let message = WorkspaceConflictCreatedMessage {
            workspace_id: workspace_id(),
            conflict_id: conflict_id(),
            conflict_revision: conflict_revision(),
            path,
            kind: WorkspaceConflictKind::Content,
            ancestor: old,
            current,
            incoming,
            created_by_operation_id: operation_id(900),
        };
        message.validate().unwrap();
        message
    }

    fn conflict_resolved(self) -> WorkspaceConflictResolvedMessage {
        assert!(self.is_live());
        let bytes = self.desired_bytes().unwrap();
        let message = WorkspaceConflictResolvedMessage {
            workspace_id: workspace_id(),
            conflict_id: conflict_id(),
            conflict_revision: conflict_revision(),
            operation_id: self.operation_id(),
            revision: WorkspaceRevision::new(1),
            choice: WorkspaceConflictChoice::Current,
            path_state: state(
                "conflict/live.bin",
                WorkspaceEntryKind::File,
                RequiredNullable::Value(content_hash(&bytes)),
                file_metadata(&bytes, false),
            ),
            resolved_by_client_id: remote_client_id(),
        };
        message.validate().unwrap();
        message
    }

    fn expected_states(self) -> Vec<WorkspacePathState> {
        if self.is_live() {
            return vec![self.conflict_resolved().path_state];
        }
        let event = self.event();
        if event.mutation.kind == WorkspaceMutationKind::Rename {
            vec![event.old_path_state.unwrap(), event.new_path_state.unwrap()]
        } else {
            vec![event.path_state]
        }
    }

    fn expected_touched(self) -> Vec<WorkspacePath> {
        self.expected_states()
            .into_iter()
            .map(|state| state.path)
            .collect()
    }

    const fn expected_namespace(self) -> ApplyNamespace {
        if self.is_live() {
            ApplyNamespace::LiveConflictResolved
        } else {
            ApplyNamespace::StreamEvent
        }
    }

    fn seed(self, workspace: &Path, engine: &mut SyncEngine) {
        match self {
            Self::FileCreateEmpty | Self::FileUpdateLargeStreamed | Self::DirectoryCreate => {}
            Self::FileUpdateBinaryExecutable => {
                seed_file_state(
                    engine,
                    workspace,
                    "binary/update.bin",
                    b"old\0binary",
                    false,
                );
            }
            Self::FileDelete => {
                seed_file_state(engine, workspace, "delete/file.txt", b"delete-me", false);
            }
            Self::FileRename => {
                seed_file_state(
                    engine,
                    workspace,
                    "rename/file-old.bin",
                    &self.desired_bytes().unwrap(),
                    false,
                );
            }
            Self::DirectoryUpdateReplace => {
                write_file(
                    workspace,
                    "directory-update-incoming/nested/new.bin",
                    b"new-tree\0bytes",
                    true,
                );
                fs::create_dir_all(workspace.join("directory-update-incoming/empty")).unwrap();
                write_file(workspace, "directory-update/old.bin", b"old-tree", false);
                seed_directory_state(engine, workspace, "directory-update-incoming");
                seed_directory_state(engine, workspace, "directory-update");
            }
            Self::DirectoryDeleteNested => {
                write_file(
                    workspace,
                    "directory-delete/nested/data.bin",
                    b"delete-tree\0bytes",
                    false,
                );
                fs::create_dir_all(workspace.join("directory-delete/empty/deep")).unwrap();
                seed_directory_state(engine, workspace, "directory-delete");
            }
            Self::DirectoryRenameNested => {
                write_file(
                    workspace,
                    "directory-old/nested/deep.bin",
                    b"nested-tree\0bytes",
                    true,
                );
                fs::create_dir_all(workspace.join("directory-old/empty/deep")).unwrap();
                seed_directory_state(engine, workspace, "directory-old");
            }
            Self::LiveConflictResolved => {
                seed_file_state(
                    engine,
                    workspace,
                    "conflict/live.bin",
                    b"conflict-old",
                    false,
                );
                engine.conflict_created(self.conflict_created()).unwrap();
            }
        }
    }

    fn assert_workspace(self, workspace: &Path) {
        match self {
            Self::FileCreateEmpty => assert_file(workspace, "empty.txt", b"", false),
            Self::FileUpdateBinaryExecutable => assert_file(
                workspace,
                "binary/update.bin",
                &self.desired_bytes().unwrap(),
                true,
            ),
            Self::FileUpdateLargeStreamed => assert_file(
                workspace,
                "large/nested/streamed.bin",
                &self.desired_bytes().unwrap(),
                false,
            ),
            Self::FileDelete => assert_missing(workspace, "delete/file.txt"),
            Self::FileRename => {
                assert_missing(workspace, "rename/file-old.bin");
                assert_file(
                    workspace,
                    "rename/file-new.bin",
                    &self.desired_bytes().unwrap(),
                    false,
                );
            }
            Self::DirectoryCreate => assert_directory(workspace, "directory-created"),
            Self::DirectoryUpdateReplace => {
                assert_missing(workspace, "directory-update-incoming");
                assert_directory(workspace, "directory-update");
                assert_file(
                    workspace,
                    "directory-update/nested/new.bin",
                    b"new-tree\0bytes",
                    true,
                );
                assert_directory(workspace, "directory-update/empty");
                assert_missing(workspace, "directory-update/old.bin");
            }
            Self::DirectoryDeleteNested => assert_missing(workspace, "directory-delete"),
            Self::DirectoryRenameNested => {
                assert_missing(workspace, "directory-old");
                assert_directory(workspace, "directory-new");
                assert_file(
                    workspace,
                    "directory-new/nested/deep.bin",
                    b"nested-tree\0bytes",
                    true,
                );
                assert_directory(workspace, "directory-new/empty/deep");
            }
            Self::LiveConflictResolved => assert_file(
                workspace,
                "conflict/live.bin",
                &self.desired_bytes().unwrap(),
                false,
            ),
        }
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("10000000-0000-4000-8000-000000000001").unwrap()
}

fn client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000002").unwrap()
}

fn remote_client_id() -> ClientId {
    ClientId::parse("10000000-0000-4000-8000-000000000004").unwrap()
}

fn stream_id() -> StreamId {
    StreamId::parse("10000000-0000-4000-8000-000000000093").unwrap()
}

fn operation_id(number: u32) -> OperationId {
    OperationId::parse(&format!("10000000-0000-4000-8000-{number:012}")).unwrap()
}

fn conflict_id() -> ConflictId {
    ConflictId::parse("10000000-0000-4000-8000-000000000030").unwrap()
}

fn conflict_revision() -> WorkspaceConflictRevision {
    WorkspaceConflictRevision::parse("1").unwrap()
}

fn config(workspace: &Path, state: &Path) -> SyncEngineConfig {
    SyncEngineConfig::new(workspace_id(), client_id(), workspace, state)
}

fn content_hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex())).unwrap()
}

fn workspace_path(path: &str) -> WorkspacePath {
    WorkspacePath::parse(path).unwrap()
}

fn zero_metadata() -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: 0,
        modified_at_ms: 0,
        executable: false,
    }
}

fn file_metadata(bytes: &[u8], executable: bool) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size: bytes.len() as u64,
        modified_at_ms: 0,
        executable,
    }
}

fn state(
    path: &str,
    kind: WorkspaceEntryKind,
    content_hash: RequiredNullable<WorkspaceContentHash>,
    metadata: WorkspaceFileMetadata,
) -> WorkspacePathState {
    WorkspacePathState {
        path: workspace_path(path),
        path_revision: WorkspaceRevision::new(1),
        kind,
        content_hash,
        metadata,
        tombstone: kind == WorkspaceEntryKind::Tombstone,
    }
}

fn directory_state(path: &str) -> WorkspacePathState {
    state(
        path,
        WorkspaceEntryKind::Directory,
        RequiredNullable::Null,
        zero_metadata(),
    )
}

fn tombstone_state(path: &str) -> WorkspacePathState {
    state(
        path,
        WorkspaceEntryKind::Tombstone,
        RequiredNullable::Null,
        zero_metadata(),
    )
}

fn conflict_side(path: WorkspacePath, bytes: &[u8]) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Value(path),
        path_revision: WorkspaceRevision::ZERO,
        content_hash: RequiredNullable::Value(content_hash(bytes)),
        metadata: file_metadata(bytes, false),
        tombstone: false,
    }
}

fn begin() -> WorkspaceSnapshotBeginMessage {
    WorkspaceSnapshotBeginMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        mode: WorkspaceSnapshotMode::Incremental,
        from_revision: WorkspaceRevision::ZERO,
        final_revision: WorkspaceRevision::new(1),
        entry_count: 0,
        event_count: 1,
        conflict_count: 0,
    }
}

fn end() -> WorkspaceSnapshotEndMessage {
    WorkspaceSnapshotEndMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        mode: WorkspaceSnapshotMode::Incremental,
        delivered_count: 1,
        final_revision: WorkspaceRevision::new(1),
    }
}

fn ack() -> WorkspaceAckRequest {
    WorkspaceAckRequest {
        workspace_id: workspace_id(),
        client_id: client_id(),
        revision: WorkspaceRevision::new(1),
    }
}

fn large_bytes() -> Vec<u8> {
    const HASH_BUFFER_BYTES: usize = 262_144;

    (0..(3 * HASH_BUFFER_BYTES + 17))
        .map(|index| ((index * 31 + 7) % 251) as u8)
        .collect()
}

fn seed_file_state(
    engine: &mut SyncEngine,
    workspace: &Path,
    path: &str,
    bytes: &[u8],
    executable: bool,
) {
    write_file(workspace, path, bytes, executable);
    let rooted = fns_fs::RootedWorkspace::open(workspace).unwrap();
    let observed = rooted.inspect(&workspace_path(path)).unwrap().unwrap();
    engine
        .state_mut()
        .put_path_state(&WorkspacePathState {
            path: workspace_path(path),
            path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceEntryKind::File,
            content_hash: RequiredNullable::Value(content_hash(bytes)),
            metadata: observed.metadata,
            tombstone: false,
        })
        .unwrap();
}

fn seed_directory_state(engine: &mut SyncEngine, workspace: &Path, path: &str) {
    fs::create_dir_all(workspace.join(path)).unwrap();
    let rooted = fns_fs::RootedWorkspace::open(workspace).unwrap();
    let observed = rooted.inspect(&workspace_path(path)).unwrap().unwrap();
    engine
        .state_mut()
        .put_path_state(&WorkspacePathState {
            path: workspace_path(path),
            path_revision: WorkspaceRevision::ZERO,
            kind: WorkspaceEntryKind::Directory,
            content_hash: RequiredNullable::Null,
            metadata: observed.metadata,
            tombstone: false,
        })
        .unwrap();
}

fn write_file(workspace: &Path, path: &str, bytes: &[u8], executable: bool) {
    let path = workspace.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    set_executable(&path, executable);
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).unwrap().permissions();
    let mode = permissions.mode();
    permissions.set_mode(if executable {
        mode | 0o100
    } else {
        mode & !0o111
    });
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) {}

struct ChunkedReader {
    inner: Cursor<Vec<u8>>,
    max_chunk: usize,
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = buffer.len().min(self.max_chunk);
        self.inner.read(&mut buffer[..limit])
    }
}

fn provide_download(engine: &mut SyncEngine, scenario: Scenario, commands: &[SyncCommand]) {
    let bytes = scenario
        .desired_bytes()
        .expect("content-bearing apply returned without aborting");
    let hash = content_hash(&bytes);
    assert!(commands.iter().any(|command| matches!(command,
        SyncCommand::DownloadBlob { content_hash, size, .. }
            if *content_hash == hash && *size == bytes.len() as u64
    )));
    engine
        .blob_available(
            hash,
            bytes.len() as u64,
            ChunkedReader {
                inner: Cursor::new(bytes),
                max_chunk: 8191,
            },
        )
        .unwrap();
}

#[test]
fn apply_journal_crash_child() {
    let Some(paths) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let scenario =
        Scenario::parse(&std::env::var(SCENARIO_ENV).expect("child scenario must be provided"))
            .expect("known child scenario");
    let mut parts = std::env::split_paths(&paths);
    let workspace = parts.next().expect("workspace path");
    let state = parts.next().expect("state path");
    let mut engine = SyncEngine::open(config(&workspace, &state)).unwrap();
    scenario.seed(&workspace, &mut engine);

    if scenario.is_live() {
        let commands = engine
            .conflict_resolved(scenario.conflict_resolved())
            .unwrap();
        provide_download(&mut engine, scenario, &commands);
    } else {
        engine.snapshot_begin(begin()).unwrap();
        let commands = engine.workspace_event(scenario.event()).unwrap();
        if !commands.is_empty() {
            provide_download(&mut engine, scenario, &commands);
        }
    }
    panic!("apply failpoint did not terminate child");
}

#[test]
fn production_apply_crash_matrix_recovers_and_settles_exactly() {
    for scenario in Scenario::ALL {
        for boundary in Boundary::ALL {
            let workspace = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            crash_child(scenario, boundary, workspace.path(), state.path());
            assert_durable_journal(scenario, boundary, state.path());
            settle_after_crash(scenario, workspace.path(), state.path(), false);
        }
    }
}

#[test]
fn production_writer_substep_crash_matrix_recovers_and_settles_exactly() {
    for scenario in Scenario::ALL {
        for boundary in WriterBoundary::ALL {
            if !scenario.reaches_writer_boundary(boundary) {
                continue;
            }
            let workspace = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            crash_child_writer(scenario, boundary, workspace.path(), state.path());
            assert_durable_journal(scenario, Boundary::FilesystemStarted, state.path());
            settle_after_crash(scenario, workspace.path(), state.path(), false);
        }
    }
}

#[test]
fn newer_local_divergence_is_durable_across_every_crash_boundary() {
    let scenario = Scenario::FileUpdateBinaryExecutable;
    for boundary in Boundary::ALL {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        crash_child(scenario, boundary, workspace.path(), state.path());
        assert_durable_journal(scenario, boundary, state.path());

        let local = format!("local-newer-after-{}\0", boundary.failpoint()).into_bytes();
        write_file(workspace.path(), "binary/update.bin", &local, false);
        settle_after_crash(scenario, workspace.path(), state.path(), true);
        assert_file(workspace.path(), "binary/update.bin", &local, false);
        assert_preserved_local_outbox(state.path(), &local);
    }
}

#[test]
fn directory_descendant_edit_is_preserved_before_filesystem_apply() {
    for (scenario, descendant) in [
        (
            Scenario::DirectoryDeleteNested,
            "directory-delete/nested/data.bin",
        ),
        (
            Scenario::DirectoryRenameNested,
            "directory-old/nested/deep.bin",
        ),
        (Scenario::DirectoryUpdateReplace, "directory-update/old.bin"),
    ] {
        for boundary in [Boundary::Prepared, Boundary::FilesystemStarted] {
            let workspace = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            crash_child(scenario, boundary, workspace.path(), state.path());
            let local = format!("local-directory-edit-{}", boundary.failpoint()).into_bytes();
            write_file(workspace.path(), descendant, &local, false);

            settle_after_crash(scenario, workspace.path(), state.path(), true);
            assert_file(workspace.path(), descendant, &local, false);

            let state = SqliteState::open(
                state.path().join("state.sqlite"),
                workspace_id(),
                client_id(),
            )
            .unwrap();
            let mutations = state
                .outbox()
                .unwrap()
                .into_iter()
                .map(|record| record.mutation().unwrap())
                .collect::<Vec<_>>();
            assert!(mutations.iter().any(|mutation| {
                mutation.path == workspace_path(descendant)
                    && mutation.kind == WorkspaceMutationKind::UpsertFile
                    && mutation.content_hash == RequiredNullable::Value(content_hash(&local))
                    && mutation.metadata.size == local.len() as u64
            }));
        }
    }
}

#[test]
fn directory_descendant_delete_is_preserved_before_filesystem_apply() {
    for (scenario, descendant) in [
        (
            Scenario::DirectoryDeleteNested,
            "directory-delete/nested/data.bin",
        ),
        (
            Scenario::DirectoryRenameNested,
            "directory-old/nested/deep.bin",
        ),
        (Scenario::DirectoryUpdateReplace, "directory-update/old.bin"),
    ] {
        for boundary in [Boundary::Prepared, Boundary::FilesystemStarted] {
            let workspace = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            crash_child(scenario, boundary, workspace.path(), state.path());
            fs::remove_file(workspace.path().join(descendant)).unwrap();

            settle_after_crash(scenario, workspace.path(), state.path(), true);
            assert_missing(workspace.path(), descendant);

            let state = SqliteState::open(
                state.path().join("state.sqlite"),
                workspace_id(),
                client_id(),
            )
            .unwrap();
            let mutations = state
                .outbox()
                .unwrap()
                .into_iter()
                .map(|record| record.mutation().unwrap())
                .collect::<Vec<_>>();
            assert!(
                !mutations.iter().any(|mutation| {
                    mutation.path == workspace_path(descendant)
                        && matches!(
                            mutation.kind,
                            WorkspaceMutationKind::UpsertFile | WorkspaceMutationKind::Mkdir
                        )
                }),
                "{scenario:?}/{boundary:?}: {mutations:?}"
            );
            assert!(!mutations.is_empty(), "{scenario:?}/{boundary:?}");
        }
    }
}

#[test]
fn directory_descendant_add_is_preserved_before_filesystem_apply() {
    for (scenario, descendant) in [
        (
            Scenario::DirectoryDeleteNested,
            "directory-delete/local-added.bin",
        ),
        (
            Scenario::DirectoryRenameNested,
            "directory-old/local-added.bin",
        ),
        (
            Scenario::DirectoryUpdateReplace,
            "directory-update/local-added.bin",
        ),
    ] {
        for boundary in [Boundary::Prepared, Boundary::FilesystemStarted] {
            let workspace = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            crash_child(scenario, boundary, workspace.path(), state.path());
            let local = format!("local-directory-add-{}\0", boundary.failpoint()).into_bytes();
            write_file(workspace.path(), descendant, &local, true);

            settle_after_crash(scenario, workspace.path(), state.path(), true);
            assert_file(workspace.path(), descendant, &local, true);

            let state = SqliteState::open(
                state.path().join("state.sqlite"),
                workspace_id(),
                client_id(),
            )
            .unwrap();
            let mutations = state
                .outbox()
                .unwrap()
                .into_iter()
                .map(|record| record.mutation().unwrap())
                .collect::<Vec<_>>();
            assert!(
                mutations.iter().any(|mutation| {
                    mutation.path == workspace_path(descendant)
                        && mutation.kind == WorkspaceMutationKind::UpsertFile
                        && mutation.content_hash == RequiredNullable::Value(content_hash(&local))
                        && mutation.metadata.size == local.len() as u64
                        && mutation.metadata.executable
                }),
                "{scenario:?}/{boundary:?}: {mutations:?}"
            );
        }
    }
}

#[test]
fn migrated_filesystem_started_preimage_never_blocks_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    write_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
    seed_v3_delete_journal(state.path(), ApplyStage::FilesystemStarted);

    let mut first = SyncEngine::open(config(workspace.path(), state.path())).unwrap();
    assert!(first.state().apply_journals().unwrap().is_empty());
    assert_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
    first.close().unwrap();

    let mut second = SyncEngine::open(config(workspace.path(), state.path())).unwrap();
    assert!(second.state().apply_journals().unwrap().is_empty());
    assert_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
    second.close().unwrap();
}

#[test]
fn migrated_v1_v2_v3_rows_converge_from_preimage_partial_and_postimage() {
    const APPLY_ID: &str = "10000000-0000-4000-8000-000000000077";
    for version in [1, 2, 3] {
        for stage in [ApplyStage::Prepared, ApplyStage::FilesystemStarted] {
            let workspace = tempfile::tempdir().unwrap();
            let state_root = tempfile::tempdir().unwrap();
            write_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
            seed_legacy_journal(
                state_root.path(),
                version,
                stage,
                fns_sync_core::model::RemoteApplyOperation::Delete {
                    state: tombstone_state("legacy.txt"),
                },
            );
            assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
            assert_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
        }

        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        write_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
        fs::rename(
            workspace.path().join("legacy.txt"),
            workspace.path().join(format!(".fns-delete-{APPLY_ID}")),
        )
        .unwrap();
        seed_legacy_journal(
            state_root.path(),
            version,
            ApplyStage::FilesystemStarted,
            fns_sync_core::model::RemoteApplyOperation::Delete {
                state: tombstone_state("legacy.txt"),
            },
        );
        assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
        assert_file(workspace.path(), "legacy.txt", b"legacy-bytes", false);
        assert_missing(workspace.path(), &format!(".fns-delete-{APPLY_ID}"));

        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        write_file(workspace.path(), "legacy.txt", b"old", false);
        fs::rename(
            workspace.path().join("legacy.txt"),
            workspace.path().join(format!(".fns-delete-{APPLY_ID}")),
        )
        .unwrap();
        write_file(
            workspace.path(),
            &format!(".fns-tmp-{APPLY_ID}"),
            b"remote",
            false,
        );
        seed_legacy_journal(
            state_root.path(),
            version,
            ApplyStage::FilesystemStarted,
            fns_sync_core::model::RemoteApplyOperation::Upsert {
                state: state(
                    "legacy.txt",
                    WorkspaceEntryKind::File,
                    RequiredNullable::Value(content_hash(b"remote")),
                    file_metadata(b"remote", false),
                ),
            },
        );
        assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
        assert_file(workspace.path(), "legacy.txt", b"old", false);
        assert_missing(workspace.path(), &format!(".fns-delete-{APPLY_ID}"));
        assert_missing(workspace.path(), &format!(".fns-tmp-{APPLY_ID}"));

        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        write_file(workspace.path(), "legacy-source/data.bin", b"source", true);
        write_file(workspace.path(), "legacy-target/old.bin", b"target", false);
        fs::rename(
            workspace.path().join("legacy-target"),
            workspace.path().join(format!(".fns-delete-{APPLY_ID}")),
        )
        .unwrap();
        fs::rename(
            workspace.path().join("legacy-source"),
            workspace.path().join(format!(".fns-rename-{APPLY_ID}")),
        )
        .unwrap();
        seed_legacy_journal(
            state_root.path(),
            version,
            ApplyStage::FilesystemStarted,
            fns_sync_core::model::RemoteApplyOperation::Rename {
                old_state: tombstone_state("legacy-source"),
                new_state: directory_state("legacy-target"),
            },
        );
        assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
        assert_file(workspace.path(), "legacy-source/data.bin", b"source", true);
        assert_file(workspace.path(), "legacy-target/old.bin", b"target", false);
        assert_missing(workspace.path(), &format!(".fns-delete-{APPLY_ID}"));
        assert_missing(workspace.path(), &format!(".fns-rename-{APPLY_ID}"));

        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        seed_legacy_journal(
            state_root.path(),
            version,
            ApplyStage::FilesystemStarted,
            fns_sync_core::model::RemoteApplyOperation::Delete {
                state: tombstone_state("already-deleted"),
            },
        );
        assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
        assert_missing(workspace.path(), "already-deleted");

        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("legacy-directory")).unwrap();
        seed_legacy_journal(
            state_root.path(),
            version,
            ApplyStage::FilesystemStarted,
            fns_sync_core::model::RemoteApplyOperation::Upsert {
                state: directory_state("legacy-directory"),
            },
        );
        assert_legacy_reopens_cleanly(workspace.path(), state_root.path());
        assert_directory(workspace.path(), "legacy-directory");
    }
}

#[test]
fn stale_missing_live_conflict_resolution_never_reaches_filesystem_or_journal() {
    for revision in [WorkspaceRevision::new(1), WorkspaceRevision::new(2)] {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let scenario = Scenario::LiveConflictResolved;
        let mut engine = SyncEngine::open(config(workspace.path(), state.path())).unwrap();
        scenario.seed(workspace.path(), &mut engine);
        engine
            .state_mut()
            .set_last_applied_revision(WorkspaceRevision::new(2))
            .unwrap();
        engine
            .state_mut()
            .set_pending_ack(WorkspaceRevision::new(2))
            .unwrap();
        let mut message = scenario.conflict_resolved();
        message.revision = revision;
        message.path_state.path_revision = revision;

        let error = engine.conflict_resolved(message.clone()).unwrap_err();
        assert_eq!(
            error,
            SyncError::StreamInvariant {
                reason: "live_revision_regression"
            }
        );
        assert_file(
            workspace.path(),
            "conflict/live.bin",
            b"conflict-old",
            false,
        );
        assert!(engine.state().apply_journals().unwrap().is_empty());
        assert_eq!(engine.state().conflicts().unwrap().len(), 1);
        assert_eq!(
            ack_revisions(&engine.pending_commands(64).unwrap()),
            vec![2]
        );
        assert_eq!(
            ack_revisions(&engine.pending_commands(64).unwrap()),
            vec![2]
        );
        engine.close().unwrap();

        let mut reopened = SyncEngine::open(config(workspace.path(), state.path())).unwrap();
        assert_eq!(reopened.conflict_resolved(message).unwrap_err(), error);
        assert_file(
            workspace.path(),
            "conflict/live.bin",
            b"conflict-old",
            false,
        );
        assert!(reopened.state().apply_journals().unwrap().is_empty());
        assert_eq!(reopened.state().conflicts().unwrap().len(), 1);
        assert_eq!(
            ack_revisions(&reopened.pending_commands(64).unwrap()),
            vec![2]
        );
        reopened.close().unwrap();
    }
}

#[test]
fn valid_json_recovery_tuple_tampering_is_rejected_before_filesystem_mutation() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    write_file(workspace.path(), "must-survive.bin", b"keep-me", false);
    crash_child(
        Scenario::FileCreateEmpty,
        Boundary::Prepared,
        workspace.path(),
        state.path(),
    );

    let rooted = fns_fs::RootedWorkspace::open(workspace.path()).unwrap();
    let victim = workspace_path("must-survive.bin");
    let observed = rooted.inspect(&victim).unwrap().unwrap();
    let tampered = fns_fs::FsOperation::Delete {
        path: victim,
        expected: fns_fs::ExpectedEntry::Present {
            kind: observed.kind,
            content_hash: Some(content_hash(b"keep-me")),
            directory_snapshot: None,
            fingerprint: observed.fingerprint,
        },
    };
    let tampered_json = fns_sync_core::canonical_json(&tampered).unwrap();
    let state_store = SqliteState::open(
        state.path().join("state.sqlite"),
        workspace_id(),
        client_id(),
    )
    .unwrap();
    let mut journal = state_store.apply_journals().unwrap().remove(0);
    journal.filesystem_operation_json = tampered_json.clone();
    journal.preimage_json = tampered_json.clone();
    let tampered_digest = fns_sync_core::apply_journal_immutable_digest(&journal);
    drop(state_store);
    let connection = Connection::open(state.path().join("state.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE apply_journal SET filesystem_operation_json = ?1, preimage_json = ?1, operation_body_digest = ?2",
            params![tampered_json, tampered_digest.as_slice()],
        )
        .unwrap();
    drop(connection);

    let error = match SyncEngine::open(config(workspace.path(), state.path())) {
        Ok(_) => panic!("tampered recovery tuple was accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        SyncError::CorruptState {
            table: "apply_journal",
            field: "filesystem_operation_json"
        }
    );
    assert_file(workspace.path(), "must-survive.bin", b"keep-me", false);
}

fn seed_v3_delete_journal(state_root: &Path, stage: ApplyStage) {
    seed_legacy_journal(
        state_root,
        3,
        stage,
        fns_sync_core::model::RemoteApplyOperation::Delete {
            state: tombstone_state("legacy.txt"),
        },
    );
}

fn seed_legacy_journal(
    state_root: &Path,
    version: u32,
    stage: ApplyStage,
    operation: fns_sync_core::model::RemoteApplyOperation,
) {
    let connection = Connection::open(state_root.join("state.sqlite")).unwrap();
    connection
        .execute_batch(include_str!("../migrations/0001_client_state.sql"))
        .unwrap();
    if version >= 2 {
        connection
            .execute_batch(include_str!(
                "../migrations/0002_applied_operation_receipts.sql"
            ))
            .unwrap();
    }
    if version >= 3 {
        connection
            .execute_batch(include_str!(
                "../migrations/0003_provisional_mutation_acceptances.sql"
            ))
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO workspace_cursor (workspace_id, client_id, last_ack_revision, last_applied_revision, pending_ack_revision, created_at_ms, updated_at_ms) VALUES (?1, ?2, '0', '0', NULL, 0, 0)",
            params![workspace_id().to_string(), client_id().to_string()],
        )
        .unwrap();
    let postimage = match &operation {
        fns_sync_core::model::RemoteApplyOperation::Upsert { state }
        | fns_sync_core::model::RemoteApplyOperation::Delete { state } => vec![state.clone()],
        fns_sync_core::model::RemoteApplyOperation::Rename {
            old_state,
            new_state,
        } => vec![old_state.clone(), new_state.clone()],
    };
    connection
        .execute(
            "INSERT INTO apply_journal (apply_id, workspace_id, stream_id, item_kind, item_key, operation_json, preimage_json, postimage_json, stage) VALUES (?1, ?2, ?3, 'event', '1', ?4, ?5, ?6, ?7)",
            params![
                "10000000-0000-4000-8000-000000000077",
                workspace_id().to_string(),
                stream_id().to_string(),
                fns_sync_core::canonical_json(&operation).unwrap(),
                b"{}".as_slice(),
                fns_sync_core::canonical_json(&postimage).unwrap(),
                stage.as_str(),
            ],
        )
        .unwrap();
}

fn assert_legacy_reopens_cleanly(workspace: &Path, state_root: &Path) {
    for _ in 0..2 {
        let mut engine = SyncEngine::open(config(workspace, state_root)).unwrap();
        assert!(engine.state().apply_journals().unwrap().is_empty());
        engine.close().unwrap();
    }
}

fn crash_child(scenario: Scenario, boundary: Boundary, workspace: &Path, state: &Path) {
    let child_paths = std::env::join_paths([workspace, state]).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "apply_journal_crash_child", "--nocapture"])
        .env(CHILD_ENV, child_paths)
        .env(SCENARIO_ENV, scenario.name())
        .env("FNS_SYNC_APPLY_FAILPOINT", boundary.failpoint())
        .output()
        .unwrap();
    assert!(
        !output.status.success() && output.status.code().is_none(),
        "{scenario:?}/{boundary:?} did not abort at the production failpoint: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn crash_child_writer(
    scenario: Scenario,
    boundary: WriterBoundary,
    workspace: &Path,
    state: &Path,
) {
    let child_paths = std::env::join_paths([workspace, state]).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "apply_journal_crash_child", "--nocapture"])
        .env(CHILD_ENV, child_paths)
        .env(SCENARIO_ENV, scenario.name())
        .env("FNS_FS_APPLY_FAILPOINT", boundary.failpoint())
        .output()
        .unwrap();
    assert!(
        !output.status.success() && output.status.code().is_none(),
        "{scenario:?}/{boundary:?} did not abort at the production writer failpoint: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_durable_journal(scenario: Scenario, boundary: Boundary, state_root: &Path) {
    let state =
        SqliteState::open(state_root.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    let journals = state.apply_journals().unwrap();
    assert_eq!(journals.len(), 1, "{scenario:?}/{boundary:?}");
    let journal = &journals[0];
    assert_eq!(journal.workspace_id, workspace_id());
    assert_eq!(journal.item_key, "1");
    assert_eq!(journal.apply_namespace, scenario.expected_namespace());
    assert_eq!(journal.stage, boundary.durable_stage());
    assert_eq!(journal.preimage_json, journal.filesystem_operation_json);
    assert_eq!(
        journal.operation_body_digest,
        fns_sync_core::apply_journal_immutable_digest(journal)
    );
    assert_eq!(
        journal.filesystem_receipt_json.is_some(),
        boundary.has_filesystem_receipt()
    );

    let operation: fns_sync_core::model::RemoteApplyOperation =
        serde_json::from_slice(&journal.operation_json).unwrap();
    let expected_operation = if scenario.is_live() {
        fns_sync_core::model::RemoteApplyOperation::Upsert {
            state: scenario.conflict_resolved().path_state,
        }
    } else {
        let event = scenario.event();
        if event.mutation.kind == WorkspaceMutationKind::Rename {
            fns_sync_core::model::RemoteApplyOperation::Rename {
                old_state: event.old_path_state.unwrap(),
                new_state: event.new_path_state.unwrap(),
            }
        } else if event.path_state.kind == WorkspaceEntryKind::Tombstone {
            fns_sync_core::model::RemoteApplyOperation::Delete {
                state: event.path_state,
            }
        } else {
            fns_sync_core::model::RemoteApplyOperation::Upsert {
                state: event.path_state,
            }
        }
    };
    assert_eq!(operation, expected_operation);
    assert_eq!(
        serde_json::from_slice::<Vec<WorkspacePathState>>(&journal.postimage_json).unwrap(),
        scenario.expected_states()
    );
    let filesystem_operation: fns_fs::FsOperation =
        serde_json::from_slice(&journal.filesystem_operation_json).unwrap();
    assert_eq!(
        fns_sync_core::canonical_json(&filesystem_operation).unwrap(),
        journal.filesystem_operation_json
    );
    let commit: ApplyCommitPlan = serde_json::from_slice(&journal.commit_json).unwrap();
    let expected_commit = if scenario.is_live() {
        ApplyCommitPlan::LiveConflictResolved {
            message: scenario.conflict_resolved(),
        }
    } else {
        ApplyCommitPlan::StreamEvent {
            event: scenario.event(),
            remove_outbox: false,
        }
    };
    assert_eq!(commit, expected_commit);

    if let Some(receipt_json) = &journal.filesystem_receipt_json {
        let receipt: fns_fs::ApplyReceipt = serde_json::from_slice(receipt_json).unwrap();
        assert_eq!(receipt.apply_id, journal.apply_id);
        assert_eq!(receipt.touched, scenario.expected_touched());
        assert_eq!(
            fns_sync_core::canonical_json(&receipt).unwrap(),
            *receipt_json
        );
    }
}

fn settle_after_crash(
    scenario: Scenario,
    workspace: &Path,
    state: &Path,
    expect_local_intent: bool,
) {
    let mut first_recovery = SyncEngine::open(config(workspace, state)).unwrap();
    first_recovery.close().unwrap();

    let mut engine = SyncEngine::open(config(workspace, state)).unwrap();
    let _ = engine.pending_commands(64).unwrap();
    assert!(engine.state().apply_journals().unwrap().is_empty());
    if !expect_local_intent {
        scenario.assert_workspace(workspace);
    }
    assert_exact_states_and_receipt(&engine, scenario);

    if scenario.is_live() {
        assert!(
            engine
                .conflict_resolved(scenario.conflict_resolved())
                .unwrap()
                .is_empty()
        );
    } else {
        assert!(engine.workspace_event(scenario.event()).unwrap().is_empty());
        engine.snapshot_end(end()).unwrap();
    }

    let first_commands = engine.pending_commands(64).unwrap();
    let second_commands = engine.pending_commands(64).unwrap();
    assert_eq!(ack_revisions(&first_commands), vec![1]);
    assert_eq!(ack_revisions(&second_commands), vec![1]);
    engine.ack_confirmed(ack()).unwrap();
    assert_final_tables(&engine, scenario, expect_local_intent);
    engine.close().unwrap();

    let mut reopened = SyncEngine::open(config(workspace, state)).unwrap();
    assert_final_tables(&reopened, scenario, expect_local_intent);
    if scenario.is_live() {
        assert!(
            reopened
                .conflict_resolved(scenario.conflict_resolved())
                .unwrap()
                .is_empty()
        );
    } else {
        assert!(reopened.event(scenario.event()).unwrap().is_empty());
    }
    assert!(ack_revisions(&reopened.pending_commands(64).unwrap()).is_empty());
    if !expect_local_intent {
        scenario.assert_workspace(workspace);
    }
    assert_no_apply_staging(workspace, state);
    reopened.close().unwrap();
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedOperationResult<'a> {
    mutation: &'a WorkspaceMutation,
    path_state: &'a WorkspacePathState,
    old_path_state: Option<&'a WorkspacePathState>,
    new_path_state: Option<&'a WorkspacePathState>,
}

fn assert_exact_states_and_receipt(engine: &SyncEngine, scenario: Scenario) {
    for expected in scenario.expected_states() {
        let record = engine
            .state()
            .path_state(expected.path.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(record.state, expected);
        assert_eq!(
            record.state_json,
            fns_sync_core::canonical_json(&expected).unwrap()
        );
        assert_eq!(
            record.state_digest,
            fns_sync_core::digest(&expected).unwrap()
        );
        assert_eq!(record.state.path_revision, WorkspaceRevision::new(1));
        assert_eq!(record.state.metadata.size, expected.metadata.size);
        assert_eq!(
            record.state.metadata.executable,
            expected.metadata.executable
        );
        assert_eq!(record.state.content_hash, expected.content_hash);
    }

    let (origin, operation_id, revision, digest, kind, mutation_json) = if scenario.is_live() {
        let message = scenario.conflict_resolved();
        (
            message.resolved_by_client_id,
            message.operation_id,
            message.revision,
            fns_sync_core::body_digest(&fns_sync_core::canonical_json(&message).unwrap()),
            AppliedOperationReceiptKind::ConflictResolution,
            None,
        )
    } else {
        let event = scenario.event();
        let result = AppliedOperationResult {
            mutation: &event.mutation,
            path_state: &event.path_state,
            old_path_state: event.old_path_state.as_ref(),
            new_path_state: event.new_path_state.as_ref(),
        };
        (
            event.origin_client_id,
            event.operation_id,
            event.revision,
            fns_sync_core::digest(&result).unwrap(),
            AppliedOperationReceiptKind::MutationResult,
            Some(fns_sync_core::canonical_json(&event.mutation).unwrap()),
        )
    };
    let receipt = engine
        .state()
        .applied_operation(origin, operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.origin_client_id, origin);
    assert_eq!(receipt.operation_id, operation_id);
    assert_eq!(receipt.revision, revision);
    assert_eq!(receipt.body_digest, digest);
    assert_eq!(receipt.receipt_kind, kind);
    assert_eq!(receipt.mutation_json, mutation_json);
    assert_eq!(engine.state().applied_operations().unwrap().len(), 1);
}

fn assert_final_tables(engine: &SyncEngine, scenario: Scenario, expect_local_intent: bool) {
    let cursor = engine.cursor().unwrap();
    assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(1));
    assert_eq!(cursor.pending_ack_revision, None);
    assert!(engine.state().apply_journals().unwrap().is_empty());
    assert!(engine.state().conflicts().unwrap().is_empty());
    assert!(engine.state().stream_state().unwrap().is_none());
    assert!(
        engine
            .state()
            .stream_entries(stream_id())
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .state()
            .stream_revision_items(stream_id())
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .state()
            .stream_conflicts(stream_id())
            .unwrap()
            .is_empty()
    );
    assert_exact_states_and_receipt(engine, scenario);
    assert_eq!(
        engine.state().outbox().unwrap().is_empty(),
        !expect_local_intent
    );
    assert!(engine.state().local_intents().unwrap().is_empty());
}

fn assert_preserved_local_outbox(state_root: &Path, bytes: &[u8]) {
    let state =
        SqliteState::open(state_root.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    let outbox = state.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        outbox[0].body_digest,
        fns_sync_core::body_digest(&outbox[0].body_json)
    );
    let mutation: WorkspaceMutation = serde_json::from_slice(&outbox[0].body_json).unwrap();
    assert_eq!(mutation.path, workspace_path("binary/update.bin"));
    assert_eq!(mutation.base_path_revision, WorkspaceRevision::new(1));
    assert_eq!(mutation.kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        mutation.content_hash,
        RequiredNullable::Value(content_hash(bytes))
    );
    assert_eq!(mutation.metadata.size, bytes.len() as u64);
    assert!(!mutation.metadata.executable);

    assert!(state.local_intents().unwrap().is_empty());
}

fn ack_revisions(commands: &[SyncCommand]) -> Vec<u64> {
    commands
        .iter()
        .filter_map(|command| match command {
            SyncCommand::SendAck(message) => Some(message.revision.get()),
            _ => None,
        })
        .collect()
}

fn assert_file(workspace: &Path, path: &str, bytes: &[u8], executable: bool) {
    let rooted = fns_fs::RootedWorkspace::open(workspace).unwrap();
    let observed = rooted.inspect(&workspace_path(path)).unwrap().unwrap();
    assert_eq!(observed.path, workspace_path(path));
    assert_eq!(observed.kind, WorkspaceEntryKind::File);
    assert_eq!(fs::read(workspace.join(path)).unwrap(), bytes);
    assert_eq!(observed.metadata.size, bytes.len() as u64);
    assert_eq!(observed.metadata.executable, executable);
    assert_eq!(
        content_hash(&fs::read(workspace.join(path)).unwrap()),
        content_hash(bytes)
    );
}

fn assert_directory(workspace: &Path, path: &str) {
    let rooted = fns_fs::RootedWorkspace::open(workspace).unwrap();
    let observed = rooted.inspect(&workspace_path(path)).unwrap().unwrap();
    assert_eq!(observed.path, workspace_path(path));
    assert_eq!(observed.kind, WorkspaceEntryKind::Directory);
}

fn assert_missing(workspace: &Path, path: &str) {
    let rooted = fns_fs::RootedWorkspace::open(workspace).unwrap();
    assert!(rooted.inspect(&workspace_path(path)).unwrap().is_none());
}

fn assert_no_apply_staging(workspace: &Path, state: &Path) {
    assert!(apply_artifacts(workspace).is_empty());
    assert!(
        fs::read_dir(state.join("tmp")).unwrap().next().is_none(),
        "content staging directory is not empty"
    );
}

fn apply_artifacts(root: &Path) -> Vec<PathBuf> {
    let mut artifacts = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            if entry.file_name().to_str().is_some_and(|name| {
                name.starts_with(".fns-tmp-")
                    || name.starts_with(".fns-delete-")
                    || name.starts_with(".fns-rename-")
            }) {
                artifacts.push(path);
            }
        }
    }
    artifacts
}

#[derive(Debug, Eq, PartialEq)]
struct LegacyGapSnapshot {
    cursor: WorkspaceCursor,
    journals: Vec<ApplyJournalRecord>,
    path_states: Vec<PathStateRecord>,
    outbox: Vec<OutboxRecord>,
    local_intents: Vec<LocalIntentRecord>,
    conflicts: Vec<ConflictRecord>,
    receipts: Vec<fns_sync_core::AppliedOperationRecord>,
    stream_state: Option<fns_sync_core::StreamStateRecord>,
    stream_entries: Vec<fns_sync_core::StreamEntryRecord>,
    stream_revisions: Vec<fns_sync_core::StreamRevisionItemRecord>,
    stream_conflicts: Vec<fns_sync_core::StreamConflictRecord>,
}

struct LegacyGapFixture {
    _workspace: tempfile::TempDir,
    _state_root: tempfile::TempDir,
    state: SqliteState,
    event: WorkspaceEventMessage,
    journal: ApplyJournalRecord,
    unrelated_outbox_operation: OperationId,
}

type LegacyGapInvalidCase = (&'static str, fn(&mut LegacyGapFixture));

impl LegacyGapFixture {
    fn new(remove_outbox: bool) -> Self {
        Self::with_shape(remove_outbox, ApplyStage::FilesystemApplied, 98, Some(98))
    }

    fn with_shape(
        remove_outbox: bool,
        stage: ApplyStage,
        last_applied_revision: u64,
        pending_ack_revision: Option<u64>,
    ) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let state_root = tempfile::tempdir().unwrap();
        let mut state = SqliteState::open(
            state_root.path().join("state.sqlite"),
            workspace_id(),
            client_id(),
        )
        .unwrap();

        let event_client = if remove_outbox {
            client_id()
        } else {
            remote_client_id()
        };
        let bytes = b"legacy-gap-revision-94\0binary";
        let event = legacy_gap_event(event_client, operation_id(940), 94, "legacy-gap.bin", bytes);
        let apply_id =
            fns_fs::ApplyId(uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000940").unwrap());
        let filesystem_operation = fns_fs::FsOperation::UpsertFile {
            path: event.path_state.path.clone(),
            content_hash: event.path_state.content_hash.clone().into_option().unwrap(),
            metadata: event.path_state.metadata.clone(),
            expected: fns_fs::ExpectedEntry::Missing,
        };
        let content_cache = fns_fs::ContentCache::open(state_root.path()).unwrap();
        content_cache
            .import(
                event
                    .path_state
                    .content_hash
                    .clone()
                    .into_option()
                    .as_ref()
                    .unwrap(),
                bytes.len() as u64,
                Cursor::new(bytes),
            )
            .unwrap();
        let filesystem_receipt_json = if stage == ApplyStage::FilesystemApplied {
            let writer = fns_fs::AtomicWorkspaceWriter::new(
                fns_fs::RootedWorkspace::open(workspace.path()).unwrap(),
                fns_fs::ContentCache::open(state_root.path()).unwrap(),
            );
            let receipt = writer.apply(apply_id, &filesystem_operation).unwrap();
            Some(fns_sync_core::canonical_json(&receipt).unwrap())
        } else {
            assert!(matches!(
                stage,
                ApplyStage::Prepared | ApplyStage::FilesystemStarted
            ));
            None
        };
        let operation = fns_sync_core::model::RemoteApplyOperation::Upsert {
            state: event.path_state.clone(),
        };
        let mut journal = ApplyJournalRecord {
            apply_id,
            workspace_id: workspace_id(),
            stream_id: stream_id(),
            item_kind: ApplyItemKind::Event,
            item_key: "94".to_owned(),
            apply_namespace: ApplyNamespace::LiveEvent,
            operation_body_digest: [0; 32],
            operation_json: fns_sync_core::canonical_json(&operation).unwrap(),
            filesystem_operation_json: fns_sync_core::canonical_json(&filesystem_operation)
                .unwrap(),
            commit_json: fns_sync_core::canonical_json(&ApplyCommitPlan::LiveEvent {
                event: event.clone(),
                remove_outbox,
            })
            .unwrap(),
            preimage_json: fns_sync_core::canonical_json(&filesystem_operation).unwrap(),
            postimage_json: fns_sync_core::canonical_json(&vec![event.path_state.clone()]).unwrap(),
            filesystem_receipt_json,
            stage,
        };
        journal.operation_body_digest = fns_sync_core::apply_journal_immutable_digest(&journal);
        state.put_apply_journal(&journal).unwrap();

        state
            .put_path_state(&legacy_gap_path_state("legacy-gap.bin", 90, b"old"))
            .unwrap();
        for revision in 96..=98 {
            let path = format!("keep-{revision}.txt");
            let body = format!("keep-revision-{revision}");
            let kept = legacy_gap_path_state(&path, revision, body.as_bytes());
            state.put_path_state(&kept).unwrap();
            let mutation = legacy_gap_mutation(
                remote_client_id(),
                operation_id(1_000 + revision as u32),
                &path,
                body.as_bytes(),
            );
            let mutation_json = fns_sync_core::canonical_json(&mutation).unwrap();
            state
                .record_applied_operation(
                    mutation.client_id,
                    mutation.operation_id,
                    WorkspaceRevision::new(revision),
                    fns_sync_core::digest(&AppliedOperationResult {
                        mutation: &mutation,
                        path_state: &kept,
                        old_path_state: None,
                        new_path_state: None,
                    })
                    .unwrap(),
                    AppliedOperationReceiptKind::MutationResult,
                    Some(&mutation_json),
                )
                .unwrap();
        }

        let unrelated_outbox_operation = operation_id(1_900);
        let unrelated_outbox = legacy_gap_mutation(
            client_id(),
            unrelated_outbox_operation,
            "keep-outbox.txt",
            b"keep-outbox",
        );
        state.enqueue_mutation(&unrelated_outbox).unwrap();
        if remove_outbox {
            state.enqueue_mutation(&event.mutation).unwrap();
        }
        state
            .put_local_intent(
                &workspace_path("keep-intent.txt"),
                br#"{"kind":"keep"}"#,
                1_800_000_000_000,
            )
            .unwrap();
        state
            .record_conflict(
                &Scenario::LiveConflictResolved.conflict_created(),
                ConflictStatus::Manual,
            )
            .unwrap();
        state
            .set_last_ack_revision(WorkspaceRevision::new(93))
            .unwrap();
        state
            .set_last_applied_revision(WorkspaceRevision::new(last_applied_revision))
            .unwrap();
        if let Some(pending_ack_revision) = pending_ack_revision {
            state
                .set_pending_ack(WorkspaceRevision::new(pending_ack_revision))
                .unwrap();
        }

        Self {
            _workspace: workspace,
            _state_root: state_root,
            state,
            event,
            journal,
            unrelated_outbox_operation,
        }
    }

    fn snapshot(&self) -> LegacyGapSnapshot {
        legacy_gap_snapshot(&self.state)
    }

    fn assert_unrelated_state_preserved(&self, before: &LegacyGapSnapshot) {
        for revision in 96..=98 {
            let path = format!("keep-{revision}.txt");
            assert_eq!(
                self.state.path_state(&path).unwrap(),
                before
                    .path_states
                    .iter()
                    .find(|state| state.state.path.as_str() == path)
                    .cloned()
            );
            let operation = operation_id(1_000 + revision as u32);
            assert_eq!(
                self.state
                    .applied_operation(remote_client_id(), operation)
                    .unwrap(),
                before
                    .receipts
                    .iter()
                    .find(|receipt| receipt.operation_id == operation)
                    .cloned()
            );
        }
        assert!(
            self.state
                .outbox()
                .unwrap()
                .iter()
                .any(|row| row.operation_id == self.unrelated_outbox_operation)
        );
        assert_eq!(self.state.local_intents().unwrap(), before.local_intents);
        assert_eq!(self.state.conflicts().unwrap(), before.conflicts);
    }
}

fn legacy_gap_snapshot(state: &SqliteState) -> LegacyGapSnapshot {
    LegacyGapSnapshot {
        cursor: state.cursor().unwrap(),
        journals: state.apply_journals().unwrap(),
        path_states: state.path_states().unwrap(),
        outbox: state.outbox().unwrap(),
        local_intents: state.local_intents().unwrap(),
        conflicts: state.conflicts().unwrap(),
        receipts: state.applied_operations().unwrap(),
        stream_state: state.stream_state().unwrap(),
        stream_entries: state.stream_entries(stream_id()).unwrap(),
        stream_revisions: state.stream_revision_items(stream_id()).unwrap(),
        stream_conflicts: state.stream_conflicts(stream_id()).unwrap(),
    }
}

fn legacy_gap_path_state(path: &str, revision: u64, bytes: &[u8]) -> WorkspacePathState {
    WorkspacePathState {
        path: workspace_path(path),
        path_revision: WorkspaceRevision::new(revision),
        kind: WorkspaceEntryKind::File,
        content_hash: RequiredNullable::Value(content_hash(bytes)),
        metadata: file_metadata(bytes, false),
        tombstone: false,
    }
}

fn legacy_gap_mutation(
    origin: ClientId,
    operation_id: OperationId,
    path: &str,
    bytes: &[u8],
) -> WorkspaceMutation {
    WorkspaceMutation {
        workspace_id: workspace_id(),
        client_id: origin,
        operation_id,
        path: workspace_path(path),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceMutationKind::UpsertFile,
        content_hash: RequiredNullable::Value(content_hash(bytes)),
        metadata: file_metadata(bytes, false),
        new_path: None,
        target_base_path_revision: None,
    }
}

fn legacy_gap_event(
    origin: ClientId,
    operation_id: OperationId,
    revision: u64,
    path: &str,
    bytes: &[u8],
) -> WorkspaceEventMessage {
    let mutation = legacy_gap_mutation(origin, operation_id, path, bytes);
    let event = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        index: 0,
        revision: WorkspaceRevision::new(revision),
        operation_id,
        origin_client_id: origin,
        mutation,
        path_state: legacy_gap_path_state(path, revision, bytes),
        old_path_state: None,
        new_path_state: None,
    };
    event.validate().unwrap();
    event
}

fn seed_legacy_gap_stream(state: &mut SqliteState, end_received: bool) {
    state
        .begin_stream(&WorkspaceSnapshotBeginMessage {
            workspace_id: workspace_id(),
            stream_id: stream_id(),
            mode: WorkspaceSnapshotMode::Incremental,
            from_revision: WorkspaceRevision::new(93),
            final_revision: WorkspaceRevision::new(98),
            entry_count: 0,
            event_count: 0,
            conflict_count: 0,
        })
        .unwrap();
    if end_received {
        state.set_stream_end_received(true).unwrap();
    }
}

fn replace_legacy_gap_with_directory(fixture: &mut LegacyGapFixture) {
    let path = workspace_path("unsupported-directory");
    let mutation = WorkspaceMutation {
        workspace_id: workspace_id(),
        client_id: remote_client_id(),
        operation_id: fixture.event.operation_id,
        path: path.clone(),
        base_path_revision: WorkspaceRevision::ZERO,
        kind: WorkspaceMutationKind::Mkdir,
        content_hash: RequiredNullable::Null,
        metadata: zero_metadata(),
        new_path: None,
        target_base_path_revision: None,
    };
    let path_state = WorkspacePathState {
        path: path.clone(),
        path_revision: WorkspaceRevision::new(94),
        kind: WorkspaceEntryKind::Directory,
        content_hash: RequiredNullable::Null,
        metadata: zero_metadata(),
        tombstone: false,
    };
    let event = WorkspaceEventMessage {
        workspace_id: workspace_id(),
        stream_id: stream_id(),
        index: 0,
        revision: WorkspaceRevision::new(94),
        operation_id: mutation.operation_id,
        origin_client_id: mutation.client_id,
        mutation,
        path_state: path_state.clone(),
        old_path_state: None,
        new_path_state: None,
    };
    event.validate().unwrap();
    let filesystem_operation = fns_fs::FsOperation::Mkdir {
        path,
        metadata: zero_metadata(),
        expected: fns_fs::ExpectedEntry::Missing,
    };
    fixture.journal.operation_json =
        fns_sync_core::canonical_json(&fns_sync_core::model::RemoteApplyOperation::Upsert {
            state: path_state.clone(),
        })
        .unwrap();
    fixture.journal.filesystem_operation_json =
        fns_sync_core::canonical_json(&filesystem_operation).unwrap();
    fixture.journal.preimage_json = fixture.journal.filesystem_operation_json.clone();
    fixture.journal.postimage_json = fns_sync_core::canonical_json(&vec![path_state]).unwrap();
    fixture.journal.commit_json = fns_sync_core::canonical_json(&ApplyCommitPlan::LiveEvent {
        event: event.clone(),
        remove_outbox: false,
    })
    .unwrap();
    fixture.journal.filesystem_receipt_json = None;
    fixture.journal.stage = ApplyStage::Prepared;
    fixture.journal.operation_body_digest =
        fns_sync_core::apply_journal_immutable_digest(&fixture.journal);
    fixture
        .state
        .remove_apply_journal(fixture.journal.apply_id)
        .unwrap();
    fixture.state.put_apply_journal(&fixture.journal).unwrap();
    fixture.event = event;
}

fn replace_legacy_gap_receipt_with_decoy(fixture: &mut LegacyGapFixture) {
    let decoy_path = workspace_path("receipt-decoy.bin");
    let decoy_bytes = b"receipt-decoy";
    fs::write(
        fixture._workspace.path().join(decoy_path.as_str()),
        decoy_bytes,
    )
    .unwrap();
    let observed = fns_fs::RootedWorkspace::open(fixture._workspace.path())
        .unwrap()
        .inspect(&decoy_path)
        .unwrap()
        .unwrap();
    let receipt = fns_fs::ApplyReceipt {
        apply_id: fixture.journal.apply_id,
        touched: vec![decoy_path],
        postimages: vec![Some(observed)],
        postimage_hashes: vec![Some(content_hash(decoy_bytes))],
        cleanup_name: None,
    };
    fixture.journal.filesystem_receipt_json =
        Some(fns_sync_core::canonical_json(&receipt).unwrap());
    fixture
        .state
        .remove_apply_journal(fixture.journal.apply_id)
        .unwrap();
    fixture.state.put_apply_journal(&fixture.journal).unwrap();
}

#[test]
fn engine_open_recovers_legacy_live_event_gap_from_every_precommit_stage() {
    for stage in [
        ApplyStage::Prepared,
        ApplyStage::FilesystemStarted,
        ApplyStage::FilesystemApplied,
    ] {
        let fixture = LegacyGapFixture::with_shape(false, stage, 98, Some(98));
        let before = fixture.snapshot();
        let LegacyGapFixture {
            _workspace: workspace,
            _state_root: state_root,
            state,
            event,
            journal: _,
            unrelated_outbox_operation,
        } = fixture;
        drop(state);

        let mut engine = SyncEngine::open(config(workspace.path(), state_root.path()))
            .unwrap_or_else(|error| panic!("{stage:?} recovery failed: {error:?}"));
        assert_eq!(
            fs::read(workspace.path().join("legacy-gap.bin")).unwrap(),
            b"legacy-gap-revision-94\0binary",
            "{stage:?}"
        );
        let cursor = engine.cursor().unwrap();
        assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(93));
        assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(94));
        assert_eq!(cursor.pending_ack_revision, None);
        assert!(engine.state().apply_journals().unwrap().is_empty());
        assert_eq!(
            engine
                .state()
                .path_state("legacy-gap.bin")
                .unwrap()
                .unwrap()
                .state,
            event.path_state
        );
        let recovered_receipt = engine
            .state()
            .applied_operation(event.origin_client_id, event.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered_receipt.revision, WorkspaceRevision::new(94));
        assert_eq!(
            recovered_receipt.receipt_kind,
            AppliedOperationReceiptKind::MutationResult
        );
        assert_eq!(engine.state().applied_operations().unwrap().len(), 4);
        for revision in 96..=98 {
            let path = format!("keep-{revision}.txt");
            assert_eq!(
                engine.state().path_state(&path).unwrap(),
                before
                    .path_states
                    .iter()
                    .find(|state| state.state.path.as_str() == path)
                    .cloned(),
                "{stage:?}"
            );
            let operation = operation_id(1_000 + revision as u32);
            assert_eq!(
                engine
                    .state()
                    .applied_operation(remote_client_id(), operation)
                    .unwrap(),
                before
                    .receipts
                    .iter()
                    .find(|receipt| receipt.operation_id == operation)
                    .cloned(),
                "{stage:?}"
            );
        }
        assert!(
            engine
                .state()
                .outbox()
                .unwrap()
                .iter()
                .any(|row| row.operation_id == unrelated_outbox_operation)
        );
        assert_eq!(
            engine.state().local_intents().unwrap(),
            before.local_intents
        );
        assert_eq!(engine.state().conflicts().unwrap(), before.conflicts);
        assert_no_apply_staging(workspace.path(), state_root.path());

        engine.close().unwrap();
        let mut reopened = SyncEngine::open(config(workspace.path(), state_root.path())).unwrap();
        assert_eq!(reopened.cursor().unwrap(), cursor);
        assert!(reopened.state().apply_journals().unwrap().is_empty());
        assert_eq!(
            reopened
                .state()
                .applied_operation(event.origin_client_id, event.operation_id)
                .unwrap(),
            Some(recovered_receipt)
        );
        reopened.close().unwrap();
    }
}

#[test]
fn engine_open_rejects_invalid_legacy_gap_before_any_filesystem_change() {
    for stage in [
        ApplyStage::Prepared,
        ApplyStage::FilesystemStarted,
        ApplyStage::FilesystemApplied,
    ] {
        let fixture = LegacyGapFixture::with_shape(false, stage, 98, None);
        let before = fixture.snapshot();
        let expected_file = fs::read(fixture._workspace.path().join("legacy-gap.bin")).ok();
        let LegacyGapFixture {
            _workspace: workspace,
            _state_root: state_root,
            state,
            ..
        } = fixture;
        drop(state);

        let result = SyncEngine::open(config(workspace.path(), state_root.path()));
        assert_eq!(
            result.err().unwrap(),
            SyncError::StreamInvariant {
                reason: "legacy_live_event_gap_pending_ack"
            },
            "{stage:?}"
        );
        assert_eq!(
            fs::read(workspace.path().join("legacy-gap.bin")).ok(),
            expected_file,
            "{stage:?}"
        );
        assert!(apply_artifacts(workspace.path()).is_empty(), "{stage:?}");
        let state = SqliteState::open(
            state_root.path().join("state.sqlite"),
            workspace_id(),
            client_id(),
        )
        .unwrap();
        assert_eq!(legacy_gap_snapshot(&state), before, "{stage:?}");
    }
}

#[test]
fn engine_open_rejects_newer_same_path_state_before_filesystem_apply() {
    let mut fixture =
        LegacyGapFixture::with_shape(false, ApplyStage::FilesystemStarted, 98, Some(98));
    let newer = legacy_gap_path_state("legacy-gap.bin", 98, b"newer-same-path");
    fixture.state.put_path_state(&newer).unwrap();
    let before = fixture.snapshot();
    let workspace_path = fixture._workspace.path().to_path_buf();
    let state_path = fixture._state_root.path().to_path_buf();
    let LegacyGapFixture { state, .. } = fixture;
    drop(state);

    let result = SyncEngine::open(config(&workspace_path, &state_path));
    assert_eq!(
        result.err().unwrap(),
        SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_newer_path_state"
        }
    );
    assert!(!workspace_path.join("legacy-gap.bin").exists());
    assert!(apply_artifacts(&workspace_path).is_empty());
    let state =
        SqliteState::open(state_path.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    assert_eq!(legacy_gap_snapshot(&state), before);
    assert_eq!(
        state.path_state("legacy-gap.bin").unwrap().unwrap().state,
        newer
    );
}

#[test]
fn engine_open_rejects_incomplete_or_completed_stream_before_filesystem_apply() {
    for end_received in [false, true] {
        let mut fixture =
            LegacyGapFixture::with_shape(false, ApplyStage::FilesystemStarted, 98, Some(98));
        seed_legacy_gap_stream(&mut fixture.state, end_received);
        let before = fixture.snapshot();
        let workspace_path = fixture._workspace.path().to_path_buf();
        let state_path = fixture._state_root.path().to_path_buf();
        let LegacyGapFixture { state, .. } = fixture;
        drop(state);

        let result = SyncEngine::open(config(&workspace_path, &state_path));
        assert_eq!(
            result.err().unwrap(),
            SyncError::StreamInvariant {
                reason: "legacy_live_event_gap_active_stream"
            },
            "end_received={end_received}"
        );
        assert!(!workspace_path.join("legacy-gap.bin").exists());
        assert!(apply_artifacts(&workspace_path).is_empty());
        let state = SqliteState::open(state_path.join("state.sqlite"), workspace_id(), client_id())
            .unwrap();
        assert_eq!(legacy_gap_snapshot(&state), before);
    }
}

#[test]
fn engine_open_rejects_unsupported_legacy_gap_operation_before_filesystem_apply() {
    let mut fixture = LegacyGapFixture::with_shape(false, ApplyStage::Prepared, 98, Some(98));
    replace_legacy_gap_with_directory(&mut fixture);
    let before = fixture.snapshot();
    let workspace_path = fixture._workspace.path().to_path_buf();
    let state_path = fixture._state_root.path().to_path_buf();
    let LegacyGapFixture { state, .. } = fixture;
    drop(state);

    let result = SyncEngine::open(config(&workspace_path, &state_path));
    assert_eq!(
        result.err().unwrap(),
        SyncError::StreamInvariant {
            reason: "legacy_live_event_gap_operation_kind"
        }
    );
    assert!(!workspace_path.join("unsupported-directory").exists());
    assert!(apply_artifacts(&workspace_path).is_empty());
    let state =
        SqliteState::open(state_path.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    assert_eq!(legacy_gap_snapshot(&state), before);
}

#[test]
fn engine_open_rejects_decoy_legacy_gap_receipt_without_side_effects() {
    let mut fixture =
        LegacyGapFixture::with_shape(false, ApplyStage::FilesystemApplied, 98, Some(98));
    replace_legacy_gap_receipt_with_decoy(&mut fixture);
    let before = fixture.snapshot();
    let workspace_path = fixture._workspace.path().to_path_buf();
    let state_path = fixture._state_root.path().to_path_buf();
    let legacy_bytes = fs::read(workspace_path.join("legacy-gap.bin")).unwrap();
    let decoy_bytes = fs::read(workspace_path.join("receipt-decoy.bin")).unwrap();
    let artifacts = apply_artifacts(&workspace_path);
    let LegacyGapFixture { state, .. } = fixture;
    drop(state);

    let result = SyncEngine::open(config(&workspace_path, &state_path));
    assert_eq!(
        result.err().unwrap(),
        SyncError::CorruptState {
            table: "apply_journal",
            field: "filesystem_receipt_json"
        }
    );
    assert_eq!(
        fs::read(workspace_path.join("legacy-gap.bin")).unwrap(),
        legacy_bytes
    );
    assert_eq!(
        fs::read(workspace_path.join("receipt-decoy.bin")).unwrap(),
        decoy_bytes
    );
    assert_eq!(apply_artifacts(&workspace_path), artifacts);
    let state =
        SqliteState::open(state_path.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    assert_eq!(legacy_gap_snapshot(&state), before);
}

#[test]
fn engine_open_rejects_tampered_legacy_gap_cleanup_without_side_effects() {
    let mut fixture =
        LegacyGapFixture::with_shape(false, ApplyStage::FilesystemApplied, 98, Some(98));
    let mut receipt: fns_fs::ApplyReceipt =
        serde_json::from_slice(fixture.journal.filesystem_receipt_json.as_deref().unwrap())
            .unwrap();
    let cleanup_name = format!("unrelated/.fns-delete-{}", fixture.journal.apply_id.0);
    receipt.cleanup_name = Some(cleanup_name.clone());
    fixture.journal.filesystem_receipt_json =
        Some(fns_sync_core::canonical_json(&receipt).unwrap());
    fixture
        .state
        .remove_apply_journal(fixture.journal.apply_id)
        .unwrap();
    fixture.state.put_apply_journal(&fixture.journal).unwrap();
    let cleanup_path = fixture._workspace.path().join(&cleanup_name);
    fs::create_dir_all(cleanup_path.parent().unwrap()).unwrap();
    fs::write(&cleanup_path, b"must-survive-rejected-recovery").unwrap();

    let before = fixture.snapshot();
    let workspace_path = fixture._workspace.path().to_path_buf();
    let state_path = fixture._state_root.path().to_path_buf();
    let legacy_bytes = fs::read(workspace_path.join("legacy-gap.bin")).unwrap();
    let cleanup_bytes = fs::read(&cleanup_path).unwrap();
    let artifacts = apply_artifacts(&workspace_path);
    let LegacyGapFixture { state, .. } = fixture;
    drop(state);

    let result = SyncEngine::open(config(&workspace_path, &state_path));
    assert_eq!(
        result.err().unwrap(),
        SyncError::CorruptState {
            table: "apply_journal",
            field: "filesystem_receipt_json"
        }
    );
    assert_eq!(
        fs::read(workspace_path.join("legacy-gap.bin")).unwrap(),
        legacy_bytes
    );
    assert_eq!(fs::read(&cleanup_path).unwrap(), cleanup_bytes);
    assert_eq!(apply_artifacts(&workspace_path), artifacts);
    let state =
        SqliteState::open(state_path.join("state.sqlite"), workspace_id(), client_id()).unwrap();
    assert_eq!(legacy_gap_snapshot(&state), before);
}

#[test]
fn engine_open_preserves_local_content_for_exact_empty_legacy_gap_receipt() {
    let fixture = LegacyGapFixture::with_shape(false, ApplyStage::FilesystemStarted, 98, Some(98));
    let before = fixture.snapshot();
    let workspace_path = fixture._workspace.path().to_path_buf();
    let event = fixture.event.clone();
    let local_bytes = b"local-content-must-win-during-recovery\0binary";
    fs::write(workspace_path.join("legacy-gap.bin"), local_bytes).unwrap();
    let LegacyGapFixture {
        _workspace: workspace,
        _state_root: state_root,
        state,
        ..
    } = fixture;
    drop(state);

    let mut engine = SyncEngine::open(config(workspace.path(), state_root.path())).unwrap();
    assert_eq!(
        fs::read(workspace.path().join("legacy-gap.bin")).unwrap(),
        local_bytes
    );
    assert_eq!(
        engine.cursor().unwrap(),
        WorkspaceCursor {
            workspace_id: workspace_id(),
            client_id: client_id(),
            last_ack_revision: WorkspaceRevision::new(93),
            last_applied_revision: WorkspaceRevision::new(94),
            pending_ack_revision: None,
            pending_segment_ack_revision: None,
        }
    );
    assert!(engine.state().apply_journals().unwrap().is_empty());
    assert_eq!(
        engine
            .state()
            .path_state("legacy-gap.bin")
            .unwrap()
            .unwrap()
            .state,
        event.path_state
    );
    let replacement = engine
        .outbox()
        .unwrap()
        .into_iter()
        .filter_map(|record| record.mutation().ok())
        .filter(|mutation| mutation.path.as_str() == "legacy-gap.bin")
        .collect::<Vec<_>>();
    assert_eq!(replacement.len(), 1);
    assert_eq!(replacement[0].kind, WorkspaceMutationKind::UpsertFile);
    assert_eq!(
        replacement[0].content_hash,
        RequiredNullable::Value(content_hash(local_bytes))
    );
    assert_eq!(
        replacement[0].base_path_revision,
        WorkspaceRevision::new(94)
    );
    assert_eq!(engine.state().applied_operations().unwrap().len(), 4);
    assert_eq!(engine.outbox().unwrap().len(), before.outbox.len() + 1);
    assert_eq!(
        engine.state().local_intents().unwrap(),
        before.local_intents
    );
    let durable_outbox = engine.outbox().unwrap();
    let durable_receipts = engine.state().applied_operations().unwrap();
    let cursor = engine.cursor().unwrap();
    assert_no_apply_staging(workspace.path(), state_root.path());

    engine.close().unwrap();
    let mut reopened = SyncEngine::open(config(workspace.path(), state_root.path())).unwrap();
    assert_eq!(reopened.cursor().unwrap(), cursor);
    assert_eq!(reopened.outbox().unwrap(), durable_outbox);
    assert_eq!(
        reopened.state().applied_operations().unwrap(),
        durable_receipts
    );
    assert_eq!(
        fs::read(workspace.path().join("legacy-gap.bin")).unwrap(),
        local_bytes
    );
    assert!(reopened.state().apply_journals().unwrap().is_empty());
    assert_no_apply_staging(workspace.path(), state_root.path());
    reopened.close().unwrap();
}

#[test]
fn legacy_live_event_gap_commit_is_atomic_and_preserves_newer_state() {
    for remove_outbox in [false, true] {
        let mut fixture = LegacyGapFixture::new(remove_outbox);
        let before = fixture.snapshot();

        fixture
            .state
            .commit_legacy_live_event_gap(&fixture.journal)
            .unwrap();

        let cursor = fixture.state.cursor().unwrap();
        assert_eq!(cursor.last_ack_revision, WorkspaceRevision::new(93));
        assert_eq!(cursor.last_applied_revision, WorkspaceRevision::new(94));
        assert_eq!(cursor.pending_ack_revision, None);
        let journals = fixture.state.apply_journals().unwrap();
        assert_eq!(journals.len(), 1);
        assert_eq!(journals[0].stage, ApplyStage::DatabaseCommitted);
        assert_eq!(
            fixture
                .state
                .path_state("legacy-gap.bin")
                .unwrap()
                .unwrap()
                .state,
            fixture.event.path_state
        );
        let receipt = fixture
            .state
            .applied_operation(fixture.event.origin_client_id, fixture.event.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.revision, WorkspaceRevision::new(94));
        assert_eq!(
            receipt.receipt_kind,
            AppliedOperationReceiptKind::MutationResult
        );
        assert_eq!(
            receipt.body_digest,
            fns_sync_core::digest(&AppliedOperationResult {
                mutation: &fixture.event.mutation,
                path_state: &fixture.event.path_state,
                old_path_state: fixture.event.old_path_state.as_ref(),
                new_path_state: fixture.event.new_path_state.as_ref(),
            })
            .unwrap()
        );
        assert_eq!(fixture.state.applied_operations().unwrap().len(), 4);
        assert_eq!(
            fixture
                .state
                .outbox()
                .unwrap()
                .iter()
                .any(|row| row.operation_id == fixture.event.operation_id),
            !remove_outbox
                && fixture.event.origin_client_id == client_id()
                && before
                    .outbox
                    .iter()
                    .any(|row| row.operation_id == fixture.event.operation_id)
        );
        fixture.assert_unrelated_state_preserved(&before);
        assert_eq!(
            fixture
                .state
                .set_last_applied_revision(WorkspaceRevision::new(93)),
            Err(SyncError::StreamInvariant {
                reason: "last_applied_regression"
            })
        );
    }
}

#[test]
fn legacy_live_event_gap_rejects_every_non_matching_state_without_side_effects() {
    let cases: &[LegacyGapInvalidCase] = &[
        ("not_contiguous", |fixture| {
            fixture
                .state
                .set_last_ack_revision(WorkspaceRevision::new(94))
                .unwrap();
        }),
        ("pending_ack_mismatch", |fixture| {
            fixture.state.clear_pending_ack().unwrap();
        }),
        ("not_superseded", |fixture| {
            *fixture =
                LegacyGapFixture::with_shape(false, ApplyStage::FilesystemApplied, 94, Some(94));
        }),
        ("receipt_present", |fixture| {
            let mutation_json = fns_sync_core::canonical_json(&fixture.event.mutation).unwrap();
            fixture
                .state
                .record_applied_operation(
                    fixture.event.origin_client_id,
                    fixture.event.operation_id,
                    fixture.event.revision,
                    fns_sync_core::body_digest(&mutation_json),
                    AppliedOperationReceiptKind::MutationResult,
                    Some(&mutation_json),
                )
                .unwrap();
        }),
        ("wrong_stage", |fixture| {
            fixture
                .state
                .set_apply_stage(fixture.journal.apply_id, ApplyStage::DatabaseCommitted)
                .unwrap();
            fixture.journal = fixture.state.apply_journals().unwrap().remove(0);
        }),
        ("digest_mismatch", |fixture| {
            fixture.journal.operation_body_digest = [0x5a; 32];
        }),
        ("wrong_plan", |fixture| {
            fixture.journal.commit_json =
                fns_sync_core::canonical_json(&ApplyCommitPlan::StreamEvent {
                    event: fixture.event.clone(),
                    remove_outbox: false,
                })
                .unwrap();
            fixture.journal.operation_body_digest =
                fns_sync_core::apply_journal_immutable_digest(&fixture.journal);
        }),
        ("wrong_identity", |fixture| {
            fixture.journal.item_key = "95".to_owned();
            fixture.journal.operation_body_digest =
                fns_sync_core::apply_journal_immutable_digest(&fixture.journal);
        }),
        ("associated_outbox_missing", |fixture| {
            *fixture = LegacyGapFixture::new(true);
            fixture
                .state
                .remove_outbox(fixture.event.operation_id)
                .unwrap();
        }),
        ("newer_same_path_state", |fixture| {
            fixture
                .state
                .put_path_state(&legacy_gap_path_state(
                    "legacy-gap.bin",
                    98,
                    b"newer-same-path",
                ))
                .unwrap();
        }),
        ("incomplete_active_stream", |fixture| {
            seed_legacy_gap_stream(&mut fixture.state, false);
        }),
        ("completed_active_stream", |fixture| {
            seed_legacy_gap_stream(&mut fixture.state, true);
        }),
        ("multiple_journals", |fixture| {
            let mut extra = fixture.journal.clone();
            extra.apply_id = fns_fs::ApplyId(
                uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000941").unwrap(),
            );
            extra.item_key = "95".to_owned();
            extra.operation_body_digest = fns_sync_core::apply_journal_immutable_digest(&extra);
            fixture.state.put_apply_journal(&extra).unwrap();
        }),
    ];

    for (name, mutate) in cases {
        let mut fixture = LegacyGapFixture::new(false);
        mutate(&mut fixture);
        let before = fixture.snapshot();
        let result = fixture.state.commit_legacy_live_event_gap(&fixture.journal);
        assert!(result.is_err(), "{name} unexpectedly recovered");
        assert_eq!(fixture.snapshot(), before, "{name} changed durable state");
    }
}

#[test]
fn persisted_filesystem_operation_is_canonical_and_round_trips() {
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("roundtrip.bin"), b"old").unwrap();
    let rooted = fns_fs::RootedWorkspace::open(workspace.path()).unwrap();
    let path = WorkspacePath::parse("roundtrip.bin").unwrap();
    let observed = rooted.inspect(&path).unwrap().unwrap();
    let operation = fns_fs::FsOperation::UpsertFile {
        path,
        content_hash: content_hash(b"new"),
        metadata: WorkspaceFileMetadata {
            size: 3,
            modified_at_ms: 0,
            executable: false,
        },
        expected: fns_fs::ExpectedEntry::Present {
            kind: observed.kind,
            content_hash: Some(content_hash(b"old")),
            directory_snapshot: None,
            fingerprint: observed.fingerprint,
        },
    };
    let json = fns_sync_core::canonical_json(&operation).unwrap();
    let decoded: fns_fs::FsOperation = serde_json::from_slice(&json).unwrap_or_else(|error| {
        panic!(
            "filesystem operation did not decode: {error}; {}",
            String::from_utf8_lossy(&json)
        )
    });
    assert_eq!(fns_sync_core::canonical_json(&decoded).unwrap(), json);
}
