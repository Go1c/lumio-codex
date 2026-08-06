#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct WorkspaceValidationError {
    pub field: String,
    pub reason: String,
}
