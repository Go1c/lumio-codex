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

#[test]
fn a_keychain_token_is_content_checked_but_needs_no_file_permissions() {
    // The macOS desktop holds its agent token in the system keychain, where
    // there is no file mode to inspect — only the token's own shape.
    let token = fns_platform::SecretToken::from_protected_store(b"sentinel.jwt.value")
        .expect("well formed token");
    assert_eq!(
        token.with_exposed(|bytes| bytes.to_vec()),
        b"sentinel.jwt.value"
    );
    assert_eq!(format!("{token:?}"), "SecretToken([REDACTED])");

    for rejected in [
        b"".as_slice(),
        b"has space".as_slice(),
        b"has\ttab".as_slice(),
        b"trailing\n".as_slice(),
    ] {
        assert!(
            matches!(
                fns_platform::SecretToken::from_protected_store(rejected)
                    .unwrap_err()
                    .code(),
                fns_platform::PlatformErrorCode::InvalidSecret
            ),
            "{rejected:?} should not be accepted as a token"
        );
    }

    let oversize = vec![b'a'; fns_platform::MAX_TOKEN_BYTES as usize + 1];
    assert!(fns_platform::SecretToken::from_protected_store(&oversize).is_err());
}
