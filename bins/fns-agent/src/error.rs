//! Agent error types with stable codes and exit code mapping.

use std::fmt;

/// Agent lifecycle phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Starting,
    Recovering,
    Connecting,
    Subscribing,
    Online,
    Stopping,
    Stopped,
    Fatal,
}

/// Agent error codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorCode {
    InvalidConfiguration,
    InsecureCredential,
    AlreadyRunning,
    AuthenticationRejected,
    Forbidden,
    Network,
    Protocol,
    Core,
    Filesystem,
    ShutdownTimeout,
}

/// Agent error carrying only a stable code.
pub struct AgentError {
    code: AgentErrorCode,
}

impl AgentError {
    pub const fn new(code: AgentErrorCode) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> AgentErrorCode {
        self.code
    }

    /// Map error code to process exit code.
    pub fn exit_code(&self) -> i32 {
        match self.code {
            AgentErrorCode::InvalidConfiguration | AgentErrorCode::InsecureCredential => 2,
            AgentErrorCode::AlreadyRunning => 4,
            AgentErrorCode::AuthenticationRejected => 5,
            AgentErrorCode::Forbidden => 5,
            AgentErrorCode::Network => 6,
            AgentErrorCode::Protocol => 6,
            AgentErrorCode::Core => 6,
            AgentErrorCode::Filesystem => 6,
            AgentErrorCode::ShutdownTimeout => 7,
        }
    }
}

impl fmt::Debug for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentError")
            .field("code", &self.code)
            .finish()
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)
    }
}

impl std::error::Error for AgentError {}
