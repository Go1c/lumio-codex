use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::fd::{AsRawFd, IntoRawFd};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(target_os = "macos")]
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_sync::cli::RunArgs;
use test_sync::effect::{
    EffectAction, EffectContext, EffectIdentity, EffectObservation, EffectReceipt,
};
use test_sync::evidence::{EvidenceWriter, Redactor};
use test_sync::harness;
use test_sync::manifest::{build_manifest, PathKind};
use test_sync::process::{OwnedChild, PinnedExecutable, ProcessSpec, Termination};
use test_sync::scenario::{apply_action, deterministic_plan, Endpoint, ScenarioAction};
#[cfg(unix)]
use test_sync::secret::{SecretMaterial, TokenSource};
use test_sync::snapshot::{
    CheckpointSample, ClientSnapshot, ConflictSnapshot, CursorSnapshot, RuntimeSnapshot,
    SnapshotExpectation, StreamSnapshot,
};
use test_sync::stability::{classify_stability, Stability};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;

#[test]
fn manifest_is_sorted_and_hashes_file_bytes_without_decoding() {
    let temporary = tempfile::tempdir().expect("temporary workspace");
    fs::create_dir_all(temporary.path().join("nested")).expect("nested directory");
    fs::write(temporary.path().join("z-empty"), []).expect("empty file");
    let invalid_utf8 = [0xff, 0xfe, 0x00, b'a'];
    fs::write(temporary.path().join("nested/a-binary"), invalid_utf8).expect("binary file");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            temporary.path().join("nested/a-binary"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("set executable bit");
    }

    let manifest = build_manifest(temporary.path()).expect("manifest");
    let paths: Vec<_> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, ["nested", "nested/a-binary", "z-empty"]);
    assert_eq!(manifest.entries[0].kind, PathKind::Directory);
    assert_eq!(manifest.entries[1].kind, PathKind::File);
    assert_eq!(manifest.entries[1].size, 4);
    assert_eq!(
        manifest.entries[1].blake3.as_deref(),
        Some(blake3::hash(&invalid_utf8).to_hex().as_str())
    );
    #[cfg(unix)]
    assert_eq!(manifest.entries[1].mode, 0o755);
    assert_eq!(
        manifest.digest,
        blake3::hash(&serde_json::to_vec(&manifest.entries).expect("canonical entries"))
            .to_hex()
            .to_string()
    );
}

#[cfg(unix)]
#[test]
fn manifest_distinguishes_complete_portable_permission_modes() {
    use std::os::unix::fs::PermissionsExt;

    let left = tempfile::tempdir().expect("left root");
    let right_root = tempfile::tempdir().expect("right root");
    for root in [left.path(), right_root.path()] {
        fs::write(root.join("same.txt"), b"same bytes").expect("write fixture");
    }
    fs::set_permissions(
        left.path().join("same.txt"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("left mode");
    fs::set_permissions(
        right_root.path().join("same.txt"),
        fs::Permissions::from_mode(0o640),
    )
    .expect("right mode");

    let left = build_manifest(left.path()).expect("left manifest");
    let right = build_manifest(right_root.path()).expect("right manifest");
    assert_eq!(left.entries[0].mode, 0o600);
    assert_eq!(right.entries[0].mode, 0o640);
    assert_ne!(left, right);
    assert!(left.sync_equivalent(&right));

    fs::set_permissions(
        right_root.path().join("same.txt"),
        fs::Permissions::from_mode(0o750),
    )
    .expect("right executable mode");
    let executable = build_manifest(right_root.path()).expect("executable manifest");
    assert!(!left.sync_equivalent(&executable));
}

#[test]
fn deterministic_plan_exercises_executable_mode_propagation_and_reversal() {
    let plan = deterministic_plan(1024 * 1024).expect("deterministic plan");
    assert!(plan.iter().any(|action| matches!(
        action,
        ScenarioAction::SetMode {
            path,
            mode: 0o755,
            ..
        } if path == "text/plain.txt"
    )));
    assert!(plan.iter().any(|action| matches!(
        action,
        ScenarioAction::SetMode {
            path,
            mode: 0o644,
            ..
        } if path == "text/plain.txt"
    )));
}

#[cfg(unix)]
#[test]
fn executable_mode_changes_are_required_for_manifest_convergence() {
    use std::os::unix::fs::PermissionsExt;

    let left = tempfile::tempdir().expect("left root");
    let right = tempfile::tempdir().expect("right root");
    for root in [left.path(), right.path()] {
        fs::write(root.join("mode.txt"), b"same bytes").expect("write mode fixture");
        fs::set_permissions(root.join("mode.txt"), fs::Permissions::from_mode(0o644))
            .expect("initial mode");
    }
    let make_executable = ScenarioAction::SetMode {
        endpoint: Endpoint::A,
        path: "mode.txt".to_owned(),
        mode: 0o755,
    };
    apply_action(left.path(), right.path(), &make_executable).expect("toggle source mode");
    assert!(!build_manifest(left.path())
        .expect("left manifest")
        .sync_equivalent(&build_manifest(right.path()).expect("right manifest")));

    fs::set_permissions(
        right.path().join("mode.txt"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("propagate executable mode");
    assert!(build_manifest(left.path())
        .expect("left propagated manifest")
        .sync_equivalent(&build_manifest(right.path()).expect("right propagated manifest")));
}

#[test]
fn evidence_redactor_removes_jwts_from_nested_strings() {
    let token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MSJ9.c2lnbmF0dXJl";
    let input = json!({
        "authorization": format!("Bearer {token}"),
        "nested": [format!("prefix={token};suffix"), "safe"],
    });
    let output = Redactor::default().redact_value(input);
    let encoded = serde_json::to_string(&output).expect("redacted JSON");
    assert!(!encoded.contains(token));
    assert!(!encoded.contains("c2lnbmF0dXJl"));
    assert!(encoded.contains("[REDACTED]"));
    assert!(encoded.contains("safe"));
}

#[test]
fn evidence_writer_honors_an_explicit_absolute_root() {
    let temporary = tempfile::tempdir().expect("temporary evidence root");
    let writer =
        EvidenceWriter::create_in(temporary.path(), "custom-evidence-root", b"eyJ9.e30.c2ln")
            .expect("create custom evidence writer");
    assert_eq!(writer.root(), temporary.path().join("custom-evidence-root"));
    writer
        .write_json("result.json", &json!({"status": "ok"}))
        .expect("write custom evidence");
    let sums = writer.finalize().expect("finalize custom evidence");
    assert_eq!(sums, writer.root().join("SHA256SUMS"));
}

#[test]
fn evidence_redactor_removes_exact_secret_and_padded_jwt_variants() {
    let padded = "eyJ9.e30=.c2ln";
    let redactor = Redactor::with_secret(padded.as_bytes());
    let encoded = redactor.redact_text(&format!(
        "exact={padded} accepted=YWJj.ZGVm.Z2hp malformed-safe=abc.def"
    ));
    assert!(!encoded.contains(padded));
    assert!(!encoded.contains("YWJj.ZGVm.Z2hp"));
    assert!(encoded.contains("malformed-safe=abc.def"));
}

#[test]
fn typed_effect_receipt_rejects_wrong_action_and_no_op() {
    let context = EffectContext {
        workspace_id: "workspace".to_owned(),
        client_id_a: "client-a".to_owned(),
        client_id_b: "client-b".to_owned(),
        agent_pid_a: 101,
        agent_pid_b: 202,
    };
    let receipt = EffectReceipt {
        schema_version: "test-sync-effect/1".to_owned(),
        action: EffectAction::Reconnect,
        context: context.clone(),
        old: EffectIdentity {
            pid: Some(301),
            generation: Some(7),
        },
        new: EffectIdentity {
            pid: Some(301),
            generation: Some(8),
        },
    };
    let before = EffectObservation::new(EffectAction::Reconnect, context.clone(), receipt.old);
    let after = EffectObservation::new(EffectAction::Reconnect, context.clone(), receipt.new);
    receipt
        .validate_observed(EffectAction::Reconnect, &context, &before, &after)
        .expect("independently observed reconnect effect");
    assert!(receipt
        .validate_observed(EffectAction::AppRestart, &context, &before, &after)
        .is_err());

    let no_op = EffectReceipt {
        new: receipt.old,
        ..receipt.clone()
    };
    assert!(no_op
        .validate_observed(EffectAction::Reconnect, &context, &before, &before)
        .is_err());

    let fabricated_change_over_unchanged_observation = EffectReceipt {
        new: EffectIdentity {
            pid: Some(301),
            generation: Some(99),
        },
        ..receipt
    };
    assert!(fabricated_change_over_unchanged_observation
        .validate_observed(EffectAction::Reconnect, &context, &before, &before)
        .is_err());
}

#[test]
fn typed_effect_receipt_requires_matching_exact_identity_kinds() {
    let context = EffectContext {
        workspace_id: "workspace".to_owned(),
        client_id_a: "client-a".to_owned(),
        client_id_b: "client-b".to_owned(),
        agent_pid_a: 101,
        agent_pid_b: 202,
    };
    let receipt = EffectReceipt {
        schema_version: "test-sync-effect/1".to_owned(),
        action: EffectAction::AgentRestart,
        context: context.clone(),
        old: EffectIdentity {
            pid: Some(101),
            generation: Some(1),
        },
        new: EffectIdentity {
            pid: None,
            generation: Some(2),
        },
    };
    let before = EffectObservation::new(EffectAction::AgentRestart, context.clone(), receipt.old);
    let after = EffectObservation::new(EffectAction::AgentRestart, context.clone(), receipt.new);
    assert!(receipt
        .validate_observed(EffectAction::AgentRestart, &context, &before, &after)
        .is_err());
}

#[test]
fn app_restart_requires_exact_observed_pid_generation_and_context_transition() {
    let context = EffectContext {
        workspace_id: "workspace".to_owned(),
        client_id_a: "client-a".to_owned(),
        client_id_b: "client-b".to_owned(),
        agent_pid_a: 101,
        agent_pid_b: 202,
    };
    let old = EffectIdentity {
        pid: Some(401),
        generation: Some(11),
    };
    let new = EffectIdentity {
        pid: Some(402),
        generation: Some(12),
    };
    let receipt =
        EffectReceipt::observed_transition(EffectAction::AppRestart, context.clone(), old, new);
    let before = EffectObservation::new(EffectAction::AppRestart, context.clone(), old);
    let after = EffectObservation::new(EffectAction::AppRestart, context.clone(), new);
    receipt
        .validate_observed(EffectAction::AppRestart, &context, &before, &after)
        .expect("exact app restart transition");

    let mut wrong_workspace = context.clone();
    wrong_workspace.workspace_id = "other-workspace".to_owned();
    assert!(receipt
        .validate_observed(EffectAction::AppRestart, &wrong_workspace, &before, &after,)
        .is_err());
    assert!(receipt
        .validate_observed(EffectAction::AppRestart, &context, &before, &before)
        .is_err());
}

#[cfg(unix)]
#[test]
fn token_source_descriptor_is_consumed_and_closed() {
    let (reader, writer) = rustix::pipe::pipe().expect("private token pipe");
    let source_fd = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 512) };
    assert!(source_fd >= 512, "allocate an isolated test descriptor");
    drop(reader);
    let mut writer = fs::File::from(writer);
    writer
        .write_all(b"eyJ9.e30.c2ln\n")
        .expect("write private token");
    drop(writer);

    let secret = SecretMaterial::read(TokenSource::Descriptor(
        u32::try_from(source_fd).expect("nonnegative descriptor"),
    ))
    .expect("read private token");
    assert_eq!(format!("{secret:?}"), "SecretMaterial([REDACTED])");
    assert_eq!(unsafe { libc::fcntl(source_fd, libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
}

#[cfg(unix)]
#[test]
fn named_fifo_is_not_accepted_as_a_private_token_source() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let fifo = temporary.path().join("token.fifo");
    let status = std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success());
    fs::set_permissions(&fifo, fs::Permissions::from_mode(0o666)).expect("set FIFO mode");
    let writer_path = fifo.clone();
    let writer = std::thread::spawn(move || {
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .open(writer_path)
            .expect("open FIFO writer");
        writer
            .write_all(b"eyJ9.e30.c2ln\n")
            .expect("write FIFO token");
    });
    let reader = fs::OpenOptions::new()
        .read(true)
        .open(&fifo)
        .expect("open FIFO reader");
    let source_fd = reader.into_raw_fd();

    let error = SecretMaterial::read(TokenSource::Descriptor(
        u32::try_from(source_fd).expect("nonnegative descriptor"),
    ))
    .expect_err("named FIFO must be rejected");
    writer.join().expect("FIFO writer");
    assert!(error.to_string().contains("private"));
}

#[cfg(unix)]
#[test]
fn group_readable_regular_file_is_not_accepted_as_a_token_source() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("token");
    fs::write(&path, b"eyJ9.e30.c2ln").expect("write token file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("set unsafe mode");
    let source_fd = fs::File::open(&path)
        .expect("open token file")
        .into_raw_fd();
    let error = SecretMaterial::read(TokenSource::Descriptor(
        u32::try_from(source_fd).expect("nonnegative descriptor"),
    ))
    .expect_err("group-readable token file must fail");
    assert!(error.to_string().contains("owner-only"));
}

#[cfg(unix)]
#[test]
fn jwt_padding_is_only_accepted_at_canonical_segment_ends() {
    fn read_token(token: &[u8]) -> test_sync::Result<SecretMaterial> {
        let (reader, writer) = rustix::pipe::pipe().expect("private token pipe");
        let source_fd = reader.into_raw_fd();
        let mut writer = fs::File::from(writer);
        writer.write_all(token).expect("write token");
        drop(writer);
        SecretMaterial::read(TokenSource::Descriptor(
            u32::try_from(source_fd).expect("nonnegative descriptor"),
        ))
    }

    read_token(b"eyJ9.e30=.c2ln").expect("canonical padded JWT");
    let malformed = read_token(b"eyJ9.e=30.c2ln").expect_err("embedded padding must fail");
    assert!(malformed.to_string().contains("JWT"));
}

#[test]
fn stability_requires_three_identical_acceptable_samples() {
    assert_eq!(
        classify_stability(&[7_u8, 7], |sample| *sample == 7),
        Stability::Collecting { identical: 2 }
    );
    assert_eq!(
        classify_stability(&[7_u8, 8, 7], |sample| *sample == 7),
        Stability::Collecting { identical: 1 }
    );
    assert_eq!(
        classify_stability(&[6_u8, 7, 7, 7], |sample| *sample == 7),
        Stability::Stable
    );
    assert_eq!(
        classify_stability(&[9_u8, 9, 9], |sample| *sample == 7),
        Stability::Rejected
    );
}

#[test]
fn checkpoint_stability_ignores_runtime_heartbeat_timestamp() {
    let expected = snapshot_expectation();
    let first = settled_sample();
    let mut second = first.clone();
    second.client_a.runtime.updated_at_ms = 2;
    second.client_b.runtime.updated_at_ms = 3;
    let mut third = second.clone();
    third.client_a.runtime.updated_at_ms = 4;
    third.client_b.runtime.updated_at_ms = 5;
    let samples = [first, second, third].map(|sample| sample.stability_projection());

    assert_eq!(
        classify_stability(&samples, |sample| sample.converged(&expected)),
        Stability::Stable
    );

    let mut changed = samples;
    changed[2].client_a.runtime.pending_commands = 1;
    assert_eq!(
        classify_stability(&changed, |sample| sample.converged(&expected)),
        Stability::Collecting { identical: 1 }
    );
}

fn settled_client(client_id: &str, pid: u32) -> ClientSnapshot {
    ClientSnapshot {
        cursor: Some(CursorSnapshot {
            client_id: client_id.to_owned(),
            last_ack_revision: "7".to_owned(),
            last_applied_revision: "7".to_owned(),
            pending_ack_revision: None,
        }),
        outbox: BTreeMap::new(),
        intents: 0,
        stream: StreamSnapshot::default(),
        journal: BTreeMap::new(),
        conflicts: Vec::new(),
        sqlite_quick_check: "ok".to_owned(),
        sqlite_journal_mode: "wal".to_owned(),
        runtime: RuntimeSnapshot {
            schema_version: "fns-agent-status/1".to_owned(),
            workspace_id: "workspace".to_owned(),
            running: true,
            phase: "online".to_owned(),
            pid: Some(pid),
            connected: true,
            last_ack_revision: "7".to_owned(),
            pending_commands: 0,
            queued_watcher_batches: 0,
            active_transfers: 0,
            reconnect_attempt: 0,
            last_error_code: None,
            updated_at_ms: 1,
        },
    }
}

fn settled_sample() -> CheckpointSample {
    let manifest = test_sync::manifest::Manifest {
        entries: Vec::new(),
        digest: "digest".to_owned(),
    };
    CheckpointSample {
        manifest_a: manifest.clone(),
        manifest_b: manifest,
        client_a: settled_client("client-a", 101),
        client_b: settled_client("client-b", 202),
    }
}

fn snapshot_expectation() -> SnapshotExpectation<'static> {
    SnapshotExpectation {
        workspace_id: "workspace",
        client_id_a: "client-a",
        client_id_b: "client-b",
        pids: (101, 202),
    }
}

#[test]
fn converged_snapshot_requires_exact_revision_ack_runtime_and_identity_consistency() {
    let expected = snapshot_expectation();
    let sample = settled_sample();
    assert_eq!(sample.global_revision(&expected), Some("7"));
    assert!(sample.converged(&expected));

    let mut applied_mismatch = sample.clone();
    applied_mismatch
        .client_a
        .cursor
        .as_mut()
        .expect("cursor")
        .last_applied_revision = "6".to_owned();
    assert!(!applied_mismatch.converged(&expected));

    let mut runtime_ack_mismatch = sample.clone();
    runtime_ack_mismatch.client_b.runtime.last_ack_revision = "6".to_owned();
    assert!(!runtime_ack_mismatch.converged(&expected));

    let mut identity_mismatch = sample;
    identity_mismatch
        .client_b
        .cursor
        .as_mut()
        .expect("cursor")
        .client_id = "client-a".to_owned();
    assert!(!identity_mismatch.converged(&expected));

    let mut global_revision_mismatch = settled_sample();
    let cursor = global_revision_mismatch
        .client_b
        .cursor
        .as_mut()
        .expect("cursor");
    cursor.last_ack_revision = "8".to_owned();
    cursor.last_applied_revision = "8".to_owned();
    global_revision_mismatch.client_b.runtime.last_ack_revision = "8".to_owned();
    assert!(!global_revision_mismatch.converged(&expected));

    let mut pending_ack = settled_sample();
    pending_ack
        .client_a
        .cursor
        .as_mut()
        .expect("cursor")
        .pending_ack_revision = Some("8".to_owned());
    assert!(!pending_ack.converged(&expected));

    let mut wrong_workspace = settled_sample();
    wrong_workspace.client_a.runtime.workspace_id = "other".to_owned();
    assert!(!wrong_workspace.converged(&expected));

    let mut wrong_pid = settled_sample();
    wrong_pid.client_b.runtime.pid = Some(203);
    assert!(!wrong_pid.converged(&expected));

    let mut queued = settled_sample();
    queued.client_a.runtime.pending_commands = 1;
    assert!(!queued.converged(&expected));

    let mut journal = settled_sample();
    journal.client_b.journal.insert("prepared".to_owned(), 1);
    assert!(!journal.converged(&expected));

    let mut runtime_error = settled_sample();
    runtime_error.client_a.runtime.last_error_code = Some("transport".to_owned());
    assert!(!runtime_error.converged(&expected));
}

#[cfg(unix)]
#[tokio::test]
async fn failed_agent_start_persists_pid_group_and_cleanup_evidence_without_secret() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("harness directories");
    let run_id = format!(
        "failed-start-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    );
    let (reader, writer) = rustix::pipe::pipe().expect("private token pipe");
    let token_fd = reader.into_raw_fd();
    let token = "eyJ9.e30.c2ln";
    let mut writer = fs::File::from(writer);
    writer
        .write_all(token.as_bytes())
        .expect("write private token");
    drop(writer);
    let observer = temporary.path().join("observer.sh");
    fs::write(&observer, "#!/bin/sh\nexit 1\n").expect("write observer fixture");
    fs::set_permissions(&observer, fs::Permissions::from_mode(0o700))
        .expect("chmod observer fixture");
    let args = RunArgs {
        endpoint_a: "ws://127.0.0.1:1/api/user/workspace-sync/v2".to_owned(),
        endpoint_b: "ws://127.0.0.1:2/api/user/workspace-sync/v2".to_owned(),
        workspace_id: "10000000-0000-4000-8000-000000000002".to_owned(),
        client_id_a: "10000000-0000-4000-8000-000000000003".to_owned(),
        client_id_b: "10000000-0000-4000-8000-000000000004".to_owned(),
        root_a: temporary.path().join("root-a"),
        root_b: temporary.path().join("root-b"),
        state_a: temporary.path().join("state-a"),
        state_b: temporary.path().join("state-b"),
        agent_binary: "/usr/bin/false".into(),
        reconnect_hook: "/usr/bin/true".into(),
        app_restart_hook: "/usr/bin/true".into(),
        effect_observer: observer,
        run_id: run_id.clone(),
        evidence_root: None,
        token_stdin: false,
        token_fd: Some(u32::try_from(token_fd).expect("nonnegative descriptor")),
        startup_timeout_seconds: 1,
        checkpoint_timeout_seconds: 1,
        sample_interval_millis: 10,
        hook_timeout_seconds: 1,
        term_grace_seconds: 1,
        kill_timeout_seconds: 1,
        large_file_bytes: 1024 * 1024,
        max_active_transfers: 1,
    };
    let error = harness::run(args, CancellationToken::new())
        .await
        .expect_err("false agent must fail startup");
    assert!(error.to_string().contains("agent") || error.to_string().contains("process"));

    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("client root")
        .join("target/e2e-evidence")
        .join(&run_id);
    let process = fs::read_to_string(evidence.join("process.jsonl")).expect("process evidence");
    assert!(process.contains("\"event\":\"spawned\""));
    assert!(process.contains("reaped_after_startup_failure"));
    assert!(process.contains("\"pid\":"));
    assert!(process.contains("\"pgid\":"));
    let mut evidence_text = process;
    evidence_text
        .push_str(&fs::read_to_string(evidence.join("result.json")).expect("result evidence"));
    evidence_text.push_str(&fs::read_to_string(evidence.join("SHA256SUMS")).expect("checksums"));
    assert!(!evidence_text.contains(token));
    fs::remove_dir_all(evidence).expect("remove test evidence");
}

#[test]
fn conflict_snapshot_requires_same_exact_scenario_conflict_on_both_clients() {
    let expected = snapshot_expectation();
    let conflict = ConflictSnapshot {
        conflict_id: "conflict-1".to_owned(),
        conflict_revision: "1".to_owned(),
        path: "conflict/concurrent.txt".to_owned(),
        kind: "content".to_owned(),
        status: "manual".to_owned(),
    };
    let mut sample = settled_sample();
    sample.client_a.conflicts.push(conflict.clone());
    sample.client_b.conflicts.push(conflict);
    assert!(sample.conflict_stable(&expected, "conflict/concurrent.txt", "content"));
    assert!(!sample.conflict_stable(&expected, "conflict/other.txt", "content"));
    assert!(!sample.conflict_stable(&expected, "conflict/concurrent.txt", "binary"));

    sample.client_b.conflicts[0].conflict_id = "other".to_owned();
    assert!(!sample.conflict_stable(&expected, "conflict/concurrent.txt", "content"));
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_escalates_term_to_kill_within_bound() {
    let mut child = OwnedChild::spawn(ProcessSpec::control(
        "ignore-term",
        "/bin/sh",
        [
            "-c",
            "trap '' TERM; printf 'ready\\n'; while :; do sleep 1; done",
        ],
    ))
    .expect("spawn owned child");
    let stdout = child.take_stdout().expect("owned stdout");
    let mut reader = BufReader::new(stdout);
    let mut ready = String::new();
    reader.read_line(&mut ready).await.expect("read readiness");
    assert_eq!(ready, "ready\n");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let started = Instant::now();
    let outcome = child
        .wait_or_cancel(
            &cancellation,
            Duration::from_millis(50),
            Duration::from_secs(1),
        )
        .await
        .expect("bounded cancellation");
    assert_eq!(outcome.termination, Termination::Killed);
    assert_eq!(outcome.status.signal(), Some(libc::SIGKILL));
    assert_eq!(outcome.group_cleanup.termination, Termination::Killed);
    assert!(outcome.group_cleanup.term_attempted);
    assert!(outcome.group_cleanup.kill_attempted);
    assert!(outcome.group_cleanup.group_empty);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(child.reap_count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn exact_child_is_reaped_once_and_repeated_wait_reuses_status() {
    let mut child = OwnedChild::spawn(ProcessSpec::quiet("exit-17", "/bin/sh", ["-c", "exit 17"]))
        .expect("spawn owned child");
    let pid = child.pid();
    let first = child.wait().await.expect("first wait");
    let second = child.wait().await.expect("repeated wait");
    assert_eq!(first.code(), Some(17));
    assert_eq!(second, first);
    assert_eq!(child.reap_count(), 1);
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn every_process_io_closes_non_cloexec_duplicate_token_source() {
    use std::os::unix::fs::PermissionsExt;

    let token = "eyJ9.e30.c2ln";
    let temporary = tempfile::tempdir().expect("private token directory");
    let token_path = temporary.path().join("token");
    fs::write(&token_path, token).expect("write private token");
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
        .expect("private token mode");
    let token_file = fs::File::open(&token_path).expect("open token source");
    let selected = rustix::io::dup(&token_file).expect("selected token descriptor");
    let inherited = rustix::io::dup(&token_file).expect("non-CLOEXEC duplicate token descriptor");
    let selected_fd = selected.into_raw_fd();
    let inherited_fd = inherited.as_raw_fd();
    let secret = SecretMaterial::read(TokenSource::Descriptor(
        u32::try_from(selected_fd).expect("nonnegative descriptor"),
    ))
    .expect("consume token source");
    assert_eq!(format!("{secret:?}"), "SecretMaterial([REDACTED])");
    let observer_marker = format!("pinned-observer-fd-probe-{}", std::process::id());
    let observer_path = temporary.path().join("observer.sh");
    fs::write(
        &observer_path,
        format!("#!/bin/sh\n# {observer_marker}\nexit 0\n"),
    )
    .expect("write pinned observer fixture");
    fs::set_permissions(&observer_path, fs::Permissions::from_mode(0o700))
        .expect("chmod pinned observer fixture");
    let pinned_observer =
        PinnedExecutable::pin(&observer_path).expect("pin observer before process probes");

    let probe = format!(
        "if [ -r /dev/fd/{inherited_fd} ]; then exit 91; fi; \
         if [ \"${{HOME+x}}\" = x ]; then exit 92; fi; \
         for descriptor in /dev/fd/*; do \
           if [ -f \"$descriptor\" ] && /usr/bin/grep -q '{observer_marker}' \"$descriptor\" 2>/dev/null; then exit 93; fi; \
         done; exit 0"
    );
    let specs = [
        ProcessSpec::quiet("quiet-fd-probe", "/bin/sh", ["-c", &probe]),
        ProcessSpec::control("control-fd-probe", "/bin/sh", ["-c", &probe]),
        ProcessSpec::output("output-fd-probe", "/bin/sh", ["-c", &probe]),
    ];
    for spec in specs {
        let mut child = OwnedChild::spawn(spec).expect("spawn isolated child");
        let status = child.wait().await.expect("reap child");
        assert!(status.success(), "child inherited token fd or parent env");
    }
    assert!(fs::metadata(format!("/dev/fd/{inherited_fd}")).is_ok());
    drop(pinned_observer);
}

#[cfg(unix)]
fn write_disposable_interpreter_fixture(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, "#!/bin/sh\nobserver=$1\nshift\n. \"$observer\"\n")
        .expect("write disposable interpreter");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("chmod disposable interpreter");
}

#[cfg(unix)]
async fn run_pinned_observer_sample(
    observer: &PinnedExecutable,
    runtime_input: &str,
) -> test_sync::Result<Vec<u8>> {
    let mut child = OwnedChild::spawn_pinned(
        ProcessSpec::output(
            "disposable-interpreter-sample",
            "pinned-effect-observer",
            [runtime_input],
        ),
        observer,
    )?;
    let mut output = Vec::new();
    child
        .take_stdout()
        .expect("pinned observer stdout")
        .read_to_end(&mut output)
        .await
        .expect("read pinned observer output");
    let status = child.wait().await?;
    if !status.success() {
        return Err(test_sync::HarnessError::Process(
            "disposable interpreter sample failed",
        ));
    }
    Ok(output)
}

#[cfg(target_os = "macos")]
fn generated_controlled_observer_wrapper(root: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("client workspace root");
    let driver = workspace.join("scripts/test-sync/controlled_ssh_e2e.py");
    let wrapper = root.join("effect-observer.py");
    let generator = r#"
import pathlib
import runpy
import sys

namespace = runpy.run_path(sys.argv[1], run_name="fns_wrapper_regression")
namespace["write_wrapper"](
    pathlib.Path(sys.argv[2]),
    pathlib.Path(sys.argv[1]).resolve(),
    pathlib.Path(sys.argv[3]),
    "observe",
    None,
)
"#;
    let output = std::process::Command::new("/usr/bin/python3")
        .args([
            "-c",
            generator,
            driver.to_str().expect("ASCII driver path"),
            wrapper.to_str().expect("ASCII wrapper path"),
            root.to_str().expect("ASCII runtime path"),
        ])
        .output()
        .expect("generate controlled observer wrapper");
    assert!(
        output.status.success(),
        "wrapper generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = fs::metadata(&wrapper).expect("generated wrapper metadata");
    assert!(
        metadata.len() > 60 * 1024,
        "regression must execute the real large controlled wrapper"
    );
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    wrapper
}

#[cfg(target_os = "macos")]
fn assert_observer_help(output: &[u8]) {
    let output = std::str::from_utf8(output).expect("observer help is UTF-8");
    assert!(
        !output.is_empty(),
        "pinned Python observer returned empty stdout"
    );
    assert!(
        output.contains("usage: observer observe") && output.contains("--phase"),
        "unexpected observer help: {output}"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_large_python_observer_has_output_across_repeated_and_concurrent_spawns() {
    let temporary = tempfile::tempdir().expect("controlled observer fixtures");
    let wrapper = generated_controlled_observer_wrapper(temporary.path());
    let observer = Arc::new(PinnedExecutable::pin(&wrapper).expect("pin generated observer"));

    for _ in 0..8 {
        let output = run_pinned_observer_sample(&observer, "--help")
            .await
            .expect("sequential pinned observer");
        assert_observer_help(&output);
    }

    let mut samples = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let observer = Arc::clone(&observer);
        samples.spawn(async move { run_pinned_observer_sample(&observer, "--help").await });
    }
    while let Some(sample) = samples.join_next().await {
        let output = sample
            .expect("concurrent observer task")
            .expect("concurrent pinned observer");
        assert_observer_help(&output);
    }
}

#[cfg(unix)]
fn malicious_interpreter(marker: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nexit 0\n",
        marker.display()
    )
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_script_interpreter_is_rejected_before_runtime_input_is_read() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("interpreter replacement fixtures");
    let fixture_root = temporary
        .path()
        .canonicalize()
        .expect("canonical fixture root");
    let interpreter = fixture_root.join("interpreter");
    write_disposable_interpreter_fixture(&interpreter);
    let observer_path = fixture_root.join("observer.sh");
    fs::write(
        &observer_path,
        format!("#!{}\nprintf '%s\\n' \"$1\"\n", interpreter.display()),
    )
    .expect("write observer fixture");
    fs::set_permissions(&observer_path, fs::Permissions::from_mode(0o700))
        .expect("chmod observer fixture");
    let observer = PinnedExecutable::pin(&observer_path).expect("pin observer execution plan");
    assert_eq!(
        run_pinned_observer_sample(&observer, "first-benign-input")
            .await
            .expect("first local sample"),
        b"first-benign-input\n"
    );

    let original = fixture_root.join("interpreter.original");
    fs::rename(&interpreter, &original).expect("move original interpreter");
    let marker = fixture_root.join("runtime-input-read");
    fs::write(&interpreter, malicious_interpreter(&marker)).expect("replace interpreter path");
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700))
        .expect("chmod replacement interpreter");

    let result = run_pinned_observer_sample(&observer, "second-sensitive-shaped-input").await;
    assert!(
        result.is_err(),
        "replaced interpreter must fail before a second sample executes"
    );
    assert!(
        !marker.exists(),
        "replacement interpreter read the second sample's runtime input"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn modifying_script_interpreter_in_place_is_rejected_before_runtime_input_is_read() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("interpreter modification fixtures");
    let fixture_root = temporary
        .path()
        .canonicalize()
        .expect("canonical fixture root");
    let interpreter = fixture_root.join("interpreter");
    write_disposable_interpreter_fixture(&interpreter);
    let observer_path = fixture_root.join("observer.sh");
    fs::write(
        &observer_path,
        format!("#!{}\nprintf '%s\\n' \"$1\"\n", interpreter.display()),
    )
    .expect("write observer fixture");
    fs::set_permissions(&observer_path, fs::Permissions::from_mode(0o700))
        .expect("chmod observer fixture");
    let observer = PinnedExecutable::pin(&observer_path).expect("pin observer execution plan");
    assert_eq!(
        run_pinned_observer_sample(&observer, "first-benign-input")
            .await
            .expect("first local sample"),
        b"first-benign-input\n"
    );

    let marker = fixture_root.join("runtime-input-read");
    fs::write(&interpreter, malicious_interpreter(&marker)).expect("modify interpreter in place");
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700))
        .expect("restore interpreter mode");

    let result = run_pinned_observer_sample(&observer, "second-sensitive-shaped-input").await;
    assert!(
        result.is_err(),
        "modified interpreter must fail before a second sample executes"
    );
    assert!(
        !marker.exists(),
        "modified interpreter read the second sample's runtime input"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn termination_reaps_leader_and_empties_owned_process_group() {
    let mut child = OwnedChild::spawn(ProcessSpec::control(
        "leader-with-descendant",
        "/bin/sh",
        [
            "-c",
            "trap 'exit 0' TERM; /bin/sh -c 'trap \"\" TERM; printf \"ready\\n\"; while :; do sleep 1; done' & wait",
        ],
    ))
    .expect("spawn owned process group");
    let pgid = child.pgid();
    let stdout = child.take_stdout().expect("owned stdout");
    let mut reader = BufReader::new(stdout);
    let mut ready = String::new();
    reader.read_line(&mut ready).await.expect("read readiness");
    assert_eq!(ready, "ready\n");

    let outcome = child
        .terminate_and_reap(Duration::from_millis(50), Duration::from_secs(1))
        .await
        .expect("bounded group termination");
    assert_eq!(outcome.termination, Termination::Terminated);
    assert_eq!(outcome.group_cleanup.termination, Termination::Killed);
    assert!(outcome.group_cleanup.term_attempted);
    assert!(outcome.group_cleanup.kill_attempted);
    assert!(outcome.group_cleanup.descendants_present);
    assert!(outcome.group_cleanup.group_empty);
    let group_state = rustix::process::test_kill_process_group(pgid);

    if group_state.is_ok() {
        rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL)
            .expect("test cleanup kills leaked descendant");
        for _ in 0..100 {
            if rustix::process::test_kill_process_group(pgid) == Err(rustix::io::Errno::SRCH) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert_eq!(group_state, Err(rustix::io::Errno::SRCH));
}
