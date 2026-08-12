use fns_agent::protocol::{ParentFrame, WorkerFrame, read_worker_frame, write_parent_frame};
use fns_agent::{AgentCommand, AgentConfig, AgentErrorCode, AgentProcess, AgentProcessOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

const AGENT_BIN: &str = env!("CARGO_BIN_EXE_fns-agent");

fn test_config(root: &Path) -> AgentConfig {
    let workspace_root = root.join("workspace");
    let state_dir = root.join("state");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    AgentConfig {
        schema_version: "fns-agent-config/1".into(),
        endpoint: "ws://127.0.0.1:9/api/user/workspace-sync/v2".into(),
        workspace_id: fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000002")
            .unwrap(),
        client_id: fns_protocol::ClientId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
        workspace_root,
        state_dir,
        token_file: root.join("unused-token-file"),
        sync: fns_agent::config::AgentSyncConfig {
            includes: vec!["**".into()],
            excludes: Vec::new(),
            protect_secrets: true,
        },
        transport: fns_agent::config::AgentTransportConfig {
            max_active_transfers: 2,
        },
    }
}

fn token(bytes: &[u8]) -> fns_platform::SecretToken {
    fns_platform::SecretToken::from_private_ipc(bytes.to_vec()).unwrap()
}

fn fixture_command(mode: &str, marker: Option<&Path>) -> AgentCommand {
    static FIXTURE_BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let binary = FIXTURE_BINARY.get_or_init(|| {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/supervisor_child.rs");
        let build_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fns-supervisor-fixture");
        std::fs::create_dir_all(&build_dir).unwrap();
        let binary = build_dir.join(if cfg!(windows) {
            "supervisor-child.exe"
        } else {
            "supervisor-child"
        });
        let output = std::process::Command::new("rustc")
            .args(["--edition=2024", "-o"])
            .arg(&binary)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        binary
    });
    let mut command = AgentCommand::new(binary).arg(mode);
    if let Some(marker) = marker {
        command = command.arg(marker);
    }
    command
}

fn fast_options() -> AgentProcessOptions {
    AgentProcessOptions {
        startup_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_millis(250),
    }
}

#[tokio::test]
async fn ignored_shutdown_is_killed_and_reaped_before_timeout_is_returned() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("ignore-shutdown", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    let pid = child.id().unwrap();
    let started = Instant::now();

    let error = child.shutdown().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::ShutdownTimeout);
    assert!(child.is_reaped());
    assert!(!process_is_alive(pid));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn control_pipe_eof_terminates_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("exit-on-eof", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    child.close_control();
    let exit = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .unwrap()
        .unwrap();

    assert!(exit.success());
    assert!(child.is_reaped());
}

#[tokio::test]
async fn real_worker_control_eof_cancels_daemon_and_persists_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let status_path = config.state_dir.join("runtime-status.json");
    let mut child = AgentProcess::spawn(
        AgentCommand::new(AGENT_BIN).arg("__worker"),
        config,
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();

    child.close_control();
    let error = child.wait().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::AbnormalExit);
    assert!(child.is_reaped());
    let status: fns_agent::AgentStatus =
        serde_json::from_slice(&std::fs::read(status_path).unwrap()).unwrap();
    assert_eq!(status.phase, fns_agent::AgentPhase::Fatal);
    assert_eq!(status.last_error_code, Some(AgentErrorCode::AbnormalExit));
}

#[tokio::test]
async fn stopped_is_not_reported_before_watcher_quiescence_marker() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("watcher-quiesced");
    let mut child = AgentProcess::spawn(
        fixture_command("quiesce-then-stop", Some(&marker)),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    child.shutdown().await.unwrap();

    assert!(marker.is_file());
    assert!(child.is_reaped());
}

#[tokio::test]
async fn fatal_before_ready_reaps_and_immediate_restart_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let error = AgentProcess::spawn(
        fixture_command("fatal-before-ready", None),
        config,
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::Core);

    let mut restarted = AgentProcess::spawn(
        fixture_command("quiesce-then-stop", Some(&dir.path().join("quiesced"))),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn real_worker_startup_fatal_releases_lease_for_immediate_restart() {
    let dir = tempfile::tempdir().unwrap();
    let command = AgentCommand::new(AGENT_BIN).arg("__worker");
    let mut invalid = test_config(dir.path());
    invalid.sync.includes = vec!["[invalid-glob".into()];

    let error = AgentProcess::spawn(
        command.clone(),
        invalid,
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::InvalidConfiguration);
    assert!(error.reaped());

    let mut restarted = AgentProcess::spawn(
        command,
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn abnormal_child_exit_is_observable_and_persisted_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let status_path = config.state_dir.join("runtime-status.json");
    let mut child = AgentProcess::spawn(
        fixture_command("abnormal-after-ready", None),
        config,
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    let error = child.wait().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::AbnormalExit);
    assert!(child.is_reaped());
    let status: fns_agent::AgentStatus =
        serde_json::from_slice(&std::fs::read(status_path).unwrap()).unwrap();
    assert_eq!(status.phase, fns_agent::AgentPhase::Fatal);
    assert_eq!(status.last_error_code, Some(AgentErrorCode::AbnormalExit));
}

#[tokio::test]
async fn startup_timeout_kills_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = fast_options();
    options.startup_timeout = Duration::from_millis(100);

    let error = AgentProcess::spawn(
        fixture_command("never-ready", None),
        test_config(dir.path()),
        token(b"test-token"),
        options,
    )
    .await
    .unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::StartupTimeout);
    assert!(error.reaped());
}

#[tokio::test]
async fn spawn_failure_is_observable() {
    let dir = tempfile::tempdir().unwrap();
    let error = AgentProcess::spawn(
        AgentCommand::new(dir.path().join("does-not-exist")),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::SpawnFailed);
}

#[tokio::test]
async fn duplicate_live_state_directory_is_rejected_and_restart_after_stop_works() {
    let dir = tempfile::tempdir().unwrap();
    let command = AgentCommand::new(AGENT_BIN).arg("__worker");
    let mut first = AgentProcess::spawn(
        command.clone(),
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();

    let duplicate = AgentProcess::spawn(
        command.clone(),
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.code(), AgentErrorCode::AlreadyRunning);
    let status: fns_agent::AgentStatus = serde_json::from_slice(
        &std::fs::read(dir.path().join("state/runtime-status.json")).unwrap(),
    )
    .unwrap();
    assert!(status.running);
    assert_eq!(status.pid, first.id());

    first.shutdown().await.unwrap();
    let mut restarted = AgentProcess::spawn(
        command,
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_is_idempotent_after_reap() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("quiesce-then-stop", Some(&dir.path().join("quiesced"))),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    child.shutdown().await.unwrap();
    child.shutdown().await.unwrap();
    assert!(child.is_reaped());
}

#[tokio::test]
async fn dropping_live_process_kills_reaps_and_allows_immediate_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let child = AgentProcess::spawn(
        fixture_command("ignore-shutdown", None),
        config,
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    let pid = child.id().unwrap();

    drop(child);

    assert!(!process_is_alive(pid), "dropped child PID {pid} remained");
    let mut restarted = AgentProcess::spawn(
        fixture_command(
            "quiesce-then-stop",
            Some(&dir.path().join("drop-restarted")),
        ),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn token_is_absent_from_argv_environment_stderr_and_persisted_status() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("observed-process-data");
    let sentinel = b"FNS_SENTINEL_TOKEN_6c26289d";
    let config = test_config(dir.path());
    let mut child = AgentProcess::spawn(
        fixture_command("record-process-data", Some(&marker)),
        config,
        token(sentinel),
        fast_options(),
    )
    .await
    .unwrap();
    child.shutdown().await.unwrap();

    let sentinel = String::from_utf8(sentinel.to_vec()).unwrap();
    let observed = std::fs::read_to_string(marker).unwrap();
    assert_eq!(
        observed,
        "argv_contains_token=false\nenvironment_contains_token=false\n"
    );
    assert!(!observed.contains(&sentinel));

    let actual_root = dir.path().join("actual-worker");
    let actual_config = test_config(&actual_root);
    let state_dir = actual_config.state_dir.clone();
    let mut actual = AgentProcess::spawn(
        AgentCommand::new(AGENT_BIN).arg("__worker"),
        actual_config,
        token(sentinel.as_bytes()),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();
    actual.shutdown().await.unwrap();
    for entry in walk_files(&state_dir) {
        assert!(
            !std::fs::read(&entry)
                .unwrap()
                .windows(sentinel.len())
                .any(|window| window == sentinel.as_bytes())
        );
    }
}

#[tokio::test]
async fn killed_after_filesystem_started_reopens_recovers_and_reconciles() {
    let dir = tempfile::tempdir().unwrap();
    let command = AgentCommand::new(AGENT_BIN).arg("__worker");
    let config = test_config(dir.path());
    let workspace_id = config.workspace_id;
    let client_id = config.client_id;
    let state_dir = config.state_dir.clone();
    let workspace_root = config.workspace_root.clone();
    let mut child = AgentProcess::spawn(
        command.clone(),
        config,
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();

    let path = fns_protocol::WorkspacePath::parse("recovered-directory").unwrap();
    std::fs::create_dir_all(workspace_root.join(path.as_str())).unwrap();
    let state = fns_protocol::WorkspacePathState {
        path: path.clone(),
        path_revision: fns_protocol::WorkspaceRevision::new(1),
        kind: fns_protocol::WorkspaceEntryKind::Directory,
        content_hash: fns_protocol::RequiredNullable::Null,
        metadata: fns_protocol::WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 0,
            executable: false,
        },
        tombstone: false,
    };
    let operation = fns_sync_core::model::RemoteApplyOperation::Upsert {
        state: state.clone(),
    };
    let mut sqlite =
        fns_sync_core::SqliteState::open(state_dir.join("state.sqlite"), workspace_id, client_id)
            .unwrap();
    sqlite
        .put_apply_journal(&fns_sync_core::ApplyJournalRecord {
            apply_id: fns_sync_core::ApplyId(uuid::Uuid::new_v4()),
            workspace_id,
            stream_id: fns_protocol::StreamId::parse("10000000-0000-4000-8000-000000000099")
                .unwrap(),
            item_kind: fns_sync_core::ApplyItemKind::Entry,
            item_key: path.as_str().to_owned(),
            apply_namespace: fns_sync_core::ApplyNamespace::SnapshotEntry,
            operation_body_digest: [0; 32],
            operation_json: fns_sync_core::canonical_json(&operation).unwrap(),
            filesystem_operation_json: Vec::new(),
            commit_json: Vec::new(),
            preimage_json: b"null".to_vec(),
            postimage_json: fns_sync_core::canonical_json(&vec![state]).unwrap(),
            filesystem_receipt_json: None,
            stage: fns_sync_core::ApplyStage::FilesystemStarted,
        })
        .unwrap();
    drop(sqlite);

    child.force_kill_and_reap().await.unwrap();
    assert!(child.is_reaped());

    let mut restarted = AgentProcess::spawn(
        command,
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();
    restarted.shutdown().await.unwrap();

    let sqlite =
        fns_sync_core::SqliteState::open(state_dir.join("state.sqlite"), workspace_id, client_id)
            .unwrap();
    assert!(sqlite.apply_journals().unwrap().is_empty());
    assert!(workspace_root.join(path.as_str()).is_dir());
}

#[tokio::test]
async fn malformed_and_oversized_frames_are_rejected() {
    let (mut writer, mut reader) = tokio::io::duplex(64);
    writer
        .write_all(&(1_048_577_u32).to_be_bytes())
        .await
        .unwrap();
    let oversized = read_worker_frame(&mut reader).await.unwrap_err();
    assert_eq!(oversized.code(), AgentErrorCode::Protocol);

    let (mut writer, mut reader) = tokio::io::duplex(64);
    writer.write_all(&4_u32.to_be_bytes()).await.unwrap();
    writer.write_all(b"nope").await.unwrap();
    drop(writer);
    let malformed = read_worker_frame(&mut reader).await.unwrap_err();
    assert_eq!(malformed.code(), AgentErrorCode::Protocol);
}

#[tokio::test]
async fn frame_write_failure_is_returned() {
    let (writer, reader) = tokio::io::duplex(64);
    drop(reader);
    let error = write_parent_frame(writer, &ParentFrame::Shutdown)
        .await
        .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::Protocol);
}

#[tokio::test]
async fn child_frame_failure_kills_and_reaps_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("malformed-after-ready", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    let error = child.shutdown().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::Protocol);
    assert!(error.reaped());
    assert!(child.is_reaped());
}

#[tokio::test]
async fn truncated_frame_read_is_returned() {
    let (mut writer, mut reader) = tokio::io::duplex(64);
    writer.write_all(&10_u32.to_be_bytes()).await.unwrap();
    writer.write_all(b"{}").await.unwrap();
    drop(writer);
    assert_eq!(
        read_worker_frame(&mut reader).await.unwrap_err().code(),
        AgentErrorCode::Protocol
    );
}

#[tokio::test]
async fn list_resolve_error_list_and_shutdown_share_one_worker_stream() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("list-fail-list", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    assert!(child.list_conflicts().await.unwrap().is_empty());
    let error = child
        .resolve_conflict(
            fns_protocol::ConflictId::parse("90000000-0000-4000-8000-000000000001").unwrap(),
            fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
            fns_protocol::WorkspaceConflictChoice::Current,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::Core);
    assert!(child.list_conflicts().await.unwrap().is_empty());

    child.shutdown().await.unwrap();
    assert!(child.is_reaped());
}

#[tokio::test]
async fn wrong_request_id_is_protocol_failure_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("wrong-request-id", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    let error = child.list_conflicts().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::Protocol);
    assert!(error.reaped());
    assert!(child.is_reaped());
}

#[tokio::test]
async fn duplicate_response_poison_is_observed_by_next_rpc_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("duplicate-response", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();

    assert!(child.list_conflicts().await.unwrap().is_empty());
    let error = child.list_conflicts().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::Protocol);
    assert!(error.reaped());
    assert!(child.is_reaped());
}

#[tokio::test]
async fn rpc_timeout_reaps_child_and_successor_starts_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        fixture_command("rpc-timeout", None),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    let pid = child.id().unwrap();

    let error = child.list_conflicts().await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::RequestTimeout);
    assert!(error.reaped());
    assert!(child.is_reaped());
    assert!(!process_is_alive(pid));

    let mut successor = AgentProcess::spawn(
        fixture_command("quiesce-then-stop", Some(&dir.path().join("successor"))),
        test_config(dir.path()),
        token(b"test-token"),
        fast_options(),
    )
    .await
    .unwrap();
    successor.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_and_broken_streams_fail_pending_rpc_and_reap_child() {
    for (mode, expected) in [
        ("fatal-during-rpc", AgentErrorCode::Core),
        ("stopped-during-rpc", AgentErrorCode::AbnormalExit),
        ("eof-during-rpc", AgentErrorCode::Protocol),
        ("truncated-during-rpc", AgentErrorCode::Protocol),
        ("oversized-during-rpc", AgentErrorCode::Protocol),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut child = AgentProcess::spawn(
            fixture_command(mode, None),
            test_config(dir.path()),
            token(b"test-token"),
            fast_options(),
        )
        .await
        .unwrap();

        let error = child.list_conflicts().await.unwrap_err();

        assert_eq!(error.code(), expected, "mode={mode}");
        assert!(error.reaped(), "mode={mode}");
        assert!(child.is_reaped(), "mode={mode}");
    }
}

#[tokio::test]
async fn real_worker_forwards_repeated_rpc_to_its_live_engine() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = AgentProcess::spawn(
        AgentCommand::new(AGENT_BIN).arg("__worker"),
        test_config(dir.path()),
        token(b"test-token"),
        AgentProcessOptions::default(),
    )
    .await
    .unwrap();

    assert!(child.list_conflicts().await.unwrap().is_empty());
    let error = child
        .resolve_conflict(
            fns_protocol::ConflictId::parse("90000000-0000-4000-8000-000000000001").unwrap(),
            fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
            fns_protocol::WorkspaceConflictChoice::Incoming,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), AgentErrorCode::ConflictUnavailable);
    assert!(child.list_conflicts().await.unwrap().is_empty());

    child.shutdown().await.unwrap();
}

#[tokio::test]
async fn noncanonical_rpc_request_id_is_rejected() {
    let payload = br#"{"type":"conflicts_listed","requestId":"90000000-0000-4000-8000-0000000000AA","conflicts":[]}"#;
    let (mut writer, mut reader) = tokio::io::duplex(256);
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    writer.write_all(payload).await.unwrap();

    let error = read_worker_frame(&mut reader).await.unwrap_err();

    assert_eq!(error.code(), AgentErrorCode::Protocol);
}

#[test]
fn resolve_control_frame_contains_only_user_selection_fields() {
    let request_id =
        fns_protocol::RequestId::parse("90000000-0000-4000-8000-000000000002").unwrap();
    let frame = ParentFrame::ResolveConflict {
        request_id,
        conflict_id: fns_protocol::ConflictId::parse("90000000-0000-4000-8000-000000000001")
            .unwrap(),
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision::parse("1").unwrap(),
        choice: fns_protocol::WorkspaceConflictChoice::Merged,
    };

    let value = serde_json::to_value(frame).unwrap();
    let object = value.as_object().unwrap();

    assert_eq!(
        object.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "choice",
            "conflictId",
            "conflictRevision",
            "requestId",
            "type"
        ]
    );
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| !output.stdout.is_empty())
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
}

#[allow(dead_code)]
fn _frame_types_are_not_token_bearing_debug() {
    let _ = WorkerFrame::Ready;
}
