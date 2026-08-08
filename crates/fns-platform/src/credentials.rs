//! Secure token loading with Linux owner/mode/symlink checks.
//!
//! On non-Linux targets all functions return `UnsupportedPlatform` at runtime;
//! the symbols are still available for cross-platform compilation.

#[cfg(target_os = "linux")]
use crate::MAX_TOKEN_BYTES;
use crate::error::{PlatformError, PlatformErrorCode};

use std::fmt;
use std::path::Path;

use zeroize::Zeroizing;

/// A token loaded from a private regular file on Linux. The inner bytes are
/// zeroized on drop and never appear in `Debug` output.
pub struct SecretToken {
    bytes: Zeroizing<Vec<u8>>,
}

impl SecretToken {
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

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Io))?;

        // Strip exactly one trailing LF and one immediately preceding CR.
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }

        // Validate remaining bytes.
        if buf.is_empty() || buf.len() as u64 > MAX_TOKEN_BYTES {
            return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
        }
        if buf
            .iter()
            .any(|&b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidSecret));
        }

        Ok(Self {
            bytes: Zeroizing::new(buf),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn read_linux_file(_path: &Path) -> Result<Self, PlatformError> {
        Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
    }

    /// Expose the raw token bytes to a closure. The closure must not retain
    /// a reference to the bytes beyond its scope.
    pub fn with_exposed<R>(&self, use_secret: impl FnOnce(&[u8]) -> R) -> R {
        use_secret(&self.bytes)
    }

    /// Construct a SecretToken from raw bytes for testing only.
    /// This bypasses Linux file permission checks and must never be used in production.
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
