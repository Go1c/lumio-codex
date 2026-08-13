#[cfg(target_os = "linux")]
#[test]
fn live_lock_is_exclusive_and_stale_lock_is_recovered() {
    use fns_platform::{ProcessLock, ProcessLockRecord, atomic_write_private_json};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("agent.lock");
    let held = ProcessLock::acquire_linux(&path).unwrap();
    assert!(ProcessLock::acquire_linux(&path).is_err());
    drop(held);
    assert!(ProcessLock::probe_linux(&path).unwrap().is_none());

    atomic_write_private_json(
        &path,
        &ProcessLockRecord {
            pid: u32::MAX,
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
            start_ticks: 1,
            nonce: uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000099").unwrap(),
        },
    )
    .unwrap();
    let recovered = ProcessLock::acquire_linux(&path).unwrap();
    assert!(ProcessLock::probe_linux(&path).unwrap().is_some());
    drop(recovered);
    assert!(ProcessLock::probe_linux(&path).unwrap().is_none());
}

#[cfg(target_family = "unix")]
#[test]
fn unix_atomic_json_is_complete_private_replaces_old_bytes_and_leaves_no_temp() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("status.json");
    fns_platform::atomic_write_private_json(
        &path,
        &serde_json::json!({"payload": "a deliberately longer first body"}),
    )
    .unwrap();
    fns_platform::atomic_write_private_json(&path, &serde_json::json!({"n": 2})).unwrap();

    assert_eq!(fs::read(&path).unwrap(), br#"{"n":2}"#);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(area.path()).unwrap().count(), 1);
}

#[cfg(target_family = "unix")]
#[test]
fn unix_atomic_json_reports_parent_directory_open_failure() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("status.json");
    fs::set_permissions(area.path(), fs::Permissions::from_mode(0o300)).unwrap();

    let result = fns_platform::atomic_write_private_json(&path, &serde_json::json!({"n": 1}));

    fs::set_permissions(area.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.expect_err("parent directory open failure was silently ignored");
    assert_eq!(error.code(), fns_platform::PlatformErrorCode::Io);
    assert_eq!(fs::read(&path).unwrap(), br#"{"n":1}"#);
}

#[test]
fn private_ipc_token_validates_and_redacts() {
    let token = fns_platform::SecretToken::from_private_ipc(b"private-token".to_vec()).unwrap();
    assert_eq!(format!("{token:?}"), "SecretToken([REDACTED])");
    assert_eq!(token.with_exposed(|bytes| bytes.to_vec()), b"private-token");
    assert!(fns_platform::SecretToken::from_private_ipc(Vec::new()).is_err());
    assert!(fns_platform::SecretToken::from_private_ipc(b"bad token".to_vec()).is_err());
}

#[test]
fn state_directory_lease_is_exclusive_owner_only_and_reacquirable() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    assert!(!fns_platform::StateDirLease::probe(&state_dir).unwrap());
    let lease = fns_platform::StateDirLease::acquire(&state_dir).unwrap();
    assert!(fns_platform::StateDirLease::probe(&state_dir).unwrap());
    assert!(fns_platform::StateDirLease::acquire(&state_dir).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            std::fs::metadata(state_dir.join("agent.lease"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        let anchor = dir.path().join(".state.fns-agent-lease");
        assert!(std::fs::metadata(&anchor).unwrap().is_dir());
        assert_eq!(
            std::fs::metadata(anchor).unwrap().permissions().mode() & 0o077,
            0
        );
    }

    drop(lease);
    assert!(!fns_platform::StateDirLease::probe(&state_dir).unwrap());
    let _reacquired = fns_platform::StateDirLease::acquire(&state_dir).unwrap();
}

#[cfg(unix)]
#[test]
fn state_directory_lease_rejects_symlinked_lease_file() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    let target = dir.path().join("attacker-controlled");
    std::fs::write(&target, b"not a lease").unwrap();
    symlink(&target, state_dir.join("agent.lease")).unwrap();

    let acquire_error = fns_platform::StateDirLease::acquire(&state_dir).unwrap_err();
    assert_eq!(
        acquire_error.code(),
        fns_platform::PlatformErrorCode::InvalidFileType
    );
    let probe_error = fns_platform::StateDirLease::probe(&state_dir).unwrap_err();
    assert_eq!(
        probe_error.code(),
        fns_platform::PlatformErrorCode::InvalidFileType
    );
}

#[cfg(unix)]
#[test]
fn held_state_directory_lease_survives_lease_file_unlink_and_recreate() {
    for iteration in 0..32 {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join(format!("state-{iteration}"));
        let held = fns_platform::StateDirLease::acquire(&state_dir).unwrap();
        let lease_path = state_dir.join("agent.lease");

        std::fs::remove_file(&lease_path).unwrap();
        std::fs::write(&lease_path, b"replacement").unwrap();

        assert_lease_remains_held(&state_dir);
        drop(held);
    }
}

#[cfg(unix)]
#[test]
fn held_state_directory_lease_survives_lease_file_rename_and_recreate() {
    for iteration in 0..32 {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join(format!("state-{iteration}"));
        let held = fns_platform::StateDirLease::acquire(&state_dir).unwrap();
        let lease_path = state_dir.join("agent.lease");

        std::fs::rename(&lease_path, state_dir.join("displaced-agent.lease")).unwrap();
        std::fs::write(&lease_path, b"replacement").unwrap();

        assert_lease_remains_held(&state_dir);
        drop(held);
    }
}

#[cfg(unix)]
#[test]
fn held_state_directory_lease_survives_state_directory_rename_and_recreate() {
    for iteration in 0..32 {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join(format!("state-{iteration}"));
        let held = fns_platform::StateDirLease::acquire(&state_dir).unwrap();

        std::fs::rename(
            &state_dir,
            dir.path().join(format!("displaced-{iteration}")),
        )
        .unwrap();
        std::fs::create_dir(&state_dir).unwrap();

        assert_lease_remains_held(&state_dir);
        drop(held);
    }
}

#[cfg(unix)]
fn assert_lease_remains_held(state_dir: &std::path::Path) {
    assert!(fns_platform::StateDirLease::probe(state_dir).unwrap());
    let error = fns_platform::StateDirLease::acquire(state_dir).unwrap_err();
    assert_eq!(
        error.code(),
        fns_platform::PlatformErrorCode::AlreadyRunning
    );
}

#[cfg(target_os = "linux")]
#[test]
fn lock_drop_only_removes_own_nonce() {
    use fns_platform::ProcessLock;

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("agent.lock");
    let lock = ProcessLock::acquire_linux(&path).unwrap();
    drop(lock);
    // After drop the lock file should be gone (or at least probe returns None).
    assert!(ProcessLock::probe_linux(&path).unwrap().is_none());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_lock_returns_unsupported() {
    use std::path::Path;
    let err = fns_platform::ProcessLock::acquire_linux(Path::new("/tmp/test.lock")).unwrap_err();
    assert_eq!(
        err.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );
}

#[cfg(not(target_family = "unix"))]
#[test]
fn non_unix_atomic_write_returns_unsupported() {
    use std::path::Path;
    let error = fns_platform::atomic_write_private_json(
        Path::new("/tmp/test.json"),
        &serde_json::json!({}),
    )
    .unwrap_err();
    assert_eq!(
        error.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );
}
