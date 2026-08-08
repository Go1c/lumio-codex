//! Platform primitives for fns-agent: secure credential loading, process locking,
//! and atomic private JSON writes. Runtime credential/lock enforcement is Linux-only;
//! non-Linux targets compile the same symbols but return `UnsupportedPlatform`.

mod credentials;
mod error;
mod process;

pub use credentials::{SecretToken, verify_private_regular_linux};
pub use error::{PlatformError, PlatformErrorCode};
pub use process::{ProcessLock, ProcessLockRecord, atomic_write_private_json};

/// Maximum token file size in bytes.
pub const MAX_TOKEN_BYTES: u64 = 8_192;
