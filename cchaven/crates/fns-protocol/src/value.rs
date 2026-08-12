use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::{MAX_BLOB_BYTES, WorkspaceValidationError};

macro_rules! uuid_newtypes {
    ($(($name:ident, $field:literal)),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
            pub struct $name(Uuid);

            impl $name {
                pub fn parse(value: &str) -> Result<Self, WorkspaceValidationError> {
                    match Uuid::parse_str(value) {
                        Ok(parsed) if parsed.to_string() == value => Ok(Self(parsed)),
                        _ => Err(uuid_validation_error($field, "invalid_uuid")),
                    }
                }

                pub const fn as_uuid(&self) -> &Uuid {
                    &self.0
                }

                pub const fn into_uuid(self) -> Uuid {
                    self.0
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.0.fmt(formatter)
                }
            }

            impl FromStr for $name {
                type Err = WorkspaceValidationError;

                fn from_str(value: &str) -> Result<Self, Self::Err> {
                    Self::parse(value)
                }
            }

            impl Serialize for $name {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    serializer.serialize_str(&self.0.to_string())
                }
            }

            impl<'de> Deserialize<'de> for $name {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    let value = String::deserialize(deserializer).map_err(|_| {
                        D::Error::custom(uuid_validation_error($field, "must_be_string"))
                    })?;
                    Self::parse(&value).map_err(D::Error::custom)
                }
            }
        )+
    };
}

uuid_newtypes!(
    (WorkspaceId, "workspaceId"),
    (ClientId, "clientId"),
    (OperationId, "operationId"),
    (RequestId, "requestId"),
    (StreamId, "streamId"),
    (TransferId, "transferId"),
    (ConflictId, "conflictId"),
);

fn uuid_validation_error(field: &str, reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceContentHash(String);

impl WorkspaceContentHash {
    pub fn parse(value: &str) -> Result<Self, WorkspaceValidationError> {
        let valid = value.len() == 71
            && value.starts_with("blake3:")
            && value[7..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(content_hash_validation_error("invalid_blake3"))
        }
    }

    pub fn decode_json(raw: &[u8]) -> Result<Self, WorkspaceValidationError> {
        let value = serde_json::from_slice::<String>(raw)
            .map_err(|_| content_hash_validation_error("must_be_string"))?;
        Self::parse(&value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspaceContentHash {
    type Err = WorkspaceValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for WorkspaceContentHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceContentHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(content_hash_validation_error("must_be_string")))?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

fn content_hash_validation_error(reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: "contentHash".to_owned(),
        reason: reason.to_owned(),
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    pub fn parse(value: &str) -> Result<Self, WorkspaceValidationError> {
        if value.is_empty() || value.len() > 4096 {
            return Err(path_validation_error("invalid_length_or_utf8"));
        }
        if value.nfc().collect::<String>() != value {
            return Err(path_validation_error("not_nfc"));
        }
        if value.starts_with('/')
            || value.ends_with('/')
            || value.contains("//")
            || value.contains('\\')
        {
            return Err(path_validation_error("not_relative_posix"));
        }

        for segment in value.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(path_validation_error("invalid_segment"));
            }
            if segment.ends_with('.') || segment.ends_with(' ') {
                return Err(path_validation_error("windows_unsafe_suffix"));
            }
            if segment.chars().any(|character| {
                character <= '\u{001f}'
                    || ('\u{007f}'..='\u{009f}').contains(&character)
                    || r#"<>:"|?*"#.contains(character)
            }) {
                return Err(path_validation_error("unsafe_character"));
            }

            let base = segment.split_once('.').map_or(segment, |(base, _)| base);
            let base = base.to_ascii_uppercase();
            let numbered_device = base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9');
            if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
                return Err(path_validation_error("windows_device_name"));
            }
        }

        Ok(Self(value.to_owned()))
    }

    pub fn decode_json(raw: &[u8]) -> Result<Self, WorkspaceValidationError> {
        let value = serde_json::from_slice::<String>(raw)
            .map_err(|_| path_validation_error("must_be_string"))?;
        Self::parse(&value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspacePath {
    type Err = WorkspaceValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for WorkspacePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspacePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(path_validation_error("must_be_string")))?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

fn path_validation_error(reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: "path".to_owned(),
        reason: reason.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceEntryKind {
    File,
    Directory,
    Symlink,
    Tombstone,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMutationKind {
    UpsertFile,
    Mkdir,
    UpsertSymlink,
    Delete,
    Rename,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSnapshotMode {
    Snapshot,
    Incremental,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceBlobDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConflictKind {
    Content,
    DeleteModify,
    Rename,
    Binary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConflictChoice {
    Current,
    Incoming,
    Merged,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMutationRejectReason {
    StaleBaseRevision,
    OperationReused,
    BlobRequired,
    ConflictCreated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceFileMetadata {
    pub size: u64,
    pub modified_at_ms: i64,
    pub executable: bool,
}

impl WorkspaceFileMetadata {
    pub fn validate(&self, kind: WorkspaceEntryKind) -> Result<(), WorkspaceValidationError> {
        if self.size > MAX_BLOB_BYTES {
            return Err(metadata_validation_error("metadata.size", "limit_exceeded"));
        }
        if !(0..=253_402_300_799_999).contains(&self.modified_at_ms) {
            return Err(metadata_validation_error(
                "metadata.modifiedAtMs",
                "out_of_range",
            ));
        }
        if matches!(
            kind,
            WorkspaceEntryKind::Directory | WorkspaceEntryKind::Tombstone
        ) {
            if self.size != 0 {
                return Err(metadata_validation_error("metadata.size", "must_be_zero"));
            }
            if self.executable {
                return Err(metadata_validation_error(
                    "metadata.executable",
                    "must_be_false",
                ));
            }
        }
        Ok(())
    }
}

fn metadata_validation_error(field: &str, reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}
