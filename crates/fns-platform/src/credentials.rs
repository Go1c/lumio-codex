//! Project-scoped credential storage and secure token loading.
//!
//! macOS stores generic passwords in Security.framework. Linux stores one
//! owner-only file per project using directory-relative, no-follow operations.
//! Other platforms fail closed.

use crate::MAX_TOKEN_BYTES;
use crate::error::{PlatformError, PlatformErrorCode};

use std::fmt;
use std::path::Path;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::{Mutex, MutexGuard};

use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
const DEFAULT_KEYCHAIN_SERVICE: &str = "fns-workspace";
#[cfg(target_os = "macos")]
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25_308;
#[cfg(target_os = "macos")]
static KEYCHAIN_OPERATION_LOCK: Mutex<()> = Mutex::new(());

trait CredentialBackend: Send + Sync {
    fn store(&self, account: &str, token: &SecretToken) -> Result<(), PlatformError>;
    fn load(&self, account: &str) -> Result<Option<SecretToken>, PlatformError>;
    fn delete(&self, account: &str) -> Result<(), PlatformError>;
}

/// Project-scoped credential storage.
///
/// `open` uses the macOS login Keychain on macOS and the supplied private
/// credential directory on Linux. The directory argument is ignored on macOS;
/// it exists so callers can use one cross-platform construction path.
#[derive(Clone)]
pub struct CredentialStore {
    backend: Arc<dyn CredentialBackend>,
}

impl CredentialStore {
    #[cfg(target_os = "macos")]
    pub fn open(linux_directory: &Path) -> Result<Self, PlatformError> {
        let _ = linux_directory;
        Self::open_macos_service(DEFAULT_KEYCHAIN_SERVICE)
    }

    #[cfg(target_os = "linux")]
    pub fn open(linux_directory: &Path) -> Result<Self, PlatformError> {
        Ok(Self::from_backend(FileCredentialBackend::open(
            linux_directory,
        )?))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn open(linux_directory: &Path) -> Result<Self, PlatformError> {
        let _ = linux_directory;
        Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
    }

    pub fn store(&self, project_id: &str, token: &SecretToken) -> Result<(), PlatformError> {
        let account = canonical_project_id(project_id)?;
        token.with_exposed(validate_secret_bytes)?;
        self.backend.store(&account, token)
    }

    pub fn load(&self, project_id: &str) -> Result<Option<SecretToken>, PlatformError> {
        let account = canonical_project_id(project_id)?;
        self.backend.load(&account)
    }

    pub fn delete(&self, project_id: &str) -> Result<(), PlatformError> {
        let account = canonical_project_id(project_id)?;
        self.backend.delete(&account)
    }

    #[cfg(any(target_os = "linux", target_os = "macos", test))]
    fn from_backend(backend: impl CredentialBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    #[cfg(target_os = "macos")]
    fn open_macos_service(service: &str) -> Result<Self, PlatformError> {
        if service.is_empty()
            || service.len() > 255
            || !service.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidCredentialPath));
        }
        Ok(Self::from_backend(MacKeychainBackend {
            service: service.to_owned(),
        }))
    }
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialStore")
    }
}

fn canonical_project_id(project_id: &str) -> Result<String, PlatformError> {
    let parsed = uuid::Uuid::parse_str(project_id)
        .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidProjectId))?;
    if parsed.is_nil() || parsed.to_string() != project_id {
        return Err(PlatformError::new(PlatformErrorCode::InvalidProjectId));
    }
    Ok(project_id.to_owned())
}

#[cfg(target_os = "macos")]
struct MacKeychainBackend {
    service: String,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeychainOperationStatus {
    Success,
    Missing,
    InteractionNotAllowed,
    AccessFailure,
}

#[cfg(target_os = "macos")]
fn classify_keychain_status(status: i32) -> KeychainOperationStatus {
    match status {
        security_framework_sys::base::errSecSuccess => KeychainOperationStatus::Success,
        security_framework_sys::base::errSecItemNotFound => KeychainOperationStatus::Missing,
        ERR_SEC_INTERACTION_NOT_ALLOWED => KeychainOperationStatus::InteractionNotAllowed,
        _ => KeychainOperationStatus::AccessFailure,
    }
}

#[cfg(target_os = "macos")]
fn keychain_error(status: KeychainOperationStatus) -> PlatformError {
    let code = match status {
        KeychainOperationStatus::InteractionNotAllowed => {
            PlatformErrorCode::CredentialInteractionNotAllowed
        }
        KeychainOperationStatus::Success
        | KeychainOperationStatus::Missing
        | KeychainOperationStatus::AccessFailure => PlatformErrorCode::CredentialAccess,
    };
    PlatformError::new(code)
}

#[cfg(target_os = "macos")]
fn keychain_operation_lock() -> Result<MutexGuard<'static, ()>, PlatformError> {
    KEYCHAIN_OPERATION_LOCK
        .lock()
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialAccess))
}

#[cfg(target_os = "macos")]
fn disable_keychain_interaction() -> Result<
    Option<security_framework::os::macos::keychain::KeychainUserInteractionLock>,
    PlatformError,
> {
    use security_framework::os::macos::keychain::SecKeychain;

    let interaction_allowed = SecKeychain::user_interaction_allowed()
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialAccess))?;
    if interaction_allowed {
        SecKeychain::disable_user_interaction()
            .map(Some)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialAccess))
    } else {
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
fn with_keychain_interaction_disabled<T>(
    operation: impl FnOnce() -> T,
) -> Result<T, PlatformError> {
    let _operation = keychain_operation_lock()?;
    let _interaction = disable_keychain_interaction()?;
    Ok(operation())
}

#[cfg(target_os = "macos")]
impl CredentialBackend for MacKeychainBackend {
    fn store(&self, account: &str, token: &SecretToken) -> Result<(), PlatformError> {
        let _operation = keychain_operation_lock()?;
        token.with_exposed(|bytes| {
            security_framework::passwords::set_generic_password(&self.service, account, bytes)
                .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialAccess))
        })
    }

    fn load(&self, account: &str) -> Result<Option<SecretToken>, PlatformError> {
        let result = with_keychain_interaction_disabled(|| {
            security_framework::passwords::get_generic_password(&self.service, account)
        })?;
        match result {
            Ok(bytes) => SecretToken::from_private_ipc(bytes).map(Some),
            Err(error)
                if classify_keychain_status(error.code()) == KeychainOperationStatus::Missing =>
            {
                Ok(None)
            }
            Err(error) => Err(keychain_error(classify_keychain_status(error.code()))),
        }
    }

    fn delete(&self, account: &str) -> Result<(), PlatformError> {
        let result = with_keychain_interaction_disabled(|| {
            security_framework::passwords::delete_generic_password(&self.service, account)
        })?;
        match result {
            Ok(()) => Ok(()),
            Err(error)
                if classify_keychain_status(error.code()) == KeychainOperationStatus::Missing =>
            {
                Ok(())
            }
            Err(error) => Err(keychain_error(classify_keychain_status(error.code()))),
        }
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
struct FileCredentialBackend {
    directory: std::fs::File,
}

#[cfg(all(unix, any(target_os = "linux", test)))]
impl FileCredentialBackend {
    fn open(path: &Path) -> Result<Self, PlatformError> {
        use rustix::fs::{FileType, Mode, OFlags};

        validate_credential_directory_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidCredentialPath))?
            .canonicalize()
            .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidCredentialPath))?;
        let name = path
            .file_name()
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidCredentialPath))?;
        let parent_fd = rustix::fs::open(
            &parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_open_error)?;

        let created = match stat_entry(&parent_fd, name)? {
            Some(stat) => {
                if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                    return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
                }
                false
            }
            None => {
                rustix::fs::mkdirat(&parent_fd, name, Mode::RWXU)
                    .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
                true
            }
        };

        let directory_fd = rustix::fs::openat(
            &parent_fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_open_error)?;
        let directory = std::fs::File::from(directory_fd);
        if created {
            rustix::fs::fchmod(&directory, Mode::RWXU)
                .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
            rustix::fs::fsync(&parent_fd).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        }
        validate_open_node(&directory, FileType::Directory, 0o700)?;
        rustix::fs::fsync(&directory).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        Ok(Self { directory })
    }

    fn credential_name(account: &str) -> String {
        format!("{account}.token")
    }

    fn validate_existing_file(&self, name: &str) -> Result<bool, PlatformError> {
        use rustix::fs::FileType;

        let Some(stat) = stat_entry(&self.directory, std::ffi::OsStr::new(name))? else {
            return Ok(false);
        };
        validate_stat(&stat, FileType::RegularFile, 0o600)?;
        Ok(true)
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
impl CredentialBackend for FileCredentialBackend {
    fn store(&self, account: &str, token: &SecretToken) -> Result<(), PlatformError> {
        use rustix::fs::{Mode, OFlags};
        use std::io::Write;

        let destination = Self::credential_name(account);
        self.validate_existing_file(&destination)?;
        let temporary = format!(".{account}-{}.tmp", uuid::Uuid::new_v4());
        let temporary_fd = rustix::fs::openat(
            &self.directory,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(map_open_error)?;
        let mut cleanup = TemporaryCredential::new(&self.directory, temporary.clone());
        let mut file = std::fs::File::from(temporary_fd);
        rustix::fs::fchmod(&file, Mode::RUSR | Mode::WUSR)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        validate_open_node(&file, rustix::fs::FileType::RegularFile, 0o600)?;
        token
            .with_exposed(|bytes| file.write_all(bytes))
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        file.sync_all()
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        drop(file);

        rustix::fs::renameat(
            &self.directory,
            temporary.as_str(),
            &self.directory,
            destination.as_str(),
        )
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        cleanup.disarm();
        rustix::fs::fsync(&self.directory)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        self.validate_existing_file(&destination)?;
        Ok(())
    }

    fn load(&self, account: &str) -> Result<Option<SecretToken>, PlatformError> {
        use rustix::fs::{FileType, Mode, OFlags};
        use std::io::Read;

        let name = Self::credential_name(account);
        if !self.validate_existing_file(&name)? {
            return Ok(None);
        }
        let fd = rustix::fs::openat(
            &self.directory,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_open_error)?;
        let mut file = std::fs::File::from(fd);
        validate_open_node(&file, FileType::RegularFile, 0o600)?;
        let mut bytes = Zeroizing::new(Vec::new());
        (&mut file)
            .take(crate::MAX_TOKEN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        SecretToken::from_zeroizing(bytes).map(Some)
    }

    fn delete(&self, account: &str) -> Result<(), PlatformError> {
        let name = Self::credential_name(account);
        if !self.validate_existing_file(&name)? {
            return Ok(());
        }
        match rustix::fs::unlinkat(&self.directory, name.as_str(), rustix::fs::AtFlags::empty()) {
            Ok(()) => {
                rustix::fs::fsync(&self.directory)
                    .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
                Ok(())
            }
            Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
            Err(_) => Err(PlatformError::new(PlatformErrorCode::Io)),
        }
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
struct TemporaryCredential<'a> {
    directory: &'a std::fs::File,
    name: String,
    armed: bool,
}

#[cfg(all(unix, any(target_os = "linux", test)))]
impl<'a> TemporaryCredential<'a> {
    fn new(directory: &'a std::fs::File, name: String) -> Self {
        Self {
            directory,
            name,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
impl Drop for TemporaryCredential<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = rustix::fs::unlinkat(
                self.directory,
                self.name.as_str(),
                rustix::fs::AtFlags::empty(),
            );
        }
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
fn validate_credential_directory_path(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidCredentialPath));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", test)))]
fn stat_entry<Fd: rustix::fd::AsFd>(
    directory: Fd,
    name: &std::ffi::OsStr,
) -> Result<Option<rustix::fs::Stat>, PlatformError> {
    match rustix::fs::statat(directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(None),
        Err(_) => Err(PlatformError::new(PlatformErrorCode::Io)),
    }
}

#[cfg(all(unix, any(target_os = "linux", test)))]
fn validate_open_node(
    file: &std::fs::File,
    expected_type: rustix::fs::FileType,
    expected_mode: u32,
) -> Result<(), PlatformError> {
    let stat = rustix::fs::fstat(file).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    validate_stat(&stat, expected_type, expected_mode)
}

#[cfg(all(unix, any(target_os = "linux", test)))]
fn validate_stat(
    stat: &rustix::fs::Stat,
    expected_type: rustix::fs::FileType,
    expected_mode: u32,
) -> Result<(), PlatformError> {
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != expected_type {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }
    if u64::from(stat.st_uid) != u64::from(rustix::process::geteuid().as_raw()) {
        return Err(PlatformError::new(PlatformErrorCode::WrongOwner));
    }
    if u64::from(stat.st_mode) & 0o777 != u64::from(expected_mode) {
        return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", test)))]
fn map_open_error(error: rustix::io::Errno) -> PlatformError {
    if matches!(
        error,
        rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR | rustix::io::Errno::ISDIR
    ) {
        PlatformError::new(PlatformErrorCode::InvalidFileType)
    } else {
        PlatformError::new(PlatformErrorCode::Io)
    }
}

/// A validated bearer token held in zeroizing memory.
///
/// Token bytes never appear in `Debug` output. Platform stores and private IPC
/// constructors apply the same content and size validation.
pub struct SecretToken {
    bytes: Zeroizing<Vec<u8>>,
}

impl SecretToken {
    /// Construct a token from bytes received over an already-private IPC channel.
    ///
    /// This applies the same content and size validation as file loading while
    /// taking ownership of the buffer so the bytes are zeroized on drop.
    pub fn from_private_ipc(bytes: Vec<u8>) -> Result<Self, PlatformError> {
        Self::from_zeroizing(Zeroizing::new(bytes))
    }

    fn from_zeroizing(bytes: Zeroizing<Vec<u8>>) -> Result<Self, PlatformError> {
        validate_secret_bytes(&bytes)?;
        Ok(Self { bytes })
    }

    /// Read a token from a Linux file that is a regular (non-symlink) file,
    /// owned by the current effective UID, with mode `0600` (no group/other bits).
    ///
    /// Exactly one trailing LF and an immediately preceding CR are stripped.
    /// The remaining bytes must be 1..=8192 with no ASCII whitespace or control bytes.
    #[cfg(target_os = "linux")]
    pub fn read_linux_file(path: &Path) -> Result<Self, PlatformError> {
        use std::fs;
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let uid = fs::metadata("/proc/self")
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?
            .uid();

        // Reject symlinks and non-regular files.
        let meta =
            fs::symlink_metadata(path).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        if meta.file_type().is_symlink() {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        if !meta.file_type().is_file() {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        if meta.uid() != uid {
            return Err(PlatformError::new(PlatformErrorCode::WrongOwner));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
        }
        if meta.len() > MAX_TOKEN_BYTES {
            return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
        }

        // Open after metadata checks.
        let mut file =
            fs::File::open(path).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

        // Re-check metadata after opening to detect replacement during read.
        let meta_after = file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
        if !meta_after.file_type().is_file() {
            return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
        }
        if meta_after.uid() != uid {
            return Err(PlatformError::new(PlatformErrorCode::WrongOwner));
        }
        if meta_after.permissions().mode() & 0o077 != 0 {
            return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
        }

        let mut buf = Zeroizing::new(Vec::new());
        file.read_to_end(&mut buf)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

        // Strip exactly one trailing LF and one immediately preceding CR.
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }

        Self::from_zeroizing(buf)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn read_linux_file(_path: &Path) -> Result<Self, PlatformError> {
        Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
    }

    /// Wrap a token that arrived from a store the OS already protects, such as
    /// the macOS keychain.
    ///
    /// [`Self::read_linux_file`] exists because a token on disk has to prove it
    /// is private; a keychain item has no file to inspect, so only the content
    /// rules are applied here. Callers on Linux that read from a file must keep
    /// using `read_linux_file`.
    pub fn from_protected_store(bytes: &[u8]) -> Result<Self, PlatformError> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_TOKEN_BYTES {
            return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
        }
        if bytes
            .iter()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
        }
        Ok(Self {
            bytes: Zeroizing::new(bytes.to_vec()),
        })
    }

    /// Expose the raw token bytes to a closure. The closure must not retain
    /// a reference to the bytes beyond its scope.
    pub fn with_exposed<R>(&self, use_secret: impl FnOnce(&[u8]) -> R) -> R {
        use_secret(&self.bytes)
    }

    /// Construct a SecretToken from raw bytes for testing only.
    /// This bypasses Linux file permission checks and must never be used in production.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn from_bytes_for_test(bytes: &[u8]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes.to_vec()),
        }
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretToken([REDACTED])")
    }
}

fn validate_secret_bytes(bytes: &[u8]) -> Result<(), PlatformError> {
    if bytes.is_empty()
        || bytes.len() as u64 > crate::MAX_TOKEN_BYTES
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
    }
    Ok(())
}

/// Verify that a path is a private regular file owned by the current UID on Linux.
/// This is used for config validation without loading token bytes.
#[cfg(target_os = "linux")]
pub fn verify_private_regular_linux(path: &Path) -> Result<(), PlatformError> {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let uid = fs::metadata("/proc/self")
        .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?
        .uid();

    let meta = fs::symlink_metadata(path).map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;
    if meta.file_type().is_symlink() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidFileType));
    }
    if meta.uid() != uid {
        return Err(PlatformError::new(PlatformErrorCode::WrongOwner));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(PlatformError::new(PlatformErrorCode::InsecurePermissions));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn verify_private_regular_linux(_path: &Path) -> Result<(), PlatformError> {
    Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT_ID: &str = "10000000-0000-4000-8000-000000000001";

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_status_classification_distinguishes_missing_interaction_and_access() {
        assert_eq!(
            classify_keychain_status(security_framework_sys::base::errSecSuccess),
            KeychainOperationStatus::Success
        );
        assert_eq!(
            classify_keychain_status(security_framework_sys::base::errSecItemNotFound),
            KeychainOperationStatus::Missing
        );
        assert_eq!(
            classify_keychain_status(ERR_SEC_INTERACTION_NOT_ALLOWED),
            KeychainOperationStatus::InteractionNotAllowed
        );
        assert_eq!(
            classify_keychain_status(security_framework_sys::base::errSecAuthFailed),
            KeychainOperationStatus::AccessFailure
        );
        assert_eq!(
            keychain_error(KeychainOperationStatus::InteractionNotAllowed).code(),
            PlatformErrorCode::CredentialInteractionNotAllowed
        );
        assert_eq!(
            keychain_error(KeychainOperationStatus::AccessFailure).code(),
            PlatformErrorCode::CredentialAccess
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_background_scope_disables_and_restores_actual_system_interaction_state() {
        use security_framework::os::macos::keychain::SecKeychain;

        let initial = SecKeychain::user_interaction_allowed().unwrap();
        let observed = with_keychain_interaction_disabled(SecKeychain::user_interaction_allowed)
            .unwrap()
            .unwrap();
        assert!(!observed);
        assert_eq!(SecKeychain::user_interaction_allowed().unwrap(), initial);

        if initial {
            let externally_disabled = SecKeychain::disable_user_interaction().unwrap();
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            let observed =
                with_keychain_interaction_disabled(SecKeychain::user_interaction_allowed)
                    .unwrap()
                    .unwrap();
            assert!(!observed);
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            drop(externally_disabled);
            assert!(SecKeychain::user_interaction_allowed().unwrap());
        }
    }

    #[derive(Default)]
    struct MemoryBackend {
        value: std::sync::Mutex<Option<(String, Zeroizing<Vec<u8>>)>>,
    }

    impl CredentialBackend for MemoryBackend {
        fn store(&self, account: &str, token: &SecretToken) -> Result<(), PlatformError> {
            let bytes = token.with_exposed(|bytes| Zeroizing::new(bytes.to_vec()));
            *self.value.lock().unwrap() = Some((account.to_owned(), bytes));
            Ok(())
        }

        fn load(&self, account: &str) -> Result<Option<SecretToken>, PlatformError> {
            let value = self.value.lock().unwrap();
            let Some((stored_account, bytes)) = value.as_ref() else {
                return Ok(None);
            };
            if stored_account != account {
                return Ok(None);
            }
            SecretToken::from_private_ipc(bytes.to_vec()).map(Some)
        }

        fn delete(&self, account: &str) -> Result<(), PlatformError> {
            let mut value = self.value.lock().unwrap();
            if value
                .as_ref()
                .is_some_and(|(stored_account, _)| stored_account == account)
            {
                *value = None;
            }
            Ok(())
        }
    }

    #[test]
    fn injected_backend_roundtrips_and_public_types_redact() {
        let store = CredentialStore::from_backend(MemoryBackend::default());
        let token = SecretToken::from_private_ipc(b"SENTINEL.JWT".to_vec()).unwrap();

        store.store(PROJECT_ID, &token).unwrap();
        let loaded = store.load(PROJECT_ID).unwrap().unwrap();
        assert_eq!(loaded.with_exposed(|bytes| bytes.to_vec()), b"SENTINEL.JWT");
        assert_eq!(format!("{store:?}"), "CredentialStore");
        assert!(!format!("{token:?} {loaded:?}").contains("SENTINEL"));

        store.delete(PROJECT_ID).unwrap();
        store.delete(PROJECT_ID).unwrap();
        assert!(store.load(PROJECT_ID).unwrap().is_none());
    }

    #[test]
    fn invalid_project_id_never_reaches_injected_backend() {
        let store = CredentialStore::from_backend(MemoryBackend::default());
        let token = SecretToken::from_private_ipc(b"secret".to_vec()).unwrap();
        let error = store.store("../secret", &token).unwrap_err();
        assert_eq!(error.code(), PlatformErrorCode::InvalidProjectId);
        assert_eq!(
            format!("{error:?}"),
            "PlatformError { code: InvalidProjectId }"
        );
        assert!(!format!("{error:?} {error}").contains("../secret"));

        let invalid = SecretToken::from_bytes_for_test(b"invalid secret");
        let error = store.store(PROJECT_ID, &invalid).unwrap_err();
        assert_eq!(error.code(), PlatformErrorCode::InvalidSecret);
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_backend_is_private_atomic_and_idempotent() {
        use std::os::unix::fs::PermissionsExt;

        let area = tempfile::tempdir().unwrap();
        let root = area.path().canonicalize().unwrap().join("credentials");
        let store = CredentialStore::from_backend(FileCredentialBackend::open(&root).unwrap());
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let first = SecretToken::from_private_ipc(b"first-long-value".to_vec()).unwrap();
        store.store(PROJECT_ID, &first).unwrap();
        let path = root.join(format!("{PROJECT_ID}.token"));
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

        let second = SecretToken::from_private_ipc(b"two".to_vec()).unwrap();
        store.store(PROJECT_ID, &second).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(
            store
                .load(PROJECT_ID)
                .unwrap()
                .unwrap()
                .with_exposed(|bytes| bytes.to_vec()),
            b"two"
        );

        store.delete(PROJECT_ID).unwrap();
        store.delete(PROJECT_ID).unwrap();
        assert!(store.load(PROJECT_ID).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_backend_rejects_paths_modes_symlinks_and_invalid_secret() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let relative = FileCredentialBackend::open(Path::new("relative"))
            .err()
            .unwrap();
        assert_eq!(relative.code(), PlatformErrorCode::InvalidCredentialPath);

        let area = tempfile::tempdir().unwrap();
        let canonical_area = area.path().canonicalize().unwrap();
        let insecure = canonical_area.join("insecure");
        std::fs::create_dir(&insecure).unwrap();
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = FileCredentialBackend::open(&insecure).err().unwrap();
        assert_eq!(error.code(), PlatformErrorCode::InsecurePermissions);

        let real_root = canonical_area.join("real-root");
        std::fs::create_dir(&real_root).unwrap();
        std::fs::set_permissions(&real_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let linked_root = canonical_area.join("linked-root");
        symlink(&real_root, &linked_root).unwrap();
        let error = FileCredentialBackend::open(&linked_root).err().unwrap();
        assert_eq!(error.code(), PlatformErrorCode::InvalidFileType);

        let store = CredentialStore::from_backend(FileCredentialBackend::open(&real_root).unwrap());
        let target = canonical_area.join("outside");
        std::fs::write(&target, b"outside").unwrap();
        let credential = real_root.join(format!("{PROJECT_ID}.token"));
        symlink(&target, &credential).unwrap();
        let error = store.load(PROJECT_ID).unwrap_err();
        assert_eq!(error.code(), PlatformErrorCode::InvalidFileType);
        let token = SecretToken::from_private_ipc(b"secret".to_vec()).unwrap();
        let error = store.store(PROJECT_ID, &token).unwrap_err();
        assert_eq!(error.code(), PlatformErrorCode::InvalidFileType);
        assert_eq!(std::fs::read(&target).unwrap(), b"outside");

        std::fs::remove_file(&credential).unwrap();
        std::fs::write(&credential, b"invalid secret").unwrap();
        std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = store.load(PROJECT_ID).unwrap_err();
        assert_eq!(error.code(), PlatformErrorCode::InvalidSecret);
        assert!(!format!("{error:?} {error}").contains("invalid secret"));
    }
}
