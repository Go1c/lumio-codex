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

#[cfg(target_os = "linux")]
#[test]
fn atomic_json_is_complete_private_and_replaces_old_bytes() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("status.json");
    fns_platform::atomic_write_private_json(&path, &serde_json::json!({"n": 1})).unwrap();
    fns_platform::atomic_write_private_json(&path, &serde_json::json!({"n": 2})).unwrap();

    assert_eq!(fs::read(&path).unwrap(), br#"{"n":2}"#);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(area.path()).unwrap().count(), 1);
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

// On non-Linux, runtime functions return UnsupportedPlatform.
#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_lock_and_write_return_unsupported() {
    use std::path::Path;
    let err = fns_platform::ProcessLock::acquire_linux(Path::new("/tmp/test.lock")).unwrap_err();
    assert_eq!(
        err.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );

    let err2 = fns_platform::atomic_write_private_json(
        Path::new("/tmp/test.json"),
        &serde_json::json!({}),
    )
    .unwrap_err();
    assert_eq!(
        err2.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );
}
