use std::fmt;

/// Stable platform error codes. Display text is derived solely from this code —
/// never from paths, tokens, JSON, PID-file contents, or OS error strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformErrorCode {
    UnsupportedPlatform,
    InvalidFileType,
    InsecurePermissions,
    WrongOwner,
    InvalidSecret,
    AlreadyRunning,
    CorruptLock,
    Io,
}

/// A platform error carrying only a stable code. No sensitive data is retained.
pub struct PlatformError {
    code: PlatformErrorCode,
}

impl fmt::Debug for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformError")
            .field("code", &self.code)
            .finish()
    }
}

impl PlatformError {
    pub(crate) const fn new(code: PlatformErrorCode) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> PlatformErrorCode {
        self.code
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Only the code variant name is printed — never paths or OS strings.
        write!(f, "{:?}", self.code)
    }
}

impl std::error::Error for PlatformError {}
