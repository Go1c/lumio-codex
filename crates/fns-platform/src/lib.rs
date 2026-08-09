//! Platform primitives for fns-agent: secure credential storage/loading, process
//! locking, and atomic private JSON writes. Credentials use the macOS Keychain or
//! owner-only Linux files; atomic private JSON writes are supported on Unix targets.

mod credentials;
mod error;
mod process;

pub use credentials::{CredentialStore, SecretToken, verify_private_regular_linux};
pub use error::{PlatformError, PlatformErrorCode};
pub use process::{ProcessLock, ProcessLockRecord, StateDirLease, atomic_write_private_json};

/// Maximum token file size in bytes.
pub const MAX_TOKEN_BYTES: u64 = 8_192;
