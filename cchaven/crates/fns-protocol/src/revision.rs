use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::WorkspaceValidationError;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceRevision(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceConflictRevision(u64);

impl WorkspaceRevision {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn parse(value: &str) -> Result<Self, WorkspaceValidationError> {
        if value.is_empty() {
            return Err(validation_error("empty"));
        }

        match value.parse::<u64>() {
            Ok(parsed) if parsed.to_string() == value => Ok(Self(parsed)),
            _ => Err(validation_error("non_canonical_decimal")),
        }
    }

    pub fn decode_json(raw: &[u8]) -> Result<Self, WorkspaceValidationError> {
        let value = serde_json::from_slice::<String>(raw)
            .map_err(|_| validation_error("must_be_string"))?;
        Self::parse(&value)
    }
}

impl fmt::Display for WorkspaceRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for WorkspaceRevision {
    type Err = WorkspaceValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for WorkspaceRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WorkspaceRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(validation_error("must_be_string")))?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

impl WorkspaceConflictRevision {
    pub fn parse(value: &str) -> Result<Self, WorkspaceValidationError> {
        if value.is_empty() {
            return Err(conflict_validation_error("empty"));
        }

        match value.parse::<u64>() {
            Ok(parsed) if parsed.to_string() == value && parsed != 0 => Ok(Self(parsed)),
            Ok(0) if value == "0" => Err(conflict_validation_error("must_be_positive")),
            _ => Err(conflict_validation_error("non_canonical_decimal")),
        }
    }

    pub fn decode_json(raw: &[u8]) -> Result<Self, WorkspaceValidationError> {
        let value = serde_json::from_slice::<String>(raw)
            .map_err(|_| conflict_validation_error("must_be_string"))?;
        Self::parse(&value)
    }
}

impl Serialize for WorkspaceConflictRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WorkspaceConflictRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| D::Error::custom(conflict_validation_error("must_be_string")))?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

fn validation_error(reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: "revision".to_owned(),
        reason: reason.to_owned(),
    }
}

fn conflict_validation_error(reason: &str) -> WorkspaceValidationError {
    WorkspaceValidationError {
        field: "conflictRevision".to_owned(),
        reason: reason.to_owned(),
    }
}
