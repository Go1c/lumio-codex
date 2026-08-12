//! Client-initiated conflict resolution (交互设计 5.5「冲突」, 附录 A).
//!
//! The engine already ingests the server's conflict pushes; these tests cover
//! the other direction — turning 「保留本地 / 保留远端 / 两者都保留」 into the
//! `WorkspaceConflictResolved` request the server expects, and keeping that
//! choice durable across a disconnect.

use fns_protocol::{
    ClientId, ConflictId, RequiredNullable, WorkspaceConflictChoice,
    WorkspaceConflictCreatedMessage, WorkspaceConflictKind, WorkspaceConflictSide,
    WorkspaceContentHash, WorkspaceFileMetadata, WorkspaceId, WorkspacePath, WorkspaceRevision,
    revision::WorkspaceConflictRevision,
};
use fns_sync_core::{ConflictStatus, SyncCommand, SyncEngine, SyncEngineConfig, SyncError};
use tempfile::TempDir;

const CONFLICT_ID: &str = "30000000-0000-4000-8000-000000000010";
const LOCAL_HASH: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const REMOTE_HASH: &str = "blake3:2222222222222222222222222222222222222222222222222222222222222222";

struct Fixture {
    engine: SyncEngine,
    _workspace: TempDir,
    _state: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let state = tempfile::tempdir().expect("state directory");
        let config =
            SyncEngineConfig::new(workspace_id(), client_id(), workspace.path(), state.path());
        Self {
            engine: SyncEngine::open(config).expect("engine"),
            _workspace: workspace,
            _state: state,
        }
    }

    fn seed(&mut self, created: &WorkspaceConflictCreatedMessage) {
        self.engine
            .state_mut()
            .record_conflict(created, ConflictStatus::Manual)
            .expect("seed conflict");
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("30000000-0000-4000-8000-000000000001").expect("workspace id")
}

fn client_id() -> ClientId {
    ClientId::parse("30000000-0000-4000-8000-000000000002").expect("client id")
}

fn metadata(size: u64) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size,
        modified_at_ms: 1_800_000_000_000,
        executable: false,
    }
}

fn file_side(path: &str, hash: &str, size: u64) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Value(WorkspacePath::parse(path).expect("side path")),
        path_revision: WorkspaceRevision::new(7),
        content_hash: RequiredNullable::Value(
            WorkspaceContentHash::parse(hash).expect("content hash"),
        ),
        metadata: metadata(size),
        tombstone: false,
    }
}

fn tombstone_side(path: &str) -> WorkspaceConflictSide {
    WorkspaceConflictSide {
        path: RequiredNullable::Value(WorkspacePath::parse(path).expect("side path")),
        path_revision: WorkspaceRevision::new(7),
        content_hash: RequiredNullable::Null,
        metadata: WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false,
        },
        tombstone: true,
    }
}

/// Both sides edited the same file — the 「本地与远端同时修改」 case.
fn both_modified() -> WorkspaceConflictCreatedMessage {
    let path = WorkspacePath::parse("src/engine.rs").expect("conflict path");
    WorkspaceConflictCreatedMessage {
        workspace_id: workspace_id(),
        conflict_id: ConflictId::parse(CONFLICT_ID).expect("conflict id"),
        conflict_revision: WorkspaceConflictRevision::parse("3").expect("conflict revision"),
        path: path.clone(),
        kind: WorkspaceConflictKind::Content,
        ancestor: file_side("src/engine.rs", REMOTE_HASH, 10),
        current: file_side("src/engine.rs", REMOTE_HASH, 20),
        incoming: file_side("src/engine.rs", LOCAL_HASH, 30),
        created_by_operation_id: fns_protocol::OperationId::parse(
            "30000000-0000-4000-8000-000000000003",
        )
        .expect("operation id"),
    }
}

/// The server deleted a file the user had edited — 「远端已删除，本地已修改」.
fn delete_modify() -> WorkspaceConflictCreatedMessage {
    WorkspaceConflictCreatedMessage {
        current: tombstone_side("Cargo.toml"),
        incoming: file_side("Cargo.toml", LOCAL_HASH, 40),
        path: WorkspacePath::parse("Cargo.toml").expect("conflict path"),
        kind: WorkspaceConflictKind::DeleteModify,
        ancestor: file_side("Cargo.toml", REMOTE_HASH, 40),
        ..both_modified()
    }
}

fn resolution(commands: &[SyncCommand]) -> &fns_protocol::WorkspaceConflictResolvedRequest {
    match commands {
        [SyncCommand::ResolveConflict(request)] => request,
        other => panic!("expected exactly one resolution command, got {other:?}"),
    }
}

#[test]
fn keeping_the_local_side_replays_it_verbatim() {
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);

    let commands = fixture
        .engine
        .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Incoming)
        .expect("resolve");

    let request = resolution(&commands);
    assert_eq!(request.choice, WorkspaceConflictChoice::Incoming);
    assert_eq!(request.path, created.path);
    assert_eq!(request.content_hash, created.incoming.content_hash);
    assert_eq!(request.metadata, created.incoming.metadata);
    assert_eq!(request.conflict_revision, created.conflict_revision);
    assert_eq!(request.client_id, client_id());
    request
        .validate_against(&created)
        .expect("the server contract accepts the request we build");
}

#[test]
fn keeping_the_remote_side_replays_the_other_half() {
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);

    let commands = fixture
        .engine
        .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Current)
        .expect("resolve");

    let request = resolution(&commands);
    assert_eq!(request.choice, WorkspaceConflictChoice::Current);
    assert_eq!(request.content_hash, created.current.content_hash);
    request
        .validate_against(&created)
        .expect("the server contract accepts the request we build");
}

#[test]
fn keeping_both_resolves_the_tracked_path_to_the_local_side() {
    // 「两者都保留」 keeps the local bytes at the conflicting path; the server
    // copy lands beside it as an ordinary new file, which syncs as its own
    // mutation rather than as part of the resolution.
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);

    let commands = fixture
        .engine
        .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Incoming)
        .expect("resolve");

    assert_eq!(
        resolution(&commands).content_hash,
        created.incoming.content_hash
    );
}

#[test]
fn choosing_a_deleted_side_becomes_a_delete_resolution() {
    let mut fixture = Fixture::new();
    let created = delete_modify();
    fixture.seed(&created);

    let commands = fixture
        .engine
        .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Current)
        .expect("resolve");

    let request = resolution(&commands);
    assert_eq!(request.choice, WorkspaceConflictChoice::Delete);
    assert!(request.content_hash.is_null());
    assert_eq!(request.metadata.size, 0);
    request
        .validate_against(&created)
        .expect("the server contract accepts a delete resolution");
}

#[test]
fn a_choice_survives_a_disconnect_and_is_replayed_once() {
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);

    let first = fixture
        .engine
        .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Incoming)
        .expect("resolve");
    let replayed = fixture.engine.pending_commands(16).expect("pending");

    assert_eq!(first, replayed, "a reconnect must resend the same request");
    assert_eq!(
        fixture
            .engine
            .state()
            .conflict(created.conflict_id)
            .expect("conflict")
            .expect("still recorded")
            .status,
        ConflictStatus::Resolving
    );
}

#[test]
fn resolving_an_unknown_conflict_is_an_error_rather_than_a_no_op() {
    let mut fixture = Fixture::new();

    assert_eq!(
        fixture.engine.resolve_conflict(
            ConflictId::parse("30000000-0000-4000-8000-000000000099").expect("conflict id"),
            WorkspaceConflictChoice::Current,
        ),
        Err(SyncError::ConflictUnavailable)
    );
}

#[test]
fn a_conflict_the_server_has_re_issued_must_be_read_again_first() {
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);
    fixture
        .engine
        .state_mut()
        .set_conflict_status(created.conflict_id, ConflictStatus::RefreshRequired)
        .expect("mark stale");

    assert_eq!(
        fixture
            .engine
            .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Current),
        Err(SyncError::ConflictRevisionStale)
    );
}

#[test]
fn a_merged_body_is_refused_because_nothing_stages_its_content() {
    let mut fixture = Fixture::new();
    let created = both_modified();
    fixture.seed(&created);

    assert!(matches!(
        fixture
            .engine
            .resolve_conflict(created.conflict_id, WorkspaceConflictChoice::Merged),
        Err(SyncError::MergeRejected { .. })
    ));
}
