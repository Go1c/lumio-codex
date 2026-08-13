//! Private parent/worker control protocol.

use crate::{AgentConfig, AgentError, AgentErrorCode};
use serde::{Deserialize, Serialize};
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::{Zeroize, Zeroizing};

const MAX_FRAME_BYTES: usize = 1_048_576;

pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn into_token(mut self) -> Result<fns_platform::SecretToken, AgentError> {
        let bytes = std::mem::take(&mut self.0);
        fns_platform::SecretToken::from_private_ipc(bytes)
            .map_err(|_| AgentError::new(AgentErrorCode::InsecureCredential))
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Serialize for SecretBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = SecretBytes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("private token bytes")
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(SecretBytes(value.to_vec()))
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(SecretBytes(value))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut value = Vec::new();
                while let Some(byte) = sequence.next_element::<u8>()? {
                    value.push(byte);
                    if value.len() > fns_platform::MAX_TOKEN_BYTES as usize {
                        return Err(serde::de::Error::custom("secret too large"));
                    }
                }
                Ok(SecretBytes(value))
            }
        }

        deserializer.deserialize_bytes(Visitor)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ParentFrame {
    Bootstrap {
        config: Box<AgentConfig>,
        token: SecretBytes,
    },
    ListConflicts {
        request_id: fns_protocol::RequestId,
    },
    ResolveConflict {
        request_id: fns_protocol::RequestId,
        conflict_id: fns_protocol::ConflictId,
        conflict_revision: fns_protocol::revision::WorkspaceConflictRevision,
        choice: fns_protocol::WorkspaceConflictChoice,
    },
    Shutdown,
}

impl fmt::Debug for ParentFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap { config, .. } => formatter
                .debug_struct("Bootstrap")
                .field("config", config.as_ref())
                .field("token", &"[REDACTED]")
                .finish(),
            Self::ListConflicts { request_id } => formatter
                .debug_struct("ListConflicts")
                .field("request_id", request_id)
                .finish(),
            Self::ResolveConflict {
                request_id,
                conflict_id,
                conflict_revision,
                choice,
            } => formatter
                .debug_struct("ResolveConflict")
                .field("request_id", request_id)
                .field("conflict_id", conflict_id)
                .field("conflict_revision", conflict_revision)
                .field("choice", choice)
                .finish(),
            Self::Shutdown => formatter.write_str("Shutdown"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkerFrame {
    Ready,
    ConflictsListed {
        request_id: fns_protocol::RequestId,
        conflicts: Vec<fns_sync_core::ConflictView>,
    },
    ConflictResolved {
        request_id: fns_protocol::RequestId,
        receipt: fns_sync_core::ConflictResolutionReceipt,
    },
    RequestFailed {
        request_id: fns_protocol::RequestId,
        code: AgentErrorCode,
    },
    Stopped,
    Fatal {
        code: AgentErrorCode,
    },
}

pub async fn write_parent_frame<W>(writer: W, frame: &ParentFrame) -> Result<(), AgentError>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, frame).await
}

pub async fn write_worker_frame<W>(writer: W, frame: &WorkerFrame) -> Result<(), AgentError>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, frame).await
}

pub async fn read_parent_frame<R>(reader: R) -> Result<Option<ParentFrame>, AgentError>
where
    R: AsyncRead + Unpin,
{
    read_frame(reader).await
}

pub async fn read_worker_frame<R>(reader: R) -> Result<WorkerFrame, AgentError>
where
    R: AsyncRead + Unpin,
{
    read_worker_frame_optional(reader)
        .await?
        .ok_or_else(|| AgentError::new(AgentErrorCode::Protocol))
}

pub(crate) async fn read_worker_frame_optional<R>(
    reader: R,
) -> Result<Option<WorkerFrame>, AgentError>
where
    R: AsyncRead + Unpin,
{
    read_frame(reader).await
}

async fn write_frame<W, T>(mut writer: W, frame: &T) -> Result<(), AgentError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = Zeroizing::new(
        serde_json::to_vec(frame).map_err(|_| AgentError::new(AgentErrorCode::Protocol))?,
    );
    if payload.len() > MAX_FRAME_BYTES {
        return Err(AgentError::new(AgentErrorCode::Protocol));
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))?;
    writer
        .flush()
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))
}

async fn read_frame<R, T>(mut reader: R) -> Result<Option<T>, AgentError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut length = [0_u8; 4];
    let first = reader
        .read(&mut length[..1])
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))?;
    if first == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut length[1..])
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(AgentError::new(AgentErrorCode::Protocol));
    }
    let mut payload = Zeroizing::new(vec![0_u8; length]);
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|_| AgentError::new(AgentErrorCode::Protocol))
}
