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
    StateCorrupt,
    ConflictUnavailable,
    ConflictRevisionStale,
    ConflictResolutionChanged,
    ConflictWaitingBlobs,
    ConflictAutomaticResolutionPending,
    ConflictResolutionPending,
    ConflictRefreshRequired,
    ConflictSelectedSideDeleted,
    MergeFileRequired,
    MergeContentUnavailable,
    ConflictRequestUnavailable,
    ConflictRequestChanged,
    RequestCancelled,
    AuthRequired,
    SpawnFailed,
    StartupTimeout,
    RequestTimeout,
    IdleTimeout,
    TransferTimeout,
    ResourceLimit,
    AbnormalExit,
    ShutdownTimeout,
}

/// Agent error carrying only a stable code.
pub struct AgentError {
    code: AgentErrorCode,
    reaped: bool,
}

impl AgentError {
    pub const fn new(code: AgentErrorCode) -> Self {
        Self {
            code,
            reaped: false,
        }
    }

    pub(crate) const fn after_reap(code: AgentErrorCode) -> Self {
        Self { code, reaped: true }
    }

    pub const fn code(&self) -> AgentErrorCode {
        self.code
    }

    pub const fn reaped(&self) -> bool {
        self.reaped
    }

    /// Map error code to process exit code.
    pub fn exit_code(&self) -> i32 {
        match self.code {
            AgentErrorCode::InvalidConfiguration
            | AgentErrorCode::InsecureCredential
            | AgentErrorCode::AuthRequired => 2,
            AgentErrorCode::AlreadyRunning => 4,
            AgentErrorCode::AuthenticationRejected => 5,
            AgentErrorCode::Forbidden => 5,
            AgentErrorCode::Network
            | AgentErrorCode::SpawnFailed
            | AgentErrorCode::StartupTimeout
            | AgentErrorCode::RequestTimeout
            | AgentErrorCode::IdleTimeout
            | AgentErrorCode::TransferTimeout
            | AgentErrorCode::ResourceLimit
            | AgentErrorCode::AbnormalExit => 6,
            AgentErrorCode::Protocol => 6,
            AgentErrorCode::Core => 6,
            AgentErrorCode::Filesystem => 6,
            AgentErrorCode::StateCorrupt
            | AgentErrorCode::ConflictUnavailable
            | AgentErrorCode::ConflictRevisionStale
            | AgentErrorCode::ConflictResolutionChanged
            | AgentErrorCode::ConflictWaitingBlobs
            | AgentErrorCode::ConflictAutomaticResolutionPending
            | AgentErrorCode::ConflictResolutionPending
            | AgentErrorCode::ConflictRefreshRequired
            | AgentErrorCode::ConflictSelectedSideDeleted
            | AgentErrorCode::MergeFileRequired
            | AgentErrorCode::MergeContentUnavailable
            | AgentErrorCode::ConflictRequestUnavailable
            | AgentErrorCode::ConflictRequestChanged
            | AgentErrorCode::RequestCancelled => 6,
            AgentErrorCode::ShutdownTimeout => 7,
        }
    }
}

impl fmt::Debug for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentError")
            .field("code", &self.code)
            .field("reaped", &self.reaped)
            .finish()
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)
    }
}

impl std::error::Error for AgentError {}

#[cfg(test)]
mod tests {
    use super::AgentErrorCode;

    #[test]
    fn transport_timeout_codes_have_stable_json_names() {
        assert_eq!(
            serde_json::to_string(&AgentErrorCode::IdleTimeout).unwrap(),
            "\"idle_timeout\""
        );
        assert_eq!(
            serde_json::to_string(&AgentErrorCode::TransferTimeout).unwrap(),
            "\"transfer_timeout\""
        );
    }
}
