//! Transport error types with stable codes and retry classification.
//!
//! `TransportError` stores only a code and a retryable flag — never the endpoint,
//! HTTP response body, tungstenite error, header value, OS error, control bytes,
//! or binary payload.

use std::fmt;

/// Stable transport error codes serialized as snake_case.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportErrorCode {
    InvalidConfiguration,
    AuthenticationRejected,
    Forbidden,
    Network,
    Protocol,
    Core,
    Filesystem,
    ResourceLimit,
    ShutdownTimeout,
}

/// A transport error carrying only a stable code and retry classification.
pub struct TransportError {
    code: TransportErrorCode,
    retryable: bool,
}

impl TransportError {
    pub(crate) const fn new(code: TransportErrorCode, retryable: bool) -> Self {
        Self { code, retryable }
    }

    pub const fn code(&self) -> TransportErrorCode {
        self.code
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Debug for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportError")
            .field("code", &self.code)
            .field("retryable", &self.retryable)
            .finish()
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)
    }
}

impl std::error::Error for TransportError {}
