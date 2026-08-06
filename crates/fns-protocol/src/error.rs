use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use crate::deserialize_optional_non_null;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct WorkspaceValidationError {
    pub field: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct ProtocolDecodeError {
    pub field: String,
    pub reason: String,
}

impl From<WorkspaceValidationError> for ProtocolDecodeError {
    fn from(error: WorkspaceValidationError) -> Self {
        Self {
            field: error.field,
            reason: error.reason,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct ProtocolEncodeError {
    pub field: String,
    pub reason: String,
}

impl From<WorkspaceValidationError> for ProtocolEncodeError {
    fn from(error: WorkspaceValidationError) -> Self {
        Self {
            field: error.field,
            reason: error.reason,
        }
    }
}

macro_rules! workspace_error_codes {
    ($(($variant:ident, $wire:literal, $message:literal, $retryable:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        pub enum WorkspaceV2ErrorCode {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl WorkspaceV2ErrorCode {
            pub const ALL: [Self; workspace_error_codes!(@count $($variant),+)] = [
                $(Self::$variant),+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            pub const fn message(self) -> &'static str {
                match self {
                    $(Self::$variant => $message),+
                }
            }

            pub const fn retryable(self) -> bool {
                match self {
                    $(Self::$variant => $retryable),+
                }
            }
        }
    };
    (@count $($variant:ident),+) => {
        <[()]>::len(&[$(workspace_error_codes!(@unit $variant)),+])
    };
    (@unit $variant:ident) => { () };
}

workspace_error_codes!(
    (
        InvalidFrame,
        "invalid_frame",
        "invalid control frame",
        false
    ),
    (InvalidJson, "invalid_json", "invalid JSON payload", false),
    (
        UnknownAction,
        "unknown_action",
        "unknown workspace action",
        false
    ),
    (
        Unauthenticated,
        "unauthenticated",
        "authentication required",
        false
    ),
    (Forbidden, "forbidden", "workspace access forbidden", false),
    (InvalidRequest, "invalid_request", "invalid request", false),
    (
        InvalidRevision,
        "invalid_revision",
        "invalid workspace revision",
        false
    ),
    (InvalidHash, "invalid_hash", "invalid content hash", false),
    (
        InvalidPath,
        "invalid_path",
        "path must be a canonical workspace-relative POSIX path",
        false
    ),
    (
        WorkspaceNotFound,
        "workspace_not_found",
        "workspace not found",
        false
    ),
    (
        WorkspaceLimitExceeded,
        "workspace_limit_exceeded",
        "workspace limit exceeded",
        false
    ),
    (
        ClientNotRegistered,
        "client_not_registered",
        "client not registered",
        false
    ),
    (
        StaleBaseRevision,
        "stale_base_revision",
        "base revision is stale",
        false
    ),
    (
        OperationReused,
        "operation_reused",
        "operation identifier was reused",
        false
    ),
    (BlobRequired, "blob_required", "blob upload required", false),
    (BlobNotFound, "blob_not_found", "blob not found", false),
    (
        BlobHashMismatch,
        "blob_hash_mismatch",
        "blob hash mismatch",
        false
    ),
    (
        BlobSizeMismatch,
        "blob_size_mismatch",
        "blob size mismatch",
        false
    ),
    (
        BlobTransferOutOfOrder,
        "blob_transfer_out_of_order",
        "blob transfer is out of order",
        false
    ),
    (
        BlobLimitExceeded,
        "blob_limit_exceeded",
        "blob transfer limit exceeded",
        false
    ),
    (
        ConflictNotFound,
        "conflict_not_found",
        "conflict not found",
        false
    ),
    (
        ConflictRevisionStale,
        "conflict_revision_stale",
        "conflict revision is stale",
        false
    ),
    (ServerBusy, "server_busy", "server is busy", true),
    (Internal, "internal", "internal server error", true),
);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceV2FieldError {
    pub field: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceV2Error {
    pub code: WorkspaceV2ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_fields"
    )]
    pub fields: Vec<WorkspaceV2FieldError>,
}

impl WorkspaceV2Error {
    pub fn new(code: WorkspaceV2ErrorCode, fields: Vec<WorkspaceV2FieldError>) -> Self {
        Self {
            code,
            message: code.message().to_owned(),
            retryable: code.retryable(),
            fields,
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        if self.message != self.code.message() {
            return Err(validation_error("error.message", "message_mismatch"));
        }
        if self.retryable != self.code.retryable() {
            return Err(validation_error("error.retryable", "retryability_mismatch"));
        }
        Ok(())
    }
}

fn deserialize_fields<'de, D>(deserializer: D) -> Result<Vec<WorkspaceV2FieldError>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_non_null(deserializer)?
        .ok_or_else(|| D::Error::custom("fields must not be null"))
}

pub(crate) fn validation_error(field: &str, reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}
