#[cfg(target_os = "linux")]
#[test]
fn token_requires_private_owner_file_and_never_formats_secret() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("token");
    fs::write(&path, b"sentinel.jwt.value\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    let token = fns_platform::SecretToken::read_linux_file(&path).unwrap();
    assert_eq!(
        token.with_exposed(|bytes| bytes.to_vec()),
        b"sentinel.jwt.value"
    );
    assert_eq!(format!("{token:?}"), "SecretToken([REDACTED])");
    assert!(!format!("{token:?}").contains("sentinel"));

    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        fns_platform::SecretToken::read_linux_file(&path)
            .unwrap_err()
            .code(),
        fns_platform::PlatformErrorCode::InsecurePermissions
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn token_rejects_symlink_whitespace_empty_and_oversize() {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    let area = tempfile::tempdir().unwrap();
    let real = area.path().join("real");
    fs::write(&real, b"abc").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    let link = area.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(fns_platform::SecretToken::read_linux_file(&link).is_err());

    for bytes in [Vec::new(), b"has space".to_vec(), vec![b'x'; 8_193]] {
        fs::write(&real, bytes).unwrap();
        assert!(fns_platform::SecretToken::read_linux_file(&real).is_err());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn token_strips_trailing_crlf_and_rejects_control_bytes() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let area = tempfile::tempdir().unwrap();
    let path = area.path().join("token");
    fs::write(&path, b"abc\r\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let token = fns_platform::SecretToken::read_linux_file(&path).unwrap();
    assert_eq!(token.with_exposed(|b| b.to_vec()), b"abc");

    // NUL byte rejected
    fs::write(&path, b"ab\x00cd").unwrap();
    assert!(fns_platform::SecretToken::read_linux_file(&path).is_err());

    // Tab rejected
    fs::write(&path, b"ab\tcd").unwrap();
    assert!(fns_platform::SecretToken::read_linux_file(&path).is_err());
}

// On non-Linux (macOS/Windows) the functions exist but return UnsupportedPlatform.
#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_returns_unsupported_platform() {
    use std::path::Path;
    let err = fns_platform::SecretToken::read_linux_file(Path::new("/dev/null")).unwrap_err();
    assert_eq!(
        err.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );

    let err2 = fns_platform::verify_private_regular_linux(Path::new("/dev/null")).unwrap_err();
    assert_eq!(
        err2.code(),
        fns_platform::PlatformErrorCode::UnsupportedPlatform
    );
}

#[cfg(target_os = "linux")]
const PROJECT_ID: &str = "10000000-0000-4000-8000-000000000001";

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn credential_store_rejects_noncanonical_project_ids_without_leaking_values() {
    let area = tempfile::tempdir().unwrap();
    let store = fns_platform::CredentialStore::open(&area.path().join("credentials")).unwrap();
    let token =
        fns_platform::SecretToken::from_private_ipc(b"SENTINEL.JWT.VALUE".to_vec()).unwrap();

    for invalid in [
        "",
        "../escape",
        "10000000000040008000000000000001",
        "10000000-0000-4000-8000-00000000000A",
        "00000000-0000-0000-0000-000000000000",
    ] {
        let error = store.store(invalid, &token).unwrap_err();
        assert_eq!(
            error.code(),
            fns_platform::PlatformErrorCode::InvalidProjectId
        );
        let rendered = format!("{error:?} {error}");
        if !invalid.is_empty() {
            assert!(!rendered.contains(invalid));
        }
        assert!(!rendered.contains("SENTINEL"));
    }

    let rendered = format!("{store:?} {token:?}");
    assert!(!rendered.contains("SENTINEL"));
    assert!(!rendered.contains(area.path().to_string_lossy().as_ref()));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_credential_store_roundtrips_atomically_with_owner_only_modes() {
    use std::os::unix::fs::PermissionsExt;

    let area = tempfile::tempdir().unwrap();
    let root = area.path().join("credentials");
    let store = fns_platform::CredentialStore::open(&root).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let first = fns_platform::SecretToken::from_private_ipc(b"first-long-token".to_vec()).unwrap();
    store.store(PROJECT_ID, &first).unwrap();
    let path = root.join(format!("{PROJECT_ID}.token"));
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

    let second = fns_platform::SecretToken::from_private_ipc(b"two".to_vec()).unwrap();
    store.store(PROJECT_ID, &second).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    let loaded = store.load(PROJECT_ID).unwrap().unwrap();
    assert_eq!(loaded.with_exposed(|bytes| bytes.to_vec()), b"two");

    store.delete(PROJECT_ID).unwrap();
    store.delete(PROJECT_ID).unwrap();
    assert!(store.load(PROJECT_ID).unwrap().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_credential_store_rejects_symlinks_and_insecure_directory() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let area = tempfile::tempdir().unwrap();
    let root = area.path().join("credentials");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = fns_platform::CredentialStore::open(&root).unwrap_err();
    assert_eq!(
        error.code(),
        fns_platform::PlatformErrorCode::InsecurePermissions
    );

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = area.path().join("outside");
    std::fs::write(&target, b"outside").unwrap();
    symlink(&target, root.join(format!("{PROJECT_ID}.token"))).unwrap();
    let store = fns_platform::CredentialStore::open(&root).unwrap();
    let error = store.load(PROJECT_ID).unwrap_err();
    assert_eq!(
        error.code(),
        fns_platform::PlatformErrorCode::InvalidFileType
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"outside");
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "modifies the user's login Keychain; run explicitly for live verification"]
fn macos_security_framework_keychain_roundtrip_cleans_up() {
    #[derive(Clone)]
    struct Cleanup {
        store: fns_platform::CredentialStore,
        project_id: String,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.store.delete(&self.project_id);
        }
    }

    let project_id = uuid::Uuid::new_v4().to_string();
    let area = tempfile::tempdir().unwrap();
    let store = fns_platform::CredentialStore::open(&area.path().join("unused-on-macos")).unwrap();
    let _cleanup = Cleanup {
        store: store.clone(),
        project_id: project_id.clone(),
    };
    store.delete(&project_id).unwrap();

    let token =
        fns_platform::SecretToken::from_private_ipc(b"live-keychain-token".to_vec()).unwrap();
    store.store(&project_id, &token).unwrap();
    let loaded = store.load(&project_id).unwrap().unwrap();
    assert_eq!(
        loaded.with_exposed(|bytes| bytes.to_vec()),
        b"live-keychain-token"
    );
    assert_eq!(format!("{store:?}"), "CredentialStore");

    store.delete(&project_id).unwrap();
    assert!(store.load(&project_id).unwrap().is_none());
}

#[test]
fn credential_module_has_no_process_or_environment_secret_transport() {
    let source = include_str!("../src/credentials.rs");
    assert!(source.contains("security_framework::passwords"));
    for forbidden in [
        "Command::new",
        "std::process::Command",
        ".arg(",
        ".args(",
        "std::env::set_var",
    ] {
        assert!(
            !source.contains(forbidden),
            "credential module contains forbidden transport: {forbidden}"
        );
    }
}

#[test]
fn automatic_keychain_load_and_delete_explicitly_disable_authentication_ui() {
    let source = include_str!("../src/credentials.rs");
    assert!(source.contains("KEYCHAIN_OPERATION_LOCK"));
    assert!(source.contains("SecKeychain::disable_user_interaction()"));
    assert!(source.contains("with_keychain_interaction_disabled(||"));
}

#[test]
fn raw_test_token_constructor_is_absent_from_release_builds() {
    let source = include_str!("../src/credentials.rs");
    assert!(
        source.contains(
            "#[cfg(debug_assertions)]\n    #[doc(hidden)]\n    pub fn from_bytes_for_test"
        )
    );
}
