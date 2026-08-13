//! Shared test support for fns-transport integration tests.

pub mod fake_server;

use fns_platform::SecretToken;

/// Create a SecretToken from raw bytes for testing (bypasses Linux file checks).
pub fn secret_token(value: &str) -> SecretToken {
    SecretToken::from_bytes_for_test(value.as_bytes())
}
